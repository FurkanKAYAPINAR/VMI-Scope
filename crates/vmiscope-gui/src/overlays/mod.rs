//! Panels and windows that float above the active tab.

use eframe::egui::{Button, Response, Stroke, Ui, WidgetText};

use crate::theme::tokens::{BAD, R_MD};
use crate::widgets::button::focus_ring;
use crate::widgets::rule::HAIRLINE;

pub(crate) mod actions;
pub(crate) mod confirm;
pub(crate) mod errorlog;
pub(crate) mod mof;
pub(crate) mod save_query;

// ---------------------------------------------------------------------------
// The destructive-action button
//
// This is the one button in the app that is filled, and deliberately so. Every
// other control in the kit is an outline on transparent, which is what makes an
// outline the *unremarkable* shape here -- so rendering "Invoke Terminate" as
// one would put a system-changing call at the same visual weight as "CSV". The
// two call sites (the Actions panel's Invoke, the confirm dialog's Yes) are the
// only ones that reach for it.
//
// It lives here rather than in `widgets/button.rs` because a danger button that
// any view can reach for stops being a gate: the kit deliberately offers no way
// to make an ordinary action look alarming, and this file is the boundary the
// two gates already sit behind.
//
// The tints are fractions of `BAD` rather than fresh colours, so the button
// tracks the status palette and never states an RGB triple of its own. They are
// heavier than the accent tints in `widgets::button` because this shape has to
// read as filled at rest, not merely on hover.
// ---------------------------------------------------------------------------

/// Resting fill: enough tint to read as a filled button on the dark ground.
const REST: f32 = 0.16;
/// Hover fill.
const HOVER: f32 = 0.28;
/// Pressed fill.
const PRESS: f32 = 0.40;

/// A filled, `BAD`-tinted button for an action that changes the system.
///
/// `text` arrives already coloured -- an icon-plus-label pairing is a
/// `LayoutJob`, and `WidgetText::color` is documented as a no-op on one, so the
/// colour has to be chosen where the job is built.
pub(crate) fn btn_danger(ui: &mut Ui, text: impl Into<WidgetText>) -> Response {
    let response = ui
        .scope(|ui| {
            {
                let w = &mut ui.visuals_mut().widgets;
                for state in [&mut w.inactive, &mut w.hovered, &mut w.active] {
                    state.bg_stroke = Stroke::new(HAIRLINE, BAD);
                    state.corner_radius = R_MD;
                }
                w.inactive.weak_bg_fill = BAD.gamma_multiply(REST);
                w.hovered.weak_bg_fill = BAD.gamma_multiply(HOVER);
                w.active.weak_bg_fill = BAD.gamma_multiply(PRESS);
            }
            ui.add(Button::new(text))
        })
        .inner;
    focus_ring(ui, &response);
    response
}
