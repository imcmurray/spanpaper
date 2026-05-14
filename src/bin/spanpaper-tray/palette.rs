//! The layout-palette popover window.
//!
//! M3 milestone: a small floating window that shows the user's actual
//! monitor topology to scale, with each output rendered as a
//! GTK4 Frame containing its name, resolution, and the basename of the
//! file currently assigned to it.
//!
//! M3 is static — no drag-and-drop, no file picker, no thumbnails.
//! Those land in M4/M5/M6 per docs/tray-applet-plan.md.

use crate::{
    daemon_client::{self, Slot},
    outputs_query::OutputInfo,
};
use gtk4::prelude::*;
use gtk4_layer_shell::LayerShell;
use spanpaper::{
    config::{Config, SpanDirection},
    thumbnail,
};
use std::path::Path;

/// Load the daemon's config, falling back to a default if anything
/// goes wrong (missing file, parse error, …). The palette can render
/// usefully even without a daemon — it shows "(unset)" placeholders.
fn load_config() -> Config {
    Config::load_or_default().unwrap_or_default()
}

/// Max pixel size of the longest monitor edge in the popover. The other
/// edge scales proportionally. Picked by eye — small enough to keep the
/// window compact, big enough to show a meaningful preview.
const MAX_EDGE_PX: i32 = 220;

/// CSS widget-name tag we set on the palette window so subsequent
/// `show()` calls can locate it via `Application::windows()` and raise
/// the existing instance instead of stacking up duplicates.
const PALETTE_WIDGET_NAME: &str = "spanpaper-palette";

/// Shared state per palette window. Lives in a Rc because both the
/// focus-out handler and the file-picker callbacks need to read/write
/// the auto-close suppression flag.
#[derive(Default)]
struct PaletteState {
    /// Set true once the window has gained focus at least once. Stops
    /// us auto-closing on the very-first-open transition (which goes
    /// from "not yet active" → "active").
    was_active: std::cell::Cell<bool>,
    /// Set true while a modal child (the FileDialog) is open. When
    /// that happens GTK4 reports the parent window as inactive even
    /// though the user hasn't actually clicked away — we must not
    /// close in that case or we destroy the dialog's parent mid-pick.
    suppress_autoclose: std::cell::Cell<bool>,
}

pub fn show(app: &gtk4::Application, click_x: i32, click_y: i32) {
    // Single-instance: if a palette window is already open, raise and
    // focus it instead of spawning a second one. Drop handlers
    // repopulate-in-place rather than closing, so this path catches
    // any case where the tray icon is clicked while the palette is
    // already visible.
    for w in app.windows() {
        if w.widget_name() == PALETTE_WIDGET_NAME {
            w.present();
            return;
        }
    }

    // Pick a default size that comfortably fits the full layout
    // (monitor rectangles + summary rows + drop-hint label) so the
    // initial paint doesn't collapse around the small async-thumbnail
    // spinners. Allow resizing so the user can grow it if their theme
    // demands more headroom — the layout still looks reasonable at
    // larger sizes.
    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .title("spanpaper")
        .default_width(540)
        .default_height(540)
        .build();
    window.set_widget_name(PALETTE_WIDGET_NAME);

    // Anchor the palette near the tray icon via wlr-layer-shell.
    // Layer shell lets us place a regular Wayland window at exact
    // screen coordinates — without it the compositor picks
    // (usually badly, sometimes off-screen for tiny defaults). We
    // anchor to top-left and use margins as the (x, y) offset from
    // the screen origin, which is where SNI's Activate(x, y) hands
    // us the tray-icon's click position.
    //
    // Fallback: when coords are (-1, -1) — menu-item activation,
    // panels that don't pass coords — let the compositor pick.
    // Likewise if layer-shell isn't supported on this compositor.
    if click_x >= 0 && click_y >= 0 {
        anchor_near_tray(&window, click_x, click_y);
    }

    // Per-window state. Attached as glib qdata so other functions
    // (notably the file-picker code in summary_row) can find it via
    // the window reference without us threading the Rc through five
    // build_* functions.
    let state = std::rc::Rc::new(PaletteState::default());
    unsafe {
        window.set_data::<std::rc::Rc<PaletteState>>(STATE_QDATA, state.clone());
    }

    // Auto-close on focus loss — matches how panel applets like
    // Caffeine / Night Light behave. is_active flips false when
    // another window grabs focus; on Wayland that's also the "user
    // clicked away" signal. Two gates keep this honest:
    //   * was_active prevents closing during the initial show, when
    //     is_active starts false and only flips true once the
    //     compositor focuses us.
    //   * suppress_autoclose covers the modal-child case (FileDialog
    //     open) — the palette becomes "inactive" because its dialog
    //     is focused, but closing here would tear down the dialog's
    //     parent mid-pick.
    let state_for_focus = state.clone();
    window.connect_is_active_notify(move |w| {
        if w.is_active() {
            state_for_focus.was_active.set(true);
        } else if state_for_focus.was_active.get() && !state_for_focus.suppress_autoclose.get() {
            w.close();
        }
    });

    populate(&window);
    window.present();
}

fn anchor_near_tray(window: &gtk4::ApplicationWindow, x: i32, y: i32) {
    use gtk4_layer_shell::{Edge, Layer};

    // Init the surface as a layer-shell surface. Must happen before
    // present(). If the compositor doesn't speak wlr-layer-shell this
    // call is harmless — we just won't get anchor behaviour.
    window.init_layer_shell();
    window.set_layer(Layer::Top);

    // Anchor to top-left and use margins to place the window's
    // top-left corner near the icon. We deliberately don't anchor
    // to two opposite edges (which would *stretch* the window) —
    // just top + left so margins act as an absolute position.
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);
    // Clamp to >= 0 so a screen-edge click can't push us negative.
    // Inset by a few pixels so the palette doesn't share an edge with
    // the panel — easier to see and to click outside to dismiss.
    window.set_margin(Edge::Top, y.max(0).saturating_add(4));
    window.set_margin(Edge::Left, x.max(0).saturating_add(4));

    // Take keyboard focus when the window is interacted with so the
    // file-picker (and any future text input inside the popover)
    // works. OnDemand grabs focus while the user types/clicks but
    // releases when they click elsewhere — which dovetails with our
    // focus-out auto-close.
    window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::OnDemand);
}

/// Glib qdata key used to stash the per-window PaletteState. Same
/// convention as `widget_name` for finding windows: stringly typed,
/// stable, internal.
const STATE_QDATA: &str = "spanpaper-palette-state";

fn palette_state(window: &gtk4::ApplicationWindow) -> Option<std::rc::Rc<PaletteState>> {
    unsafe {
        window
            .data::<std::rc::Rc<PaletteState>>(STATE_QDATA)
            .map(|ptr| (*ptr.as_ref()).clone())
    }
}

/// (Re)build the palette window's child from current config + outputs.
/// Called once from `show()` and again from each drop handler after a
/// successful `spanpaper set`, so the new thumbnail appears in place
/// without the user re-clicking the tray. Reads `spanpaper outputs`
/// and `~/.config/spanpaper/config.toml` fresh on every call.
fn populate(window: &gtk4::ApplicationWindow) {
    window.set_child(Some(&build_content()));
}

fn build_content() -> gtk4::Widget {
    let outputs = match crate::outputs_query::list() {
        Ok(v) => v,
        Err(e) => {
            return error_widget(&format!("Could not enumerate outputs:\n{e:#}"));
        }
    };
    let cfg = load_config();

    let root = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    root.append(&build_layout_row(&outputs, &cfg));
    root.append(&summary_row("span", Slot::Span, cfg.span.as_deref()));
    root.append(&summary_row("side", Slot::Side, cfg.side.as_deref()));

    let hint = gtk4::Label::builder()
        .label("Drop an image or video onto a box to assign it")
        .css_classes(vec!["dim-label"])
        .build();
    hint.set_xalign(0.0);
    root.append(&hint);
    root.upcast()
}

fn error_widget(msg: &str) -> gtk4::Widget {
    gtk4::Label::builder()
        .label(msg)
        .margin_top(20)
        .margin_bottom(20)
        .margin_start(20)
        .margin_end(20)
        .selectable(true)
        .build()
        .upcast()
}

/// Build the horizontal row of monitor rectangles. Span group on the
/// left (oriented per `span_direction`), side output on the right.
fn build_layout_row(outputs: &[OutputInfo], cfg: &Config) -> gtk4::Box {
    let row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(16)
        .halign(gtk4::Align::Center)
        .build();

    let span_outs: Vec<&OutputInfo> = cfg
        .span_outputs
        .iter()
        .filter_map(|name| outputs.iter().find(|o| &o.name == name))
        .collect();
    let side_out: Option<&OutputInfo> = cfg
        .side_output
        .as_deref()
        .and_then(|name| outputs.iter().find(|o| o.name == name));

    // Pixel-per-monitor-pixel scale, chosen so the largest dimension
    // anywhere in the popover hits MAX_EDGE_PX. We base it on the
    // overall pixel max across all rendered outputs so span and side
    // share a single, comparable scale.
    let max_dim = outputs
        .iter()
        .map(|o| o.width.max(o.height))
        .max()
        .unwrap_or(1);
    let scale = MAX_EDGE_PX as f32 / max_dim as f32;

    // Build the two top-level boxes (span group + standalone side),
    // then append them in the order their leftmost x-coordinate
    // appears on the desktop. That way "what you see is what you
    // set" — a side monitor placed to the left of the span pair on
    // the user's desk renders on the left of the popover too.
    let span_box = (!span_outs.is_empty()).then(|| {
        let dir = match cfg.span_direction {
            SpanDirection::Horizontal => gtk4::Orientation::Horizontal,
            SpanDirection::Vertical => gtk4::Orientation::Vertical,
        };
        let group = gtk4::Box::builder().orientation(dir).spacing(2).build();
        for o in &span_outs {
            group.append(&build_output_frame(
                o,
                cfg.span.as_deref(),
                scale,
                Slot::Span,
            ));
        }
        let leftmost_x = span_outs.iter().map(|o| o.x).min().unwrap_or(0);
        (leftmost_x, group)
    });
    let side_frame = side_out.map(|o| {
        (
            o.x,
            build_output_frame(o, cfg.side.as_deref(), scale, Slot::Side),
        )
    });

    let mut placed: Vec<(i32, gtk4::Widget)> = Vec::new();
    if let Some((x, w)) = span_box {
        placed.push((x, w.upcast::<gtk4::Widget>()));
    }
    if let Some((x, w)) = side_frame {
        placed.push((x, w.upcast::<gtk4::Widget>()));
    }
    placed.sort_by_key(|(x, _)| *x);
    for (_, w) in placed {
        row.append(&w);
    }

    // Fallback when neither is configured.
    if span_outs.is_empty() && side_out.is_none() {
        row.append(
            &gtk4::Label::builder()
                .label("No outputs configured.\nRun `spanpaper set --span … --side …`.")
                .justify(gtk4::Justification::Center)
                .build(),
        );
    }
    row
}

fn build_output_frame(
    out: &OutputInfo,
    assigned: Option<&Path>,
    scale: f32,
    slot: Slot,
) -> gtk4::Frame {
    let role = match slot {
        Slot::Span => "span",
        Slot::Side => "side",
    };
    let w = ((out.width as f32) * scale).round().max(40.0) as i32;
    let h = ((out.height as f32) * scale).round().max(40.0) as i32;

    let assigned_text = match assigned {
        Some(p) => basename(p),
        None => "(unset)".into(),
    };
    let frame = gtk4::Frame::builder()
        .label(format!("{}  ({})", out.name, role))
        .label_xalign(0.5)
        .width_request(w)
        .height_request(h)
        // Hover/screen-reader text: full output info + current file +
        // hint that this is a drop target.
        .tooltip_text(format!(
            "{} ({}×{})\nCurrently: {}\nDrop an image or video here to assign it to the {} slot.",
            out.name, out.width, out.height, assigned_text, role,
        ))
        .build();

    // Drop target: accept any gio::File (local files) dropped onto
    // this frame. On drop, assign the file to this slot via the
    // daemon CLI and close the popover so the user reopens it and
    // sees the freshly-rendered thumbnail. Non-local files (Flatpak
    // sandbox crossings) have no .path() — declined; M6's file
    // picker will cover those via xdg-desktop-portal.
    let drop_target =
        gtk4::DropTarget::new(gtk4::gio::File::static_type(), gtk4::gdk::DragAction::COPY);
    let frame_for_close = frame.clone();
    drop_target.connect_drop(move |_target, value, _x, _y| {
        let file = match value.get::<gtk4::gio::File>() {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("drop value not a File: {e}");
                return false;
            }
        };
        let Some(path) = file.path() else {
            tracing::warn!(
                "dropped file is not local (no path); M6 file picker will cover this case"
            );
            return false;
        };
        tracing::info!("drop on {role}: {}", path.display());
        if let Err(e) = daemon_client::set_for(slot, &path) {
            tracing::warn!("set {role}: {e:#}");
            return false;
        }
        // Refresh the popover in place: rebuild the window's child
        // from the now-updated config. Window stays put, the new
        // thumbnail materialises where the dropped-on frame used to
        // be. Note: this closure holds a clone of the source frame,
        // so the old widget tree (which includes that frame) stays
        // alive until this callback returns — set_child on the
        // window doesn't immediately destroy it.
        if let Some(root) = frame_for_close.root() {
            if let Ok(win) = root.downcast::<gtk4::ApplicationWindow>() {
                populate(&win);
            }
        }
        true
    });
    frame.add_controller(drop_target);

    let inner = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(2)
        .halign(gtk4::Align::Fill)
        .valign(gtk4::Align::Fill)
        .hexpand(true)
        .vexpand(true)
        .margin_top(4)
        .margin_bottom(4)
        .margin_start(4)
        .margin_end(4)
        .build();

    // Thumbnails are pre-computed daemon-side at `spanpaper set` time
    // (see cli.rs::cmd_set), so the palette is now a synchronous PNG
    // load — no spinner, no gio::spawn_blocking, no weak refs. If the
    // pre-compute hadn't run yet (e.g. first-ever open with a config
    // written before this version), `thumbnail::ensure` will run
    // synchronously on the GTK thread once and cache for next time.
    // It's bounded to ~500 ms even in the worst case, and the
    // dropped async machinery saved ~80 LoC.
    let res_text = format!("{}×{}", out.width, out.height);
    match assigned.and_then(|p| match thumbnail::ensure(p) {
        Ok(thumb) => Some(thumb),
        Err(e) => {
            tracing::warn!("thumbnail for {}: {e:#}", p.display());
            None
        }
    }) {
        Some(thumb_path) => {
            let pic = gtk4::Picture::for_filename(&thumb_path);
            pic.set_can_shrink(true);
            pic.set_content_fit(gtk4::ContentFit::Cover);
            pic.set_hexpand(true);
            pic.set_vexpand(true);
            inner.append(&pic);
        }
        None => {
            inner.append(&fallback_label(&res_text));
        }
    }

    let file = gtk4::Label::builder()
        .label(&assigned_text)
        .ellipsize(gtk4::pango::EllipsizeMode::Middle)
        .max_width_chars(18)
        .halign(gtk4::Align::Center)
        .build();
    inner.append(&file);

    frame.set_child(Some(&inner));
    frame
}

fn summary_row(role: &str, slot: Slot, path: Option<&Path>) -> gtk4::Box {
    let row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .build();
    let role_label = gtk4::Label::builder()
        .label(format!("{role}:"))
        .width_request(48)
        .xalign(0.0)
        .build();
    row.append(&role_label);

    let path_text: std::borrow::Cow<'_, str> = match path {
        Some(p) => p.to_string_lossy(),
        None => std::borrow::Cow::Borrowed("(unset)"),
    };
    let path_label = gtk4::Label::builder()
        .label(path_text.as_ref())
        .ellipsize(gtk4::pango::EllipsizeMode::Middle)
        .hexpand(true)
        .xalign(0.0)
        .build();
    row.append(&path_label);

    // Portal-aware file picker via GTK 4.10's FileDialog (wraps
    // xdg-desktop-portal on Wayland automatically). Same end-state as
    // a drop: set the slot, populate the window in place.
    let change = gtk4::Button::builder()
        .label("Change…")
        .tooltip_text(format!(
            "Open a file picker to assign a new image or video to the {role} slot"
        ))
        .build();
    change.connect_clicked(move |btn| {
        open_file_picker_for(btn, slot);
    });
    row.append(&change);
    row
}

fn open_file_picker_for(button: &gtk4::Button, slot: Slot) {
    let role = match slot {
        Slot::Span => "span",
        Slot::Side => "side",
    };

    // Walk up to the window so the dialog has a parent and so we can
    // call `populate()` after the picker resolves.
    let window: Option<gtk4::ApplicationWindow> = button
        .root()
        .and_then(|r| r.downcast::<gtk4::ApplicationWindow>().ok());

    let dialog = gtk4::FileDialog::builder()
        .title(format!("Set spanpaper {role}"))
        .modal(true)
        .build();

    // Filter: same MIME types we list in contrib/spanpaper-set-*.desktop.
    let filter = gtk4::FileFilter::new();
    filter.set_name(Some("Images & videos"));
    for mt in [
        "image/jpeg",
        "image/png",
        "image/webp",
        "image/bmp",
        "image/gif",
        "image/tiff",
        "image/avif",
        "image/heif",
        "image/jxl",
        "video/mp4",
        "video/x-matroska",
        "video/webm",
        "video/quicktime",
        "video/x-msvideo",
        "video/x-ms-wmv",
        "video/x-flv",
        "video/mp2t",
        "video/mpeg",
        "video/ogg",
        "video/3gpp",
        "video/3gpp2",
    ] {
        filter.add_mime_type(mt);
    }
    let filters = gtk4::gio::ListStore::new::<gtk4::FileFilter>();
    filters.append(&filter);
    dialog.set_filters(Some(&filters));
    dialog.set_default_filter(Some(&filter));

    // Suppress focus-out auto-close while the modal picker is open —
    // the palette window will report itself as inactive (its modal
    // child has focus), but we must not close, or we'd destroy the
    // dialog's parent mid-pick.
    let state = window.as_ref().and_then(palette_state);
    if let Some(s) = &state {
        s.suppress_autoclose.set(true);
    }

    // FileDialog::open_future returns a glib future. spawn_future_local
    // runs it on the GTK main loop — no tokio runtime needed on this
    // thread.
    let window_for_set = window.clone();
    let state_for_finish = state.clone();
    gtk4::glib::spawn_future_local(async move {
        let result = dialog.open_future(window_for_set.as_ref()).await;

        // Clear suppression before any further work — once we get here
        // the modal dialog is gone and the palette is the front window
        // again. (Strictly, we should clear AFTER populate() too, but
        // populate is sync and the focus signal can't fire mid-call.)
        if let Some(s) = state_for_finish {
            s.suppress_autoclose.set(false);
        }

        match result {
            Ok(file) => {
                let Some(path) = file.path() else {
                    tracing::warn!("file picker returned non-local file");
                    return;
                };
                tracing::info!("picker assigned {role}: {}", path.display());
                if let Err(e) = daemon_client::set_for(slot, &path) {
                    tracing::warn!("set {role}: {e:#}");
                    return;
                }
                if let Some(win) = window_for_set {
                    populate(&win);
                }
            }
            Err(e) => {
                // User cancelled the dialog → glib::Error with code
                // matching Gtk::DialogError::Dismissed. Nothing to log
                // for cancellation; warn on anything else.
                let msg = e.to_string();
                if !msg.contains("Dismissed") && !msg.contains("cancelled") {
                    tracing::warn!("file picker: {e}");
                }
            }
        }
    });
}

fn fallback_label(text: &str) -> gtk4::Label {
    gtk4::Label::builder()
        .label(text)
        .css_classes(vec!["dim-label"])
        .vexpand(true)
        .valign(gtk4::Align::Center)
        .build()
}

fn basename(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}
