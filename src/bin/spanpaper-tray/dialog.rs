//! Small one-off GTK4 windows the tray spawns from menu actions.
//!
//! Currently just the "Save current as…" preset-name prompt; if more
//! lightweight dialogs land, they can live alongside.

use crate::daemon_client;
use gtk4::prelude::*;
use gtk4_layer_shell::LayerShell;

/// Show a small layer-shell-anchored window with a single text entry +
/// OK/Cancel buttons. On OK, shells out `spanpaper preset save NAME`
/// and closes; on Cancel or Escape, closes silently.
///
/// `click_x` / `click_y` are the tray-click coordinates the dialog
/// anchors near (same trick the palette uses). Pass `(-1, -1)` to fall
/// back to compositor placement.
pub fn save_preset(app: &gtk4::Application, click_x: i32, click_y: i32) {
    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .title("Save preset")
        .resizable(false)
        .default_width(320)
        .build();
    window.set_widget_name("spanpaper-save-preset");

    if click_x >= 0 && click_y >= 0 {
        window.init_layer_shell();
        window.set_layer(gtk4_layer_shell::Layer::Top);
        window.set_anchor(gtk4_layer_shell::Edge::Top, true);
        window.set_anchor(gtk4_layer_shell::Edge::Left, true);
        window.set_margin(
            gtk4_layer_shell::Edge::Top,
            click_y.max(0).saturating_add(4),
        );
        window.set_margin(
            gtk4_layer_shell::Edge::Left,
            click_x.max(0).saturating_add(4),
        );
        // Need keyboard input — set to Exclusive so the GtkEntry
        // gets keystrokes regardless of compositor focus rules.
        // (OnDemand can be flaky for transient layer-shell windows
        // that the user clicks into.)
        window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::Exclusive);
    }

    let outer = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let prompt = gtk4::Label::builder()
        .label("Name this preset:")
        .xalign(0.0)
        .build();
    outer.append(&prompt);

    let entry = gtk4::Entry::builder()
        .placeholder_text("e.g. nature-still")
        .hexpand(true)
        .build();
    outer.append(&entry);

    let buttons = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk4::Align::End)
        .build();

    let cancel = gtk4::Button::builder().label("Cancel").build();
    let ok = gtk4::Button::builder()
        .label("Save")
        .css_classes(vec!["suggested-action"])
        .build();
    buttons.append(&cancel);
    buttons.append(&ok);
    outer.append(&buttons);

    window.set_child(Some(&outer));

    // Submit closure — shared between OK button click, Entry activate
    // (Enter key), and any future hot-key wiring.
    let win_for_ok = window.clone();
    let entry_for_ok = entry.clone();
    let submit = std::rc::Rc::new(move || {
        let name = entry_for_ok.text().trim().to_string();
        if name.is_empty() {
            entry_for_ok.add_css_class("error");
            return;
        }
        if let Err(e) = daemon_client::preset_save(&name) {
            tracing::warn!("preset save {name:?}: {e:#}");
            entry_for_ok.add_css_class("error");
            return;
        }
        tracing::info!("saved preset {name:?}");
        win_for_ok.close();
    });

    let submit_for_ok = submit.clone();
    ok.connect_clicked(move |_| submit_for_ok());
    let submit_for_entry = submit.clone();
    entry.connect_activate(move |_| submit_for_entry());

    let win_for_cancel = window.clone();
    cancel.connect_clicked(move |_| win_for_cancel.close());

    // Escape-to-cancel.
    let win_for_esc = window.clone();
    let key_controller = gtk4::EventControllerKey::new();
    key_controller.connect_key_pressed(move |_, key, _, _| {
        if key == gtk4::gdk::Key::Escape {
            win_for_esc.close();
            return gtk4::glib::Propagation::Stop;
        }
        gtk4::glib::Propagation::Proceed
    });
    window.add_controller(key_controller);

    window.present();
    entry.grab_focus();
}
