//! Rules -- the design's signature 1px line that ramps out of nothing at both
//! ends instead of stopping dead.
//!
//! egui has no gradient anywhere: `Frame::fill` and `RectShape::fill` are a
//! single flat `Color32`, and there is no shape that carries per-corner colour.
//! The fade therefore has to be a hand-built `epaint::Mesh` -- three quads
//! (transparent to colour, solid, colour back to transparent) whose vertex
//! colours the tessellator interpolates for free. Six triangles a rule against
//! roughly forty visible rows is nothing.
//!
//! Two rules of the road:
//!
//! * `colored_vertex` debug-asserts `TextureId::default()`, so a mesh built
//!   here must never be mixed with textured vertices.
//! * `ui.separator()` is banned project-wide -- `Style::separator_style`
//!   hard-codes a 6.0 gap and is a method rather than a field, so there is no
//!   way to retune it -- which makes this module the only separator in the kit.

#![allow(dead_code)] // The views adopt the kit in the next commit.

use eframe::egui::{epaint::Mesh, Color32, CornerRadius, Painter, Pos2, Rect, Shape, Ui, Vec2};

use crate::theme::tokens::{DIVIDER, S3};

/// How far a rule ramps from transparent to full colour at each end. One deck
/// baseline unit in the design. Deliberately *not* on the density scale: the
/// fade is a fixed optical effect, not part of the spacing rhythm, and a 34px
/// ramp at Compact would read as a different treatment rather than a tighter
/// one.
pub(crate) const FADE: f32 = 48.0;

/// Every rule in the system is one hairline thick.
pub(crate) const HAIRLINE: f32 = 1.0;

/// The ramp length actually used for a rule spanning `length` points.
///
/// Below `2 * FADE` the two ramps would overlap: the middle quad would take a
/// negative width, the mesh would fold back through itself and the overlap
/// would paint at double alpha. Clamping the ramp to half the span turns a
/// short rule into a clean symmetric peak instead, which is what the CSS
/// gradient does too once its two colour stops cross.
fn ramp_len(length: f32) -> f32 {
    if !length.is_finite() || length <= 0.0 {
        return 0.0;
    }
    FADE.min(length * 0.5)
}

/// Geometry for a rule that fades along one axis of `rect`.
///
/// Split out from the painting so the mesh can be asserted on in a unit test:
/// building it needs no `Context`, no fonts and no texture atlas.
fn fade_mesh(rect: Rect, color: Color32, along_x: bool) -> Mesh {
    let (near, far) = if along_x {
        (rect.left(), rect.right())
    } else {
        (rect.top(), rect.bottom())
    };
    // The cross axis: the rule's thickness, constant across every stop.
    let (lo, hi) = if along_x {
        (rect.top(), rect.bottom())
    } else {
        (rect.left(), rect.right())
    };

    let ramp = ramp_len(far - near);
    let stops = [near, near + ramp, far - ramp, far];
    // Vertex colours are premultiplied, so interpolating towards
    // `TRANSPARENT` (0,0,0,0) fades the alpha without dragging the hue to
    // black on the way out.
    let colors = [Color32::TRANSPARENT, color, color, Color32::TRANSPARENT];

    let mut mesh = Mesh::default();
    mesh.reserve_vertices(stops.len() * 2);
    mesh.reserve_triangles((stops.len() - 1) * 2);

    for (stop, color) in stops.into_iter().zip(colors) {
        let (a, b) = if along_x {
            (Pos2::new(stop, lo), Pos2::new(stop, hi))
        } else {
            (Pos2::new(lo, stop), Pos2::new(hi, stop))
        };
        mesh.colored_vertex(a, color);
        mesh.colored_vertex(b, color);
    }

    // Stop `i` owns vertices `2i` (near edge) and `2i + 1` (far edge), so each
    // band between two stops is the quad `2i, 2i+1, 2i+3, 2i+2`.
    for band in 0..(stops.len() as u32 - 1) {
        let i = band * 2;
        mesh.add_triangle(i, i + 1, i + 3);
        mesh.add_triangle(i, i + 3, i + 2);
    }

    mesh
}

/// Paint a horizontal rule filling `rect`, fading out over [`FADE`] at each end.
pub(crate) fn faded_hline(painter: &Painter, rect: Rect, color: Color32) {
    painter.add(Shape::mesh(fade_mesh(rect, color, true)));
}

/// Paint a vertical rule filling `rect`, fading out over [`FADE`] at each end.
pub(crate) fn faded_vline(painter: &Painter, rect: Rect, color: Color32) {
    painter.add(Shape::mesh(fade_mesh(rect, color, false)));
}

/// Paint a horizontal rule filling `rect` with no fade.
///
/// The design fades freestanding rules and table row rules; box outlines and
/// separators *inside* a control stay solid, because a segmented control whose
/// internal seams fade reads as a rendering bug rather than as a flourish.
pub(crate) fn solid_hline(painter: &Painter, rect: Rect, color: Color32) {
    painter.rect_filled(rect, CornerRadius::ZERO, color);
}

/// Paint a vertical rule filling `rect` with no fade. See [`solid_hline`].
pub(crate) fn solid_vline(painter: &Painter, rect: Rect, color: Color32) {
    painter.rect_filled(rect, CornerRadius::ZERO, color);
}

/// A freestanding horizontal rule across the width of `ui`, with the design's
/// breathing room above and below. This is the replacement for
/// `ui.separator()`.
pub(crate) fn hrule(ui: &mut Ui) {
    hrule_colored(ui, DIVIDER);
}

/// [`hrule`] in a given colour -- section marks under an accent heading, or a
/// status-tinted divider.
pub(crate) fn hrule_colored(ui: &mut Ui, color: Color32) {
    ui.add_space(S3);
    let (_, rect) = ui.allocate_space(Vec2::new(ui.available_width(), HAIRLINE));
    faded_hline(ui.painter(), rect, color);
    ui.add_space(S3);
}

/// A vertical hairline down the height of `ui`, for splitting a toolbar row.
pub(crate) fn vrule(ui: &mut Ui, color: Color32) {
    ui.add_space(S3);
    let (_, rect) = ui.allocate_space(Vec2::new(HAIRLINE, ui.available_height()));
    faded_vline(ui.painter(), rect, color);
    ui.add_space(S3);
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{epaint::TextureId, Pos2};

    fn wide() -> Rect {
        Rect::from_min_max(Pos2::new(0.0, 10.0), Pos2::new(200.0, 11.0))
    }

    /// Three quads, eight vertices, and no texture. If the mesh ever picks up a
    /// texture id, `colored_vertex` starts debug-asserting and the rule
    /// disappears in release while panicking in debug.
    #[test]
    fn fade_is_three_untextured_quads() {
        let mesh = fade_mesh(wide(), DIVIDER, true);
        assert_eq!(mesh.vertices.len(), 8);
        assert_eq!(mesh.indices.len(), 18, "3 quads = 6 triangles");
        assert_eq!(mesh.texture_id, TextureId::default());
        assert!(mesh.is_valid());
    }

    /// The whole point of the shape: invisible at the ends, full strength in
    /// the middle.
    #[test]
    fn ends_are_clear_and_the_middle_is_solid() {
        let mesh = fade_mesh(wide(), DIVIDER, true);
        for i in [0, 1, 6, 7] {
            assert_eq!(mesh.vertices[i].color, Color32::TRANSPARENT, "vertex {i}");
        }
        for i in [2, 3, 4, 5] {
            assert_eq!(mesh.vertices[i].color, DIVIDER, "vertex {i}");
        }
    }

    /// A rule with room for both ramps gets the full 48px at each end and keeps
    /// its thickness across every stop.
    #[test]
    fn a_wide_rule_uses_the_full_ramp() {
        let rect = wide();
        let mesh = fade_mesh(rect, DIVIDER, true);
        let xs: Vec<f32> = mesh.vertices.iter().step_by(2).map(|v| v.pos.x).collect();
        assert_eq!(xs, vec![0.0, FADE, 200.0 - FADE, 200.0]);
        for v in &mesh.vertices {
            assert!(v.pos.y == rect.top() || v.pos.y == rect.bottom());
        }
    }

    /// Narrower than two ramps, the fade has to shrink or the middle quad
    /// inverts. The stops must stay monotonic -- that is what "not a broken
    /// mesh" means here.
    #[test]
    fn a_narrow_rule_splits_the_ramp_instead_of_folding() {
        for width in [96.0_f32, 60.0, 8.0, 1.0, 0.0] {
            let rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(width, HAIRLINE));
            let mesh = fade_mesh(rect, DIVIDER, true);
            let xs: Vec<f32> = mesh.vertices.iter().step_by(2).map(|v| v.pos.x).collect();
            assert!(
                xs.windows(2).all(|w| w[0] <= w[1]),
                "width {width}: stops {xs:?} are not monotonic"
            );
            assert!(
                xs[1] <= width * 0.5 + f32::EPSILON,
                "width {width}: ramp too long"
            );
            assert_eq!(xs[0], 0.0);
            assert_eq!(xs[3], width);
        }
    }

    /// The vertical variant must fade down the rule, not across it -- easy to
    /// get backwards, and it renders as a rule that is simply missing.
    #[test]
    fn the_vertical_variant_fades_along_y() {
        let rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(HAIRLINE, 200.0));
        let mesh = fade_mesh(rect, DIVIDER, false);
        let ys: Vec<f32> = mesh.vertices.iter().step_by(2).map(|v| v.pos.y).collect();
        assert_eq!(ys, vec![0.0, FADE, 200.0 - FADE, 200.0]);
        for v in &mesh.vertices {
            assert!(v.pos.x == rect.left() || v.pos.x == rect.right());
        }
    }
}
