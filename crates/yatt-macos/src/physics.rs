//! Damped springs for card motion: position and rotation each chase a target.
//! Plain math with no AppKit, so it is tested on any OS. The constants are fitted
//! to the reference clip (docs/reference/card-motion.md).

/// A spring with unit mass: `k` is stiffness, `zeta` the damping ratio (1 = critical).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub k: f64,
    pub zeta: f64,
}

impl Spring {
    pub const fn new(k: f64, zeta: f64) -> Self {
        Self { k, zeta }
    }

    fn damping(self) -> f64 {
        2.0 * self.zeta * self.k.sqrt()
    }
}

/// Resting in a stack or fan.
pub const PILE: Spring = Spring::new(320.0, 0.95);
/// The card under the cursor while dragging: nearly attached, a touch of lag.
pub const LEAD: Spring = Spring::new(1200.0, 0.95);
/// A card settling into its list slot after a drop.
pub const SLOT: Spring = Spring::new(420.0, 1.0);
/// A new card flying from the new-task field into its column.
pub const FLY: Spring = Spring::new(170.0, 0.95);
/// The task modal growing out of its card and shrinking back.
pub const MORPH: Spring = Spring::new(260.0, 1.0);
/// Rotation follows faster than position, so tilt reads as a reaction to motion.
pub const ROTATION: Spring = Spring::new(500.0, 0.95);

/// The `i`-th card behind the lead (1-based): softer, so the train stretches out.
pub fn follower(i: usize) -> Spring {
    Spring::new(420.0 / (1.0 + 0.15 * i as f64), 0.9)
}

/// Degrees of tilt per point/second of horizontal speed.
pub const TILT_PER_SPEED: f64 = 0.012;
pub const LEAD_MAX_TILT: f64 = 4.0;
pub const FOLLOWER_MAX_TILT: f64 = 0.0;

/// Tilt in degrees (positive = clockwise on screen) for a horizontal speed in points/second.
pub fn tilt(vx: f64, max: f64) -> f64 {
    (vx * TILT_PER_SPEED).clamp(-max, max)
}

/// One card's motion state: top-left corner `(x, y)` in a y-down space, rotation `r` in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    pub x: f64,
    pub y: f64,
    pub r: f64,
    pub vx: f64,
    pub vy: f64,
    pub vr: f64,
    pub tx: f64,
    pub ty: f64,
    pub tr: f64,
    pub spring: Spring,
}

impl Body {
    pub fn at(x: f64, y: f64) -> Self {
        Self {
            x,
            y,
            r: 0.0,
            vx: 0.0,
            vy: 0.0,
            vr: 0.0,
            tx: x,
            ty: y,
            tr: 0.0,
            spring: PILE,
        }
    }

    pub fn target(&mut self, x: f64, y: f64, r: f64) {
        self.tx = x;
        self.ty = y;
        self.tr = r;
    }

    /// Advances by `dt` seconds, in small substeps so stiff springs stay stable.
    pub fn step(&mut self, dt: f64) {
        let steps = (dt / (1.0 / 480.0)).ceil().max(1.0) as usize;
        let h = dt / steps as f64;
        let (k, c) = (self.spring.k, self.spring.damping());
        let (kr, cr) = (ROTATION.k, ROTATION.damping());
        for _ in 0..steps {
            self.vx += (-k * (self.x - self.tx) - c * self.vx) * h;
            self.vy += (-k * (self.y - self.ty) - c * self.vy) * h;
            self.vr += (-kr * (self.r - self.tr) - cr * self.vr) * h;
            self.x += self.vx * h;
            self.y += self.vy * h;
            self.r += self.vr * h;
        }
    }

    /// Close enough to the target that drawing another frame changes nothing visible.
    pub fn at_rest(&self) -> bool {
        (self.x - self.tx).abs() < 0.25
            && (self.y - self.ty).abs() < 0.25
            && (self.r - self.tr).abs() < 0.05
            && self.vx.abs() < 2.0
            && self.vy.abs() < 2.0
            && self.vr.abs() < 0.5
    }

    pub fn settle(&mut self) {
        self.x = self.tx;
        self.y = self.ty;
        self.r = self.tr;
        self.vx = 0.0;
        self.vy = 0.0;
        self.vr = 0.0;
    }
}

/// The `i`-th card of a collapsed stack (0 = top): how far down its centre sits
/// and its scale. The cards behind are a touch narrower and peek out below the
/// top card as thin slivers; past the third they hide behind it.
pub fn pile(i: usize) -> (f64, f64) {
    let i = i.min(3) as f64;
    (8.0 * i, 1.0 - 0.06 * i)
}

/// Fan rotations in degrees (positive = clockwise), top card first: about −45° on
/// top to +20° at the bottom.
pub fn fan_angles(n: usize) -> Vec<f64> {
    if n <= 1 {
        return vec![0.0; n];
    }
    let sweep = (13.0 * (n - 1) as f64).min(65.0);
    let top = -sweep * 0.7;
    let step = sweep / (n - 1) as f64;
    (0..n).map(|i| top + step * i as f64).collect()
}

/// Fanned stack: the `i`-th card's centre relative to the stack's centre (y down)
/// and its rotation. The cards spread around a ring in stack order, starting just
/// right of the top and going clockwise, turning progressively as they go.
pub fn fan(i: usize, n: usize, radius: f64) -> (f64, f64, f64) {
    if n <= 1 {
        return (0.0, 0.0, 0.0);
    }
    let step = (300.0 / (n - 1) as f64).min(58.0);
    let t = (-80.0 + step * i as f64).to_radians();
    (radius * t.cos(), radius * 0.9 * t.sin(), fan_angles(n)[i])
}

/// Where a card dropped with its centre at `y` lands in a list whose first slot
/// starts at `top`, slots `pitch` apart, holding `n` cards.
pub fn insert_index(y: f64, top: f64, pitch: f64, n: usize) -> usize {
    (((y - top) / pitch).floor().max(0.0) as usize).min(n)
}

/// The `i`-th card trailing a dragged group (0 = lead) shrinks the further back it is.
pub fn trail_scale(i: usize) -> f64 {
    (1.0 - 0.07 * i as f64).max(0.6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pile_spring_settles_without_visible_overshoot() {
        let mut b = Body::at(0.0, 0.0);
        b.target(100.0, 0.0, 0.0);
        let mut max = 0.0f64;
        let mut t = 0.0;
        while !b.at_rest() && t < 2.0 {
            b.step(1.0 / 120.0);
            max = max.max(b.x);
            t += 1.0 / 120.0;
        }
        assert!(max < 100.3, "overshoot {max}");
        assert!(t > 0.2 && t < 0.7, "settled after {t}s");
    }

    #[test]
    fn followers_lag_more_the_further_back() {
        let mut lead = Body::at(0.0, 0.0);
        lead.spring = LEAD;
        let mut f1 = Body::at(0.0, 0.0);
        f1.spring = follower(1);
        let mut f3 = Body::at(0.0, 0.0);
        f3.spring = follower(3);
        for b in [&mut lead, &mut f1, &mut f3] {
            b.target(200.0, 0.0, 0.0);
            for _ in 0..6 {
                b.step(1.0 / 120.0);
            }
        }
        assert!(lead.x > f1.x && f1.x > f3.x);
    }

    #[test]
    fn insert_index_by_slot() {
        assert_eq!(insert_index(-50.0, 10.0, 100.0, 3), 0);
        assert_eq!(insert_index(60.0, 10.0, 100.0, 3), 0);
        assert_eq!(insert_index(115.0, 10.0, 100.0, 3), 1);
        assert_eq!(insert_index(900.0, 10.0, 100.0, 3), 3);
    }

    #[test]
    fn morph_spring_settles_quickly() {
        let mut b = Body::at(0.0, 0.0);
        b.spring = MORPH;
        b.target(400.0, 300.0, 0.0);
        let mut t = 0.0;
        while !b.at_rest() && t < 2.0 {
            b.step(1.0 / 120.0);
            t += 1.0 / 120.0;
        }
        assert!(t < 0.8, "settled after {t}s");
    }

    #[test]
    fn tilt_is_clamped_and_signed() {
        assert_eq!(tilt(10_000.0, LEAD_MAX_TILT), LEAD_MAX_TILT);
        assert_eq!(tilt(-10_000.0, LEAD_MAX_TILT), -LEAD_MAX_TILT);
        assert!((tilt(100.0, 12.0) - 1.2).abs() < 1e-9);
    }

    #[test]
    fn fan_spans_minus_45_to_plus_20_for_six_cards() {
        let a = fan_angles(6);
        assert!((a[0] + 45.5).abs() < 0.01);
        assert!((a[5] - 19.5).abs() < 0.01);
        assert_eq!(fan_angles(1), vec![0.0]);
    }

    #[test]
    fn fan_spreads_around_the_ring_and_trail_shrinks() {
        let (x0, y0, r0) = fan(0, 6, 100.0);
        let (_, y3, _) = fan(3, 6, 100.0);
        assert!(y0 < 0.0 && x0 > 0.0 && r0 < -40.0);
        assert!(y3 > 0.0);
        assert!(trail_scale(1) < 1.0 && trail_scale(3) < trail_scale(1));
        assert_eq!(pile(5), pile(3));
    }
}
