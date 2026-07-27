//! Buttons, the focus ring, and the 1-of-N segmented control.
//!
//! Every button in this design is an outline on transparent. `Visuals` is set
//! up for that in `theme::base_visuals` (the resting `weak_bg_fill` is
//! `TRANSPARENT`), and the helpers here only change which colour the outline,
//! the label and the hover tint are drawn from. Nothing in the kit ever calls
//! `Button::fill`: `Button::fill` applies to every widget state at once, so it
//! would take the hover and pressed feedback with it.
//!
//! This module also owns the two accent accessors the rest of the kit reads
//! from, because `focus_ring` -- the one helper every other file calls -- needs
//! them anyway.

#![allow(dead_code)] // The views adopt the kit in the next commit.

use std::sync::Arc;

use eframe::egui::{
    Button, Color32, Frame, Rect, Response, RichText, Stroke, StrokeKind, Ui, Vec2, WidgetText,
};

use crate::theme::icons;
use crate::theme::tokens::{a500, muted, AMBER, DIVIDER, R_MD, R_SM, S1, STEEL, TEAL, TEXT};
use crate::widgets::rule::{self, HAIRLINE};

// ---------------------------------------------------------------------------
// Tints
//
// Straight from the mock's `color-mix(in srgb, <token> N%, transparent)`.
// `Color32::gamma_multiply` on an opaque token is the same operation: it scales
// the premultiplied channels and the alpha together.
// ---------------------------------------------------------------------------

/// Primary hover: 12% of the accent.
const HOVER_ACCENT: f32 = 0.12;
/// Primary pressed: 22% of the accent -- the same figure `apply_accent` uses
/// for `widgets.active`, so a hand-rolled button matches a kit one.
const PRESS_ACCENT: f32 = 0.22;
/// Secondary hover: 7% of the body colour.
const HOVER_TEXT: f32 = 0.07;
/// Secondary pressed: 14% of the body colour.
const PRESS_TEXT: f32 = 0.14;
/// Ghost hover: 10% of the accent. Lighter than primary because a ghost button
/// has no border to hold the tint in.
const HOVER_GHOST: f32 = 0.10;
/// Ghost pressed: 18% of the accent.
const PRESS_GHOST: f32 = 0.18;

/// Focus ring width. Two points, so it survives a HiDPI downscale.
const FOCUS_W: f32 = 2.0;
/// How far the focus ring sits outside the widget.
const FOCUS_OFFSET: f32 = 2.0;

// ---------------------------------------------------------------------------
// Accent access
// ---------------------------------------------------------------------------

/// The live accent.
///
/// No view is allowed to name an accent. `theme::apply_accent` writes the
/// current one into `Visuals::hyperlink_color`, so reading it back is how a
/// widget tracks accent switches without being handed a `Theme` -- and it
/// cannot go stale, because the style is rebuilt on every switch.
pub(crate) fn accent(ui: &Ui) -> Color32 {
    ui.visuals().hyperlink_color
}

/// The whole ramp behind the live accent.
///
/// `Visuals` has room for exactly one accent colour, so the 100 and 800 steps a
/// tinted badge needs cannot be read back out of the style at all. Recovering
/// the ramp by matching its 500 step is the only way to keep "no view names a
/// colour" and still reach the rest of the scale. It falls back to the default
/// accent rather than panicking, so a `Ui` that was never themed -- a test
/// harness, say -- still renders something sane.
pub(crate) fn accent_ramp(ui: &Ui) -> &'static [Color32; 9] {
    ramp_of(accent(ui))
}

fn ramp_of(accent: Color32) -> &'static [Color32; 9] {
    [&STEEL, &TEAL, &AMBER]
        .into_iter()
        .find(|ramp| a500(ramp) == accent)
        .unwrap_or(&STEEL)
}

// ---------------------------------------------------------------------------
// Focus
// ---------------------------------------------------------------------------

/// Paint the 2px accent focus ring around `response`, if it holds focus.
///
/// egui's `Widgets` has no `focused` slot -- `Response::widget_state` folds
/// focus into `Active` -- so a keyboard-focused control is indistinguishable
/// from a pressed one and, at rest, from an unfocused one. There is no hook to
/// install this globally either, so it has to be called explicitly. Every
/// helper in this file calls it on its own response; views must call it after
/// any interactive widget they assemble by hand.
pub(crate) fn focus_ring(ui: &Ui, response: &Response) {
    if !response.has_focus() {
        return;
    }
    ui.painter().rect_stroke(
        response.rect.expand(FOCUS_OFFSET),
        R_SM,
        Stroke::new(FOCUS_W, accent(ui)),
        StrokeKind::Outside,
    );
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

/// How one button variant differs from the next. Everything else -- the
/// transparent rest state, the corner radius, the padding -- is shared.
struct Skin {
    /// The outline, in every state. `Stroke::NONE` for the borderless variants.
    stroke: Stroke,
    /// What the hover and pressed tints are mixed from.
    tint: Color32,
    hover: f32,
    press: f32,
    /// Horizontal padding override, for the ghost button's tighter box.
    pad_x: Option<f32>,
}

/// Paint `text` in `color`, whichever shape it arrived in.
///
/// Each variant below owns its label colour, and `WidgetText::color` is
/// documented as a no-op on a `LayoutJob` -- which is exactly what an
/// icon-plus-label pairing is, since its two halves need two font families.
/// Without this, `btn_primary(ui, icons::labelled(..))` would keep the body
/// colour the helper built it with instead of taking the accent.
fn painted_in(text: impl Into<WidgetText>, color: Color32) -> WidgetText {
    match text.into() {
        WidgetText::LayoutJob(job) => {
            let mut job = Arc::unwrap_or_clone(job);
            for section in &mut job.sections {
                section.format.color = color;
            }
            WidgetText::LayoutJob(Arc::new(job))
        }
        other => other.color(color),
    }
}

fn skinned(ui: &mut Ui, skin: &Skin, text: WidgetText) -> Response {
    let response = ui
        .scope(|ui| {
            if let Some(pad_x) = skin.pad_x {
                ui.spacing_mut().button_padding.x = pad_x;
            }
            {
                let w = &mut ui.visuals_mut().widgets;
                for state in [&mut w.inactive, &mut w.hovered, &mut w.active] {
                    state.bg_stroke = skin.stroke;
                    state.corner_radius = R_MD;
                }
                w.inactive.weak_bg_fill = Color32::TRANSPARENT;
                w.hovered.weak_bg_fill = skin.tint.gamma_multiply(skin.hover);
                w.active.weak_bg_fill = skin.tint.gamma_multiply(skin.press);
            }
            ui.add(Button::new(text))
        })
        .inner;
    focus_ring(ui, &response);
    response
}

/// The primary action: an accent hairline outline on transparent, with accent
/// text. Never a fill -- a filled button on this ground reads as a slab and
/// pulls more weight than any single action in this tool deserves.
///
/// The label colour is set explicitly because `Visuals::override_text_color` is
/// `Some(TEXT)` globally, which otherwise wins over the widget's `fg_stroke`.
pub(crate) fn btn_primary(ui: &mut Ui, text: impl Into<WidgetText>) -> Response {
    let a = accent(ui);
    let skin = Skin {
        stroke: Stroke::new(HAIRLINE, a),
        tint: a,
        hover: HOVER_ACCENT,
        press: PRESS_ACCENT,
        pad_x: None,
    };
    skinned(ui, &skin, painted_in(text, a))
}

/// The default action: a divider outline and body text. Everything that is not
/// the one thing the panel is for.
pub(crate) fn btn_secondary(ui: &mut Ui, text: impl Into<WidgetText>) -> Response {
    let skin = Skin {
        stroke: Stroke::new(HAIRLINE, DIVIDER),
        tint: TEXT,
        hover: HOVER_TEXT,
        press: PRESS_TEXT,
        pad_x: None,
    };
    skinned(ui, &skin, painted_in(text, TEXT))
}

/// A borderless accent action, padded tight enough to sit inline in a sentence
/// or at the end of a toolbar without reading as a third button.
pub(crate) fn btn_ghost(ui: &mut Ui, text: impl Into<WidgetText>) -> Response {
    let a = accent(ui);
    let skin = Skin {
        stroke: Stroke::NONE,
        tint: a,
        hover: HOVER_GHOST,
        press: PRESS_GHOST,
        pad_x: Some(S1),
    };
    skinned(ui, &skin, painted_in(text, a))
}

/// A square, frameless icon button sized to the current row height, so icon
/// buttons line up with the rows and fields beside them at either density.
///
/// `glyph` is a [`crate::theme::icons`] constant.
pub(crate) fn btn_icon(ui: &mut Ui, glyph: &str) -> Response {
    btn_icon_sized(ui, glyph, ui.spacing().interact_size.y)
}

/// [`btn_icon`] at an explicit side length, for the title bar and the rail,
/// whose heights are fixed rather than density-scaled.
pub(crate) fn btn_icon_sized(ui: &mut Ui, glyph: &str, side: f32) -> Response {
    let response = ui
        .scope(|ui| {
            {
                let w = &mut ui.visuals_mut().widgets;
                for state in [&mut w.inactive, &mut w.hovered, &mut w.active] {
                    state.bg_stroke = Stroke::NONE;
                    state.corner_radius = R_SM;
                }
                w.inactive.weak_bg_fill = Color32::TRANSPARENT;
                w.hovered.weak_bg_fill = TEXT.gamma_multiply(HOVER_TEXT);
                w.active.weak_bg_fill = TEXT.gamma_multiply(PRESS_TEXT);
            }
            ui.spacing_mut().button_padding = Vec2::ZERO;
            ui.add_sized(
                Vec2::splat(side),
                Button::new(icons::glyph(glyph).color(muted(70))).corner_radius(R_SM),
            )
        })
        .inner;
    focus_ring(ui, &response);
    response
}

// ---------------------------------------------------------------------------
// Segmented control
// ---------------------------------------------------------------------------

/// The design's 1-of-N control: one bordered group, a hairline seam between
/// options, and an inset accent ring plus accent text on the selected one.
///
/// Returns true when the selection changed, so a caller can persist or
/// re-request in the same frame.
///
/// It is not built from `Button::selectable`: that routes through
/// `Visuals::selection`, which is the *text* selection colour and is shared
/// with tables and the text cursor. The design wants an outline here, not a
/// fill, and stealing the selection slot for it would tint every selected table
/// row to match.
pub(crate) fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    current: &mut T,
    options: &[(T, &str)],
) -> bool {
    let a = accent(ui);
    let mut picked: Option<T> = None;

    Frame::NONE
        .stroke(Stroke::new(HAIRLINE, DIVIDER))
        .corner_radius(R_MD)
        .show(ui, |ui| {
            // The group border is the only border; the options butt together.
            ui.spacing_mut().item_spacing.x = 0.0;
            {
                let w = &mut ui.visuals_mut().widgets;
                for state in [&mut w.inactive, &mut w.hovered, &mut w.active] {
                    state.bg_stroke = Stroke::NONE;
                    state.corner_radius = R_SM;
                }
                w.inactive.weak_bg_fill = Color32::TRANSPARENT;
                w.hovered.weak_bg_fill = TEXT.gamma_multiply(HOVER_TEXT);
                w.active.weak_bg_fill = TEXT.gamma_multiply(PRESS_TEXT);
            }

            ui.horizontal(|ui| {
                for (index, &(value, label)) in options.iter().enumerate() {
                    let selected = value == *current;
                    let color = if selected { a } else { TEXT };
                    let response = ui.add(Button::new(RichText::new(label).color(color)));

                    if response.clicked() {
                        picked = Some(value);
                    }
                    if selected {
                        ui.painter().rect_stroke(
                            response.rect,
                            R_SM,
                            Stroke::new(HAIRLINE, a),
                            StrokeKind::Inside,
                        );
                    }
                    if index > 0 {
                        // In-control separators stay solid; see widgets::rule.
                        let seam = Rect::from_min_max(
                            response.rect.left_top(),
                            response.rect.left_bottom() + Vec2::new(HAIRLINE, 0.0),
                        );
                        rule::solid_vline(ui.painter(), seam, DIVIDER);
                    }
                    focus_ring(ui, &response);
                }
            });
        });

    match picked {
        Some(value) if value != *current => {
            *current = value;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reverse lookup is what lets a badge reach the 100 and 800 steps of
    /// whichever accent is live. If it ever stopped round-tripping, every
    /// tinted chip would quietly fall back to steel on the teal and amber
    /// themes -- which looks like a missing token, not like a bug here.
    #[test]
    fn every_accent_round_trips_through_its_500_step() {
        for ramp in [&STEEL, &TEAL, &AMBER] {
            assert!(
                std::ptr::eq(ramp_of(a500(ramp)), ramp),
                "{:?} did not round-trip",
                a500(ramp)
            );
        }
    }

    /// An unthemed or half-themed `Ui` must still render. Panicking here would
    /// take out the whole frame over a colour lookup.
    #[test]
    fn an_unknown_accent_falls_back_to_the_default() {
        assert!(std::ptr::eq(ramp_of(Color32::TRANSPARENT), &STEEL));
    }
}
