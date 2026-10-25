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

/// How much later than the one ahead each drawn follower of a dragged group
/// replays the lead's path, in seconds; the first one has none. The one place to
/// tune the tail.
pub const TRAIL_DELAY: f64 = 0.07;
/// Followers drawn under the lead of a dragged group; the rest hide until the drop.
pub const TRAIL_SHOWN: usize = 3;
/// Each drawn follower sits this much lower than the one ahead and this much
/// smaller, so it peeks out under it as a sliver: a stack being carried.
const TRAIL_STEP: f64 = 7.0;
const TRAIL_SHRINK: f64 = 0.06;

/// The `i`-th drawn follower (1-based): its offset down from the lead and scale.
pub fn trail(i: usize) -> (f64, f64) {
    let i = i.min(TRAIL_SHOWN) as f64;
    (TRAIL_STEP * i, 1.0 - TRAIL_SHRINK * i)
}

/// How long ago the lead was where the `i`-th follower (1-based) is now: a plain
/// delay on the lead's own path, no springs between the cards.
pub fn trail_delay(i: usize) -> f64 {
    i.clamp(1, TRAIL_SHOWN).saturating_sub(1) as f64 * TRAIL_DELAY
}

/// Where the lead was at time `t`, from its recorded path `(time, x, y)` (oldest
/// first), interpolated between samples; before the first sample, the first.
pub fn path_at(path: &[(f64, f64, f64)], t: f64) -> Option<(f64, f64)> {
    let first = path.first()?;
    if t <= first.0 {
        return Some((first.1, first.2));
    }
    let k = path.partition_point(|p| p.0 <= t);
    let a = path[k - 1];
    let Some(&b) = path.get(k) else {
        return Some((a.1, a.2));
    };
    let f = (t - a.0) / (b.0 - a.0).max(1e-9);
    Some((a.1 + (b.1 - a.1) * f, a.2 + (b.2 - a.2) * f))
}

/// Whether a card shows its selection ring: while choosing, never while dragging.
pub fn ring_shown(selected: bool, dragging: bool) -> bool {
    selected && !dragging
}

/// Degrees of tilt per point/second of horizontal speed.
pub const TILT_PER_SPEED: f64 = 0.012;
pub const LEAD_MAX_TILT: f64 = 4.0;

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

/// The most cards a fan draws; deeper ones stay hidden in the stack.
pub const FAN_SHOWN: usize = 12;
/// Half the fan's spread for three cards and its widest, in degrees.
const FAN_HALF_MIN: f64 = 20.0;
const FAN_HALF_MAX: f64 = 45.0;
/// How far below the stacked card's centre the pivot may sit, and at least.
const FAN_REACH_MAX: f64 = 260.0;
const FAN_REACH_MIN: f64 = 60.0;

/// Half of a fan's spread in degrees for `n` cards: about ±20° for three, opening
/// up with every card to ±45°.
pub fn fan_half_spread(n: usize) -> f64 {
    match n {
        0 | 1 => 0.0,
        2 => 12.0,
        _ => (FAN_HALF_MIN + 2.5 * (n - 3) as f64).min(FAN_HALF_MAX),
    }
}

/// A swatch fan: like colour swatches on one pin, the cards turn about a single
/// pivot below the stack, evenly from left to right on one arc. The middle of the
/// arc is where the stacked card lies, so the stack opens from itself. Returns the
/// `i`-th card's centre offset from the stacked card's centre and its turn in
/// degrees (positive is clockwise), for a `card` (width, height) and a column
/// `col_w` wide: the pivot comes closer when the arc would not fit the column.
/// Cards past [`FAN_SHOWN`] stay on the stack.
pub fn fan(i: usize, n: usize, card: (f64, f64), col_w: f64) -> (f64, f64, f64) {
    let n = n.min(FAN_SHOWN);
    if n <= 1 || i >= n {
        return (0.0, 0.0, 0.0);
    }
    let half = fan_half_spread(n);
    let deg = -half + 2.0 * half * i as f64 / (n - 1) as f64;
    let r = fan_reach(n, card, col_w);
    let at = deg.to_radians();
    // The pivot is `r` below the stacked card's centre; turning about it moves the
    // centre along the arc.
    (r * at.sin(), r * (1.0 - at.cos()), deg)
}

/// The distance from the pivot to each card's centre: as far as the column's
/// width allows the outermost cards, turned by the full half spread.
pub fn fan_reach(n: usize, (w, h): (f64, f64), col_w: f64) -> f64 {
    let half = fan_half_spread(n.min(FAN_SHOWN)).to_radians();
    if half <= 0.0 {
        return FAN_REACH_MIN;
    }
    // Half the width of a card turned by `half`, around its centre.
    let turned = 0.5 * (w * half.cos() + h * half.sin());
    ((col_w / 2.0 - turned) / half.sin()).clamp(FAN_REACH_MIN, FAN_REACH_MAX)
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
    fn the_tail_replays_the_leads_path_later() {
        // No delay for the first follower, then the same step for each next one.
        assert_eq!(trail_delay(1), 0.0);
        assert!((trail_delay(2) - TRAIL_DELAY).abs() < 1e-12);
        assert!((trail_delay(3) - 2.0 * TRAIL_DELAY).abs() < 1e-12);
        assert_eq!(trail_delay(9), trail_delay(TRAIL_SHOWN));
        // The lead's path, sampled: interpolated, held at either end.
        let path = [(0.0, 0.0, 0.0), (0.1, 100.0, 10.0), (0.2, 100.0, 30.0)];
        assert_eq!(path_at(&path, 0.05), Some((50.0, 5.0)));
        assert_eq!(path_at(&path, -1.0), Some((0.0, 0.0)));
        assert_eq!(path_at(&path, 9.0), Some((100.0, 30.0)));
        assert_eq!(path_at(&[], 0.0), None);
        // Each sliver smaller and lower than the one ahead; no more than three.
        assert!(trail(2).1 < trail(1).1 && trail(2).0 > trail(1).0);
        assert_eq!(trail(9), trail(TRAIL_SHOWN));
    }

    #[test]
    fn no_selection_ring_while_dragging() {
        assert!(ring_shown(true, false));
        assert!(!ring_shown(true, true));
        assert!(!ring_shown(false, false));
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
    fn the_fan_is_one_even_symmetric_arc() {
        let card = (230.0, 101.0);
        for n in [2, 3, 6, 12, 30] {
            let k = n.min(FAN_SHOWN);
            let arms: Vec<(f64, f64, f64)> = (0..k).map(|i| fan(i, n, card, 366.0)).collect();
            let degs: Vec<f64> = arms.iter().map(|a| a.2).collect();
            // One arc: never 90° or more between any two cards.
            let spread = degs[k - 1] - degs[0];
            assert!(spread < 90.0 + 1e-9, "{n} cards spread {spread}");
            // Even steps, left to right.
            let step = degs[1] - degs[0];
            assert!(step > 0.0);
            for w in degs.windows(2) {
                assert!((w[1] - w[0] - step).abs() < 1e-9);
            }
            if n >= 6 {
                assert!(step <= 12.0, "{n} cards step {step}");
            }
            // Symmetric about the stack: mirrored offsets and turns.
            for i in 0..k {
                let (a, b) = (arms[i], arms[k - 1 - i]);
                assert!((a.0 + b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9);
                assert!((a.2 + b.2).abs() < 1e-9);
            }
        }
        assert!((fan_half_spread(3) - 20.0).abs() < 1e-9);
        assert_eq!(fan_half_spread(40), 45.0);
        // Nothing past the twelfth card leaves the stack.
        assert_eq!(fan(12, 30, card, 366.0), (0.0, 0.0, 0.0));
    }

    #[test]
    fn the_fan_fits_its_column() {
        let card = (230.0, 101.0);
        for n in [3, 6, 12] {
            let r = fan_reach(n, card, 366.0);
            let half = fan_half_spread(n).to_radians();
            let edge = r * half.sin() + 0.5 * (card.0 * half.cos() + card.1 * half.sin());
            assert!(
                edge <= 183.0 + 1e-6 || r == FAN_REACH_MIN,
                "{n} cards reach {edge}"
            );
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
