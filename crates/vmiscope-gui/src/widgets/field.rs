//! Inputs: text fields, the filter box, the Settings row, radios and combos.
//!
//! The text fields are deliberately monospace. Almost everything this tool
//! takes as input is a path, a class name or a WQL fragment, and proportional
//! digits in a namespace path make two similar strings hard to tell apart.

#![allow(dead_code)] // The views adopt the kit in the next commit.

use eframe::egui::{
    Align, ComboBox, Layout, Pos2, Rect, Response, RichText, Sense, Stroke, TextEdit, TextStyle,
    Ui, Vec2,
};

use crate::theme::icons;
use crate::theme::tokens::{muted, BG, DIVIDER, S2, SURFACE, TEXT};
use crate::widgets::button::{accent, focus_ring};
use crate::widgets::rule::{solid_hline, HAIRLINE};

/// The Settings row's key size. It sits between `Body` (13) and `Button` (12),
/// which is why it is a literal and not a text style: the row is the only place
/// in the design that uses it.
const KEY_SIZE: f32 = 12.5;

/// Strength of the hairline under a settings row -- 6% of the body colour, the
/// lightest rule in the system.
const ROW_RULE: u8 = 6;

/// Radio dot diameter, and the ring and core that make up the checked state.
const RADIO_D: f32 = 16.0;
const RADIO_RING: f32 = 1.5;
const RADIO_CORE: f32 = 2.5;

/// A single-line monospace field on the surface colour.
///
/// The border comes from `widgets.inactive.bg_stroke` and turns accent on focus
/// through `widgets.active.bg_stroke`, both of which `theme::install` already
/// sets -- so nothing here restates them.
pub(crate) fn mono_input(ui: &mut Ui, value: &mut String, hint: &str) -> Response {
    let response = ui.add(
        TextEdit::singleline(value)
            .font(TextStyle::Monospace)
            .hint_text(RichText::new(hint).color(muted(38)))
            .background_color(SURFACE),
    );
    focus_ring(ui, &response);
    response
}

/// The filter box that heads every list in the app: a leading magnifier, a
/// monospace body, and a hint instead of a label.
///
/// The icon is a `prefix` atom rather than a separate `ui.label`, so it lives
/// inside the field's frame and the whole box -- glyph included -- is one click
/// target and one focus stop.
pub(crate) fn filter_box(ui: &mut Ui, value: &mut String, hint: &str) -> Response {
    let response = ui.add(
        TextEdit::singleline(value)
            .font(TextStyle::Monospace)
            .prefix(icons::glyph(icons::MAGNIFYING_GLASS).color(muted(45)))
            .hint_text(RichText::new(hint).color(muted(38)))
            .background_color(SURFACE),
    );
    focus_ring(ui, &response);
    response
}

/// The Settings row: a key over a smaller muted note on the left, whatever the
/// caller wants on the right, and a hairline underneath.
///
/// The rule is solid, not faded. Freestanding rules and table row rules fade;
/// this one closes a list item, and a list of settings whose separators all
/// taper looks like a rendering fault rather than a flourish.
pub(crate) fn labelled_row<R>(
    ui: &mut Ui,
    key: &str,
    note: &str,
    value_ui: impl FnOnce(&mut Ui) -> R,
) -> R {
    let mut out = None;
    let row = ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(key).size(KEY_SIZE));
            ui.label(
                RichText::new(note)
                    .text_style(TextStyle::Name("caption".into()))
                    .color(muted(42)),
            );
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            out = Some(value_ui(ui));
        });
    });

    let rect = row.response.rect;
    solid_hline(
        ui.painter(),
        Rect::from_min_size(
            Pos2::new(rect.left(), rect.bottom()),
            Vec2::new(rect.width(), HAIRLINE),
        ),
        muted(ROW_RULE),
    );

    // `ui.horizontal` runs its closure exactly once, so this is infallible; the
    // `Option` only exists because the closure cannot return through the
    // borrow.
    out.expect("labelled_row closure did not run")
}

/// A vertical 1-of-N radio group. Returns true when the selection changed.
///
/// The dot is painted rather than assembled from `ui.radio`, because egui's
/// radio draws a filled disc from `widgets.*.bg_fill` and the design wants an
/// accent ring with a gap and a small core -- three concentric shapes that no
/// single `WidgetVisuals` can express.
pub(crate) fn radio_group<T: PartialEq + Copy>(
    ui: &mut Ui,
    current: &mut T,
    options: &[(T, &str)],
) -> bool {
    let mut picked = None;
    for &(value, label) in options {
        if radio_row(ui, value == *current, label).clicked() {
            picked = Some(value);
        }
    }
    match picked {
        Some(value) if value != *current => {
            *current = value;
            true
        }
        _ => false,
    }
}

fn radio_row(ui: &mut Ui, selected: bool, label: &str) -> Response {
    let a = accent(ui);
    let font = TextStyle::Body.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, TEXT);

    let size = Vec2::new(
        RADIO_D + S2 + galley.size().x,
        galley
            .size()
            .y
            .max(RADIO_D)
            .max(ui.spacing().interact_size.y),
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    if ui.is_rect_visible(rect) {
        let centre = Pos2::new(rect.left() + RADIO_D * 0.5, rect.center().y);
        let rim = if selected || response.hovered() {
            a
        } else {
            DIVIDER
        };
        let painter = ui.painter();
        painter.circle_stroke(
            centre,
            (RADIO_D - RADIO_RING) * 0.5,
            Stroke::new(RADIO_RING, rim),
        );
        if selected {
            // The mock's `inset 0 0 0 4px var(--color-bg)` -- a gap punched out
            // of the ground colour between the rim and the core.
            painter.circle_filled(centre, RADIO_CORE + RADIO_RING, BG);
            painter.circle_filled(centre, RADIO_CORE, a);
        }
        painter.galley(
            Pos2::new(
                rect.left() + RADIO_D + S2,
                rect.center().y - galley.size().y * 0.5,
            ),
            galley,
            TEXT,
        );
    }

    focus_ring(ui, &response);
    response
}

/// A drop-down over a fixed option list. Returns true when the selection
/// changed.
///
/// `id_salt` has to be stable and unique per call site: `ComboBox` stores its
/// open/closed state under it, so two combos sharing a salt open together.
pub(crate) fn combo<T: PartialEq + Copy>(
    ui: &mut Ui,
    id_salt: &str,
    current: &mut T,
    options: &[(T, &str)],
) -> bool {
    let selected = options
        .iter()
        .find(|(value, _)| value == current)
        .map_or("", |(_, label)| *label);

    let mut changed = false;
    let inner = ComboBox::from_id_salt(id_salt)
        .selected_text(RichText::new(selected))
        .show_ui(ui, |ui| {
            for &(value, label) in options {
                let picked = ui.selectable_label(value == *current, label).clicked();
                if picked && value != *current {
                    *current = value;
                    changed = true;
                }
            }
        });

    focus_ring(ui, &inner.response);
    changed
}
