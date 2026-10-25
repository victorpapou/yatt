//! The card motion engine. A card's position chases its target on a critically
//! damped spring (no overshoot, no jitter); its angle hangs like a damped
//! pendulum from the point that moves it, so it tilts against a sudden move,
//! swings back past level once or twice and settles, as if on a string. One
//! integrator with fixed substeps for both ([`Body::step`]). A dragged group is
//! a chain: each follower hangs off the card ahead. Plain math with no AppKit,
//! so it is tested on any OS. The constants are measured from the reference clip
//! (docs/reference/tima/motion/notes.md); the fan shape is fitted to the swatch
//! frames (docs/reference/tima/swatch).

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

/// Where a card goes when it is put somewhere: critically damped, settled in
/// about 0.35 s. The swing of its angle is the hanging part ([`SWING_PERIOD`]).
pub const SETTLE: Spring = Spring::new(380.0, 1.0);
/// Settling into a stack or a list slot.
pub const PILE: Spring = SETTLE;
/// A stack opening into its fan: a little brisker, as a wave ([`fan_wave`]); the
/// cards swing into their angles.
pub const FAN_OPEN: Spring = Spring::new(560.0, 1.0);
/// A fan folding back into its stack: quicker.
pub const FAN_FOLD: Spring = Spring::new(800.0, 1.0);

/// The hanging swing of a card's angle: natural period, damping ratio, the
/// lever from the point that moves it (sets how far a push tilts it: about 8° at
/// 4000 pt/s² sideways), and the most it tilts. Measured from the reference clip.
pub const SWING_PERIOD: f64 = 0.45;
pub const SWING_ZETA: f64 = 0.45;
pub const SWING_LEVER: f64 = 150.0;
pub const SWING_MAX: f64 = 12.0;

/// When the `i`-th card of a fan of `n` starts to move as the fan opens, in
/// seconds: a wave through the fan, at most 12 ms apart and 60 ms in all
/// whatever the count.
pub fn fan_wave(i: usize, n: usize) -> f64 {
    let step = (0.06 / n.max(1) as f64).min(0.012);
    i as f64 * step
}
/// The lead of a dragged group: nearly attached, a touch of lag.
pub const LEAD: Spring = Spring::new(1200.0, 1.0);
/// A single dragged card: with the pointer (about 10 ms behind), so its swing
/// comes from the pointer's own moves.
pub const HELD: Spring = Spring::new(40_000.0, 1.0);
/// A card already in a list moving over to make room or close a gap: soft,
/// ease in and out.
pub const SHIFT: Spring = Spring::new(300.0, 1.0);
/// A card settling into its list slot after a drop.
pub const SLOT: Spring = SETTLE;
/// A new card flying from the new-task field into its column.
pub const FLY: Spring = Spring::new(220.0, 1.0);
/// The task modal growing out of its card and shrinking back: no wobble, the
/// panel's text has to stay readable all the way.
pub const MORPH: Spring = Spring::new(520.0, 0.88);

/// How far behind the card ahead each follower of a dragged group hangs, in
/// seconds of lag at a steady speed: for a group of two, of three, and of four
/// or more. Small groups get longer links, so the chain reads about as long
/// whatever the size. The one place to tune it.
pub const TRAIL_DELAYS: (f64, f64, f64) = (0.08, 0.06, 0.04);

/// The lag of one link for a dragged group of `n` cards: 80 ms for two, 60 ms
/// for three, 40 ms for four or more.
pub fn trail_step(n: usize) -> f64 {
    let (two, three, many) = TRAIL_DELAYS;
    match n {
        0..=2 => two,
        3 => three,
        _ => many,
    }
}
/// Followers drawn under the lead of a dragged group; the rest hide until the drop.
pub const TRAIL_SHOWN: usize = 8;
/// Each drawn follower is this much smaller than the one ahead (never below
/// 0.6), centred under the lead: at rest none of it shows outside the lead; only
/// the lag along the path shows it while the group moves.
const TRAIL_SHRINK: f64 = 0.05;

/// The `i`-th drawn follower (1-based): its offset down from the lead (none) and
/// its scale.
pub fn trail(i: usize) -> (f64, f64) {
    let i = i.min(TRAIL_SHOWN) as f64;
    (0.0, (1.0 - TRAIL_SHRINK * i).max(0.6))
}

/// The spring a follower of a group of `n` hangs off the card ahead with: a
/// critically damped spring trails a steady motion by twice its time constant,
/// so the constant is half the link's lag.
pub fn chain_spring(n: usize) -> Spring {
    let tau = trail_step(n) / 2.0;
    Spring::new(1.0 / (tau * tau), 1.0)
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
    /// Whether the angle hangs and swings (cards), or just eases to its target
    /// (the task panel).
    pub swings: bool,
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
            swings: true,
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
        let w0 = std::f64::consts::TAU / SWING_PERIOD;
        let deg_per_accel = 180.0 / std::f64::consts::PI / SWING_LEVER;
        for _ in 0..steps {
            let vx0 = self.vx;
            self.vx += (-k * (self.x - self.tx) - c * self.vx) * h;
            self.vy += (-k * (self.y - self.ty) - c * self.vy) * h;
            self.x += self.vx * h;
            self.y += self.vy * h;
            // The angle hangs from the moving point: a push to the right leaves
            // the bottom behind, turning the card clockwise, and it swings back.
            let ax = (self.vx - vx0) / h;
            let swing = self.r - self.tr;
            let (zeta, push) = if self.swings {
                (SWING_ZETA, deg_per_accel * ax)
            } else {
                (1.0, 0.0)
            };
            self.vr += (-w0 * w0 * swing - 2.0 * zeta * w0 * self.vr + push) * h;
            self.r += self.vr * h;
            let limit = SWING_MAX;
            if (self.r - self.tr).abs() > limit {
                self.r = self.tr + limit.copysign(self.r - self.tr);
                self.vr = 0.0;
            }
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
    fn the_fan_opens_as_a_short_wave() {
        assert_eq!(fan_wave(0, 8), 0.0);
        assert!(fan_wave(1, 3) <= 0.012 + 1e-12);
        assert!(fan_wave(19, 20) <= 0.06 + 1e-12);
        assert!(fan_wave(5, 8) > fan_wave(4, 8));
    }

    #[test]
    fn position_settles_without_overshoot() {
        let mut b = Body::at(0.0, 0.0);
        // The position alone (a hanging card's angle swings on a while longer).
        b.swings = false;
        b.target(100.0, 0.0, 0.0);
        let (mut max, mut t) = (0.0f64, 0.0);
        while !b.at_rest() && t < 2.0 {
            b.step(1.0 / 120.0);
            max = max.max(b.x);
            t += 1.0 / 120.0;
        }
        assert!(max <= 100.0 + 1e-6, "overshoot {max}");
        assert!(t < 0.8, "settled after {t}s");
    }

    #[test]
    fn a_card_swings_like_it_hangs_and_settles() {
        // Carried right and stopped: it tilts clockwise (against the move),
        // swings back past level, and settles.
        let mut b = Body::at(0.0, 0.0);
        b.spring = LEAD;
        let dt = 1.0 / 120.0;
        let mut tilt = 0.0f64;
        for k in 0..30 {
            b.target(40.0 * k as f64, 0.0, 0.0);
            b.step(dt);
            tilt = tilt.max(b.r);
        }
        assert!(tilt > 2.0 && tilt <= SWING_MAX, "tilt {tilt}");
        let mut back = 0.0f64;
        let mut t = 0.0;
        while t < 3.0 {
            b.step(dt);
            back = back.min(b.r);
            t += dt;
            if b.at_rest() {
                break;
            }
        }
        assert!(back < -0.2, "no swing back ({back})");
        assert!(b.r.abs() < 0.05, "not settled: {}", b.r);
        assert!(t < 1.5, "took {t}s");
    }

    #[test]
    fn a_fast_zigzag_stays_stable_and_within_the_limit() {
        let mut b = Body::at(0.0, 0.0);
        b.spring = LEAD;
        let (dt, mut x, mut dir) = (1.0 / 120.0, 0.0f64, 1.0f64);
        for k in 0..600 {
            if k % 12 == 0 {
                dir = -dir;
            }
            x += 3000.0 * dt * dir;
            b.target(x, 0.0, 0.0);
            b.step(dt);
            assert!(
                b.r.is_finite() && b.r.abs() <= SWING_MAX + 1e-9,
                "angle {}",
                b.r
            );
            assert!(b.x.is_finite());
        }
    }

    #[test]
    fn a_turned_target_is_reached() {
        // A fan card swings into its angle and stays there.
        let mut b = Body::at(0.0, 0.0);
        b.spring = FAN_OPEN;
        b.target(30.0, -20.0, 40.0);
        let mut t = 0.0;
        while !b.at_rest() && t < 3.0 {
            b.step(1.0 / 120.0);
            t += 1.0 / 120.0;
        }
        assert!((b.r - 40.0).abs() < 0.05 && t < 2.0, "{} after {t}s", b.r);
    }

    #[test]
    fn a_chain_link_trails_one_step() {
        // A follower on the chain spring, behind a lead at a steady 1000 pt/s,
        // lags about one link's step.
        let mut f = Body::at(0.0, 0.0);
        f.spring = chain_spring(4);
        let dt = 1.0 / 240.0;
        let mut t = 0.0;
        for _ in 0..480 {
            t += dt;
            f.target(1000.0 * t, 0.0, 0.0);
            f.step(dt);
        }
        let lag = (1000.0 * t - f.x) / 1000.0;
        assert!((lag - trail_step(4)).abs() < 0.005, "lag {lag}");
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
    fn a_resting_follower_lies_inside_the_lead() {
        // At rest a follower is where the lead is (its path delay has caught up),
        // scaled about the same centre: its frame is inside the lead's.
        let (w, h) = (230.0, 101.0);
        let lead = (0.0, 0.0, w, h);
        for i in 1..=TRAIL_SHOWN {
            let (dy, s) = trail(i);
            let (fw, fh) = (w * s, h * s);
            let (x0, y0) = ((w - fw) / 2.0, (h - fh) / 2.0 + dy);
            assert!(x0 >= lead.0 && y0 >= lead.1, "follower {i}");
            assert!(x0 + fw <= lead.2 && y0 + fh <= lead.3, "follower {i}");
            assert!(s >= 0.6);
        }
    }

    #[test]
    fn no_selection_ring_while_dragging() {
        assert!(ring_shown(true, false));
        assert!(!ring_shown(true, true));
        assert!(!ring_shown(false, false));
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
        b.swings = false;
        b.target(400.0, 300.0, 0.0);
        let mut t = 0.0;
        while !b.at_rest() && t < 2.0 {
            b.step(1.0 / 120.0);
            t += 1.0 / 120.0;
        }
        assert!(t < 0.8, "settled after {t}s");
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
