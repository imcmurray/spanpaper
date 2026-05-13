//! spanpaper-tray — StatusNotifierItem applet for the system tray.
//!
//! M2 walking skeleton: a tray icon that exposes a right-click menu
//! (Start / Stop / Quit). No popover, no thumbnails — those land in
//! M3+ per docs/tray-applet-plan.md.
//!
//! The applet is a CLI client of the daemon: actions shell out to the
//! existing `spanpaper start --background` / `spanpaper stop`; liveness
//! is a pid-file + kill(pid, 0) probe every 2 s. No new IPC, no shared
//! library — the daemon never learns the tray exists.

mod daemon_client;

use anyhow::Result;
use ksni::{menu::StandardItem, MenuItem, ToolTip, Tray, TrayMethods};
use std::time::Duration;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[derive(Debug)]
struct SpanpaperTray {
    daemon_running: bool,
}

impl Tray for SpanpaperTray {
    fn id(&self) -> String {
        "spanpaper-tray".into()
    }

    fn title(&self) -> String {
        "spanpaper".into()
    }

    fn icon_name(&self) -> String {
        // Matches Icon= in contrib/spanpaper-set-*.desktop so all three
        // spanpaper-related entries look the same in panel + Open With.
        "preferences-desktop-wallpaper".into()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "spanpaper".into(),
            description: if self.daemon_running {
                "daemon running"
            } else {
                "daemon stopped"
            }
            .into(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let running = self.daemon_running;
        vec![
            StandardItem {
                label: "Start daemon".into(),
                enabled: !running,
                activate: Box::new(|tray: &mut Self| {
                    if let Err(e) = daemon_client::start_daemon() {
                        tracing::warn!("start daemon: {e}");
                    } else {
                        // Optimistically flip; the 2 s poll will correct
                        // it if the daemon failed to come up.
                        tray.daemon_running = true;
                    }
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Stop daemon".into(),
                enabled: running,
                activate: Box::new(|tray: &mut Self| {
                    if let Err(e) = daemon_client::stop_daemon() {
                        tracing::warn!("stop daemon: {e}");
                    } else {
                        tray.daemon_running = false;
                    }
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit tray".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|_| std::process::exit(0)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,spanpaper_tray=debug"));
    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false))
        .with(filter)
        .init();

    let initial = daemon_client::daemon_alive();
    tracing::info!(
        "spanpaper-tray starting; daemon currently {}",
        if initial { "running" } else { "stopped" }
    );

    let tray = SpanpaperTray { daemon_running: initial };
    let handle = tray
        .spawn()
        .await
        .map_err(|e| anyhow::anyhow!("spawn tray service: {e:?}"))?;

    // Poll daemon liveness every 2 s and push state changes into the
    // tray so the Start/Stop enabled flags stay correct even when the
    // daemon is started or killed outside our menu (e.g. via systemctl
    // or another spanpaper CLI invocation).
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    interval.tick().await; // first tick fires immediately; consume it
    let mut last_state = initial;

    // Ctrl-C from a terminal launch: exit cleanly so the panel removes
    // the icon. SIGTERM (logout) is handled by the runtime tearing
    // down.
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let now_state = daemon_client::daemon_alive();
                if now_state != last_state {
                    tracing::debug!(
                        "daemon state changed: {last_state} → {now_state}"
                    );
                    handle.update(|t: &mut SpanpaperTray| {
                        t.daemon_running = now_state;
                    }).await;
                    last_state = now_state;
                }
            }
            _ = &mut ctrl_c => {
                tracing::info!("ctrl-c received; tray exiting");
                return Ok(());
            }
        }
    }
}
