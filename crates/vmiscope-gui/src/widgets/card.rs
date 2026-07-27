//! Surface cards and the grid that lays them out.
//!
//! Elevation on a dark ground is an edge plus ambient darkness, never a stack
//! of shadows: a card is `SURFACE` behind a hairline, with one soft shadow. The
//! design's own rule, and the reason there is no `elev_lg` here -- anything
//! needing more lift is a dialog, and dialogs get their elevation from egui.

#![allow(dead_code)] // The views adopt the kit in the next commit.

use eframe::egui::{Frame, Margin, Response, Shadow, Stroke, Ui, Vec2};

use crate::theme::tokens::{DIVIDER, R_MD, S3, SURFACE};
use crate::widgets::rule::HAIRLINE;

/// The ambient shadow every card shares. Offset down rather than out, so cards
/// read as lying on the page rather than floating over it.
fn card_shadow() -> Shadow {
    Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: eframe::egui::Color32::from_black_alpha(90),
    }
}

/// A content card. `add_contents` draws inside the padded surface.
pub(crate) fn card<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(SURFACE)
        .corner_radius(R_MD)
        .stroke(Stroke::new(HAIRLINE, DIVIDER))
        .shadow(card_shadow())
        .inner_margin(Margin::same(S3 as i8))
        .show(ui, add_contents)
        .inner
}

/// A card that reports whether it was clicked, for grids where the whole card
/// is the target (saved queries, method cards).
pub(crate) fn clickable_card<R>(
    ui: &mut Ui,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> (R, Response) {
    let out = Frame::new()
        .fill(SURFACE)
        .corner_radius(R_MD)
        .stroke(Stroke::new(HAIRLINE, DIVIDER))
        .shadow(card_shadow())
        .inner_margin(Margin::same(S3 as i8))
        .show(ui, add_contents);
    let response = out.response.interact(eframe::egui::Sense::click());
    (out.inner, response)
}

/// How many columns fit `min_width` cards in `available`, mirroring CSS
/// `repeat(auto-fill, minmax(min_width, 1fr))`.
///
/// Always at least one: a pane narrower than one card shows one clipped card
/// rather than dividing by zero.
pub(crate) fn columns_for(available: f32, min_width: f32, gap: f32) -> usize {
    if min_width <= 0.0 {
        return 1;
    }
    // n cards need n*min_width + (n-1)*gap.
    let n = ((available + gap) / (min_width + gap)).floor() as usize;
    n.max(1)
}

/// Lay `items` out as cards in an auto-filling grid.
///
/// Cards share a row height because egui lays out rows, not a masonry: a card
/// grid with wildly uneven content will show gaps rather than interleave.
pub(crate) fn card_grid<T>(
    ui: &mut Ui,
    min_width: f32,
    items: &[T],
    mut show_item: impl FnMut(&mut Ui, &T),
) {
    if items.is_empty() {
        return;
    }
    let gap = ui.spacing().item_spacing.x;
    let cols = columns_for(ui.available_width(), min_width, gap);
    let width = (ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32;

    for chunk in items.chunks(cols) {
        ui.horizontal_top(|ui| {
            for item in chunk {
                ui.allocate_ui(Vec2::new(width, 0.0), |ui| {
                    ui.set_width(width);
                    show_item(ui, item);
                });
            }
        });
        ui.add_space(gap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The grid arithmetic has to account for the gaps between cards, or a
    /// row overflows its pane by exactly (cols - 1) * gap and the last card
    /// gets clipped.
    #[test]
    fn columns_account_for_the_gaps() {
        // 1000 wide, 330 cards, 6 gap: 3 cards need 990 + 12 = 1002 > 1000.
        assert_eq!(columns_for(1000.0, 330.0, 6.0), 2);
        assert_eq!(columns_for(1010.0, 330.0, 6.0), 3);
        // With no gap the naive division is right again.
        assert_eq!(columns_for(990.0, 330.0, 0.0), 3);
    }

    /// A pane narrower than one card must still show one, not zero.
    #[test]
    fn never_fewer_than_one_column() {
        assert_eq!(columns_for(100.0, 330.0, 6.0), 1);
        assert_eq!(columns_for(0.0, 330.0, 6.0), 1);
        assert_eq!(columns_for(500.0, 0.0, 6.0), 1);
    }
}
