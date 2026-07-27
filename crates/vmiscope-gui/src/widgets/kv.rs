//! The key/value grid.
//!
//! Four places in the app used to hand-roll this shape with `egui::Grid`, each
//! with slightly different spacing and a different idea of what an empty value
//! looks like. One implementation, so a row-detail pane and a method result
//! read the same way.

#![allow(dead_code)] // The views adopt the kit in the next commit.

use eframe::egui::{Grid, Label, RichText, TextStyle, Ui};

use crate::theme::tokens::muted;

/// Rendered in place of a value that is present but empty.
///
/// An empty cell is ambiguous -- it could mean the property is absent, or NULL,
/// or the empty string -- and in a tool people read to draw conclusions from,
/// that ambiguity is the whole problem. An em dash at least says "we looked".
pub(crate) const EMPTY: &str = "\u{2014}";

/// A key/value grid. Keys are muted, values are monospace and selectable so
/// they can be copied out.
pub(crate) fn kv_grid<'a>(
    ui: &mut Ui,
    id: &str,
    rows: impl IntoIterator<Item = (&'a str, &'a str)>,
) {
    Grid::new(id)
        .num_columns(2)
        .spacing([
            ui.spacing().item_spacing.x * 2.0,
            ui.spacing().item_spacing.y,
        ])
        .show(ui, |ui| {
            for (key, value) in rows {
                ui.add(Label::new(RichText::new(key).color(muted(60))).wrap());
                value_cell(ui, value);
                ui.end_row();
            }
        });
}

/// One monospace, selectable value cell with the empty-value convention applied.
pub(crate) fn value_cell(ui: &mut Ui, value: &str) {
    if value.is_empty() {
        ui.add(Label::new(RichText::new(EMPTY).color(muted(35))));
    } else {
        ui.add(
            Label::new(RichText::new(value).text_style(TextStyle::Monospace))
                .wrap()
                .selectable(true),
        );
    }
}
