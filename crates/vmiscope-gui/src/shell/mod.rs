//! The application shell: the window chrome, the title bar, the rail and the
//! status bar.
//!
//! Everything in here frames the views without being one. The split is
//! deliberate: `chrome` owns the parts that only exist because the window is
//! undecorated (drag, resize, the outer border), while `titlebar`, `rail` and
//! `statusbar` are ordinary panels that would look the same under a real OS
//! caption.
//!
//! The shell's metrics are stated once here rather than at their panels,
//! because each is load-bearing twice over: once in the `exact_size` that
//! reserves the space, and again in the geometry that has to line up inside it.

pub(crate) mod chrome;
pub(crate) mod rail;
pub(crate) mod statusbar;
pub(crate) mod titlebar;

use eframe::egui::{Align, Direction, Label, Layout, Rect, Ui, UiBuilder, WidgetText};

use crate::theme::tokens::SURFACE;

/// Title bar height. This is the *outer* panel size -- see the `exact_size`
/// note in [`titlebar`].
pub(crate) const TITLEBAR_H: f32 = 40.0;

/// Rail width. 64px leaves 56px of usable width once the 4px selection-pill
/// inset is taken off each side, which is what `View::rail_label` is sized
/// against.
pub(crate) const RAIL_W: f32 = 64.0;

/// Status bar height.
pub(crate) const STATUS_H: f32 = 24.0;

/// Horizontal breathing room at the outer edges of the title and status bars.
///
/// Applied with `add_space` rather than as a panel `inner_margin`, so that
/// `ui.max_rect()` inside each panel closure is still the panel's *outer* rect
/// and the hairline separators can be painted full-bleed.
pub(crate) const PAD_X: f32 = 10.0;

/// The chrome's ground: `SURFACE` at 55% over the page.
///
/// `gamma_multiply` on an opaque token scales the premultiplied channels and
/// the alpha together, so this composites over the shell frame's `BG` fill and
/// lands exactly on "SURFACE 55% over BG" -- the design's figure for the status
/// strip, reused for the title bar and the rail so the three read as one frame
/// around the content rather than as three unrelated bars.
pub(crate) fn chrome_fill() -> eframe::egui::Color32 {
    SURFACE.gamma_multiply(0.55)
}

/// Place `text` centred in `rect` without disturbing the parent's cursor.
///
/// `Ui::put` would work, but it routes through `scope_builder`, which advances
/// the cursor a second time over a rect that `allocate_exact_size` already
/// accounted for. `new_child` is the same placement with none of that
/// bookkeeping -- the child registers its own `min_rect` when it drops and the
/// parent never hears about it.
pub(crate) fn centered(ui: &mut Ui, rect: Rect, text: impl Into<WidgetText>) {
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::centered_and_justified(Direction::TopDown)),
    );
    child.add(Label::new(text).selectable(false));
}

/// A child `Ui` filling `rect`, laid out top-down and centred.
///
/// The rail's icon-over-label stack. Same reasoning as [`centered`]: the rect
/// is already allocated, so the parent's cursor must not move again.
pub(crate) fn stacked_in(ui: &mut Ui, rect: Rect) -> Ui {
    ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Center)),
    )
}
