//! Font installation.
//!
//! Three faces are embedded in the binary: Inter for UI text, JetBrains Mono for
//! every value, path and identifier, and Phosphor as an icon font. Nothing is
//! loaded from disk or from the network at runtime -- a tool that inspects other
//! machines has no business fetching a webfont.
//!
//! Two things about egui 0.35 shape this module. It rasterizes with `skrifa` and
//! shapes with `harfrust`, which means (a) variable fonts work, so one Inter blob
//! covers every weight, and (b) OpenType ligatures now fire and cannot be turned
//! off -- hence the `NL` ("no ligatures") build of JetBrains Mono, without which
//! `!=` in a WQL filter would silently render as a single glyph.

use std::sync::Arc;

use egui::epaint::text::VariationCoords;
use egui::{FontData, FontDefinitions, FontFamily, FontTweak};

/// Variable: carries wght 100-900 and opsz 14-32, so both UI weights come from
/// this one blob.
const INTER: &[u8] = include_bytes!("../../assets/fonts/InterVariable.ttf");
/// The no-ligature build, deliberately. See the module comment.
const JETBRAINS_MONO: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMonoNL-Regular.ttf");
/// Phosphor regular, pinned to v2.1.2 to match the codepoints in `icons.rs`.
const PHOSPHOR: &[u8] = include_bytes!("../../assets/fonts/Phosphor.ttf");

/// Body text.
pub const UI: &str = "ui";
/// Headings. The design asks for weight 500 and never bolder.
pub const UI_MEDIUM: &str = "ui-med";
/// Values, paths, WQL, generated code.
pub const MONO: &str = "mono";
/// Icons, registered as a fallback rather than a family anyone selects.
pub const ICONS: &str = "icons";

/// Inter instanced at a weight. `include_bytes!` emits the blob once however many
/// times this is called, so the second weight is free.
fn inter_at(weight: f32) -> FontData {
    FontData::from_static(INTER).tweak(FontTweak {
        coords: VariationCoords::new([(b"wght", weight), (b"opsz", 14.0)]),
        ..Default::default()
    })
}

/// Install the font set. Call once at startup: `set_fonts` compares the whole TTF
/// byte-for-byte on every call, so this is not something to run per frame.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
}

/// The font map, separated from installation so the tests can inspect the real
/// thing rather than a copy of it that can drift.
fn definitions() -> FontDefinitions {
    // `empty()`, not `default()`: eframe is built without `default_fonts`, and
    // starting from the defaults would pull in ~1.4 MB of fonts we replace.
    // Every built-in family must therefore be populated by hand -- `TextStyle`
    // resolution panics on a family that resolves to nothing.
    let mut fonts = FontDefinitions::empty();

    fonts.font_data.insert(UI.into(), Arc::new(inter_at(400.0)));
    fonts
        .font_data
        .insert(UI_MEDIUM.into(), Arc::new(inter_at(500.0)));
    fonts
        .font_data
        .insert(MONO.into(), Arc::new(FontData::from_static(JETBRAINS_MONO)));
    fonts
        .font_data
        .insert(ICONS.into(), Arc::new(FontData::from_static(PHOSPHOR)));

    // Icons get their OWN family, and the text families do not fall back to
    // them. Mixing the two cannot be made to work in either order, which was
    // established by parsing the three cmaps rather than by reasoning:
    //
    //   Phosphor holds 1,513 Private Use Area glyphs.
    //   Inter holds 745 of its own, colliding with 32 of our 94 icons.
    //   Phosphor also covers 26 Latin letters and the space, to carry the
    //   ligatures that spell its icon names.
    //
    // So with the icon font last, Inter answers a third of the icons first and
    // they render as unrelated letters -- a download arrow came out as "S with
    // caron". With it first, Phosphor answers lowercase text and its own name
    // ligatures fire, turning the words "copy", "key" and "folder" into
    // pictures. Separate families is the only arrangement where neither font
    // can be asked for a character it should not answer.
    //
    // Icons are therefore rendered explicitly: see `theme::icons::glyph`.
    fonts
        .families
        .insert(FontFamily::Proportional, vec![UI.into()]);
    fonts
        .families
        .insert(FontFamily::Monospace, vec![MONO.into()]);
    fonts
        .families
        .insert(FontFamily::Name(UI_MEDIUM.into()), vec![UI_MEDIUM.into()]);
    fonts
        .families
        .insert(FontFamily::Name(ICONS.into()), vec![ICONS.into()]);

    fonts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No family may mix the icon font with a text font, in either order.
    ///
    /// The previous version of this test asserted the opposite -- that every
    /// text family should *fall back* to the icon font -- and that assertion is
    /// what guaranteed the bug. egui resolves a family per character, first
    /// match wins, and Inter carries 745 Private Use Area glyphs of its own,
    /// 32 of which collide with icons we use. With the icon font trailing,
    /// Inter answered a third of them and they rendered as unrelated letters.
    /// Putting it first is no better: Phosphor covers 26 Latin letters and the
    /// space to carry its name ligatures, so it would answer lowercase text and
    /// turn the words "copy", "key" and "folder" into pictures.
    ///
    /// This inspects the real map rather than rebuilding one, so it cannot
    /// drift away from what ships.
    #[test]
    fn no_family_mixes_icons_with_text() {
        let fonts = definitions();

        for family in [
            FontFamily::Proportional,
            FontFamily::Monospace,
            FontFamily::Name(UI_MEDIUM.into()),
            FontFamily::Name(ICONS.into()),
        ] {
            let chain = fonts.families.get(&family).expect("family is registered");
            assert!(!chain.is_empty(), "{family:?} resolves to no font");

            let has_icons = chain.iter().any(|f| f == ICONS);
            let has_text = chain.iter().any(|f| f != ICONS);
            assert!(
                !(has_icons && has_text),
                "{family:?} mixes the icon font with a text font: {chain:?}"
            );

            for name in chain {
                assert!(
                    fonts.font_data.contains_key(name),
                    "{family:?} names {name}, which has no font data"
                );
            }
        }
    }

    /// The icon family must exist and must be the icon font, since every icon
    /// is rendered by naming it explicitly.
    #[test]
    fn the_icon_family_is_the_icon_font() {
        let fonts = definitions();
        let chain = fonts
            .families
            .get(&FontFamily::Name(ICONS.into()))
            .expect("icon family is registered");
        assert_eq!(chain, &[ICONS.to_string()]);
    }

    /// The embedded files must be the ones we think they are: a truncated or
    /// swapped asset would only show up as tofu at runtime.
    #[test]
    fn embedded_fonts_are_the_pinned_files() {
        assert_eq!(INTER.len(), 879_708, "InterVariable.ttf (rsms/inter v4.1)");
        assert_eq!(
            JETBRAINS_MONO.len(),
            208_576,
            "JetBrainsMonoNL-Regular.ttf (JetBrains/JetBrainsMono v2.304)"
        );
        assert_eq!(
            PHOSPHOR.len(),
            488_636,
            "Phosphor.ttf (phosphor-icons/web v2.1.2) -- icons.rs codepoints are pinned to this"
        );
        for ttf in [INTER, JETBRAINS_MONO, PHOSPHOR] {
            assert_eq!(&ttf[..4], b"\x00\x01\x00\x00", "not a TrueType file");
        }
    }

    /// Inter must actually be variable, or asking for weight 500 silently gives
    /// back weight 400 and the heading/body distinction disappears.
    #[test]
    fn inter_carries_a_weight_axis() {
        let axes = FontData::from_static(INTER).variation_axes();
        let wght = egui::epaint::text::Tag::new(b"wght");
        assert!(
            axes.iter().any(|a| a.tag == wght),
            "InterVariable has no wght axis; axes present: {:?}",
            axes.iter().map(|a| a.tag.to_string()).collect::<Vec<_>>()
        );
    }
}
