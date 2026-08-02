//! The keyboard map (F1).
//!
//! Every key the application binds, on one screen. It is **generated** from
//! [`palette::BINDINGS`] and [`palette::MODAL_KEYS`] -- the same tables
//! `handle_shortcuts` dispatches from and the palette footer advertises -- so
//! it cannot drift from what the keys actually do. That is the whole design
//! requirement (task 7.5): a hand-maintained cheat sheet is a document that
//! starts wrong the first time someone adds a binding and never gets fixed,
//! because nothing fails when it does.
//!
//! The chord text is `Context::format_shortcut`, so it prints the platform's
//! own modifier names rather than a hardcoded "Ctrl".

use eframe::egui::{self, RichText, TextStyle, Ui};

use crate::app::VmiScopeApp;
use crate::overlays::palette::{KeyAction, Scope, BINDINGS, MODAL_KEYS};
use crate::theme::icons;
use crate::theme::tokens::{muted, S1, S2, S4, TEXT};
use crate::widgets::button::btn_secondary;
use crate::widgets::rule::hrule;

/// The chord that opens this window, looked up in the same table it renders.
///
/// By `Action`, not by scanning the descriptions: a lookup that matches on
/// prose would break the moment the wording changed, which is the exact class
/// of drift this whole overlay exists to avoid.
fn own_shortcut() -> Option<egui::KeyboardShortcut> {
    BINDINGS
        .iter()
        .find(|b| b.action == KeyAction::ToggleKeyboardMap)
        .map(|b| b.shortcut)
}

/// Width of the key column. Wide enough for the longest chord the table can
/// produce (`Ctrl+Enter`) plus the monospace padding, so the descriptions line
/// up into a readable second column instead of ragging.
const KEY_W: f32 = 108.0;

/// The window's resting size. Tall enough for both groups without scrolling at
/// the current binding count; it scrolls if that changes.
const SIZE: [f32; 2] = [460.0, 380.0];

impl VmiScopeApp {
    pub(crate) fn ui_keymap_window(&mut self, ctx: &egui::Context) {
        if !self.keymap_open {
            return;
        }
        let mut open = true;
        egui::Window::new("Keyboard")
            .open(&mut open)
            .default_size(SIZE)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, keymap_body);
            });
        if !open {
            self.keymap_open = false;
        }
    }
}

/// The body, free of `self` so it can be handed straight to the scroll area.
fn keymap_body(ui: &mut Ui) {
    group(ui, icons::KEY, "Global", "Everywhere in the application.");
    for binding in BINDINGS {
        let chord = ui.ctx().format_shortcut(&binding.shortcut);
        // A key that stands down inside a text field is a real limit and worth
        // saying: "Escape does nothing" is otherwise indistinguishable from a
        // broken binding to whoever hit it while typing a WQL string.
        let note = match binding.scope {
            Scope::Always => None,
            Scope::OutsideText => Some("not while typing in a field"),
        };
        row(ui, &chord, binding.description, note);
    }

    ui.add_space(S4);
    group(
        ui,
        icons::LIGHTNING,
        "Command palette",
        "While the palette is open. It consumes these itself, before its input \
         field can claim them.",
    );
    for (chord, what) in MODAL_KEYS {
        row(ui, chord, what, None);
    }

    ui.add_space(S4);
    ui.label(
        RichText::new(
            "This list is generated from the binding table the application dispatches from, \
             so it is complete by construction.",
        )
        .text_style(TextStyle::Name("caption".into()))
        .color(muted(38)),
    );
}

/// A group heading and its one-line explanation.
fn group(ui: &mut Ui, icon: &str, title: &str, note: &str) {
    ui.label(icons::labelled_styled(
        ui,
        icon,
        title,
        TextStyle::Body,
        TEXT,
    ));
    ui.label(
        RichText::new(note)
            .text_style(TextStyle::Name("caption".into()))
            .color(muted(42)),
    );
    ui.add_space(S1);
    hrule(ui);
}

/// One key and what it does.
///
/// The note goes *under* the description rather than right-aligned beside it.
/// It was beside it, and the capture showed the two overlapping: a
/// right-to-left sub-layout starts from whatever the description left, which on
/// a 460px window is not enough, and egui overlaps rather than clipping. A
/// second line always fits.
fn row(ui: &mut Ui, chord: &str, what: &str, note: Option<&str>) {
    ui.horizontal_top(|ui| {
        ui.scope(|ui| {
            ui.set_min_width(KEY_W);
            // Monospace, like every other literal value in the tool: a chord is
            // something you type, not prose about it.
            ui.label(
                RichText::new(chord)
                    .text_style(TextStyle::Name("code".into()))
                    .color(ui.visuals().hyperlink_color),
            );
        });
        ui.vertical(|ui| {
            ui.label(RichText::new(what).color(muted(85)));
            if let Some(note) = note {
                ui.label(
                    RichText::new(note)
                        .text_style(TextStyle::Small)
                        .color(muted(35)),
                );
            }
        });
    });
    ui.add_space(S2);
}

/// The button that opens this window, for the status bar.
///
/// Lives here rather than in `shell::statusbar` so the one control that opens
/// the map sits next to the map: a shortcut nobody can discover is a shortcut
/// nobody has.
pub(crate) fn open_button(app: &mut VmiScopeApp, ui: &mut Ui) {
    // No fallback chord if the lookup misses: the button would then advertise a
    // key that does not exist. It simply does not draw, and the unit test below
    // is what stops that being silent.
    let Some(shortcut) = own_shortcut() else {
        return;
    };
    let label = ui.ctx().format_shortcut(&shortcut);
    if btn_secondary(ui, format!("{label} keys"))
        .on_hover_text("Every keyboard shortcut in the application")
        .clicked()
    {
        app.keymap_open = !app.keymap_open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The map has to be openable from the shell, not only by knowing the key
    /// already -- and `open_button` silently draws nothing if this is missing.
    #[test]
    fn the_map_has_a_key_of_its_own() {
        assert!(
            own_shortcut().is_some(),
            "nothing in BINDINGS opens the keyboard map"
        );
    }

    /// Two rows with the same chord would be one row the reader distrusts. The
    /// palette's own tests pin uniqueness within `BINDINGS`; this one covers the
    /// seam between the two groups the map prints together.
    #[test]
    fn the_two_groups_do_not_advertise_the_same_chord_differently() {
        // Escape appears in both, deliberately -- globally it closes the
        // frontmost overlay, and inside the palette the palette itself takes
        // it. The groups are headed separately for exactly that reason, so what
        // must not happen is the *same* description appearing twice.
        for (_, palette_what) in MODAL_KEYS {
            assert!(
                !BINDINGS.iter().any(|b| b.description == *palette_what),
                "{palette_what:?} is described identically in both groups"
            );
        }
    }
}
