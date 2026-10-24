//! Vector shapes: the notch outline, squircles (continuous corners),
//! capsules and progress rings.

use std::f64::consts::{FRAC_PI_2, TAU};

use vello::kurbo::{Arc, BezPath, Point, Rect, RoundedRect, SvgArc, Vec2};

/// Cubic Bézier constant for a quarter circle.
const KAPPA: f64 = 0.552_284_749_831;
/// Corner smoothing of iOS continuous corners.
pub const SQUIRCLE_SMOOTHING: f64 = 0.6;
/// Radius of the inverted "shoulders" that flare into the top screen edge.
pub const SHOULDER_RADIUS: f64 = 14.0;

/// Parameters of one smoothed corner (Figma's corner smoothing model):
/// a straight run `a`, a transition curve (`b`, `c`, `d`), a circular arc
/// whose chord spans `arc` on both axes, and the mirrored transition.
#[derive(Clone, Copy, Debug)]
struct CornerParams {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    p: f64,
    arc: f64,
    radius: f64,
}

impl CornerParams {
    fn new(radius: f64, smoothing: f64, budget: f64) -> Self {
        let radius = radius.clamp(0.0, budget.max(0.0));
        if radius <= 0.01 {
            return Self { a: 0.0, b: 0.0, c: 0.0, d: 0.0, p: 0.0, arc: 0.0, radius: 0.0 };
        }
        let mut p = (1.0 + smoothing) * radius;
        let max_smoothing = (budget / radius - 1.0).max(0.0);
        let smoothing = smoothing.min(max_smoothing);
        p = p.min(budget);
        let arc_measure = 90.0 * (1.0 - smoothing);
        let arc = (arc_measure / 2.0).to_radians().sin() * radius * std::f64::consts::SQRT_2;
        let angle_alpha = (90.0 - arc_measure) / 2.0;
        let p3_to_p4 = radius * (angle_alpha / 2.0).to_radians().tan();
        let angle_beta = (45.0 * smoothing).to_radians();
        let c = p3_to_p4 * angle_beta.cos();
        let d = c * angle_beta.tan();
        let b = ((p - arc - c - d) / 3.0).max(0.0);
        let a = 2.0 * b;
        Self { a, b, c, d, p, arc, radius }
    }

    /// Appends the corner to `path`. The current point must be
    /// `corner - p * u`; the corner ends at `corner + p * v`. `u` is the
    /// direction of travel before the corner, `v` after it (a clockwise turn
    /// in screen coordinates).
    fn append(&self, path: &mut BezPath, corner: Point, u: Vec2, v: Vec2) {
        if self.p <= 0.0 {
            path.line_to(corner);
            return;
        }
        let Self { a, b, c, d, p, arc, radius } = *self;
        let start = corner - u * p;
        let at = |x: f64, y: f64| start + u * x + v * y;
        path.curve_to(at(a, 0.0), at(a + b, 0.0), at(a + b + c, d));
        let arc_from = at(a + b + c, d);
        let arc_to = at(a + b + c + arc, d + arc);
        let svg = SvgArc {
            from: arc_from,
            to: arc_to,
            radii: Vec2::new(radius, radius),
            x_rotation: 0.0,
            large_arc: false,
            sweep: true,
        };
        match Arc::from_svg_arc(&svg) {
            Some(arc_shape) => path.extend(arc_shape.append_iter(0.05)),
            None => path.line_to(arc_to),
        }
        let base = arc_to;
        let at2 = |x: f64, y: f64| base + u * x + v * y;
        path.curve_to(at2(d, c), at2(d, b + c), at2(d, a + b + c));
    }
}

/// Outline of the notch: flush with the top edge (y = 0), concave shoulders
/// of radius `shoulder` outside the body at the top-left/right, straight
/// sides and continuous (squircle) bottom corners.
pub fn notch_path(center_x: f64, width: f64, height: f64, radius: f64, shoulder: f64) -> BezPath {
    let width = width.max(2.0);
    let height = height.max(2.0);
    let left = center_x - width / 2.0;
    let right = center_x + width / 2.0;
    let s = shoulder.clamp(0.0, (height * 0.45).min(width * 0.25));
    let budget = (width / 2.0).min(height - s).max(0.0);
    let corner = CornerParams::new(radius, SQUIRCLE_SMOOTHING, budget);
    let p = corner.p;

    let mut path = BezPath::new();
    path.move_to((left - s, 0.0));
    path.line_to((right + s, 0.0));
    // Right shoulder: concave quarter circle centered at (right + s, s).
    path.curve_to((right + s - s * KAPPA, 0.0), (right, s - s * KAPPA), (right, s));
    path.line_to((right, height - p));
    corner.append(&mut path, Point::new(right, height), Vec2::new(0.0, 1.0), Vec2::new(-1.0, 0.0));
    path.line_to((left + p, height));
    corner.append(&mut path, Point::new(left, height), Vec2::new(-1.0, 0.0), Vec2::new(0.0, -1.0));
    path.line_to((left, s));
    // Left shoulder: concave quarter circle centered at (left - s, s).
    path.curve_to((left, s - s * KAPPA), (left - s + s * KAPPA, 0.0), (left - s, 0.0));
    path.close_path();
    path
}

/// Rectangle with continuous (squircle) corners on all four sides.
pub fn squircle(rect: Rect, radius: f64) -> BezPath {
    let budget = rect.width().min(rect.height()) / 2.0;
    let corner = CornerParams::new(radius, SQUIRCLE_SMOOTHING, budget);
    let p = corner.p;
    let mut path = BezPath::new();
    path.move_to((rect.x0 + p, rect.y0));
    path.line_to((rect.x1 - p, rect.y0));
    corner.append(&mut path, Point::new(rect.x1, rect.y0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0));
    path.line_to((rect.x1, rect.y1 - p));
    corner.append(&mut path, Point::new(rect.x1, rect.y1), Vec2::new(0.0, 1.0), Vec2::new(-1.0, 0.0));
    path.line_to((rect.x0 + p, rect.y1));
    corner.append(&mut path, Point::new(rect.x0, rect.y1), Vec2::new(-1.0, 0.0), Vec2::new(0.0, -1.0));
    path.line_to((rect.x0, rect.y0 + p));
    corner.append(&mut path, Point::new(rect.x0, rect.y0), Vec2::new(0.0, -1.0), Vec2::new(1.0, 0.0));
    path.close_path();
    path
}

/// Fully rounded rectangle (pill).
pub fn capsule(rect: Rect) -> RoundedRect {
    RoundedRect::from_rect(rect, rect.width().min(rect.height()) / 2.0)
}

/// Arc of a progress ring starting at 12 o'clock, clockwise. `progress` is
/// clamped to `0..=1`.
pub fn progress_arc(center: Point, radius: f64, progress: f64) -> Arc {
    Arc::new(center, (radius, radius), -FRAC_PI_2, TAU * progress.clamp(0.0, 1.0), 0.0)
}

/// Full circle as an arc (for stroked ring tracks).
pub fn ring(center: Point, radius: f64) -> Arc {
    Arc::new(center, (radius, radius), -FRAC_PI_2, TAU, 0.0)
}
