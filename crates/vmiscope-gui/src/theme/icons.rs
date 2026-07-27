//! Phosphor icon codepoints (regular weight, pinned to phosphor-icons/web v2.1.2).
//!
//! Generated from the upstream `style.css` and verified against the `cmap` table of
//! the bundled `Phosphor.ttf` -- every constant below resolves to a real glyph.
//! Codepoints are NOT stable across Phosphor major versions: regenerate if the
//! bundled font is ever bumped, and never hand-edit a value here.
//!
//! The icon font is its OWN family, deliberately not a fallback of the text
//! families -- see `theme::fonts` for the measurements that forced that. So an
//! icon is never simply concatenated into a label: use [`glyph`] for an icon on
//! its own, or [`labelled`] for the icon-plus-text pairing that most buttons
//! want. Concatenating a constant into a `format!` renders it in whatever font
//! answers that codepoint first, which for a third of these is Inter.

#![allow(dead_code)]

use eframe::egui::text::LayoutJob;
use eframe::egui::{Align, FontFamily, FontId, RichText, TextFormat, TextStyle, Ui};

/// The family icons are rendered in. Matches `theme::fonts::ICONS`.
fn family() -> FontFamily {
    FontFamily::Name(crate::theme::fonts::ICONS.into())
}

/// An icon on its own, in the icon font.
///
/// Pass the result anywhere a `RichText` goes; add `.color(..)` or `.size(..)`
/// as usual.
pub(crate) fn glyph(icon: &str) -> RichText {
    RichText::new(icon).family(family())
}

/// An icon followed by a label, as one piece of text.
///
/// The two halves need different fonts, which a single `RichText` cannot
/// express, so this builds a `LayoutJob` with a section each. It is accepted
/// anywhere `impl Into<WidgetText>` is -- buttons, labels, selectable labels.
pub(crate) fn labelled(ui: &Ui, icon: &str, label: &str) -> LayoutJob {
    labelled_styled(
        ui,
        icon,
        label,
        TextStyle::Button,
        ui.visuals().text_color(),
    )
}

/// [`labelled`], with the text style and colour spelled out.
pub(crate) fn labelled_styled(
    ui: &Ui,
    icon: &str,
    label: &str,
    style: TextStyle,
    color: eframe::egui::Color32,
) -> LayoutJob {
    let text_font = style.resolve(ui.style());
    // The icon rides slightly smaller than its label: Phosphor's glyphs are
    // drawn to a full em box while Inter's lowercase is not, so matching the
    // nominal sizes makes every icon look oversized next to its word.
    let icon_font = FontId::new(text_font.size * 0.95, family());

    let mut job = LayoutJob::default();
    job.append(
        icon,
        0.0,
        TextFormat {
            font_id: icon_font,
            color,
            valign: Align::Center,
            ..Default::default()
        },
    );
    job.append(
        label,
        ui.spacing().item_spacing.x * 0.9,
        TextFormat {
            font_id: text_font,
            color,
            valign: Align::Center,
            ..Default::default()
        },
    );
    job
}

// Rail and shell
pub const TREE_STRUCTURE: &str = "\u{e67c}";
pub const TERMINAL_WINDOW: &str = "\u{eae8}";
pub const BROADCAST: &str = "\u{e0f2}";
pub const CPU: &str = "\u{e610}";
pub const GLOBE_HEMISPHERE_WEST: &str = "\u{e28c}";
pub const SHIELD_WARNING: &str = "\u{e412}";
pub const PLUGS_CONNECTED: &str = "\u{eb5a}";
pub const BOOKMARK_SIMPLE: &str = "\u{e0ea}";
pub const GIT_DIFF: &str = "\u{e27c}";
pub const DESKTOP_TOWER: &str = "\u{e562}";
pub const GEAR_SIX: &str = "\u{e272}";
pub const QUESTION: &str = "\u{e3e8}";
pub const MAGNIFYING_GLASS: &str = "\u{e30c}";
pub const ARROWS_CLOCKWISE: &str = "\u{e094}";
pub const PULSE: &str = "\u{e000}";
pub const PAUSE: &str = "\u{e39e}";
pub const MINUS: &str = "\u{e32a}";
pub const SQUARE: &str = "\u{e45e}";
pub const CORNERS_IN: &str = "\u{e1ce}";
pub const X: &str = "\u{e4f6}";

// Explorer tree and classes
pub const DATABASE: &str = "\u{e1de}";
pub const FOLDER: &str = "\u{e24a}";
pub const FOLDER_OPEN: &str = "\u{e256}";
pub const DOT: &str = "\u{ecde}";
pub const CARET_DOWN: &str = "\u{e136}";
pub const CARET_RIGHT: &str = "\u{e13a}";
pub const CARET_UP: &str = "\u{e13c}";
pub const CARET_LEFT: &str = "\u{e138}";
pub const CUBE: &str = "\u{e1da}";
pub const ARROWS_IN_LINE_VERTICAL: &str = "\u{e532}";
pub const FUNNEL: &str = "\u{e266}";
pub const FUNNEL_SIMPLE: &str = "\u{e268}";

// Detail, schema and methods
pub const COPY: &str = "\u{e1ca}";
pub const FUNCTION: &str = "\u{ebe4}";
pub const LINK_SIMPLE: &str = "\u{e2e6}";
pub const KEY: &str = "\u{e2d6}";
pub const HASH: &str = "\u{e2a2}";
pub const TEXT_AA: &str = "\u{e6ee}";
pub const CLOCK: &str = "\u{e19a}";
pub const PENCIL_SIMPLE: &str = "\u{e3b4}";
pub const CROSSHAIR_SIMPLE: &str = "\u{e1d8}";
pub const ARROW_ELBOW_DOWN_RIGHT: &str = "\u{e046}";
pub const ARROW_SQUARE_OUT: &str = "\u{e5de}";
pub const LIGHTNING: &str = "\u{e2de}";
pub const PLAY: &str = "\u{e3d0}";
pub const STOP: &str = "\u{e46c}";
pub const INFO: &str = "\u{e2ce}";
pub const LIST_BULLETS: &str = "\u{e2f2}";

// Export, files and code
pub const EXPORT: &str = "\u{eaf0}";
pub const DOWNLOAD_SIMPLE: &str = "\u{e20c}";
pub const UPLOAD_SIMPLE: &str = "\u{e4c0}";
pub const FILE_CSV: &str = "\u{eb1c}";
pub const BRACKETS_CURLY: &str = "\u{e860}";
pub const FILE_TEXT: &str = "\u{e23a}";
pub const CLIPBOARD_TEXT: &str = "\u{e198}";
pub const FILE_ARROW_DOWN: &str = "\u{e232}";
pub const FLOPPY_DISK: &str = "\u{e248}";
pub const CODE: &str = "\u{e1bc}";

// Status and feedback
pub const CHECK_CIRCLE: &str = "\u{e184}";
pub const WARNING_CIRCLE: &str = "\u{e4e2}";
pub const WARNING: &str = "\u{e4e0}";
pub const PROHIBIT: &str = "\u{e3de}";
pub const CIRCLE_NOTCH: &str = "\u{eb44}";
pub const TIMER: &str = "\u{e492}";
pub const SPINNER_GAP: &str = "\u{e66c}";
pub const SEAL_CHECK: &str = "\u{e606}";
pub const BUG: &str = "\u{e5f4}";

// Network and processes
pub const APP_WINDOW: &str = "\u{e5da}";
pub const WIFI_HIGH: &str = "\u{e4ea}";
pub const WIFI_SLASH: &str = "\u{e4f2}";
pub const GLOBE_SIMPLE: &str = "\u{e28e}";
pub const ARROW_FAT_LINE_RIGHT: &str = "\u{e520}";
pub const ARROWS_LEFT_RIGHT: &str = "\u{e0a0}";
pub const CHART_LINE: &str = "\u{e154}";

// Machines, auth and settings
pub const HARD_DRIVES: &str = "\u{e2a0}";
pub const SLIDERS_HORIZONTAL: &str = "\u{e434}";
pub const USER: &str = "\u{e4c2}";
pub const USERS: &str = "\u{e4d6}";
pub const LOCK_KEY: &str = "\u{e2fe}";
pub const LOCK_KEY_OPEN: &str = "\u{e300}";
pub const TOGGLE_LEFT: &str = "\u{e674}";
pub const TOGGLE_RIGHT: &str = "\u{e676}";
pub const PALETTE: &str = "\u{e6c8}";
pub const PLUS: &str = "\u{e3d4}";
pub const TRASH: &str = "\u{e4a6}";
pub const STAR: &str = "\u{e46a}";
pub const EYE: &str = "\u{e220}";
pub const EYE_SLASH: &str = "\u{e224}";
pub const SORT_ASCENDING: &str = "\u{e444}";
pub const SORT_DESCENDING: &str = "\u{e446}";
pub const ARROW_COUNTER_CLOCKWISE: &str = "\u{e038}";
pub const NOTE_PENCIL: &str = "\u{e34c}";
pub const SHIELD_CHECK: &str = "\u{e40c}";
pub const FINGERPRINT: &str = "\u{e23e}";

/// Every icon, for the consistency tests.
const ALL: &[(&str, &str)] = &[
    ("TREE_STRUCTURE", TREE_STRUCTURE),
    ("TERMINAL_WINDOW", TERMINAL_WINDOW),
    ("BROADCAST", BROADCAST),
    ("CPU", CPU),
    ("GLOBE_HEMISPHERE_WEST", GLOBE_HEMISPHERE_WEST),
    ("SHIELD_WARNING", SHIELD_WARNING),
    ("PLUGS_CONNECTED", PLUGS_CONNECTED),
    ("BOOKMARK_SIMPLE", BOOKMARK_SIMPLE),
    ("GIT_DIFF", GIT_DIFF),
    ("DESKTOP_TOWER", DESKTOP_TOWER),
    ("GEAR_SIX", GEAR_SIX),
    ("QUESTION", QUESTION),
    ("MAGNIFYING_GLASS", MAGNIFYING_GLASS),
    ("ARROWS_CLOCKWISE", ARROWS_CLOCKWISE),
    ("PULSE", PULSE),
    ("PAUSE", PAUSE),
    ("MINUS", MINUS),
    ("SQUARE", SQUARE),
    ("CORNERS_IN", CORNERS_IN),
    ("X", X),
    ("DATABASE", DATABASE),
    ("FOLDER", FOLDER),
    ("FOLDER_OPEN", FOLDER_OPEN),
    ("DOT", DOT),
    ("CARET_DOWN", CARET_DOWN),
    ("CARET_RIGHT", CARET_RIGHT),
    ("CARET_UP", CARET_UP),
    ("CARET_LEFT", CARET_LEFT),
    ("CUBE", CUBE),
    ("ARROWS_IN_LINE_VERTICAL", ARROWS_IN_LINE_VERTICAL),
    ("FUNNEL", FUNNEL),
    ("FUNNEL_SIMPLE", FUNNEL_SIMPLE),
    ("COPY", COPY),
    ("FUNCTION", FUNCTION),
    ("LINK_SIMPLE", LINK_SIMPLE),
    ("KEY", KEY),
    ("HASH", HASH),
    ("TEXT_AA", TEXT_AA),
    ("CLOCK", CLOCK),
    ("PENCIL_SIMPLE", PENCIL_SIMPLE),
    ("CROSSHAIR_SIMPLE", CROSSHAIR_SIMPLE),
    ("ARROW_ELBOW_DOWN_RIGHT", ARROW_ELBOW_DOWN_RIGHT),
    ("ARROW_SQUARE_OUT", ARROW_SQUARE_OUT),
    ("LIGHTNING", LIGHTNING),
    ("PLAY", PLAY),
    ("STOP", STOP),
    ("INFO", INFO),
    ("LIST_BULLETS", LIST_BULLETS),
    ("EXPORT", EXPORT),
    ("DOWNLOAD_SIMPLE", DOWNLOAD_SIMPLE),
    ("UPLOAD_SIMPLE", UPLOAD_SIMPLE),
    ("FILE_CSV", FILE_CSV),
    ("BRACKETS_CURLY", BRACKETS_CURLY),
    ("FILE_TEXT", FILE_TEXT),
    ("CLIPBOARD_TEXT", CLIPBOARD_TEXT),
    ("FILE_ARROW_DOWN", FILE_ARROW_DOWN),
    ("FLOPPY_DISK", FLOPPY_DISK),
    ("CODE", CODE),
    ("CHECK_CIRCLE", CHECK_CIRCLE),
    ("WARNING_CIRCLE", WARNING_CIRCLE),
    ("WARNING", WARNING),
    ("PROHIBIT", PROHIBIT),
    ("CIRCLE_NOTCH", CIRCLE_NOTCH),
    ("TIMER", TIMER),
    ("SPINNER_GAP", SPINNER_GAP),
    ("SEAL_CHECK", SEAL_CHECK),
    ("BUG", BUG),
    ("APP_WINDOW", APP_WINDOW),
    ("WIFI_HIGH", WIFI_HIGH),
    ("WIFI_SLASH", WIFI_SLASH),
    ("GLOBE_SIMPLE", GLOBE_SIMPLE),
    ("ARROW_FAT_LINE_RIGHT", ARROW_FAT_LINE_RIGHT),
    ("ARROWS_LEFT_RIGHT", ARROWS_LEFT_RIGHT),
    ("CHART_LINE", CHART_LINE),
    ("HARD_DRIVES", HARD_DRIVES),
    ("SLIDERS_HORIZONTAL", SLIDERS_HORIZONTAL),
    ("USER", USER),
    ("USERS", USERS),
    ("LOCK_KEY", LOCK_KEY),
    ("LOCK_KEY_OPEN", LOCK_KEY_OPEN),
    ("TOGGLE_LEFT", TOGGLE_LEFT),
    ("TOGGLE_RIGHT", TOGGLE_RIGHT),
    ("PALETTE", PALETTE),
    ("PLUS", PLUS),
    ("TRASH", TRASH),
    ("STAR", STAR),
    ("EYE", EYE),
    ("EYE_SLASH", EYE_SLASH),
    ("SORT_ASCENDING", SORT_ASCENDING),
    ("SORT_DESCENDING", SORT_DESCENDING),
    ("ARROW_COUNTER_CLOCKWISE", ARROW_COUNTER_CLOCKWISE),
    ("NOTE_PENCIL", NOTE_PENCIL),
    ("SHIELD_CHECK", SHIELD_CHECK),
    ("FINGERPRINT", FINGERPRINT),
];

#[cfg(test)]
mod tests {
    /// Every constant must be exactly one char in the Private Use Area. A stray
    /// empty string or a two-char value would render as nothing or as garbage,
    /// and neither is visible in a screenshot review.
    #[test]
    fn every_icon_is_one_private_use_char() {
        for (name, s) in super::ALL {
            let mut it = s.chars();
            let c = it.next().unwrap_or_else(|| panic!("{name} is empty"));
            assert!(it.next().is_none(), "{name} is more than one char");
            assert!(
                ('\u{e000}'..='\u{f8ff}').contains(&c),
                "{name} is U+{:04X}, outside the Private Use Area",
                c as u32
            );
        }
    }

    /// Two names pointing at one glyph is almost always a copy-paste slip.
    #[test]
    fn icons_are_distinct() {
        let mut seen = std::collections::HashMap::new();
        for (name, s) in super::ALL {
            if let Some(prev) = seen.insert(s, name) {
                panic!("{name} and {prev} share a codepoint");
            }
        }
    }
}
