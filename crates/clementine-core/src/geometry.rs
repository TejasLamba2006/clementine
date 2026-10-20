//! Radial petal geometry, ported 1:1 from Tangerine `home.js` (`F`, `be`, `we`, `Ke`).
use std::f64::consts::PI;

pub const WHEEL_DIAMETER: f64 = 272.0;
pub const INNER_RADIUS: f64 = 48.64;
pub const OUTER_RADIUS: f64 = 119.52;
pub const BACKDROP_RADIUS: f64 = 125.92;
pub const HOLE_MASK_RADIUS: f64 = 42.24;
pub const PETAL_GAP: f64 = 4.0;
pub const PETAL_CORNER: f64 = 20.16;
pub const LABEL_INNER: f64 = 40.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point2D {
    pub x: f64,
    pub y: f64,
}

/// Control points of one petal. Path order:
/// M start, L outer_line_end, Q outer_quad1_ctrl outer_arc_start,
/// A outer (sweep 0) outer_arc_end, Q outer_quad2_ctrl inner_line_start,
/// L inner_line_end, Q inner_quad1_ctrl inner_arc_start,
/// A inner (sweep 1) inner_arc_end, Q inner_quad2_ctrl start, Z.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PetalGeometry {
    pub start: Point2D,
    pub outer_line_end: Point2D,
    pub outer_quad1_ctrl: Point2D,
    pub outer_arc_start: Point2D,
    pub outer_arc_end: Point2D,
    pub outer_arc_large: bool,
    pub outer_quad2_ctrl: Point2D,
    pub inner_line_start: Point2D,
    pub inner_line_end: Point2D,
    pub inner_quad1_ctrl: Point2D,
    pub inner_arc_start: Point2D,
    pub inner_arc_end: Point2D,
    pub inner_arc_large: bool,
    pub inner_quad2_ctrl: Point2D,
}

/// `F(e, t)`
pub fn petal_angle(index: usize, count: usize) -> f64 {
    let base = if count == 3 { PI } else { PI / 2.0 };
    base - PI * 2.0 * index as f64 / count as f64
}

/// `be(e, t)`: y axis points down.
pub fn polar_to_cartesian(radius: f64, angle: f64) -> (f64, f64) {
    (angle.cos() * radius, -angle.sin() * radius)
}

fn pt(radius: f64, angle: f64) -> Point2D {
    let (x, y) = polar_to_cartesian(radius, angle);
    Point2D { x, y }
}

/// `we(e, t)` as structured points.
pub fn build_petal_geometry(index: usize, count: usize) -> PetalGeometry {
    let (inner, outer) = (INNER_RADIUS, OUTER_RADIUS);
    let a = petal_angle(index, count);
    let n = PI * 2.0 / count as f64;
    let l = a - n / 2.0;
    let s = a + n / 2.0;
    let r = |j: f64| (PETAL_GAP / 2.0 / j.max(PETAL_GAP / 2.0 + 0.01)).min(0.95).asin();
    let d = l + r(outer);
    let i = s - r(outer);
    let c = l + r(inner);
    let g = s - r(inner);
    let f = 0.15_f64.min((i - d) * 0.24);
    let u = 0.18_f64.min((g - c) * 0.18);
    let v = inner / u.cos().max(0.01);
    let y = v + inner * u.tan();
    let o = outer - PETAL_CORNER;
    PetalGeometry {
        start: pt(y, l + r(y)),
        outer_line_end: pt(o, l + r(o)),
        outer_quad1_ctrl: pt(outer, d),
        outer_arc_start: pt(outer, d + f),
        outer_arc_end: pt(outer, i - f),
        outer_arc_large: i - f - d - f > PI,
        outer_quad2_ctrl: pt(outer, i),
        inner_line_start: pt(o, s - r(o)),
        inner_line_end: pt(y, s - r(y)),
        inner_quad1_ctrl: pt(v, s - r(v)),
        inner_arc_start: pt(inner, g - u),
        inner_arc_end: pt(inner, c + u),
        inner_arc_large: g - u - c - u > PI,
        inner_quad2_ctrl: pt(v, l + r(v)),
    }
}

fn w(p: Point2D) -> String {
    format!("{:.4} {:.4}", p.x, p.y)
}

/// SVG `d` string identical to `we(e, t)`.
pub fn build_petal_svg_path(index: usize, count: usize) -> String {
    let g = build_petal_geometry(index, count);
    format!(
        "M {} L {} Q {} {} A {} {} 0 {} 0 {} Q {} {} L {} Q {} {} A {} {} 0 {} 1 {} Q {} {} Z",
        w(g.start),
        w(g.outer_line_end),
        w(g.outer_quad1_ctrl),
        w(g.outer_arc_start),
        OUTER_RADIUS,
        OUTER_RADIUS,
        g.outer_arc_large as u8,
        w(g.outer_arc_end),
        w(g.outer_quad2_ctrl),
        w(g.inner_line_start),
        w(g.inner_line_end),
        w(g.inner_quad1_ctrl),
        w(g.inner_arc_start),
        INNER_RADIUS,
        INNER_RADIUS,
        g.inner_arc_large as u8,
        w(g.inner_arc_end),
        w(g.inner_quad2_ctrl),
        w(g.start),
    )
}

fn quad(p0: Point2D, c: Point2D, p1: Point2D, steps: usize, out: &mut Vec<Point2D>) {
    for k in 1..=steps {
        let t = k as f64 / steps as f64;
        let m = 1.0 - t;
        out.push(Point2D {
            x: m * m * p0.x + 2.0 * m * t * c.x + t * t * p1.x,
            y: m * m * p0.y + 2.0 * m * t * c.y + t * t * p1.y,
        });
    }
}

/// Arc around the origin; `ccw` = increasing math angle (SVG sweep 0 here).
fn arc(p0: Point2D, p1: Point2D, ccw: bool, steps: usize, out: &mut Vec<Point2D>) {
    let rad = p0.x.hypot(p0.y);
    let a0 = (-p0.y).atan2(p0.x);
    let mut a1 = (-p1.y).atan2(p1.x);
    if ccw {
        while a1 < a0 {
            a1 += PI * 2.0;
        }
    } else {
        while a1 > a0 {
            a1 -= PI * 2.0;
        }
    }
    for k in 1..=steps {
        out.push(pt(rad, a0 + (a1 - a0) * k as f64 / steps as f64));
    }
}

impl PetalGeometry {
    /// Closed polygon approximating the petal outline. Quads use 8 steps.
    pub fn sample_polygon(&self, arc_steps: usize) -> Vec<Point2D> {
        let arc_steps = arc_steps.max(1);
        let mut p = vec![self.start, self.outer_line_end];
        quad(self.outer_line_end, self.outer_quad1_ctrl, self.outer_arc_start, 8, &mut p);
        arc(self.outer_arc_start, self.outer_arc_end, true, arc_steps, &mut p);
        quad(self.outer_arc_end, self.outer_quad2_ctrl, self.inner_line_start, 8, &mut p);
        p.push(self.inner_line_end);
        quad(self.inner_line_end, self.inner_quad1_ctrl, self.inner_arc_start, 8, &mut p);
        arc(self.inner_arc_start, self.inner_arc_end, false, arc_steps, &mut p);
        quad(self.inner_arc_end, self.inner_quad2_ctrl, self.start, 8, &mut p);
        p
    }

    /// Even-odd point-in-polygon on the sampled outline.
    pub fn contains_point(&self, x: f64, y: f64) -> bool {
        let poly = self.sample_polygon(48);
        let mut inside = false;
        let mut j = poly.len() - 1;
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[j]);
            if (a.y > y) != (b.y > y) && x < (b.x - a.x) * (y - a.y) / (b.y - a.y) + a.x {
                inside = !inside;
            }
            j = i;
        }
        inside
    }
}

/// Label center: `r = LABEL_INNER + (OUTER_RADIUS - LABEL_INNER) * 0.56`.
pub fn label_position(index: usize, count: usize) -> Point2D {
    pt(
        LABEL_INNER + (OUTER_RADIUS - LABEL_INNER) * 0.56,
        petal_angle(index, count),
    )
}

/// `Ke(e, t, a)`: analytic polar hit-test. `dx`/`dy` relative to wheel center, y down.
pub fn hit_test_petal(dx: f64, dy: f64, count: usize) -> Option<usize> {
    let n = dx.hypot(dy);
    if count == 0 || n < INNER_RADIUS || n > OUTER_RADIUS {
        return None;
    }
    let l = (-dy).atan2(dx);
    let s = PI * 2.0 / count as f64;
    let r = ((petal_angle(0, count) - l) % (PI * 2.0) + PI * 2.0) % (PI * 2.0);
    let d = (r / s).round() as usize % count;
    let diff = l - petal_angle(d, count);
    if diff.sin().atan2(diff.cos()).abs() > s / 2.0 - (PETAL_GAP / 2.0 / n).asin() {
        None
    } else {
        Some(d)
    }
}

/// `cubic-bezier(0.18, 0.82, 0.24, 1)` easing.
pub fn bloom_ease(t: f64) -> f64 {
    let (x1, y1, x2, y2) = (0.18, 0.82, 0.24, 1.0);
    let t = t.clamp(0.0, 1.0);
    let b = |a: f64, c: f64, u: f64| {
        let m = 1.0 - u;
        3.0 * m * m * u * a + 3.0 * m * u * u * c + u * u * u
    };
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if b(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    b(y1, y2, (lo + hi) / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_matches_js_reference() {
        assert_eq!(build_petal_svg_path(0, 4), "M 39.7786 -42.6070 L 68.8297 -71.6581 Q 83.0874 -85.9158 69.3153 -97.3675 A 119.52 119.52 0 0 0 -69.3153 -97.3675 Q -83.0874 -85.9158 -68.8297 -71.6581 L -39.7786 -42.6070 Q -33.5156 -36.3441 -26.0126 -41.0998 A 48.64 48.64 0 0 1 26.0126 -41.0998 Q 33.5156 -36.3441 39.7786 -42.6070 Z");
        assert_eq!(build_petal_svg_path(1, 3), "M 58.2554 -2.0000 L 99.3399 -2.0000 Q 119.5033 -2.0000 117.8625 -19.8359 A 119.52 119.52 0 0 0 -41.7529 -111.9899 Q -58.0196 -104.4929 -47.9379 -87.0309 L -27.3957 -51.4507 Q -22.9671 -43.7802 -14.4888 -46.4319 A 48.64 48.64 0 0 1 47.4556 -10.6683 Q 49.3983 -2.0000 58.2554 -2.0000 Z");
        assert_eq!(build_petal_svg_path(0, 1), "M 2.0000 58.2554 L 2.0000 99.3399 Q 2.0000 119.5033 19.8359 117.8625 A 119.52 119.52 0 1 0 -19.8359 117.8625 Q -2.0000 119.5033 -2.0000 99.3399 L -2.0000 58.2554 Q -2.0000 49.3983 -10.6683 47.4556 A 48.64 48.64 0 1 1 10.6683 47.4556 Q 2.0000 49.3983 2.0000 58.2554 Z");
    }

    #[test]
    fn angles_and_label() {
        assert!((petal_angle(0, 3) - PI).abs() < 1e-12);
        assert!((petal_angle(0, 4) - PI / 2.0).abs() < 1e-12);
        let l = label_position(0, 4);
        assert!(l.x.abs() < 1e-9 && (l.y + 84.5312).abs() < 1e-3);
    }

    #[test]
    fn petals_1_to_9_hit_own_center_and_polygon() {
        for count in 1..=9 {
            for i in 0..count {
                let p = build_petal_svg_path(i, count);
                assert!(p.starts_with("M ") && p.ends_with(" Z"));
                let g = build_petal_geometry(i, count);
                assert!(g.sample_polygon(16).len() > 20);
                let c = label_position(i, count);
                assert_eq!(hit_test_petal(c.x, c.y, count), Some(i), "{count}/{i}");
                assert!(g.contains_point(c.x, c.y), "poly {count}/{i}");
                for j in (0..count).filter(|j| *j != i) {
                    assert!(!build_petal_geometry(j, count).contains_point(c.x, c.y));
                }
            }
        }
    }

    #[test]
    fn hit_test_gap_hole_and_outside() {
        let a = PI / 4.0;
        let (x, y) = polar_to_cartesian(80.0, a);
        assert_eq!(hit_test_petal(x, y, 4), None);
        assert_eq!(hit_test_petal(10.0, 10.0, 4), None);
        assert_eq!(hit_test_petal(0.0, 0.0, 4), None);
        assert_eq!(hit_test_petal(OUTER_RADIUS + 1.0, 0.0, 4), None);
        assert_eq!(hit_test_petal(0.0, -80.0, 0), None);
        let (x, y) = polar_to_cartesian(80.0, a + 0.2);
        assert_eq!(hit_test_petal(x, y, 4), Some(0));
    }

    #[test]
    fn bloom_endpoints_and_monotonic() {
        assert!(bloom_ease(0.0).abs() < 1e-9);
        assert!((bloom_ease(1.0) - 1.0).abs() < 1e-9);
        let mut prev = 0.0;
        for k in 1..=100 {
            let v = bloom_ease(k as f64 / 100.0);
            assert!(v >= prev);
            prev = v;
        }
        assert!(bloom_ease(0.5) > 0.5);
    }
}
