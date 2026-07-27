//! Waiting states: spinners, skeletons and elapsed-time badges.
//!
//! Thirteen places used a bare `ui.spinner()`, which draws in egui's own colour
//! and says nothing about what is being waited on. These say what, and how long.

#![allow(dead_code)] // The views adopt the kit in the next commit.

use eframe::egui::{CornerRadius, Label, Response, RichText, Spinner, TextStyle, Ui};

use crate::theme::tokens::{muted, NEUTRAL, WARN};
use crate::widgets::button::accent;

/// Anything slower than this is worth flagging in an elapsed badge.
///
/// A second is roughly where a query stops feeling instant, and where the
/// design's history panel switches the figure to the warn colour.
pub(crate) const SLOW_MS: u64 = 1000;

/// A spinner in the accent colour, with a label for what is being waited on.
pub(crate) fn spinner(ui: &mut Ui, what: &str) {
    ui.horizontal(|ui| {
        ui.add(Spinner::new().color(accent(ui)).size(13.0));
        ui.add(Label::new(RichText::new(what).color(muted(60))));
    });
}

/// A placeholder row for content that has not arrived, sized like the real
/// thing so the layout does not jump when it does.
pub(crate) fn skeleton_row(ui: &mut Ui, width: f32) {
    let height = ui.text_style_height(&TextStyle::Body) * 0.6;
    let (rect, _) = ui.allocate_exact_size(
        eframe::egui::vec2(width, height),
        eframe::egui::Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, CornerRadius::same(3), NEUTRAL[8]);
}

/// Format a duration the way the design's status strips do: sub-second in
/// milliseconds, past that in seconds to one decimal.
///
/// Milliseconds past a second are noise -- nobody reads "1483 ms" as anything
/// other than "about one and a half seconds", and the extra digits crowd a
/// status line that has other things to say.
pub(crate) fn format_ms(ms: u64) -> String {
    if ms < SLOW_MS {
        format!("{ms} ms")
    } else {
        format!("{:.1} s", ms as f64 / 1000.0)
    }
}

/// A small monospace badge carrying an elapsed time, warn-coloured once slow.
pub(crate) fn elapsed_badge(ui: &mut Ui, ms: u64) -> Response {
    let color = if ms >= SLOW_MS { WARN } else { muted(55) };
    ui.add(
        Label::new(
            RichText::new(format_ms(ms))
                .text_style(TextStyle::Monospace)
                .size(10.5)
                .color(color),
        )
        .wrap(),
    )
}

/// A one-line note about a result that is not whole, or nothing when it is.
///
/// Takes the note rather than the `Completion` so the widget layer does not
/// depend on the core's enum shape.
pub(crate) fn partial_note(ui: &mut Ui, note: Option<String>) {
    if let Some(note) = note {
        ui.add(Label::new(
            RichText::new(note)
                .size(10.5)
                .color(WARN)
                .text_style(TextStyle::Small),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The badge switches units at a second, and the threshold is inclusive --
    /// exactly 1000 ms should read as seconds, not as a four-digit millisecond
    /// figure.
    #[test]
    fn durations_switch_units_at_a_second() {
        assert_eq!(format_ms(0), "0 ms");
        assert_eq!(format_ms(412), "412 ms");
        assert_eq!(format_ms(999), "999 ms");
        assert_eq!(format_ms(1000), "1.0 s");
        assert_eq!(format_ms(1483), "1.5 s");
        assert_eq!(format_ms(11_600), "11.6 s");
    }
}
