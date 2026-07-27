//! The Nocturne palette and metrics.
//!
//! This module is the only place in the GUI allowed to name a colour. Views ask
//! for a token, never for an RGB triple -- `check.ps1` enforces that, because a
//! stray literal is invisible in review and only shows up as one widget that
//! ignores the accent switch.
//!
//! The ramps are generated on a shared perceptual lightness scale, so the same
//! index of any ramp carries the same visual weight. On this dark ground: 700-900
//! for tinted fills and subtle borders, 500 as the role's base, 100-300 for text
//! sitting on those tints.

use egui::{Color32, CornerRadius};

use vmiscope_core::{Protocol, Risk};

// ---------------------------------------------------------------------------
// Ground
// ---------------------------------------------------------------------------

/// The page.
pub(crate) const BG: Color32 = Color32::from_rgb(0x16, 0x18, 0x26);
/// Cards, inputs, popovers -- anything that lifts off the page.
pub(crate) const SURFACE: Color32 = Color32::from_rgb(0x23, 0x25, 0x32);
/// Body text.
pub(crate) const TEXT: Color32 = Color32::from_rgb(0xe9, 0xe9, 0xed);

/// 16% white. `Color32::from_white_alpha` is not const, so this is its
/// premultiplied equivalent worked out by hand: 0xe9 * 0.16 = 37.
pub(crate) const DIVIDER: Color32 = Color32::from_rgba_premultiplied(37, 37, 37, 41);

// ---------------------------------------------------------------------------
// Status
//
// Deliberately desaturated: in a tool that shows a lot of red, a screaming red
// stops meaning anything.
// ---------------------------------------------------------------------------

pub(crate) const OK: Color32 = Color32::from_rgb(0x7f, 0xbf, 0x9a);
pub(crate) const WARN: Color32 = Color32::from_rgb(0xc9, 0xac, 0x6b);
pub(crate) const BAD: Color32 = Color32::from_rgb(0xcf, 0x8a, 0x84);

// ---------------------------------------------------------------------------
// Ramps, 100 (lightest) .. 900 (darkest)
// ---------------------------------------------------------------------------

/// Surfaces, borders, muted text. Chroma stays low outside the accent.
///
/// The ramps are `static`, not `const`, and that matters: a `const` is inlined
/// at every use site, so `&STEEL` in two places can be two different addresses
/// and the reverse lookup in `widgets::button::ramp_of` -- which recovers a
/// whole ramp from the live accent by identity -- would silently never match.
pub(crate) static NEUTRAL: [Color32; 9] = [
    Color32::from_rgb(0xf3, 0xf5, 0xfe),
    Color32::from_rgb(0xe4, 0xe7, 0xf5),
    Color32::from_rgb(0xcf, 0xd3, 0xe5),
    Color32::from_rgb(0xb2, 0xb6, 0xca),
    Color32::from_rgb(0x93, 0x97, 0xab),
    Color32::from_rgb(0x75, 0x79, 0x8c),
    Color32::from_rgb(0x59, 0x5d, 0x6c),
    Color32::from_rgb(0x3f, 0x42, 0x4d),
    Color32::from_rgb(0x29, 0x2b, 0x31),
];

/// Steel -- the default accent.
pub(crate) static STEEL: [Color32; 9] = [
    Color32::from_rgb(0xf2, 0xf8, 0xfb),
    Color32::from_rgb(0xdf, 0xec, 0xf3),
    Color32::from_rgb(0xbe, 0xda, 0xe8),
    Color32::from_rgb(0x95, 0xc2, 0xd6),
    Color32::from_rgb(0x6f, 0xa9, 0xc6),
    Color32::from_rgb(0x4f, 0x87, 0xa4),
    Color32::from_rgb(0x3c, 0x68, 0x80),
    Color32::from_rgb(0x2b, 0x4a, 0x5c),
    Color32::from_rgb(0x1d, 0x31, 0x40),
];

pub(crate) static TEAL: [Color32; 9] = [
    Color32::from_rgb(0xf1, 0xfb, 0xf8),
    Color32::from_rgb(0xd5, 0xf0, 0xe9),
    Color32::from_rgb(0xb9, 0xe2, 0xd9),
    Color32::from_rgb(0x8e, 0xcd, 0xc0),
    Color32::from_rgb(0x5f, 0xb3, 0xa5),
    Color32::from_rgb(0x41, 0x90, 0x84),
    Color32::from_rgb(0x31, 0x6d, 0x64),
    Color32::from_rgb(0x23, 0x4f, 0x49),
    Color32::from_rgb(0x17, 0x33, 0x2f),
];

pub(crate) static AMBER: [Color32; 9] = [
    Color32::from_rgb(0xfb, 0xf7, 0xee),
    Color32::from_rgb(0xf2, 0xe7, 0xce),
    Color32::from_rgb(0xe6, 0xd6, 0xae),
    Color32::from_rgb(0xd4, 0xbc, 0x86),
    Color32::from_rgb(0xc2, 0xa0, 0x5f),
    Color32::from_rgb(0x9c, 0x7f, 0x45),
    Color32::from_rgb(0x76, 0x60, 0x36),
    Color32::from_rgb(0x54, 0x45, 0x27),
    Color32::from_rgb(0x37, 0x2d, 0x1a),
];

// Ramp accessors, so call sites read as design tokens rather than as indices.

/// Text on an accent tint.
#[inline]
pub(crate) fn a100(ramp: &[Color32; 9]) -> Color32 {
    ramp[0]
}
/// Paragraph-size accent text. The accent itself only clears 3:1 against the
/// ground -- enough for icons, chrome and large text, not for body copy.
#[inline]
pub(crate) fn a300(ramp: &[Color32; 9]) -> Color32 {
    ramp[2]
}
/// The accent.
#[inline]
pub(crate) fn a500(ramp: &[Color32; 9]) -> Color32 {
    ramp[4]
}
/// Tinted fills and badges.
#[inline]
pub(crate) fn a800(ramp: &[Color32; 9]) -> Color32 {
    ramp[7]
}

/// Muted text at a given strength, as a fraction of the body colour.
#[inline]
pub(crate) fn muted(percent: u8) -> Color32 {
    TEXT.gamma_multiply(f32::from(percent) / 100.0)
}

// ---------------------------------------------------------------------------
// Shape
// ---------------------------------------------------------------------------

pub(crate) const R_SM: CornerRadius = CornerRadius::same(4);
pub(crate) const R_MD: CornerRadius = CornerRadius::same(8);
pub(crate) const R_LG: CornerRadius = CornerRadius::same(14);

/// The 0.7x density scale. Kept fractional because egui takes f32 for spacing;
/// `Margin` is four i8 and rounds, so exactness only survives in `item_spacing`,
/// `button_padding`, `interact_size`, `indent` and friends.
pub(crate) const S1: f32 = 2.8;
pub(crate) const S2: f32 = 5.6;
pub(crate) const S3: f32 = 8.4;
pub(crate) const S4: f32 = 11.2;
pub(crate) const S6: f32 = 16.8;
pub(crate) const S8: f32 = 22.4;

// ---------------------------------------------------------------------------
// Semantic lookups
// ---------------------------------------------------------------------------

/// Colour for a subscription risk level.
pub(crate) fn risk_color(risk: Risk) -> Color32 {
    match risk {
        Risk::High => BAD,
        Risk::Medium => WARN,
        Risk::Low => NEUTRAL[4],
    }
}

/// Colour a connection by protocol/state, before the fade alpha is applied.
///
/// UDP is stateless, so it takes the listening colour rather than pretending to
/// have a lifecycle.
pub(crate) fn state_color(state: &str, proto: Protocol) -> Color32 {
    if proto == Protocol::Udp {
        return STEEL[3];
    }
    match state {
        "Established" => OK,
        "Listen" | "Bound" => STEEL[3],
        "SynSent" | "SynReceived" | "FinWait1" | "FinWait2" | "CloseWait" | "Closing"
        | "LastAck" | "TimeWait" => WARN,
        "Closed" | "DeleteTCB" => NEUTRAL[5],
        _ => NEUTRAL[3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every ramp must run light to dark. A ramp entered in the wrong order
    /// still compiles and still renders -- it just quietly inverts every tint
    /// built from it.
    #[test]
    fn ramps_run_light_to_dark() {
        for (name, ramp) in [
            ("neutral", &NEUTRAL),
            ("steel", &STEEL),
            ("teal", &TEAL),
            ("amber", &AMBER),
        ] {
            let lum = |c: &Color32| {
                0.2126 * f32::from(c.r()) + 0.7152 * f32::from(c.g()) + 0.0722 * f32::from(c.b())
            };
            for i in 1..ramp.len() {
                assert!(
                    lum(&ramp[i]) < lum(&ramp[i - 1]),
                    "{name} step {i} is not darker than step {}",
                    i - 1
                );
            }
        }
    }

    /// The accent sits at index 4 of its ramp; the design's stated hex values
    /// are the contract with the design system, so pin them.
    #[test]
    fn accents_are_the_documented_hexes() {
        assert_eq!(a500(&STEEL), Color32::from_rgb(0x6f, 0xa9, 0xc6));
        assert_eq!(a500(&TEAL), Color32::from_rgb(0x5f, 0xb3, 0xa5));
        assert_eq!(a500(&AMBER), Color32::from_rgb(0xc2, 0xa0, 0x5f));
    }
}
