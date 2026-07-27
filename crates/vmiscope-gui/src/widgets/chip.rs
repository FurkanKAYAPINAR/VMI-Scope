//! Chips: tags, count pills, kind badges and status dots.
//!
//! All of them are the same shape at different tints -- a short run of text in
//! a small rounded box -- so they share one builder and differ only in fill,
//! stroke and text colour. None of them is interactive; a chip that can be
//! clicked is a `btn_ghost` with a chip skin, not a chip.

#![allow(dead_code)] // The views adopt the kit in the next commit.

use eframe::egui::{
    Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Margin, Response, RichText, Sense,
    Stroke, TextStyle, Ui, Vec2,
};

use crate::theme::tokens::{a100, a300, a800, muted, NEUTRAL, R_SM, S1, S2, S3, TEXT};
use crate::widgets::button::{accent, accent_ramp};
use crate::widgets::rule::HAIRLINE;

/// Chip corner radius. The mock asks for `calc(var(--radius-md) * 0.75)`, which
/// is 6 -- between `R_SM` and `R_MD`, and deliberately so: a chip rounded to
/// `R_MD` reads as a small card rather than as a label.
const R_CHIP: CornerRadius = CornerRadius::same(6);

/// The kind badge is a fixed 15px square, because it leads every row of the
/// class list and the column has to stay the same width whatever the letter is.
const BADGE_SIDE: f32 = 15.0;
/// The badge's own radius -- tighter than `R_SM`, or the square reads as a dot.
const R_BADGE: CornerRadius = CornerRadius::same(3);
/// One letter in a 15px box needs its own size; the smallest text style (11) is
/// too big to sit inside the square with air around it.
const BADGE_FONT: f32 = 9.0;

/// Diameter of a status dot.
const DOT: f32 = 6.0;

/// Background strength for the untinted chips -- count pills and the like.
const CHIP_TINT: f32 = 0.06;

/// The shared chip body. `Frame` gives us fill, stroke, radius and padding in
/// one shape, which is exactly a chip.
fn chip(ui: &mut Ui, text: RichText, fill: Color32, stroke: Stroke) -> Response {
    Frame::NONE
        .fill(fill)
        .stroke(stroke)
        .corner_radius(R_CHIP)
        // `Margin` is four i8, so the fractional density steps round here.
        .inner_margin(Margin::symmetric(S3.round() as i8, S1.round() as i8))
        .show(ui, |ui| {
            ui.label(text.text_style(TextStyle::Small));
        })
        .response
}

/// The plain tag: chip metrics, no tint. For inline metadata that should read
/// as a label rather than as a status.
pub(crate) fn tag(ui: &mut Ui, text: &str) -> Response {
    chip(
        ui,
        RichText::new(text).color(muted(70)),
        Color32::TRANSPARENT,
        Stroke::NONE,
    )
}

/// An accent-tinted tag: the 800 step of the live accent behind its 100 step.
/// Same index of any ramp carries the same visual weight, so this reads
/// identically under steel, teal and amber.
pub(crate) fn tag_accent(ui: &mut Ui, text: &str) -> Response {
    let ramp = accent_ramp(ui);
    let (fill, fg) = (a800(ramp), a100(ramp));
    chip(ui, RichText::new(text).color(fg), fill, Stroke::NONE)
}

/// A neutral tag, for facts that carry no status -- a namespace, a provider
/// name, a count of something uninteresting.
pub(crate) fn tag_neutral(ui: &mut Ui, text: &str) -> Response {
    chip(
        ui,
        RichText::new(text).color(a100(&NEUTRAL)),
        a800(&NEUTRAL),
        Stroke::NONE,
    )
}

/// An outlined accent tag. Same weight as [`tag_accent`] without the filled
/// block, for when several tags sit in a row and the fills would band.
pub(crate) fn tag_outline(ui: &mut Ui, text: &str) -> Response {
    let a = accent(ui);
    chip(
        ui,
        RichText::new(text).color(a),
        Color32::TRANSPARENT,
        Stroke::new(HAIRLINE, a),
    )
}

/// A trailing count, in the mono face so digits line up between adjacent rows.
pub(crate) fn count_pill(ui: &mut Ui, count: usize) -> Response {
    Frame::NONE
        .fill(TEXT.gamma_multiply(CHIP_TINT))
        .corner_radius(R_SM)
        .inner_margin(Margin::symmetric(S2.round() as i8, 0))
        .show(ui, |ui| {
            ui.label(
                RichText::new(count.to_string())
                    .text_style(TextStyle::Name("code".into()))
                    .color(muted(55)),
            );
        })
        .response
}

/// What a kind badge marks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    /// An ordinary WMI class.
    Class,
    /// An association class -- one that only exists to join two others.
    Association,
    /// An event class, intrinsic or extrinsic.
    Event,
}

impl Kind {
    /// The single letter shown in the badge.
    fn letter(self) -> &'static str {
        match self {
            Self::Class => "C",
            Self::Association => "A",
            Self::Event => "E",
        }
    }
}

/// The 15px square that leads every row of the class list.
///
/// Only classes get the accent; associations and events are neutral, and
/// associations are the darker of the two because they are structural rather
/// than something you would go looking for.
pub(crate) fn kind_badge(ui: &mut Ui, kind: Kind) -> Response {
    let ramp = accent_ramp(ui);
    let (fill, fg) = match kind {
        Kind::Class => (a800(ramp), a100(ramp)),
        // The 900 step has no accessor -- it is only ever used here, as the one
        // fill that has to sit *below* the 800 tints without becoming the page.
        Kind::Association => (NEUTRAL[8], a300(&NEUTRAL)),
        Kind::Event => (a800(&NEUTRAL), a300(&NEUTRAL)),
    };

    let (rect, response) = ui.allocate_exact_size(Vec2::splat(BADGE_SIDE), Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, R_BADGE, fill);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            kind.letter(),
            FontId::new(BADGE_FONT, FontFamily::Proportional),
            fg,
        );
    }
    response
}

/// A filled dot plus a label: the status shape used in the machine list, the
/// connection indicator and the event-kind column.
///
/// `color` comes from a status token or `tokens::state_color`, so this helper
/// takes one rather than deriving it -- the caller is the only thing that knows
/// what the dot means.
pub(crate) fn dot_chip(ui: &mut Ui, color: Color32, label: &str) -> Response {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(DOT), Sense::hover());
        if ui.is_rect_visible(rect) {
            ui.painter().circle_filled(rect.center(), DOT * 0.5, color);
        }
        ui.label(
            RichText::new(label)
                .text_style(TextStyle::Small)
                .color(muted(70)),
        )
    })
    .inner
}
