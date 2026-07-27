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

/// Floor for the key column.
///
/// `Grid` defaults `min_col_width` to `spacing.interact_size.x`, which this
/// theme sets to 0 so buttons size to their content. A wrapping label handed
/// zero available width wraps to one character per line, and the grid then
/// records *that* as the column width -- so every key rendered as a vertical
/// column of letters. Naming a floor here fixes it for every call site at once.
const KEY_MIN_W: f32 = 96.0;

/// A key/value grid. Keys are muted, values are monospace and selectable so
/// they can be copied out.
pub(crate) fn kv_grid<'a>(
    ui: &mut Ui,
    id: &str,
    rows: impl IntoIterator<Item = (&'a str, &'a str)>,
) {
    kv_grid_sized(ui, id, KEY_MIN_W, rows);
}

/// [`kv_grid`] with the key column's floor spelled out.
///
/// Worth setting where the keys are known to be long -- WMI property names in
/// a row-detail pane run past the default -- so the column does not have to be
/// discovered from the first row.
pub(crate) fn kv_grid_sized<'a>(
    ui: &mut Ui,
    id: &str,
    min_key_width: f32,
    rows: impl IntoIterator<Item = (&'a str, &'a str)>,
) {
    Grid::new(id)
        .num_columns(2)
        .min_col_width(min_key_width)
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
