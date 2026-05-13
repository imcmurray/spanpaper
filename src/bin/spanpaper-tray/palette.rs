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
    thumbnail,
};
use gtk4::prelude::*;
use serde::Deserialize;
use std::path::Path;

/// Subset of the daemon's config we render. Extra fields in the TOML
/// are ignored (serde default behavior), so this stays compatible
/// across daemon config-schema additions.
#[derive(Debug, Default, Deserialize)]
struct PartialConfig {
    span: Option<String>,
    side: Option<String>,
    #[serde(default)]
    span_outputs: Vec<String>,
    side_output: Option<String>,
    #[serde(default)]
    span_direction: Option<String>,
}

impl PartialConfig {
    fn load() -> Self {
        let Some(home) = dirs::config_dir() else { return Self::default() };
        let path = home.join("spanpaper").join("config.toml");
        let Ok(text) = std::fs::read_to_string(&path) else { return Self::default() };
        toml::from_str(&text).unwrap_or_default()
    }
}

/// Max pixel size of the longest monitor edge in the popover. The other
/// edge scales proportionally. Picked by eye — small enough to keep the
/// window compact, big enough to show a meaningful preview.
const MAX_EDGE_PX: i32 = 220;

/// CSS widget-name tag we set on the palette window so subsequent
/// `show()` calls can locate it via `Application::windows()` and raise
/// the existing instance instead of stacking up duplicates.
const PALETTE_WIDGET_NAME: &str = "spanpaper-palette";

pub fn show(app: &gtk4::Application) {
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

    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .title("spanpaper")
        .resizable(false)
        .default_width(480)
        .build();
    window.set_widget_name(PALETTE_WIDGET_NAME);
    populate(&window);
    window.present();
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
    let cfg = PartialConfig::load();

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
fn build_layout_row(outputs: &[OutputInfo], cfg: &PartialConfig) -> gtk4::Box {
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
        let dir = if cfg.span_direction.as_deref() == Some("horizontal") {
            gtk4::Orientation::Horizontal
        } else {
            gtk4::Orientation::Vertical
        };
        let group = gtk4::Box::builder()
            .orientation(dir)
            .spacing(2)
            .build();
        for o in &span_outs {
            group.append(&build_output_frame(o, cfg.span.as_deref(), scale, Slot::Span));
        }
        let leftmost_x = span_outs.iter().map(|o| o.x).min().unwrap_or(0);
        (leftmost_x, group)
    });
    let side_frame = side_out.map(|o| {
        (o.x, build_output_frame(o, cfg.side.as_deref(), scale, Slot::Side))
    });

    let mut placed: Vec<(i32, gtk4::Widget)> = Vec::new();
    if let Some((x, w)) = span_box { placed.push((x, w.upcast::<gtk4::Widget>())); }
    if let Some((x, w)) = side_frame { placed.push((x, w.upcast::<gtk4::Widget>())); }
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
    assigned: Option<&str>,
    scale: f32,
    slot: Slot,
) -> gtk4::Frame {
    let role = match slot {
        Slot::Span => "span",
        Slot::Side => "side",
    };
    let w = ((out.width as f32) * scale).round().max(40.0) as i32;
    let h = ((out.height as f32) * scale).round().max(40.0) as i32;

    let frame = gtk4::Frame::builder()
        .label(&format!("{}  ({})", out.name, role))
        .label_xalign(0.5)
        .width_request(w)
        .height_request(h)
        .build();

    // Drop target: accept any gio::File (local files) dropped onto
    // this frame. On drop, assign the file to this slot via the
    // daemon CLI and close the popover so the user reopens it and
    // sees the freshly-rendered thumbnail. Non-local files (Flatpak
    // sandbox crossings) have no .path() — declined; M6's file
    // picker will cover those via xdg-desktop-portal.
    let drop_target = gtk4::DropTarget::new(
        gtk4::gio::File::static_type(),
        gtk4::gdk::DragAction::COPY,
    );
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

    // Try the thumbnail first; on any failure fall back to a
    // resolution-text placeholder so the popover always renders.
    let picture_packed = match assigned {
        Some(p) => match thumbnail::ensure(Path::new(p)) {
            Ok(thumb) => {
                let pic = gtk4::Picture::for_filename(&thumb);
                pic.set_can_shrink(true);
                pic.set_content_fit(gtk4::ContentFit::Cover);
                pic.set_hexpand(true);
                pic.set_vexpand(true);
                inner.append(&pic);
                true
            }
            Err(e) => {
                tracing::warn!("thumbnail for {p}: {e:#}");
                false
            }
        },
        None => false,
    };
    if !picture_packed {
        let res = gtk4::Label::builder()
            .label(&format!("{}×{}", out.width, out.height))
            .css_classes(vec!["dim-label"])
            .vexpand(true)
            .valign(gtk4::Align::Center)
            .build();
        inner.append(&res);
    }

    let file_label = match assigned {
        Some(p) => basename(Path::new(p)),
        None => "(unset)".into(),
    };
    let file = gtk4::Label::builder()
        .label(&file_label)
        .ellipsize(gtk4::pango::EllipsizeMode::Middle)
        .max_width_chars(18)
        .halign(gtk4::Align::Center)
        .build();
    inner.append(&file);

    frame.set_child(Some(&inner));
    frame
}

fn summary_row(role: &str, slot: Slot, path: Option<&str>) -> gtk4::Box {
    let row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .build();
    let role_label = gtk4::Label::builder()
        .label(&format!("{role}:"))
        .width_request(48)
        .xalign(0.0)
        .build();
    row.append(&role_label);

    let path_label = gtk4::Label::builder()
        .label(path.unwrap_or("(unset)"))
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
        .tooltip_text("Pick an image or video to assign to this slot")
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
        .title(&format!("Set spanpaper {role}"))
        .modal(true)
        .build();

    // Filter: same MIME types we list in contrib/spanpaper-set-*.desktop.
    let filter = gtk4::FileFilter::new();
    filter.set_name(Some("Images & videos"));
    for mt in [
        "image/jpeg", "image/png", "image/webp", "image/bmp", "image/gif",
        "image/tiff", "image/avif", "image/heif", "image/jxl",
        "video/mp4", "video/x-matroska", "video/webm", "video/quicktime",
        "video/x-msvideo", "video/x-ms-wmv", "video/x-flv", "video/mp2t",
        "video/mpeg", "video/ogg", "video/3gpp", "video/3gpp2",
    ] {
        filter.add_mime_type(mt);
    }
    let filters = gtk4::gio::ListStore::new::<gtk4::FileFilter>();
    filters.append(&filter);
    dialog.set_filters(Some(&filters));
    dialog.set_default_filter(Some(&filter));

    // FileDialog::open_future returns a glib future. spawn_future_local
    // runs it on the GTK main loop — no tokio runtime needed on this
    // thread.
    let window_for_set = window.clone();
    gtk4::glib::spawn_future_local(async move {
        match dialog.open_future(window.as_ref()).await {
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

fn basename(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

