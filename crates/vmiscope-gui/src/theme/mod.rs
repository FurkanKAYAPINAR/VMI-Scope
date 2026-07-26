//! The visual layer: tokens, fonts, icons and style installation.
//!
//! A design-token module defines the whole vocabulary up front and the views
//! grow into it, so parts of this are unused between phases. The allow is
//! scoped to this module rather than sprinkled per item, and should come off
//! at 1.0 -- by then anything still unused is a token nothing needed.
#![allow(dead_code)]

pub(crate) mod fonts;
pub(crate) mod icons;
pub(crate) mod tokens;

use egui::{Color32, Margin, Stroke, TextStyle, ThemePreference};
use serde::{Deserialize, Serialize};

use tokens::*;

/// The accent voice. Only one is active at a time; the design is a mono scheme.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum Accent {
    #[default]
    Steel,
    Teal,
    Amber,
}

impl Accent {
    pub(crate) fn ramp(self) -> &'static [Color32; 9] {
        match self {
            Self::Steel => &STEEL,
            Self::Teal => &TEAL,
            Self::Amber => &AMBER,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Steel => "Steel",
            Self::Teal => "Teal",
            Self::Amber => "Amber",
        }
    }

    pub(crate) const ALL: [Self; 3] = [Self::Steel, Self::Teal, Self::Amber];
}

/// Row height and spacing. This system is dense on purpose; Comfortable exists
/// for projectors and tired eyes, not as the default.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum Density {
    #[default]
    Compact,
    Comfortable,
}

impl Density {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Comfortable => "Comfortable",
        }
    }

    pub(crate) const ALL: [Self; 2] = [Self::Compact, Self::Comfortable];
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Theme {
    pub(crate) accent: Accent,
    pub(crate) density: Density,
}

/// Pixel metrics derived from the density. Views measure from here rather than
/// writing literals, so a density switch actually moves everything.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Metrics {
    pub(crate) row_h: f32,
    pub(crate) header_h: f32,
    pub(crate) rail_item_h: f32,
    pub(crate) tree_indent: f32,
    pub(crate) card_min_w: f32,
    pub(crate) s1: f32,
    pub(crate) s2: f32,
    pub(crate) s3: f32,
    pub(crate) s4: f32,
    pub(crate) s6: f32,
    pub(crate) s8: f32,
}

impl Metrics {
    pub(crate) fn for_density(density: Density) -> Self {
        // Only the rhythm scales. Font sizes stay put: the 13px body against the
        // 9px rail label is a designed relationship, and zooming type would also
        // mean `set_zoom_factor`, which scales the whole context.
        let k = match density {
            Density::Compact => 1.0,
            Density::Comfortable => 1.3,
        };
        Self {
            row_h: 21.0 * k,
            header_h: 22.0,
            rail_item_h: 44.0,
            tree_indent: 13.0,
            card_min_w: 330.0,
            s1: S1 * k,
            s2: S2 * k,
            s3: S3 * k,
            s4: S4 * k,
            s6: S6 * k,
            s8: S8 * k,
        }
    }
}

/// Install the whole visual style. Cheap enough to call on every accent or
/// density change -- it rebuilds `Style`, not the font atlas.
pub(crate) fn install(ctx: &egui::Context, theme: Theme) {
    let ramp = theme.accent.ramp();
    let m = Metrics::for_density(theme.density);

    // `all_styles_mut`, not `set_visuals`: it writes both the light and dark
    // styles, so an OS theme flip can never expose stock egui colours through a
    // half-configured light variant. (`Context::set_style` does not exist in
    // 0.35 -- see the invariants in docs/REDESIGN.md.)
    ctx.all_styles_mut(|style| {
        style.visuals = base_visuals();
        apply_accent(&mut style.visuals, ramp);

        style.text_styles = text_styles();

        style.spacing.item_spacing = egui::vec2(m.s2, m.s1);
        style.spacing.button_padding = egui::vec2(m.s3, m.s1);
        style.spacing.interact_size = egui::vec2(0.0, m.row_h);
        style.spacing.indent = m.s6;
        style.spacing.menu_spacing = m.s1;
        style.spacing.window_margin = Margin::same(8);
        style.spacing.menu_margin = Margin::symmetric(6, 3);

        style.spacing.scroll = egui::style::ScrollStyle {
            floating: false,
            bar_width: 8.0,
            handle_min_length: 22.4,
            bar_inner_margin: 3.0,
            bar_outer_margin: 0.0,
            ..egui::style::ScrollStyle::solid()
        };
        // The design has no fading scrollbars; a scrollbar that vanishes is a
        // scrollbar you cannot see the extent of.
        style.spacing.scroll.fade.strength = 0.0;
    });

    ctx.set_theme(ThemePreference::Dark);
}

fn base_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();

    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.extreme_bg_color = BG;
    v.override_text_color = Some(TEXT);
    v.window_stroke = Stroke::new(1.0, DIVIDER);
    v.window_corner_radius = R_LG;
    v.menu_corner_radius = R_MD;

    // No zebra striping in this design -- rows are separated by a rule and a 4%
    // hover tint, nothing else.
    v.striped = false;

    // Elevation on a dark ground is a hairline plus ambient darkness, never a
    // stack of heavy shadows.
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(140),
    };
    v.popup_shadow = v.window_shadow;

    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = R_MD;
        w.bg_stroke = Stroke::new(1.0, DIVIDER);
        w.fg_stroke = Stroke::new(1.0, TEXT);
    }

    v.widgets.noninteractive.bg_fill = BG;
    v.widgets.noninteractive.weak_bg_fill = BG;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, muted(70));

    // Buttons are outlined, never filled: `weak_bg_fill` is what egui paints for
    // a resting button, so it stays transparent and the border does the work.
    v.widgets.inactive.bg_fill = Color32::TRANSPARENT;
    v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;

    v.widgets.hovered.bg_fill = TEXT.gamma_multiply(0.07);
    v.widgets.hovered.weak_bg_fill = TEXT.gamma_multiply(0.07);

    v.widgets.open.bg_fill = SURFACE;
    v.widgets.open.weak_bg_fill = SURFACE;

    v
}

/// egui has no single accent field, so it has to be fanned out. Miss one of
/// these and the accent switch half-applies -- the selection changes colour but
/// the text caret or the focus stroke does not.
fn apply_accent(v: &mut egui::Visuals, ramp: &[Color32; 9]) {
    let accent = a500(ramp);

    v.selection.bg_fill = accent.gamma_multiply(0.30);
    v.selection.stroke = Stroke::new(1.0, TEXT);
    v.hyperlink_color = accent;
    v.text_cursor.stroke = Stroke::new(1.0, accent);

    v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent);
    v.widgets.active.bg_stroke = Stroke::new(1.0, accent);
    v.widgets.active.bg_fill = accent.gamma_multiply(0.22);
    v.widgets.active.weak_bg_fill = accent.gamma_multiply(0.22);
}

/// `TextStyle::resolve` panics on a missing key, so all five built-ins are
/// mandatory. The named styles are the design's smaller roles.
fn text_styles() -> std::collections::BTreeMap<TextStyle, egui::FontId> {
    use egui::{FontFamily, FontId};

    let ui = FontFamily::Proportional;
    let mono = FontFamily::Monospace;
    let med = FontFamily::Name(fonts::UI_MEDIUM.into());

    [
        (TextStyle::Small, FontId::new(11.0, ui.clone())),
        (TextStyle::Body, FontId::new(13.0, ui.clone())),
        (TextStyle::Button, FontId::new(12.0, ui.clone())),
        (TextStyle::Monospace, FontId::new(12.0, mono.clone())),
        (TextStyle::Heading, FontId::new(19.0, med)),
        (TextStyle::Name("rail".into()), FontId::new(9.0, ui.clone())),
        (
            TextStyle::Name("caption".into()),
            FontId::new(10.5, ui.clone()),
        ),
        (TextStyle::Name("th".into()), FontId::new(11.0, ui)),
        (TextStyle::Name("code".into()), FontId::new(11.5, mono)),
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every accent must actually reach all six sites. This is the failure that
    /// looks like "the theme mostly works".
    #[test]
    fn accent_reaches_every_site() {
        for accent in Accent::ALL {
            let ramp = accent.ramp();
            let mut v = base_visuals();
            apply_accent(&mut v, ramp);

            let a = a500(ramp);
            assert_eq!(v.hyperlink_color, a, "{accent:?}: hyperlink");
            assert_eq!(v.text_cursor.stroke.color, a, "{accent:?}: caret");
            assert_eq!(v.widgets.hovered.bg_stroke.color, a, "{accent:?}: hover");
            assert_eq!(v.widgets.active.bg_stroke.color, a, "{accent:?}: active");
            assert_eq!(
                v.selection.bg_fill,
                a.gamma_multiply(0.30),
                "{accent:?}: selection"
            );
            assert_eq!(
                v.widgets.active.weak_bg_fill,
                a.gamma_multiply(0.22),
                "{accent:?}: active fill"
            );
        }
    }

    /// Buttons in this design are an outline on transparent. If a fill creeps
    /// into the resting state every button becomes a slab.
    #[test]
    fn resting_buttons_are_not_filled() {
        let v = base_visuals();
        assert_eq!(v.widgets.inactive.weak_bg_fill, Color32::TRANSPARENT);
        assert_eq!(v.widgets.inactive.bg_fill, Color32::TRANSPARENT);
    }

    /// All five built-in text styles must be present or text layout panics at
    /// runtime rather than failing to compile.
    #[test]
    fn every_builtin_text_style_is_defined() {
        let styles = text_styles();
        for required in [
            TextStyle::Small,
            TextStyle::Body,
            TextStyle::Button,
            TextStyle::Monospace,
            TextStyle::Heading,
        ] {
            assert!(styles.contains_key(&required), "{required:?} is missing");
        }
    }

    /// Comfortable must actually be roomier, and Compact must stay the dense
    /// default the design asks for.
    #[test]
    fn density_changes_the_rhythm_but_not_the_indent() {
        let compact = Metrics::for_density(Density::Compact);
        let comfy = Metrics::for_density(Density::Comfortable);
        assert!(comfy.row_h > compact.row_h);
        assert!(comfy.s3 > compact.s3);
        assert_eq!(comfy.tree_indent, compact.tree_indent);
    }
}
