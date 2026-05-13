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

pub fn show(app: &gtk4::Application) {
    let outputs = match crate::outputs_query::list() {
        Ok(v) => v,
        Err(e) => {
            // Render an error window instead of silently failing — the
            // user clicked the icon expecting something to happen.
            present_error(app, &format!("Could not enumerate outputs:\n{e:#}"));
            return;
        }
    };
    let cfg = PartialConfig::load();

    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .title("spanpaper")
        .resizable(false)
        .default_width(480)
        .build();

    let root = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let layout_row = build_layout_row(&outputs, &cfg);
    root.append(&layout_row);

    root.append(&summary_row("span", cfg.span.as_deref()));
    root.append(&summary_row("side", cfg.side.as_deref()));

    let hint = gtk4::Label::builder()
        .label("Drop an image or video onto a box to assign it")
        .css_classes(vec!["dim-label"])
        .build();
    hint.set_xalign(0.0);
    root.append(&hint);

    window.set_child(Some(&root));
    window.present();
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
        // Close the popover. The user reopens (left-click the tray)
        // to see refreshed thumbnails. In-place refresh is a future
        // polish item.
        if let Some(root) = frame_for_close.root() {
            if let Ok(win) = root.downcast::<gtk4::Window>() {
                win.close();
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

fn summary_row(role: &str, path: Option<&str>) -> gtk4::Box {
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

    // Disabled in M3 — M6 wires this to the xdg-desktop-portal picker.
    let change = gtk4::Button::builder()
        .label("Change…")
        .sensitive(false)
        .tooltip_text("File picker lands in M6")
        .build();
    row.append(&change);
    row
}

fn basename(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

fn present_error(app: &gtk4::Application, msg: &str) {
    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .title("spanpaper")
        .resizable(false)
        .build();
    let label = gtk4::Label::builder()
        .label(msg)
        .margin_top(20)
        .margin_bottom(20)
        .margin_start(20)
        .margin_end(20)
        .selectable(true)
        .build();
    window.set_child(Some(&label));
    window.present();
}

