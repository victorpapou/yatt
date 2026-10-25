//! A small spring engine for card motion: position and rotation each chase a
//! target on a unit-mass spring, and the card squashes and stretches a little with
//! its velocity, so it lands like jelly. Plain math with no AppKit, so it is
//! tested on any OS. The fan shape is fitted to the reference clip
//! (docs/reference/card-motion.md).

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

/// Settling into a stack or fan: stiff and a little underdamped (ζ 0.8), so the
/// card overshoots a touch, about 1.5%, and settles at once, like firm jelly.
pub const PILE: Spring = Spring::new(420.0, 0.8);
/// The card under the cursor while dragging: nearly attached, a touch of lag.
pub const LEAD: Spring = Spring::new(1200.0, 0.95);
/// A card settling into its list slot after a drop: the same jelly landing.
pub const SLOT: Spring = Spring::new(560.0, 0.8);
/// A new card flying from the new-task field into its column.
pub const FLY: Spring = Spring::new(220.0, 0.8);
/// The task modal growing out of its card and shrinking back: no wobble, the
/// panel's text has to stay readable all the way.
pub const MORPH: Spring = Spring::new(260.0, 1.0);
/// Rotation follows faster than position, so tilt reads as a reaction to motion.
pub const ROTATION: Spring = Spring::new(500.0, 0.8);

/// How far behind the lead the last card of a group of three follows, in
/// seconds (a spring's time constant); bigger groups spread less.
const FOLLOW_TAIL: f64 = 0.045;

/// Lag of the `i`-th card behind the lead (1-based) of a dragged group of `n`, in
/// seconds. The first follower has none: it rides exactly under the lead. Past it
/// the lag grows along an ease-out curve over the index, in big steps right
/// behind the head and smaller ones towards the end, and the bigger the group, the
/// less the whole tail lags.
pub fn follow_lag(i: usize, n: usize) -> f64 {
    if i <= 1 || n <= 2 {
        return 0.0;
    }
    let t = ((i - 1) as f64 / (n - 2) as f64).min(1.0);
    let eased = 1.0 - (1.0 - t) * (1.0 - t);
    FOLLOW_TAIL / (1.0 + 0.25 * (n - 3) as f64) * eased
}

/// The spring for the `i`-th follower of `n`, chasing the lead with
/// [`follow_lag`] (the first one is held on the lead outright).
pub fn follower(i: usize, n: usize) -> Spring {
    let lag = follow_lag(i, n).max(0.002);
    Spring::new(1.0 / (lag * lag), 1.0)
}

/// Degrees of tilt per point/second of horizontal speed.
pub const TILT_PER_SPEED: f64 = 0.012;
pub const LEAD_MAX_TILT: f64 = 4.0;
pub const FOLLOWER_MAX_TILT: f64 = 0.0;

/// Tilt in degrees (positive = clockwise on screen) for a horizontal speed in points/second.
pub fn tilt(vx: f64, max: f64) -> f64 {
    (vx * TILT_PER_SPEED).clamp(-max, max)
}

/// Stretch per point/second of speed, and the most a card stretches.
const STRETCH_PER_SPEED: f64 = 0.000_06;
const MAX_STRETCH: f64 = 0.07;

/// Squash and stretch for a velocity: the card gets longer along the way it moves
/// and thinner across it, keeping roughly its area. Returns (x, y) scale factors.
pub fn squash(vx: f64, vy: f64) -> (f64, f64) {
    let sx = (vx.abs() * STRETCH_PER_SPEED).min(MAX_STRETCH);
    let sy = (vy.abs() * STRETCH_PER_SPEED).min(MAX_STRETCH);
    (1.0 + sx - 0.5 * sy, 1.0 + sy - 0.5 * sx)
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
/// top card as thin slivers, two of them; the rest hide behind the second.
pub fn pile(i: usize) -> (f64, f64) {
    let i = i.min(2) as f64;
    (8.0 * i, 1.0 - 0.06 * i)
}

/// The most cards on the fan's outer ring; the rest go on an inner one, so a big
/// stack stays about as wide as its column instead of one huge arc.
pub const RING: usize = 8;

/// How far a fanned card's centre sits from the stack's centre on the outer ring:
/// the rosette opens wider as it holds more cards.
pub fn fan_radius(n: usize, w: f64) -> f64 {
    w * (0.30 + 0.022 * n.min(RING) as f64).min(0.48)
}

/// Fanned stack (a rosette), fitted to the reference: the `i`-th card's centre
/// relative to the stack's centre (y down) and its rotation in degrees (positive =
/// clockwise). Cards go clockwise round the centre from just left of the top, each
/// turned a little further than the one before; the more cards, the further round
/// they go (about 250° for three, 315° from eight on) and the finer the turn steps.
/// Past [`RING`] cards the rest wrap onto an inner ring, half a step out of phase.
pub fn fan(i: usize, n: usize, w: f64) -> (f64, f64, f64) {
    let r = fan_radius(n, w);
    if n <= RING {
        ring(i, n, r, 0.0)
    } else if i < RING {
        ring(i, RING, r, 0.0)
    } else {
        ring(i - RING, n - RING, r * 0.5, 0.5)
    }
}

/// The `i`-th of `n` cards on a ring of radius `r`, its angle shifted by `phase` steps.
fn ring(i: usize, n: usize, r: f64, phase: f64) -> (f64, f64, f64) {
    if n <= 1 {
        let at = (-95.0 + 40.0 * phase).to_radians();
        return if phase > 0.0 {
            (r * at.cos(), r * at.sin(), -20.0)
        } else {
            (0.0, 0.0, 0.0)
        };
    }
    let gaps = (n - 1) as f64;
    let sweep = (125.0 * gaps).min(315.0);
    let turn = (37.0 * gaps).min(105.0);
    let t = (i as f64 + phase) / (gaps + phase);
    let at = (-95.0 + sweep * t).to_radians();
    (r * at.cos(), r * at.sin(), -0.68 * turn + turn * t)
}

/// A plain click on a fanned stack: true opens the card (the second click of a
/// double-click whose first click also landed on the fan), false folds the fan.
/// A double-click that started by fanning the stack out does not count.
pub fn fan_click_opens(clicks: isize, first_click_was_on_fan: bool) -> bool {
    clicks >= 2 && first_click_was_on_fan
}

/// Left edge of a `card_w` wide card centred on a column at `x` that is `w` wide.
pub fn centred_left(x: f64, w: f64, card_w: f64) -> f64 {
    (x + (w - card_w) / 2.0).round()
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
    fn settling_is_a_short_jelly_wobble() {
        let mut b = Body::at(0.0, 0.0);
        b.target(100.0, 0.0, 0.0);
        let mut max = 0.0f64;
        let mut t = 0.0;
        while !b.at_rest() && t < 2.0 {
            b.step(1.0 / 120.0);
            max = max.max(b.x);
            t += 1.0 / 120.0;
        }
        assert!(max > 100.5 && max < 103.0, "overshoot {max}");
        assert!(t < 0.8, "settled after {t}s");
    }

    #[test]
    fn the_tail_lags_on_an_ease_out_curve() {
        // The first follower rides on the lead.
        assert_eq!(follow_lag(1, 6), 0.0);
        // Later cards lag more, in shrinking steps.
        let lags: Vec<f64> = (1..6).map(|i| follow_lag(i, 6)).collect();
        let steps: Vec<f64> = lags.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(steps.iter().all(|s| *s > 0.0));
        assert!(steps.windows(2).all(|w| w[1] < w[0]));
        // The last of a big group lags less than the last of a small one.
        assert!(follow_lag(9, 10) < follow_lag(2, 3));
        // And the springs agree: within 50 ms a lagging card covers less ground.
        let reach = |spring: Spring| {
            let mut b = Body::at(0.0, 0.0);
            b.spring = spring;
            b.target(200.0, 0.0, 0.0);
            for _ in 0..6 {
                b.step(1.0 / 120.0);
            }
            b.x
        };
        assert!(reach(follower(2, 6)) > reach(follower(5, 6)));
    }

    #[test]
    fn squash_stretches_along_the_motion() {
        assert_eq!(squash(0.0, 0.0), (1.0, 1.0));
        let (sx, sy) = squash(1000.0, 0.0);
        assert!(sx > 1.0 && sy < 1.0);
        let (sx, _) = squash(1e9, 0.0);
        assert!((sx - 1.0 - MAX_STRETCH).abs() < 1e-9);
    }

    #[test]
    fn cards_centre_on_their_column() {
        // A wide window: the third of 1800 pt starts at 1200.
        let left = centred_left(1200.0, 600.0, 230.0);
        assert_eq!(left + 115.0, 1500.0);
        assert_eq!(centred_left(0.0, 366.0, 230.0), 68.0);
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
    fn fan_spreads_around_the_ring() {
        // Three cards: top, lower right, left; turning from about −50° to +24°.
        let (x0, y0, r0) = fan(0, 3, 230.0);
        let (x1, y1, _) = fan(1, 3, 230.0);
        let (x2, _, r2) = fan(2, 3, 230.0);
        assert!(y0 < 0.0 && x0.abs() < 10.0 && (r0 + 50.3).abs() < 0.5);
        assert!(x1 > 0.0 && y1 > 0.0);
        assert!(x2 < 0.0 && (r2 - 24.0).abs() < 0.5);
        // Nine cards turn in finer steps and spread further out.
        let (_, _, a) = fan(0, 6, 230.0);
        let (_, _, b) = fan(1, 6, 230.0);
        assert!(b - a < 25.0 && fan_radius(6, 230.0) > fan_radius(3, 230.0));
        assert_eq!(pile(5), pile(2));
    }

    #[test]
    fn a_big_fan_wraps_onto_an_inner_ring() {
        let w = 230.0;
        let outer = fan_radius(12, w);
        assert_eq!(outer, fan_radius(RING, w));
        for i in 0..12 {
            let (x, y, _) = fan(i, 12, w);
            let r = x.hypot(y);
            if i < RING {
                assert!((r - outer).abs() < 1e-6);
            } else {
                assert!(r < outer * 0.6, "card {i} at {r}");
            }
        }
    }

    #[test]
    fn second_click_folds_the_fan_unless_it_is_a_real_double_click() {
        // The click that fanned the stack plus the next one make clickCount 2: fold.
        assert!(!fan_click_opens(2, false));
        // A single click on the fan folds it.
        assert!(!fan_click_opens(1, true));
        // Two clicks both on the fan open the card.
        assert!(fan_click_opens(2, true));
    }
}
