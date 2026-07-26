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

    // Fallback is per character and first match wins, so the icon font goes last
    // in every family. Phosphor ships each icon name as a ligature -- if it ever
    // led a family, the words "copy", "key", "star" and "folder" would render as
    // pictures.
    fonts
        .families
        .insert(FontFamily::Proportional, vec![UI.into(), ICONS.into()]);
    fonts
        .families
        .insert(FontFamily::Monospace, vec![MONO.into(), ICONS.into()]);
    // No trailing text fallback on the medium family: both Inter entries are the
    // same blob at different weights, so their coverage is identical and a
    // fallback to the regular weight could never resolve anything the medium
    // weight had already missed.
    fonts.families.insert(
        FontFamily::Name(UI_MEDIUM.into()),
        vec![UI_MEDIUM.into(), ICONS.into()],
    );

    fonts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every family must exist, must not lead with the icon font, and must fall
    /// back to it. Getting this order wrong is silent: text keeps rendering,
    /// just occasionally as pictures, because Phosphor ships each icon name as
    /// a ligature -- "copy", "key", "star" and "folder" are all icon names.
    ///
    /// This inspects the real map rather than rebuilding one, so it cannot drift
    /// away from what actually ships.
    #[test]
    fn icon_font_is_always_the_fallback() {
        let fonts = definitions();

        for family in [
            FontFamily::Proportional,
            FontFamily::Monospace,
            FontFamily::Name(UI_MEDIUM.into()),
        ] {
            let chain = fonts.families.get(&family).expect("family is registered");
            assert!(!chain.is_empty(), "{family:?} resolves to no font");
            assert_ne!(chain[0], ICONS, "{family:?} leads with the icon font");
            assert_eq!(
                chain.last().map(String::as_str),
                Some(ICONS),
                "{family:?} does not fall back to the icon font"
            );
            for name in chain {
                assert!(
                    fonts.font_data.contains_key(name),
                    "{family:?} names {name}, which has no font data"
                );
            }
        }
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
