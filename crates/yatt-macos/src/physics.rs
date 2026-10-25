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
/// A stack opening into its fan: quick, settled in about 0.3 s whatever the
/// number of cards, with only a trace of the jelly.
pub const FAN_OPEN: Spring = Spring::new(900.0, 0.85);
/// A fan folding back into its stack: quicker still, about 0.22 s.
pub const FAN_FOLD: Spring = Spring::new(1500.0, 0.9);
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

/// How much later than the one ahead each follower of a dragged group replays
/// the lead's path, and how far apart they land after the drop, in seconds; the
/// first one has no delay. The one place to tune the tail.
pub const TRAIL_DELAY: f64 = 0.055;
/// Followers drawn under the lead of a dragged group; the rest hide until the drop.
pub const TRAIL_SHOWN: usize = 8;
/// Each drawn follower sits this much lower than the one ahead and this much
/// smaller, so it peeks out under it as a sliver: a stack being carried.
const TRAIL_STEP: f64 = 6.0;
const TRAIL_SHRINK: f64 = 0.05;

/// The `i`-th drawn follower (1-based): its offset down from the lead and scale.
pub fn trail(i: usize) -> (f64, f64) {
    let i = i.min(TRAIL_SHOWN) as f64;
    (TRAIL_STEP * i, 1.0 - TRAIL_SHRINK * i)
}

/// How long ago the lead was where the `i`-th follower (1-based) is now: a plain
/// delay on the lead's own path, no springs between the cards.
pub fn trail_delay(i: usize) -> f64 {
    i.max(1).saturating_sub(1) as f64 * TRAIL_DELAY
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

/// The stacking order of the `k`-th card of a dragged or landing group (0 = the
/// lead): the lead on top, each next one under the one before, for the whole
/// flight and landing, above everything else on the board.
pub fn group_z(k: usize) -> f64 {
    1000.0 - k as f64
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
pub const FAN_SHOWN: usize = 20;
/// Where the newest card sits round the fan's centre, in degrees (y down, so −90
/// is straight up): just right of the top, as in the reference frames.
const FAN_START: f64 = -86.0;
/// The most the cards go round, and the most they turn from first to last.
const FAN_SWEEP: f64 = 320.0;
const FAN_TURN: f64 = 108.0;

/// How far a fanned card's centre sits from the fan's centre: wider with more
/// cards: about 0.17 card widths for three, 0.36 for nine, so each card hides
/// most of the one under it (fitted to the
/// reference frames).
pub fn fan_radius(n: usize, w: f64) -> f64 {
    w * (0.08 + 0.031 * n.min(FAN_SHOWN) as f64).min(0.45)
}

/// The swatch fan of the reference frames (docs/reference/tima/swatch): card
/// `i` (0 = the newest, on top of the stack) of `n`. From the newest just right
/// of the top, the cards wind clockwise round one centre, down the right, round
/// the bottom and up the left to the oldest, about 40° apart (fewer cards go less far
/// round), each turned about 13° further clockwise than the next newer one: from
/// about −70° for the newest to about +38° for the oldest of nine. Returns the
/// card's centre offset from the fan's centre and its turn in degrees (positive
/// is clockwise). Past [`FAN_SHOWN`] cards the steps get finer instead of going
/// round further; deeper cards stay on the stack.
pub fn fan(i: usize, n: usize, w: f64) -> (f64, f64, f64) {
    let n = n.min(FAN_SHOWN);
    if n <= 1 || i >= n {
        return (0.0, 0.0, 0.0);
    }
    let gaps = (n - 1) as f64;
    let sweep = (125.0 * gaps).min(FAN_SWEEP);
    let turn = (37.0 * gaps).min(FAN_TURN);
    let t = i as f64 / gaps;
    let at = (FAN_START + sweep * t).to_radians();
    let r = fan_radius(n, w);
    (r * at.cos(), r * at.sin(), -0.65 * turn + turn * t)
}

/// The smallest a fan may shrink its cards to fit its column, so their text stays
/// legible; past that it draws fewer cards.
pub const FAN_MIN_SCALE: f64 = 0.6;

/// The fan's bounding box around its centre at full size: (min x, min y, max x,
/// max y) of the turned cards.
pub fn fan_bounds(n: usize, (w, h): (f64, f64)) -> (f64, f64, f64, f64) {
    let k = n.clamp(1, FAN_SHOWN);
    let mut b = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for i in 0..k {
        let (x, y, deg) = fan(i, k, w);
        let (c, s) = (deg.to_radians().cos().abs(), deg.to_radians().sin().abs());
        let (hx, hy) = ((w * c + h * s) / 2.0, (w * s + h * c) / 2.0);
        b = (
            b.0.min(x - hx),
            b.1.min(y - hy),
            b.2.max(x + hx),
            b.3.max(y + hy),
        );
    }
    b
}

/// How many of `n` stacked cards a fan draws and at what scale, so the whole
/// rosette fits a box `room` (width, height) wide: the reference layout, shrunk
/// uniformly (angles unchanged), never below [`FAN_MIN_SCALE`]; where even that
/// is too big, fewer cards.
pub fn fan_fit(n: usize, card: (f64, f64), room: (f64, f64)) -> (usize, f64) {
    let mut k = n.clamp(1, FAN_SHOWN);
    loop {
        let (x0, y0, x1, y1) = fan_bounds(k, card);
        let scale = (room.0 / (x1 - x0)).min(room.1 / (y1 - y0)).min(1.0);
        if scale >= FAN_MIN_SCALE || k <= 2 {
            return (k, scale);
        }
        k -= 1;
    }
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
        // The same step all the way down a long tail.
        assert!((trail_delay(9) - 8.0 * TRAIL_DELAY).abs() < 1e-12);
        // The lead's path, sampled: interpolated, held at either end.
        let path = [(0.0, 0.0, 0.0), (0.1, 100.0, 10.0), (0.2, 100.0, 30.0)];
        assert_eq!(path_at(&path, 0.05), Some((50.0, 5.0)));
        assert_eq!(path_at(&path, -1.0), Some((0.0, 0.0)));
        assert_eq!(path_at(&path, 9.0), Some((100.0, 30.0)));
        assert_eq!(path_at(&[], 0.0), None);
        // Each sliver smaller and lower than the one ahead; no more than three.
        assert!(trail(2).1 < trail(1).1 && trail(2).0 > trail(1).0);
        assert_eq!(trail(TRAIL_SHOWN + 3), trail(TRAIL_SHOWN));
    }

    #[test]
    fn a_group_keeps_its_stacking_order_all_the_way() {
        // Before the drop, in flight and while landing the same values: the lead
        // on top, then each next card under the one before, above the board.
        let z: Vec<f64> = (0..20).map(group_z).collect();
        assert!(z.windows(2).all(|w| w[0] > w[1]));
        assert!(z[19] > 900.0);
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
    fn the_fan_matches_the_reference_and_has_no_right_angles() {
        let w = 230.0;
        // Nine cards, measured from docs/reference/tima/swatch/n9.png: newest on
        // top just right of the top, turned about −70°; oldest upper left, about
        // +38°; about 40° apart round the centre and 13.5° apart in turn.
        let (x0, y0, r0) = fan(0, 9, w);
        let (x8, y8, r8) = fan(8, 9, w);
        assert!(x0.abs() < 0.15 * w && y0 < 0.0, "newest at {x0}, {y0}");
        assert!((r0 + 70.0).abs() < 4.0, "newest turned {r0}");
        assert!(
            x8 < 0.0 && y8 < 0.0 && (r8 - 38.0).abs() < 4.0,
            "oldest {x8} {y8} {r8}"
        );
        assert!((fan_radius(9, w) / w - 0.36).abs() < 0.02);
        for n in [3, 5, 7, 8, 9, 12, 30] {
            let k = n.min(FAN_SHOWN);
            let arms: Vec<(f64, f64, f64)> = (0..k).map(|i| fan(i, n, w)).collect();
            for pair in arms.windows(2) {
                // Neighbours turn by the same small step, never a right angle.
                let d = (pair[1].2 - pair[0].2).abs();
                assert!(d > 1.0 && d < 45.0, "{n} cards: neighbours {d}° apart");
            }
            // Never more than once round: the first and last never coincide.
            let a = arms[0].1.atan2(arms[0].0);
            let b = arms[k - 1].1.atan2(arms[k - 1].0);
            assert!((a - b).abs() > 0.1);
        }
        // A stable size: the most it draws or thirty, the same fan.
        assert_eq!(fan(5, FAN_SHOWN, w), fan(5, 30, w));
        assert_eq!(fan(FAN_SHOWN, 30, w), (0.0, 0.0, 0.0));
    }

    #[test]
    fn the_fan_fits_a_320_pt_column() {
        let card = (230.0, 101.0);
        for n in 3..=12 {
            let (k, scale) = fan_fit(n, card, (320.0 - 16.0, 2000.0));
            let (x0, _, x1, _) = fan_bounds(k, card);
            assert!(
                (x1 - x0) * scale <= 304.0 + 1e-6,
                "{n} cards: {} wide",
                (x1 - x0) * scale
            );
            assert!(
                (FAN_MIN_SCALE - 1e-9..=1.0).contains(&scale),
                "{n} cards at {scale}"
            );
            assert!(k <= n);
        }
        // A wide column needs no shrinking for a few cards.
        assert_eq!(fan_fit(3, card, (800.0, 800.0)), (3, 1.0));
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
