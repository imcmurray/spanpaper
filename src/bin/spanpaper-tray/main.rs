// Hand-aligned ASCII tables in the architecture doc-comment below
// trigger clippy's `doc_overindented_list_items` lint; the alignment
// is the point.
#![allow(clippy::doc_overindented_list_items)]

//! spanpaper-tray — StatusNotifierItem applet for the system tray.
//!
//! Architecture:
//!   * Main thread       — GTK4 main loop. Owns the gtk4::Application
//!                         and (lazily, on left-click) the layout
//!                         palette window.
//!   * Worker thread     — tokio current-thread runtime that hosts the
//!                         ksni tray service and the 2-second daemon-
//!                         liveness poll.
//!   * Bridge            — async_channel<()>. ksni's `activate()` (left
//!                         click) does a non-blocking `try_send(())`;
//!                         a glib::spawn_future_local task on the GTK
//!                         thread receives and calls palette::show.
//!
//! The tray remains a pure CLI client of the daemon: actions shell out
//! to `spanpaper start --background` / `spanpaper stop`; liveness is a
//! pid-file probe; layout queries go through `spanpaper outputs`.

mod daemon_client;
mod dialog;
mod outputs_query;
mod palette;

use anyhow::Result;
use gtk4::prelude::*;
use ksni::{
    menu::{StandardItem, SubMenu},
    MenuItem, ToolTip, Tray, TrayMethods,
};
use std::time::Duration;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Messages from the tokio/ksni thread to the GTK main thread.
#[derive(Debug)]
enum UiMsg {
    /// Open (or raise) the layout palette. `x` and `y` are the screen
    /// coordinates of the tray-icon click, used by gtk-layer-shell to
    /// anchor the popover near the icon. (-1, -1) means "no position
    /// hint" — happens for menu-item activations and on panels that
    /// don't pass coords through to SNI.
    ShowPalette { x: i32, y: i32 },
    /// Open the "Save current as…" preset-name prompt. Same (x, y)
    /// convention as ShowPalette.
    SavePresetDialog { x: i32, y: i32 },
}

#[derive(Debug)]
struct SpanpaperTray {
    daemon_running: bool,
    // Last-commanded playback state, not a live read of mpv. Kept here
    // so the right-click menu can show Pause or Resume appropriately.
    // After a daemon restart workers come back unpaused regardless, so
    // this can briefly disagree with reality — clicking Resume is a
    // no-op in that case and clicking Pause resyncs.
    paused: bool,
    // Cloned into ksni's activate/menu callbacks so left-click and
    // menu items can request UI on the GTK thread without blocking
    // the tokio runtime.
    ui_tx: async_channel::Sender<UiMsg>,
}

impl Tray for SpanpaperTray {
    fn id(&self) -> String {
        "spanpaper-tray".into()
    }

    fn title(&self) -> String {
        "spanpaper".into()
    }

    fn icon_name(&self) -> String {
        // Reflect daemon + playback state in the panel icon. Returns
        // the highest-priority match; all three names are standard
        // freedesktop icon-naming-spec entries present in any modern
        // icon theme.
        match (self.daemon_running, self.paused) {
            (false, _) => "media-playback-stop".into(),
            (true, true) => "media-playback-pause".into(),
            (true, false) => "preferences-desktop-wallpaper".into(),
        }
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "spanpaper".into(),
            description: if self.daemon_running {
                "daemon running — click to open the layout palette"
            } else {
                "daemon stopped — right-click to start"
            }
            .into(),
            ..Default::default()
        }
    }

    /// Left-click handler. Request the GTK thread to open the palette
    /// at the icon's screen coordinates.
    fn activate(&mut self, x: i32, y: i32) {
        tracing::debug!("tray activate (left-click) at ({x},{y})");
        if let Err(e) = self.ui_tx.try_send(UiMsg::ShowPalette { x, y }) {
            tracing::warn!("send ShowPalette: {e}");
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        // KEY DESIGN POINT: we conditionally INCLUDE/EXCLUDE entire
        // groups of items based on daemon state, rather than always
        // emitting them and just toggling `enabled`. Per ksni's source,
        // only structural diffs (children-list changes) fire
        // LayoutUpdated; pure enabled-flag flips emit
        // ItemsPropertiesUpdated, which Budgie's tray applet refreshes
        // poorly — the result was Budgie cached the cold-start menu
        // and silently swallowed clicks on items it thought were
        // disabled. Show only what's actionable, hide everything else.
        let running = self.daemon_running;
        let paused = self.paused;
        let tx = self.ui_tx.clone();
        let mut items: Vec<MenuItem<Self>> = Vec::new();

        if running {
            let tx_palette = tx.clone();
            items.push(
                StandardItem {
                    label: "Open palette".into(),
                    icon_name: "preferences-desktop-wallpaper".into(),
                    activate: Box::new(move |_| {
                        // No click coords for menu-item activation —
                        // let palette::show fall back to compositor
                        // default placement.
                        let _ = tx_palette.try_send(UiMsg::ShowPalette { x: -1, y: -1 });
                    }),
                    ..Default::default()
                }
                .into(),
            );
            items.push(MenuItem::Separator);

            // Pause / Resume swaps label based on tray-side state.
            items.push(if paused {
                StandardItem {
                    label: "Resume playback".into(),
                    icon_name: "media-playback-start".into(),
                    activate: Box::new(|tray: &mut Self| {
                        daemon_client::resume_playback();
                        tray.paused = false;
                    }),
                    ..Default::default()
                }
                .into()
            } else {
                StandardItem {
                    label: "Pause playback".into(),
                    icon_name: "media-playback-pause".into(),
                    activate: Box::new(|tray: &mut Self| {
                        daemon_client::pause_playback();
                        tray.paused = true;
                    }),
                    ..Default::default()
                }
                .into()
            });

            items.push(
                SubMenu {
                    label: "Span fit".into(),
                    submenu: vec![
                        span_fit_item("Crop (zoom-fill, default)", "crop"),
                        span_fit_item("Fit (letterbox)", "fit"),
                        span_fit_item("Stretch", "stretch"),
                    ],
                    ..Default::default()
                }
                .into(),
            );

            items.push(
                SubMenu {
                    // Same three options as Span fit, independently
                    // applied to side (image AND video). swaybg's
                    // backend-specific modes (center / tile) remain
                    // available via `spanpaper set --side-mode …` for
                    // power users — kept out of the menu so the two
                    // fit options stay symmetric.
                    label: "Side fit".into(),
                    submenu: vec![
                        side_fit_item("Crop (zoom-fill, default)", "crop"),
                        side_fit_item("Fit (letterbox)", "fit"),
                        side_fit_item("Stretch", "stretch"),
                    ],
                    ..Default::default()
                }
                .into(),
            );

            items.push(
                SubMenu {
                    label: "Audio".into(),
                    submenu: vec![audio_item("On", true), audio_item("Off (default)", false)],
                    ..Default::default()
                }
                .into(),
            );

            // Presets submenu. Read fresh from config on every render so
            // CLI-side preset edits show up without restarting the tray.
            items.push(build_presets_submenu(&tx));

            items.push(MenuItem::Separator);
        }

        items.push(
            StandardItem {
                label: "Open config folder".into(),
                icon_name: "folder-open".into(),
                activate: Box::new(|_| {
                    if let Err(e) = daemon_client::open_config_folder() {
                        tracing::warn!("open config folder: {e}");
                    }
                }),
                ..Default::default()
            }
            .into(),
        );

        if running {
            items.push(
                StandardItem {
                    label: "Reload config".into(),
                    icon_name: "view-refresh".into(),
                    activate: Box::new(|_| {
                        if let Err(e) = daemon_client::reload_daemon() {
                            tracing::warn!("reload daemon: {e}");
                        }
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }

        items.push(MenuItem::Separator);

        items.push(if running {
            StandardItem {
                label: "Stop daemon".into(),
                activate: Box::new(|tray: &mut Self| {
                    if let Err(e) = daemon_client::stop_daemon() {
                        tracing::warn!("stop daemon: {e}");
                    } else {
                        tray.daemon_running = false;
                        tray.paused = false;
                    }
                }),
                ..Default::default()
            }
            .into()
        } else {
            StandardItem {
                label: "Start daemon".into(),
                activate: Box::new(|tray: &mut Self| {
                    if let Err(e) = daemon_client::start_daemon() {
                        tracing::warn!("start daemon: {e}");
                    } else {
                        tray.daemon_running = true;
                        tray.paused = false;
                    }
                }),
                ..Default::default()
            }
            .into()
        });

        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: "Quit tray".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|_| std::process::exit(0)),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

// ---- per-submenu item factories -------------------------------------------
// Plain closures; declared as free functions to keep menu() readable.

fn span_fit_item(label: &str, value: &'static str) -> MenuItem<SpanpaperTray> {
    StandardItem {
        label: label.into(),
        activate: Box::new(move |_| {
            if let Err(e) = daemon_client::set_span_fit(value) {
                tracing::warn!("set span-fit {value}: {e}");
            }
        }),
        ..Default::default()
    }
    .into()
}

fn side_fit_item(label: &str, value: &'static str) -> MenuItem<SpanpaperTray> {
    StandardItem {
        label: label.into(),
        activate: Box::new(move |_| {
            if let Err(e) = daemon_client::set_side_fit(value) {
                tracing::warn!("set side-fit {value}: {e}");
            }
        }),
        ..Default::default()
    }
    .into()
}

fn audio_item(label: &str, on: bool) -> MenuItem<SpanpaperTray> {
    StandardItem {
        label: label.into(),
        activate: Box::new(move |_| {
            if let Err(e) = daemon_client::set_audio(on) {
                tracing::warn!("set audio {on}: {e}");
            }
        }),
        ..Default::default()
    }
    .into()
}

fn build_presets_submenu(tx: &async_channel::Sender<UiMsg>) -> MenuItem<SpanpaperTray> {
    // Snapshot config; render fails closed (empty preset list) if
    // anything goes wrong.
    let cfg = spanpaper::config::Config::load_or_default().unwrap_or_default();
    let active = cfg.active_preset.clone();

    let mut submenu: Vec<MenuItem<SpanpaperTray>> = Vec::new();

    for preset in &cfg.presets {
        // Prefix "✓ " on the active preset's label. We tried
        // ksni's CheckmarkItem but it implies a toggle widget and
        // some panels render it ambiguously; a label prefix is
        // unambiguous and theme-agnostic.
        let label = if active.as_deref() == Some(preset.name.as_str()) {
            format!("✓ {}", preset.name)
        } else {
            format!("   {}", preset.name)
        };
        let name = preset.name.clone();
        submenu.push(
            StandardItem {
                label,
                activate: Box::new(move |_| {
                    if let Err(e) = daemon_client::preset_load(&name) {
                        tracing::warn!("preset load {name:?}: {e:#}");
                    }
                }),
                ..Default::default()
            }
            .into(),
        );
    }

    if !cfg.presets.is_empty() {
        submenu.push(MenuItem::Separator);
        submenu.push(
            StandardItem {
                label: "Next preset".into(),
                activate: Box::new(|_| {
                    if let Err(e) = daemon_client::preset_next() {
                        tracing::warn!("preset next: {e:#}");
                    }
                }),
                ..Default::default()
            }
            .into(),
        );
        submenu.push(
            StandardItem {
                label: "Previous preset".into(),
                activate: Box::new(|_| {
                    if let Err(e) = daemon_client::preset_prev() {
                        tracing::warn!("preset prev: {e:#}");
                    }
                }),
                ..Default::default()
            }
            .into(),
        );
        submenu.push(MenuItem::Separator);
    }

    // Always reachable: save the active config as a new preset.
    let tx = tx.clone();
    submenu.push(
        StandardItem {
            label: "Save current as…".into(),
            activate: Box::new(move |_| {
                // Menu activations don't carry click coords through
                // ksni, so we let the dialog fall back to compositor
                // placement. (-1, -1) is the sentinel for "no anchor".
                let _ = tx.try_send(UiMsg::SavePresetDialog { x: -1, y: -1 });
            }),
            ..Default::default()
        }
        .into(),
    );

    SubMenu {
        label: "Presets".into(),
        submenu,
        ..Default::default()
    }
    .into()
}

fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,spanpaper_tray=debug"));
    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false))
        .with(filter)
        .init();

    let (ui_tx, ui_rx) = async_channel::unbounded::<UiMsg>();

    // Spawn the tokio + ksni worker thread. It owns the ksni service
    // and the poll loop; the main thread mustn't touch ksni state.
    let ui_tx_worker = ui_tx.clone();
    std::thread::Builder::new()
        .name("ksni-worker".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build tokio runtime");
            rt.block_on(async move {
                if let Err(e) = run_tray_service(ui_tx_worker).await {
                    tracing::error!("tray service exited: {e:#}");
                }
            });
        })
        .expect("spawn ksni worker thread");

    // Build the GTK Application and run its main loop on this thread.
    // NON_UNIQUE so multiple tray instances coexist (e.g. while testing
    // a freshly-built binary alongside an old one) instead of D-Bus-
    // squatting on each other.
    let app = gtk4::Application::builder()
        .application_id("dev.spanpaper.Tray")
        .flags(gtk4::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        // Hold a reference so the Application doesn't quit when the
        // user closes the palette window — we want it to keep running
        // in the background driving the tray icon. `hold()` returns
        // a guard whose Drop releases the hold, so it MUST be kept
        // alive — leaking it intentionally is the documented pattern.
        std::mem::forget(app.hold());

        // GLib-side receiver. spawn_future_local runs the future on
        // the GTK main loop; calling palette::show from inside it is
        // safe because we're on the GTK thread.
        let app_w = app.downgrade();
        let ui_rx = ui_rx.clone();
        gtk4::glib::spawn_future_local(async move {
            tracing::debug!("ui receiver task started on GTK thread");
            while let Ok(msg) = ui_rx.recv().await {
                tracing::debug!("ui msg received: {msg:?}");
                let Some(app) = app_w.upgrade() else { break };
                match msg {
                    UiMsg::ShowPalette { x, y } => palette::show(&app, x, y),
                    UiMsg::SavePresetDialog { x, y } => dialog::save_preset(&app, x, y),
                }
            }
            tracing::debug!("ui receiver loop ended");
        });
    });

    let initial = daemon_client::daemon_alive();
    tracing::info!(
        "spanpaper-tray starting; daemon currently {}",
        if initial { "running" } else { "stopped" }
    );

    // GtkApplication::run wants argv as a slice of &str. We don't
    // forward our own argv because GTK's command-line parsing would
    // collide with future CLI flags we might add.
    let empty: [&str; 0] = [];
    let exit = app.run_with_args(&empty);
    std::process::exit(exit.value());
}

async fn run_tray_service(ui_tx: async_channel::Sender<UiMsg>) -> Result<()> {
    let initial = daemon_client::daemon_alive();
    let tray = SpanpaperTray {
        daemon_running: initial,
        paused: false,
        ui_tx,
    };
    let handle = tray
        .spawn()
        .await
        .map_err(|e| anyhow::anyhow!("spawn tray service: {e:?}"))?;

    // Poll daemon liveness every 2 s. Same logic as M2.
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    interval.tick().await;
    let mut last_state = initial;

    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let now_state = daemon_client::daemon_alive();
                let state_changed = now_state != last_state;
                if state_changed {
                    tracing::debug!(
                        "daemon state changed: {last_state} → {now_state}"
                    );
                    last_state = now_state;
                }
                // Always call update() — ksni re-runs `menu()` inside,
                // and the Presets submenu reads the config file fresh
                // each render so CLI-side preset edits (save/delete/
                // rename) show up within one poll cycle. update_menu's
                // diff is cheap when nothing actually changed, and only
                // emits LayoutUpdated when the children diverge.
                handle.update(|t: &mut SpanpaperTray| {
                    if state_changed {
                        t.daemon_running = now_state;
                        // A fresh daemon always boots unpaused (workers
                        // spawn with pause=yes and the supervisor sync-
                        // unpauses them). Reset our stale flag so the
                        // menu shows Pause, not Resume.
                        if !now_state {
                            t.paused = false;
                        }
                    }
                }).await;
            }
            _ = &mut ctrl_c => {
                tracing::info!("ctrl-c received; tray exiting");
                std::process::exit(0);
            }
        }
    }
}
