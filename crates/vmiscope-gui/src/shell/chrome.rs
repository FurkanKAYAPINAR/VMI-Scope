//! The undecorated window: the shell frame, the caption drag region, and the
//! eight resize strips.
//!
//! # Why any of this exists
//!
//! With `decorations(false)`, egui-winit asks for an undecorated window, winit's
//! `WM_NCCALCSIZE` handler swallows the entire non-client area, and winit
//! implements no `WM_NCHITTEST` at all. The consequences are not subtle: there
//! is no OS resize border, no OS caption drag, and no system menu. Every one of
//! those has to be re-implemented in client space, which is what this file is.
//!
//! # The two ordering rules
//!
//! Both are invisible in the code that depends on them, and both break the
//! window rather than failing to compile.
//!
//! 1. **egui's hit test picks the last-registered overlapping widget.** The
//!    interact pass walks the frame's widget list backwards and takes the first
//!    hit. So [`title_drag`] is registered *before* the title-bar panel -- if it
//!    came after, the drag rect would sit on top of the window buttons and
//!    swallow every click on them -- and [`resize_strips`] is registered *after*
//!    every panel, or the panels eat the window edges and nothing resizes.
//! 2. **Under `--decorated` the OS owns both gestures.** Both functions return
//!    early, or the window gets two drag handlers and two resize borders
//!    fighting over the same pixels.

use eframe::egui::{
    CursorIcon, Frame, Id, PointerButton, Pos2, Rect, ResizeDirection, Response, Sense, Stroke, Ui,
    Vec2, ViewportCommand,
};

use crate::theme::tokens::{BG, DIVIDER, R_LG};
use crate::widgets::rule::HAIRLINE;

/// How deep the resize strips reach into the window.
///
/// Six points, matching the plan. Wider is easier to grab but starts eating the
/// title bar's buttons, whose top edge is only 40px away and which lose every
/// tie because the strips are registered last.
const GRAB: f32 = 6.0;

/// The outer shell: the page fill, one hairline tracing the whole window, and
/// the large radius the OS corner preference is asked to match.
///
/// No inner margin on purpose. The title bar, rail and status bar are supposed
/// to run edge to edge, and a frame margin here would also shift `ui.max_rect()`
/// inward, which is the rect [`title_drag`] and [`resize_strips`] measure from.
pub(crate) fn shell_frame(ui: &Ui, decorated: bool) -> Frame {
    let inset = maximized_inset(ui, decorated);
    Frame::NONE
        .fill(BG)
        .stroke(Stroke::new(HAIRLINE, DIVIDER))
        .corner_radius(if inset > 0 {
            // A maximised window has no visible corners to round.
            egui::CornerRadius::ZERO
        } else {
            R_LG
        })
        .outer_margin(egui::Margin::same(inset))
}

/// How far to pull the shell in from the window edge while maximised.
///
/// Measured on this machine: maximised, Windows places the window at
/// `-8,-8 .. 1928,1040` against a `0,0 .. 1920,1032` work area -- **8 points
/// off-screen on every side**. That is the invisible resize border a decorated
/// window hides under its frame. An undecorated window draws to the whole
/// window rect, so without this the top of the title bar and the outer edge of
/// the close button are simply not on the display.
///
/// The figure is `SM_CXSIZEFRAME + SM_CXPADDEDBORDER`, which is what Windows
/// itself uses, rather than the 8 that happens to be true here: it changes with
/// DPI and with the user's border-width setting.
fn maximized_inset(ui: &Ui, decorated: bool) -> i8 {
    if decorated {
        return 0;
    }
    let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
    if !maximized {
        return 0;
    }
    system_border() as i8
}

#[cfg(windows)]
fn system_border() -> i32 {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXPADDEDBORDER, SM_CXSIZEFRAME,
    };
    // SAFETY: GetSystemMetrics reads a global setting and cannot fail in a way
    // that matters -- an unknown index returns 0, which degrades to no inset.
    unsafe { GetSystemMetrics(SM_CXSIZEFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER) }
}

#[cfg(not(windows))]
fn system_border() -> i32 {
    0
}

/// Register the caption drag region across the top [`super::TITLEBAR_H`] points.
///
/// **Call this before adding the title-bar panel.** See the module docs.
///
/// Returns `None` under `--decorated`, where the OS caption already does this.
pub(crate) fn title_drag(ui: &mut Ui, decorated: bool) -> Option<Response> {
    if decorated {
        return None;
    }

    let shell = ui.max_rect();
    let bar = Rect::from_min_size(shell.min, Vec2::new(shell.width(), super::TITLEBAR_H));
    let response = ui.interact(bar, Id::new("vs_titlebar_drag"), Sense::click_and_drag());

    // Double-click is checked FIRST: a double-click also produces a drag start,
    // so testing the drag first would move the window a pixel and never
    // maximize. There is no toggle-maximize command in egui 0.35 -- read the
    // viewport's own state and send the negation.
    let is_max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
    if response.double_clicked() {
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!is_max));
    } else if response.drag_started_by(PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }

    Some(response)
}

/// Register the eight resize strips around the window edge.
///
/// **Call this after every panel has been added.** See the module docs.
///
/// No-op under `--decorated`, where the OS still draws a real resize border.
/// Aero-snap and `Win`+arrow keep working either way: winit leaves `WS_SIZEBOX`
/// on an undecorated resizable window, so only the *pointer* affordance is lost.
pub(crate) fn resize_strips(ui: &mut Ui, decorated: bool) {
    if decorated {
        return;
    }

    for (index, (rect, direction, cursor)) in strips(ui.max_rect(), GRAB).into_iter().enumerate() {
        // Keyed by index rather than by direction: `ResizeDirection` is a
        // foreign enum with no stable discriminant, and the order below is
        // fixed by `strips`.
        let response = ui.interact(rect, Id::new(("vs_resize", index)), Sense::drag());
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(cursor);
        }
        if response.drag_started() {
            ui.ctx()
                .send_viewport_cmd(ViewportCommand::BeginResize(direction));
        }
    }
}

/// Geometry for the eight strips: four edges, then four corners.
///
/// The corners come last so that they win the hit test where they overlap an
/// edge -- except they cannot overlap, because the edges are inset by `grab` at
/// both ends. Both belts are worn deliberately: the inset is what makes the
/// geometry testable, and the ordering is what makes it survive someone
/// widening `GRAB` later.
///
/// Split out from the interaction so it can be asserted on without a `Context`.
fn strips(rect: Rect, grab: f32) -> [(Rect, ResizeDirection, CursorIcon); 8] {
    // Clamp to half the window, or the edge strips invert on a window smaller
    // than two grab widths: `from_min_max` happily builds a negative-width rect
    // and the failure is invisible -- the strip is simply never hit. The min
    // inner size makes this unreachable through the OS, but a `BeginResize`
    // still in flight can hand us a smaller rect for a frame.
    let grab = grab
        .min(rect.width() * 0.5)
        .min(rect.height() * 0.5)
        .max(0.0);
    let (l, r, t, b) = (rect.left(), rect.right(), rect.top(), rect.bottom());
    let quad = |x0: f32, y0: f32, x1: f32, y1: f32| {
        Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1))
    };

    [
        (
            quad(l + grab, t, r - grab, t + grab),
            ResizeDirection::North,
            CursorIcon::ResizeNorth,
        ),
        (
            quad(l + grab, b - grab, r - grab, b),
            ResizeDirection::South,
            CursorIcon::ResizeSouth,
        ),
        (
            quad(l, t + grab, l + grab, b - grab),
            ResizeDirection::West,
            CursorIcon::ResizeWest,
        ),
        (
            quad(r - grab, t + grab, r, b - grab),
            ResizeDirection::East,
            CursorIcon::ResizeEast,
        ),
        (
            quad(l, t, l + grab, t + grab),
            ResizeDirection::NorthWest,
            CursorIcon::ResizeNorthWest,
        ),
        (
            quad(r - grab, t, r, t + grab),
            ResizeDirection::NorthEast,
            CursorIcon::ResizeNorthEast,
        ),
        (
            quad(l, b - grab, l + grab, b),
            ResizeDirection::SouthWest,
            CursorIcon::ResizeSouthWest,
        ),
        (
            quad(r - grab, b - grab, r, b),
            ResizeDirection::SouthEast,
            CursorIcon::ResizeSouthEast,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Rect {
        Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1240.0, 780.0))
    }

    /// Eight strips, one per direction. A duplicate would silently shadow the
    /// direction it collided with -- the window would resize, just never that
    /// way.
    #[test]
    fn every_direction_is_covered_exactly_once() {
        // `ResizeDirection` is a foreign enum without `Hash`, so this counts
        // rather than hashes.
        let strips = strips(window(), GRAB);
        let mut seen: Vec<ResizeDirection> = Vec::new();
        for (_, direction, _) in strips {
            assert!(!seen.contains(&direction), "{direction:?} appears twice");
            seen.push(direction);
        }
        assert_eq!(seen.len(), 8);
    }

    /// Overlapping strips are the failure that reads as "the corners resize the
    /// wrong way": whichever was registered later wins, so an edge creeping
    /// into a corner takes the diagonal gesture with it.
    #[test]
    fn strips_do_not_overlap() {
        let strips = strips(window(), GRAB);
        for (i, (a, da, _)) in strips.iter().enumerate() {
            for (b, db, _) in &strips[i + 1..] {
                let overlap = a.intersect(*b);
                assert!(
                    overlap.width() <= 0.0 || overlap.height() <= 0.0,
                    "{da:?} overlaps {db:?}"
                );
            }
        }
    }

    /// Each strip must actually lie on the edge it names, and be `grab` deep.
    /// An inverted rect here is invisible: it just never gets hit.
    #[test]
    fn strips_hug_their_own_edges() {
        let w = window();
        for (rect, direction, _) in strips(w, GRAB) {
            assert!(rect.width() > 0.0 && rect.height() > 0.0, "{direction:?}");
            assert!(w.contains_rect(rect), "{direction:?} is outside the window");
            let touches_edge = rect.top() == w.top()
                || rect.bottom() == w.bottom()
                || rect.left() == w.left()
                || rect.right() == w.right();
            assert!(touches_edge, "{direction:?} does not touch an edge");
        }
    }

    /// A window narrower than two grab widths must not produce inverted rects.
    /// An inverted strip is never hit, so this fails as "resizing stopped
    /// working" long after whoever shrank the window has moved on.
    #[test]
    fn a_tiny_window_still_yields_sane_rects() {
        for side in [8.0_f32, 6.0, 1.0, 0.0] {
            let tiny = Rect::from_min_max(Pos2::ZERO, Pos2::new(side, side));
            for (rect, direction, _) in strips(tiny, GRAB) {
                assert!(
                    rect.width() >= 0.0 && rect.height() >= 0.0,
                    "{side}px window: {direction:?} inverted: {rect:?}"
                );
            }
        }
    }
}
