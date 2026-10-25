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
pub const PILE: Spring = Spring::new(320.0, 0.75);
/// The card under the cursor while dragging: nearly attached, a touch of lag.
pub const LEAD: Spring = Spring::new(1200.0, 0.85);
/// A card settling into its list slot after a drop.
pub const SLOT: Spring = Spring::new(420.0, 0.82);
/// A new card flying from the new-task field into its column.
pub const FLY: Spring = Spring::new(170.0, 0.72);
/// The task modal growing out of its card and shrinking back.
pub const MORPH: Spring = Spring::new(260.0, 0.84);
/// Rotation follows faster than position, so tilt reads as a reaction to motion.
pub const ROTATION: Spring = Spring::new(500.0, 0.8);

/// The `i`-th card behind the lead (1-based): softer, so the train stretches out.
pub fn follower(i: usize) -> Spring {
    Spring::new(420.0 / (1.0 + 0.15 * i as f64), 0.7)
}

/// Degrees of tilt per point/second of horizontal speed.
pub const TILT_PER_SPEED: f64 = 0.012;
pub const LEAD_MAX_TILT: f64 = 4.0;
pub const FOLLOWER_MAX_TILT: f64 = 12.0;

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

/// Where the `i`-th card of a collapsed stack sits relative to the top card,
/// as fractions of the card's width and height (index 0 is the top card).
pub fn pile_offset(i: usize) -> (f64, f64) {
    const OFFSETS: [(f64, f64); 6] = [
        (0.0, 0.0),
        (0.10, -0.23),
        (0.22, -0.23),
        (-0.06, 0.18),
        (-0.10, 0.62),
        (-0.07, 0.82),
    ];
    OFFSETS[i.min(OFFSETS.len() - 1)]
}

/// Fan angles in degrees (positive = clockwise), top card first: a swatch book
/// turning about the stack's bottom-left corner, about −45° on top to +20° at the bottom.
pub fn fan_angles(n: usize) -> Vec<f64> {
    if n <= 1 {
        return vec![0.0; n];
    }
    let sweep = (13.0 * (n - 1) as f64).min(65.0);
    let top = -sweep * 0.7;
    let step = sweep / (n - 1) as f64;
    (0..n).map(|i| top + step * i as f64).collect()
}

/// Top-left corner of a `w × h` card turned by `deg` about `pivot`, where the pivot is the
/// card's bottom-left corner. Returned for an unrotated frame that is then turned about
/// its centre, which is how AppKit applies rotation.
pub fn fan_origin(pivot: (f64, f64), w: f64, h: f64, deg: f64) -> (f64, f64) {
    let t = deg.to_radians();
    // Centre relative to the bottom-left corner, y down: (w/2, −h/2), turned by t.
    let (cx, cy) = (w / 2.0, -h / 2.0);
    let rx = cx * t.cos() - cy * t.sin();
    let ry = cx * t.sin() + cy * t.cos();
    (pivot.0 + rx - w / 2.0, pivot.1 + ry - h / 2.0)
}

/// Where a card dropped with its centre at `y` lands in a list whose first slot
/// starts at `top`, slots `pitch` apart, holding `n` cards.
pub fn insert_index(y: f64, top: f64, pitch: f64, n: usize) -> usize {
    (((y - top) / pitch).floor().max(0.0) as usize).min(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pile_spring_settles_with_small_overshoot_in_about_300ms() {
        let mut b = Body::at(0.0, 0.0);
        b.target(100.0, 0.0, 0.0);
        let mut max = 0.0f64;
        let mut t = 0.0;
        while !b.at_rest() && t < 2.0 {
            b.step(1.0 / 120.0);
            max = max.max(b.x);
            t += 1.0 / 120.0;
        }
        assert!(max > 100.0 && max < 104.0, "overshoot {max}");
        assert!(t > 0.25 && t < 0.7, "settled after {t}s");
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
        assert_eq!(tilt(-10_000.0, FOLLOWER_MAX_TILT), -FOLLOWER_MAX_TILT);
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
    fn unrotated_fan_card_sits_on_its_pivot() {
        let (x, y) = fan_origin((10.0, 100.0), 200.0, 80.0, 0.0);
        assert!((x - 10.0).abs() < 1e-9 && (y - 20.0).abs() < 1e-9);
    }
}
