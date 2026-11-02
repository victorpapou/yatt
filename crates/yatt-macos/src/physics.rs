//! The card motion engine. Cards of a stack or a fan, and each stack's
//! openness, move by one law: gravity ([`Fall`]): a fall onto the place with
//! bounded acceleration and a soft, critically damped landing, never past it.
//! A fanned card falls toward and away from the stack's centre and round it
//! ([`Body::fall_to`]), so the fan opens and folds as one mass ([`Opening`])
//! and its cards slide round the spiral to new places. Elsewhere (lists, the
//! modal) a card chases its target on a critically damped spring and its angle
//! hangs like a damped pendulum from the point that moves it; a dragged group
//! is a chain, each follower hanging off the card ahead. One integrator with
//! fixed substeps for all of it ([`Body::step`]). Plain math with no AppKit,
//! so it is tested on any OS. The timings are measured from the reference clip
//! (docs/reference/tima/motion/analysis.md); the fan shape is fitted to the
//! swatch frames (docs/reference/tima/swatch).

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
/// Settling into a list slot (and the default for a new body).
pub const PILE: Spring = SETTLE;

/// The one law every card of a stack or a fan moves by, and each stack's
/// openness itself: gravity. A body falls toward its place with at most `g` of
/// acceleration, may go no faster than it can still brake at `brake` before
/// the place, and lands on it over the last stretch with time constant `tau`
/// and damping `zeta`: under 1 it carries a little past its place and settles
/// back (the action's overshoot), at 1 it never passes. Far moves take longer
/// than short ones, as a fall does (time grows with the root of the distance),
/// instead of every move taking the same time as on a spring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fall {
    pub g: f64,
    pub brake: f64,
    pub tau: f64,
    pub zeta: f64,
}

impl Fall {
    /// The same fall measured in another unit, `unit` points to one of it.
    pub const fn per(self, unit: f64) -> Self {
        Self {
            g: self.g / unit,
            brake: self.brake / unit,
            tau: self.tau,
            zeta: self.zeta,
        }
    }

    /// The acceleration of a body `e` from its place, moving at `v` (both
    /// along one axis; positive `e` is past the place).
    pub fn accel(self, e: f64, v: f64) -> f64 {
        let d = e.abs();
        // The fastest it may go here and still land softly: braking at `brake`
        // far off, an exponential approach near.
        let reach = (2.0 * self.brake * d).sqrt().min(d / self.tau);
        let want = -reach.copysign(e);
        // Chasing that speed at 4ζ² over the landing's time constant: the final
        // approach is a damped oscillator of damping ratio ζ.
        ((want - v) * 4.0 * self.zeta * self.zeta / self.tau).clamp(-self.g, self.g)
    }
}

/// A card of a stack or a fan, in points: a re-layout of about 100 pt goes
/// about 11% past its place (the reference: 6–19%) and is back within 5% of it
/// after about 0.44 s; a long move goes less far past, a short nudge a little
/// further.
pub const FALL: Fall = Fall {
    g: 8000.0,
    brake: 3000.0,
    tau: 0.075,
    zeta: 0.52,
};
/// The same fall for a card's turn, a degree weighing as much as 2.5 pt.
pub const FALL_TURN: Fall = FALL.per(2.5);
/// A card's turn as its stack folds: level on the pile quickly and calmly,
/// no swing past it (about 0.25 s from a fan's steepest card).
pub const FALL_TURN_FOLD: Fall = Fall {
    g: 9000.0,
    brake: 4500.0,
    tau: 0.05,
    zeta: 1.0,
};
/// Within this distance of its stack's centre a card falls straight onto its
/// place; past it, blended over the next [`ROUND_BLEND`] points, round the
/// centre.
pub const ROUND_FROM: f64 = 25.0;
pub const ROUND_BLEND: f64 = 25.0;
/// How far a stack's cards travel between folded and fanned, taken as one
/// point of travel for the openness ([`Opening`]).
pub const FAN_TRAVEL: f64 = 160.0;
/// The fall of a stack's openness (0 folded, 1 fanned): a little softer than a
/// card's, about 3.5% past fully open (or folded) and back; within 5% of its
/// end after about 0.4 s (the reference opens in about 0.47 s).
pub const FALL_OPEN: Fall = Fall {
    g: 4000.0 / FAN_TRAVEL,
    brake: 1500.0 / FAN_TRAVEL,
    tau: 0.09,
    zeta: 0.6,
};

/// How open a stack is, falling toward how open it should be ([`FALL_OPEN`]).
/// Every card of the stack sits at its place for this openness, on the straight
/// way from its pile place to its fan place, so the stack opens and folds as
/// one mass; a card off its place falls back onto it ([`Body::fall`]).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Opening {
    pub at: f64,
    pub v: f64,
}

impl Opening {
    /// Falls toward `to` for `dt` seconds; returns whether it is still moving.
    pub fn step(&mut self, to: f64, dt: f64) -> bool {
        let steps = (dt / (1.0 / 480.0)).ceil().max(1.0) as usize;
        let h = dt / steps as f64;
        for _ in 0..steps {
            self.v += FALL_OPEN.accel(self.at - to, self.v) * h;
            self.at += self.v * h;
        }
        if (self.at - to).abs() < 1e-3 && self.v.abs() < 1e-2 {
            self.at = to;
            self.v = 0.0;
            return false;
        }
        true
    }
}

/// A card's place at openness `q`: on the straight way from `pile` to `fan`
/// (x, y, turn), and on past either end while the openness overshoots.
pub fn fan_mix(pile: (f64, f64, f64), fan: (f64, f64, f64), q: f64) -> (f64, f64, f64) {
    let mix = |a: f64, b: f64| a + (b - a) * q;
    (mix(pile.0, fan.0), mix(pile.1, fan.1), mix(pile.2, fan.2))
}

/// The weighted tilt of a moving card: it leans into its sideways motion, about
/// 3° per 1000 pt/s and at most 7°, following through a damped response
/// (period 0.45 s, ζ 0.7: one slight overshoot, no swinging). Matched to the
/// reference clip (docs/reference/tima/motion).
pub const SWING_PERIOD: f64 = 0.45;
pub const SWING_ZETA: f64 = 0.7;
pub const SWING_PER_SPEED: f64 = 0.003;
pub const SWING_MAX: f64 = 7.0;

/// The lead of a dragged group: nearly attached, a touch of lag.
pub const LEAD: Spring = Spring::new(1200.0, 1.0);
/// A single dragged card: with the pointer (about 10 ms behind), so its swing
/// comes from the pointer's own moves.
pub const HELD: Spring = Spring::new(40_000.0, 1.0);
/// A card already in a list moving over to make room or close a gap: soft,
/// ease in and out.
pub const SHIFT: Spring = Spring::new(300.0, 1.0);
/// A card settling into its list slot after a drop: it lands in about 0.12 s,
/// as in the reference clip.
pub const SLOT: Spring = Spring::new(1500.0, 1.0);
/// A new card flying from the new-task field into its column.
pub const FLY: Spring = Spring::new(220.0, 1.0);
/// The task modal growing out of its card and back: like the cards themselves,
/// critically damped (no give, no jelly), most of the way in about 0.25 s and
/// still in about 0.5 s; the angle rides the same spring. The panel's contents
/// are clipped by its frame, so nothing inside moves.
pub const MORPH: Spring = Spring::new(300.0, 1.0);
pub const MORPH_BACK: Spring = MORPH;

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

/// How far a scroll moves the fingers (or the wheel) down, in points, from an
/// event's vertical delta: with natural scrolling (`inverted`, the delta follows
/// the fingers) the delta is the motion itself; without it, the delta opposes it.
/// The same gesture reads the same whatever the system setting.
pub fn gesture_down(delta_y: f64, inverted: bool) -> f64 {
    if inverted { delta_y } else { -delta_y }
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

/// A held card's turn about the grab point: natural period and damping ratio of
/// its return to level, and the most it turns.
pub const GRIP_PERIOD: f64 = 0.5;
pub const GRIP_ZETA: f64 = 0.7;
pub const GRIP_MAX: f64 = 8.0;
/// How much of the full rigid-body turn a held card takes: matched frame by
/// frame to the reference pull (a few degrees of lean, docs/reference/tima/
/// motion/compare), so it reads as weight, not as hanging.
pub const GRIP_GAIN: f64 = 0.9;

/// A card held at the point it was grabbed: a rigid body hanging from the
/// pointer there. The grab point follows the pointer (very stiff, critically
/// damped); the card turns about it as its own inertia makes it, so it swings
/// round in the direction it is moved when held by a corner, and mostly just
/// slides when held at its centre; a damped spring brings it back level.
/// Angles in degrees, positive clockwise (y down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grip {
    /// The grab point from the card's centre, in the card's own unturned frame.
    pub r: (f64, f64),
    /// The grab point on screen, and its velocity.
    pub px: f64,
    pub py: f64,
    pub vx: f64,
    pub vy: f64,
    /// The card's turn and its rate, in degrees and degrees per second.
    pub theta: f64,
    pub omega: f64,
}

impl Grip {
    /// Held at `p` on screen, `r` from the centre in the card's frame, turned `theta`.
    pub fn new(r: (f64, f64), p: (f64, f64), theta: f64) -> Self {
        Self {
            r,
            px: p.0,
            py: p.1,
            vx: 0.0,
            vy: 0.0,
            theta,
            omega: 0.0,
        }
    }

    /// Moves the grab point towards `to` for `dt` seconds; `size` is the card's
    /// (width, height) as drawn.
    pub fn step(&mut self, to: (f64, f64), dt: f64, size: (f64, f64)) {
        let steps = (dt / (1.0 / 480.0)).ceil().max(1.0) as usize;
        let h = dt / steps as f64;
        let (k, c) = (HELD.k, HELD.damping());
        let w0 = std::f64::consts::TAU / GRIP_PERIOD;
        // Moment of inertia of the card about its centre, per unit mass.
        let i_cm = (size.0 * size.0 + size.1 * size.1) / 12.0;
        for _ in 0..steps {
            let (vx0, vy0) = (self.vx, self.vy);
            self.vx += (-k * (self.px - to.0) - c * self.vx) * h;
            self.vy += (-k * (self.py - to.1) - c * self.vy) * h;
            self.px += self.vx * h;
            self.py += self.vy * h;
            let (ax, ay) = ((self.vx - vx0) / h, (self.vy - vy0) / h);
            // From the grab point to the centre, turned with the card.
            let (gx, gy) = turn(self.r, self.theta);
            let (cx, cy) = (-gx, -gy);
            // Seen from the moving grab point the centre is pushed against the
            // acceleration; its torque about the grab point turns the card.
            let torque = cx * (-ay) - cy * (-ax);
            let inertia = i_cm + cx * cx + cy * cy;
            let theta = self.theta.to_radians();
            let omega = self.omega.to_radians();
            let alpha =
                GRIP_GAIN * torque / inertia - w0 * w0 * theta - 2.0 * GRIP_ZETA * w0 * omega;
            let omega = omega + alpha * h;
            self.omega = omega.to_degrees();
            self.theta = (theta + omega * h).to_degrees();
            if self.theta.abs() > GRIP_MAX {
                self.theta = GRIP_MAX.copysign(self.theta);
                self.omega = 0.0;
            }
        }
    }

    /// Where the card's centre is now.
    pub fn centre(&self) -> (f64, f64) {
        let (gx, gy) = turn(self.r, self.theta);
        (self.px - gx, self.py - gy)
    }
}

/// `v` turned by `deg` degrees clockwise (y down).
pub fn turn(v: (f64, f64), deg: f64) -> (f64, f64) {
    let (s, c) = deg.to_radians().sin_cos();
    (v.0 * c - v.1 * s, v.0 * s + v.1 * c)
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
    /// In a stack or a fan: the stack's centre, in the same top-left frame as
    /// `x, y` (the centre less half a card). The body then falls onto its place
    /// by [`FALL`] instead of its spring: toward and away from that centre, and
    /// round it, so a fan's cards slide round the spiral to new places and a
    /// card dropped in curves in toward the centre. Its angle falls too
    /// ([`FALL_TURN`]), with no swing.
    pub fall: Option<(f64, f64)>,
    /// Its stack is folding: the turn levels calmly ([`FALL_TURN_FOLD`]).
    pub folding: bool,
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
            fall: None,
            folding: false,
            swings: true,
        }
    }

    /// A new place, by its spring (no longer in a stack).
    pub fn target(&mut self, x: f64, y: f64, r: f64) {
        self.tx = x;
        self.ty = y;
        self.tr = r;
        self.fall = None;
    }

    /// A place in a stack or a fan whose centre is `centre` (in the top-left
    /// frame of `x, y`): the body falls onto it ([`Body::fall`]).
    pub fn fall_to(&mut self, (x, y, r): (f64, f64, f64), centre: (f64, f64)) {
        self.tx = x;
        self.ty = y;
        self.tr = r;
        self.fall = Some(centre);
    }

    /// The fall's acceleration for this body (see [`Body::fall`]).
    fn fall_accel(&self, (cx, cy): (f64, f64)) -> (f64, f64) {
        let (px, py) = (self.x - cx, self.y - cy);
        let (qx, qy) = (self.tx - cx, self.ty - cy);
        let (rho, reach) = (px.hypot(py), qx.hypot(qy));
        let straight = (
            FALL.accel(self.x - self.tx, self.vx),
            FALL.accel(self.y - self.ty, self.vy),
        );
        // Near the centre (a pile place, or a card on its way to one), round
        // it means a whirl on the spot: a card a few points off the centre
        // swung most of a turn round it. There it goes straight; out on the fan
        // round the centre, with a smooth blend between.
        let w = ((reach.min(rho) - ROUND_FROM) / ROUND_BLEND).clamp(0.0, 1.0);
        if w == 0.0 {
            return straight;
        }
        let u = (px / rho, py / rho);
        let t = (-u.1, u.0);
        let (v_out, v_round) = (self.vx * u.0 + self.vy * u.1, self.vx * t.0 + self.vy * t.1);
        let round = wrap(py.atan2(px) - qy.atan2(qx)) * rho;
        // Going round takes a pull in toward the centre, or the body would
        // drift outward off the circle.
        let a_out = FALL.accel(rho - reach, v_out) - v_round * v_round / rho;
        let a_round = FALL.accel(round, v_round);
        let polar = (a_out * u.0 + a_round * t.0, a_out * u.1 + a_round * t.1);
        (
            straight.0 + (polar.0 - straight.0) * w,
            straight.1 + (polar.1 - straight.1) * w,
        )
    }

    /// Advances by `dt` seconds, in small substeps so stiff springs stay stable.
    pub fn step(&mut self, dt: f64) {
        let steps = (dt / (1.0 / 480.0)).ceil().max(1.0) as usize;
        let h = dt / steps as f64;
        let (k, c) = (self.spring.k, self.spring.damping());
        let w0 = std::f64::consts::TAU / SWING_PERIOD;
        for _ in 0..steps {
            if let Some(c) = self.fall {
                let (ax, ay) = self.fall_accel(c);
                self.vx += ax * h;
                self.vy += ay * h;
            } else {
                self.vx += (-k * (self.x - self.tx) - c * self.vx) * h;
                self.vy += (-k * (self.y - self.ty) - c * self.vy) * h;
            }
            // However the springs are pushed (a stall, a big kick), no card
            // flies faster than this.
            let speed = self.vx.hypot(self.vy);
            if speed > MAX_SPEED {
                self.vx *= MAX_SPEED / speed;
                self.vy *= MAX_SPEED / speed;
            }
            self.x += self.vx * h;
            self.y += self.vy * h;
            if self.fall.is_some() {
                let turn = if self.folding {
                    FALL_TURN_FOLD
                } else {
                    FALL_TURN
                };
                self.vr += turn.accel(self.r - self.tr, self.vr) * h;
                self.r += self.vr * h;
                continue;
            }
            // The angle leans into the sideways motion (clockwise moving right,
            // as the bottom lags), through a damped response.
            let lean = if self.swings {
                (self.vx * SWING_PER_SPEED).clamp(-SWING_MAX, SWING_MAX)
            } else {
                0.0
            };
            let swing = self.r - self.tr - lean;
            if self.swings {
                self.vr += (-w0 * w0 * swing - 2.0 * SWING_ZETA * w0 * self.vr) * h;
            } else {
                // The panel turns on its own spring, with its frame, all the way
                // from the card's angle: no swing, no limit.
                self.vr += (-k * swing - c * self.vr) * h;
            }
            self.r += self.vr * h;
            if self.swings && (self.r - self.tr).abs() > SWING_MAX {
                self.r = self.tr + SWING_MAX.copysign(self.r - self.tr);
                self.vr = 0.0;
            }
        }
        // Something gave a non-number: land rather than vanish or fly.
        if ![self.x, self.y, self.r, self.vx, self.vy, self.vr]
            .iter()
            .all(|v| v.is_finite())
        {
            self.settle();
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

/// How far the fingers go down over a stack (points) to fan it all the way out:
/// the fan follows the scroll, a slow scroll leaving it part open.
pub const FAN_SCRUB: f64 = 140.0;
/// A scroll step this big (points in one event) is a fling: the fan goes all
/// the way at once, in its direction.
pub const FAN_FLING: f64 = 40.0;
/// A part-open fan left alone this long (seconds) opens or folds, whichever is
/// nearer.
pub const FAN_SNAP_IDLE: f64 = 0.12;

/// The opening speed (openness a second) a fan yanked open by fingers moving
/// `speed` points a second down keeps: nothing for an ordinary scroll, then
/// growing with the yank, never past [`FAN_YANK_MAX`], so a hard yank
/// recoils past fully open a little and settles back.
pub fn fan_yank(speed: f64) -> f64 {
    ((speed - FAN_YANK_FROM) / FAN_SCRUB).clamp(0.0, FAN_YANK_MAX)
}

/// Finger speed (pt/s) under which opening a fan gives no extra recoil.
pub const FAN_YANK_FROM: f64 = 600.0;
/// The most extra opening speed a yank gives: about 6% more past fully open.
pub const FAN_YANK_MAX: f64 = 1.2;

/// How open a stack is (0 folded, 1 fanned) after a scroll of `down` points.
pub fn fan_scrub(open: f64, down: f64) -> f64 {
    if down.abs() >= FAN_FLING {
        return if down > 0.0 { 1.0 } else { 0.0 };
    }
    (open + down / FAN_SCRUB).clamp(0.0, 1.0)
}

/// Where a part-open fan left alone goes: open when past half.
pub fn fan_snap(open: f64) -> bool {
    open > 0.5
}

/// The fastest any card moves, in points a second.
pub const MAX_SPEED: f64 = 6000.0;

/// An angle brought into −π..π.
fn wrap(a: f64) -> f64 {
    (a + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
}

/// How far a fanned card strays from its exact place, so the fan looks laid
/// down by hand: at 1.0, up to ±4° in its turn, ±4 pt out from the centre and
/// ±4° round it.
pub const FAN_JITTER: f64 = 1.0;
const JITTER_MAX: f64 = 4.0;

/// A card's own stray from its fan place, fixed by `seed` (its id), so it is
/// the same every time the fan opens: (turn in degrees, out in points, round
/// in degrees).
pub fn fan_jitter(seed: u64) -> (f64, f64, f64) {
    let mut z = seed;
    let mut next = || {
        // splitmix64
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut x = z;
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^= x >> 31;
        (x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    let a = JITTER_MAX * FAN_JITTER;
    (next() * a, next() * a, next() * a)
}

/// The top of something `tall` high centred between `top` and `bottom`, but
/// never closer than 10 pt under `top`.
pub fn centred_top(top: f64, bottom: f64, tall: f64) -> f64 {
    top + ((bottom - top - tall) / 2.0).max(10.0)
}

/// The `i`-th card of a collapsed stack (0 = top): how far down its centre sits
/// and its scale. The cards behind are a touch narrower and peek out below the
/// top card as thin slivers, two of them; the rest hide behind the second.
pub fn pile(i: usize) -> (f64, f64) {
    let i = i.min(2) as f64;
    (11.0 * i, 1.0 - 0.035 * i)
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
    fn a_fall_carries_a_little_past_and_settles() {
        // From 1 pt to 600 pt, from rest or thrown away from the place: past it
        // by a little (never a fifth), back and still within a second and a
        // half; longer falls take longer, but not in proportion.
        let mut last = 0.0;
        for d in [1.0, 10.0, 50.0, 100.0, 200.0, 600.0] {
            for v0 in [0.0, 800.0] {
                let (mut e, mut v, mut t, h) = (d, v0, 0.0, 1.0 / 480.0);
                let (mut past, mut arrived, mut still) = (0.0_f64, None, 0.0);
                while t < 3.0 {
                    v += FALL.accel(e, v) * h;
                    e += v * h;
                    t += h;
                    past = past.max(-e);
                    if arrived.is_none() && e < 0.05 * d {
                        arrived = Some(t);
                    }
                    if e.abs() > 0.05 * d {
                        still = t;
                    }
                }
                // Thrown, a short fall carries on by what the throw gives it.
                let room = if v0 == 0.0 {
                    0.2 * d
                } else {
                    0.2 * d.max(60.0)
                };
                assert!(past < room, "{d} pt, thrown {v0}: past by {past}");
                assert!(still < 1.5, "{d} pt, thrown {v0}: settled after {still}s");
                if v0 == 0.0 {
                    let arrived = arrived.unwrap();
                    assert!(arrived >= last, "{d} pt in {arrived}s");
                    last = arrived;
                }
            }
        }
        // A re-layout of 100 pt: a visible overshoot, as in the reference (6–19%).
        let (mut e, mut v, mut past) = (100.0, 0.0, 0.0_f64);
        for _ in 0..960 {
            v += FALL.accel(e, v) / 480.0;
            e += v / 480.0;
            past = past.max(-e);
        }
        assert!(past > 5.0 && past < 19.0, "past by {past}");
    }

    #[test]
    fn a_stack_opens_and_folds_as_one_fall() {
        // Opening from folded, and back: a little past the end (the action's
        // overshoot), back, and at rest within a second and a half; never a
        // jump between frames.
        for (from, to) in [(0.0, 1.0), (1.0, 0.0), (0.3, 1.0)] {
            let mut o = Opening { at: from, v: 0.0 };
            let (mut t, mut prev, mut past) = (0.0, from, 0.0_f64);
            while o.step(to, 1.0 / 120.0) {
                t += 1.0 / 120.0;
                past = past.max((o.at - to) * (to - from).signum());
                assert!((o.at - prev).abs() < 0.06, "jumped at {t}s");
                prev = o.at;
                assert!(t < 1.5, "{from}→{to} still moving");
            }
            assert!(past > 0.02 && past < 0.1, "{from}→{to}: past by {past}");
        }
        // Its start is gentle: after one frame it has barely moved.
        let mut o = Opening::default();
        o.step(1.0, 1.0 / 60.0);
        assert!(o.at < 0.01, "{}", o.at);
        // A scroll moving the goal along is followed without a jump.
        let mut o = Opening::default();
        let mut prev = 0.0;
        for f in 0..120 {
            let goal = (f as f64 * 0.02).min(1.0);
            o.step(goal, 1.0 / 60.0);
            assert!((o.at - prev).abs() < 0.1, "jumped at frame {f}");
            prev = o.at;
        }
        assert_eq!(
            fan_mix((0.0, 0.0, 0.0), (10.0, 20.0, 30.0), 0.5),
            (5.0, 10.0, 15.0)
        );
        assert_eq!(
            fan_mix((0.0, 0.0, 0.0), (10.0, 20.0, 30.0), 1.1),
            (11.0, 22.0, 33.0)
        );
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
            b.target(10.0 * k as f64, 0.0, 0.0);
            b.step(dt);
            tilt = tilt.max(b.r);
        }
        assert!(tilt > 1.0 && tilt <= SWING_MAX, "tilt {tilt}");
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
        // One slight overshoot past level, no more.
        assert!(back < -0.01 && back > -0.4 * tilt, "swing back {back}");
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
        // A fan card falls into its angle, a little past, and stays there.
        let mut b = Body::at(0.0, 0.0);
        b.fall_to((30.0, -20.0, 40.0), (0.0, 60.0));
        let (mut t, mut most) = (0.0, 0.0_f64);
        while !b.at_rest() && t < 3.0 {
            b.step(1.0 / 120.0);
            most = most.max(b.r);
            t += 1.0 / 120.0;
        }
        assert!((b.r - 40.0).abs() < 0.05 && t < 1.6, "{} after {t}s", b.r);
        assert!(most <= 40.0 * 1.2, "turned past to {most}");
    }

    #[test]
    fn a_fanned_card_slides_round_its_spiral() {
        // Moved one slot round (40°) a fan of radius 100: it goes round the
        // centre, not along the chord (which dips to 94 from it), and lands a
        // little past its place, then settles.
        let rad = 100.0_f64;
        let (a0, a1) = (0.0_f64, 40.0_f64.to_radians());
        let mut b = Body::at(rad * a0.cos(), rad * a0.sin());
        b.fall_to((rad * a1.cos(), rad * a1.sin(), 0.0), (0.0, 0.0));
        let (mut low, mut high, mut past, mut t) = (f64::MAX, 0.0_f64, 0.0_f64, 0.0);
        while !b.at_rest() && t < 3.0 {
            b.step(1.0 / 120.0);
            t += 1.0 / 120.0;
            let r = b.x.hypot(b.y);
            low = low.min(r);
            high = high.max(r);
            past = past.max(b.y.atan2(b.x) - a1);
        }
        assert!(low > 98.0 && high < 102.0, "off the circle: {low}..{high}");
        assert!(past > 0.0 && past < 0.2 * a1, "past by {past}");
        assert!(t < 1.6, "{t}s");
    }

    #[test]
    fn a_stack_centres_between_the_header_and_the_capsule() {
        // A 110 pt pile in a column from 40 to 600: its middle at 320.
        let y = centred_top(40.0, 600.0, 110.0);
        assert!((y + 55.0 - 320.0).abs() < 1e-9);
        // No room: kept just under the header.
        assert_eq!(centred_top(40.0, 100.0, 110.0), 50.0);
    }

    #[test]
    fn a_hard_yank_recoils_a_little_and_a_scroll_not_at_all() {
        assert_eq!(fan_yank(300.0), 0.0);
        assert!(fan_yank(1200.0) > 0.0);
        assert_eq!(fan_yank(1e6), FAN_YANK_MAX);
        let mut most = 0.0_f64;
        let mut o = Opening {
            at: 0.8,
            v: FAN_YANK_MAX,
        };
        let mut t = 0.0;
        while o.step(1.0, 1.0 / 120.0) && t < 2.0 {
            most = most.max(o.at);
            t += 1.0 / 120.0;
        }
        assert!(most > 1.02 && most < 1.15 && t < 1.6, "to {most} in {t}s");
    }

    #[test]
    fn the_second_card_falls_into_the_pile_without_a_whirl() {
        // From its fan place 50 pt up-right of the centre to its pile place
        // 11 pt below it: no swing round the centre, near the straight line.
        let a = (-46.0_f64).to_radians();
        let (fx, fy) = (50.0 * a.cos(), 50.0 * a.sin());
        let mut b = Body::at(fx, fy);
        b.fall_to((0.0, 11.0, 0.0), (0.0, 0.0));
        let (lx, ly) = (0.0 - fx, 11.0 - fy);
        let len = lx.hypot(ly);
        let (mut off, mut t) = (0.0_f64, 0.0);
        while !b.at_rest() && t < 3.0 {
            b.step(1.0 / 120.0);
            off = off.max(((b.x - fx) * ly - (b.y - fy) * lx).abs() / len);
            t += 1.0 / 120.0;
        }
        assert!(off < 8.0, "{off} pt off the line");
        // And its turn levels calmly as the stack folds: never past level.
        let mut b = Body::at(0.0, 11.0);
        b.r = -60.0;
        b.folding = true;
        b.fall_to((0.0, 11.0, 0.0), (0.0, 0.0));
        let (mut past, mut t) = (0.0_f64, 0.0);
        while !b.at_rest() && t < 2.0 {
            b.step(1.0 / 120.0);
            past = past.max(b.r);
            t += 1.0 / 120.0;
        }
        assert!(past < 1e-6 && t < 0.45, "past by {past}, level after {t}s");
    }

    #[test]
    fn the_fan_follows_the_scroll() {
        let p = fan_scrub(0.0, 14.0);
        assert!((p - 0.1).abs() < 1e-9, "{p}");
        let mut p = 0.0;
        for _ in 0..5 {
            p = fan_scrub(p, 7.0);
        }
        assert!((p - 0.25).abs() < 1e-9 && !fan_snap(p));
        assert_eq!(fan_scrub(0.9, 30.0), 1.0);
        assert_eq!(fan_scrub(p, -20.0), 0.25 - 20.0 / FAN_SCRUB);
        assert!(fan_snap(0.6));
        // A fling goes all the way.
        assert_eq!(fan_scrub(0.1, 60.0), 1.0);
        assert_eq!(fan_scrub(0.9, -60.0), 0.0);
    }

    #[test]
    fn no_card_runs_wild_under_random_handling() {
        // Random targets, places on fans, and throws, with frames from
        // 1/120 s to 2 s: every card stays a number, within reach of the board
        // and under the speed limit, and comes to rest on its place.
        let mut z = 0x1234_5678_9ABC_DEF0_u64;
        let mut rnd = || {
            z ^= z << 13;
            z ^= z >> 7;
            z ^= z << 17;
            (z >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut cards: Vec<Body> = (0..8).map(|i| Body::at(100.0 * i as f64, 50.0)).collect();
        for round_ in 0..400 {
            let i = (rnd() * 8.0) as usize % 8;
            let (x, y) = (rnd() * 1500.0, rnd() * 1000.0);
            match (rnd() * 5.0) as u32 {
                0 => cards[i].target(x, y, rnd() * 60.0 - 30.0),
                1 | 2 | 3 => {
                    let a = rnd() * std::f64::consts::TAU;
                    let r = [0.0, 0.5, 40.0 + rnd() * 120.0][(rnd() * 3.0) as usize % 3];
                    let (dx, dy) = (r * a.cos(), r * a.sin());
                    cards[i].fall_to((x + dx, y + dy, rnd() * 90.0 - 45.0), (x, y));
                }
                _ => {
                    cards[i].vx += rnd() * 1e5 - 5e4;
                    cards[i].vy += rnd() * 1e5 - 5e4;
                }
            }
            let dt = [1.0 / 120.0, 1.0 / 60.0, 1.0 / 30.0, 0.5, 2.0][round_ % 5];
            for b in &mut cards {
                b.step(dt);
                assert!(
                    b.x.is_finite() && b.y.is_finite() && b.r.is_finite(),
                    "{b:?}"
                );
                assert!(b.vx.hypot(b.vy) <= MAX_SPEED + 1e-6, "{b:?}");
                assert!(
                    b.x > -3000.0 && b.x < 4500.0 && b.y > -3000.0 && b.y < 4000.0,
                    "{b:?}"
                );
            }
        }
        for _ in 0..600 {
            for b in &mut cards {
                b.step(1.0 / 60.0);
            }
        }
        for b in &cards {
            assert!((b.x - b.tx).hypot(b.y - b.ty) < 1.0, "{b:?}");
        }
    }

    #[test]
    fn fan_jitter_is_small_and_fixed_per_card() {
        assert_eq!(fan_jitter(42), fan_jitter(42));
        assert_ne!(fan_jitter(42), fan_jitter(43));
        for s in 0..200 {
            let (a, b, c) = fan_jitter(s);
            assert!(a.abs() <= JITTER_MAX && b.abs() <= JITTER_MAX && c.abs() <= JITTER_MAX);
        }
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

    /// Drags a card held at `r` (from its centre) to the right and stops;
    /// returns the largest turn either way and the turn at the end.
    fn grip_drag(r: (f64, f64)) -> (f64, f64, f64) {
        let size = (230.0, 101.0);
        let mut g = Grip::new(r, (0.0, 0.0), 0.0);
        let dt = 1.0 / 120.0;
        let (mut most, mut least) = (0.0f64, 0.0f64);
        for k in 0..240 {
            let x = if k < 30 { 40.0 * k as f64 } else { 1200.0 };
            g.step((x, 0.0), dt, size);
            most = most.max(g.theta);
            least = least.min(g.theta);
        }
        (most, least, g.theta)
    }

    #[test]
    fn where_a_card_is_held_decides_how_it_turns() {
        // Held by the top-left corner and pulled right, it swings clockwise; by
        // the bottom-left, the other way; at the centre it hardly turns.
        let (top, _, end_top) = grip_drag((-115.0, -50.0));
        let (_, bottom, end_bottom) = grip_drag((-115.0, 50.0));
        let (c_most, c_least, _) = grip_drag((0.0, 0.0));
        assert!(top > 1.0 && top <= GRIP_MAX, "top-left: {top}");
        assert!(bottom < -1.0, "bottom-left: {bottom}");
        assert!(c_most.max(-c_least) < 0.3, "centre: {c_most} {c_least}");
        // And it comes back level while still held.
        assert!(
            end_top.abs() < 0.2 && end_bottom.abs() < 0.2,
            "{end_top} {end_bottom}"
        );
    }

    #[test]
    fn a_held_card_stays_stable_at_3000_pt_per_s() {
        let mut g = Grip::new((-115.0, -50.0), (0.0, 0.0), 0.0);
        let (dt, mut x, mut dir) = (1.0 / 120.0, 0.0f64, 1.0f64);
        for k in 0..600 {
            if k % 12 == 0 {
                dir = -dir;
            }
            x += 3000.0 * dt * dir;
            g.step((x, x * 0.3), dt, (230.0, 101.0));
            assert!(g.theta.is_finite() && g.theta.abs() <= GRIP_MAX + 1e-9);
            let (cx, cy) = g.centre();
            assert!(cx.is_finite() && cy.is_finite());
        }
    }

    #[test]
    fn a_downward_gesture_reads_down_with_either_scrolling_setting() {
        // Fingers moving down: natural scrolling reports a positive delta, the
        // classic setting a negative one; both are the same gesture.
        assert_eq!(gesture_down(5.0, true), 5.0);
        assert_eq!(gesture_down(-5.0, false), 5.0);
        // And up.
        assert_eq!(gesture_down(-5.0, true), -5.0);
        assert_eq!(gesture_down(5.0, false), -5.0);
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
    fn a_card_dropped_into_a_stack_falls_in_and_settles() {
        // From anywhere round a pile, at any speed: once it has got there it
        // carries on past by little (never a fifth of its way), and lies still
        // within a second and a half.
        for k in 0..64 {
            let a = k as f64 / 64.0 * std::f64::consts::TAU;
            let mut b = Body::at(250.0 * a.cos(), 250.0 * a.sin());
            b.vx = 600.0 * (a * 3.0).sin();
            b.vy = 600.0 * (a * 5.0).cos();
            b.fall_to((0.0, 0.0, 0.0), (0.0, 0.0));
            let (mut t, mut reached, mut past) = (0.0, false, 0.0_f64);
            while !b.at_rest() && t < 3.0 {
                b.step(1.0 / 120.0);
                let d = b.x.hypot(b.y);
                if d < 5.0 {
                    reached = true;
                } else if reached {
                    past = past.max(d);
                }
                t += 1.0 / 120.0;
            }
            assert!(past < 0.2 * 250.0, "past the pile by {past} from {a}");
            assert!(t < 1.5, "settled after {t}s from {a}");
        }
    }

    #[test]
    fn the_panel_turns_from_a_fanned_card_without_a_jump() {
        // A card turned −70° in a fan grows into the upright panel: the angle
        // eases all the way, never more than a few degrees a frame.
        let mut b = Body::at(0.0, 0.0);
        b.spring = MORPH;
        b.swings = false;
        b.r = -70.0;
        b.tr = -70.0;
        b.target(200.0, 100.0, 0.0);
        let (dt, mut last, mut t) = (1.0 / 120.0, -70.0_f64, 0.0);
        while !b.at_rest() && t < 2.0 {
            b.step(dt);
            assert!((b.r - last).abs() < 6.0, "{last} -> {} at {t}", b.r);
            last = b.r;
            t += dt;
        }
        assert!(t < 0.8, "settled after {t}s");
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
        // No give past its size: it comes to rest like a card does.
        let mut b = Body::at(0.0, 0.0);
        b.spring = MORPH;
        b.swings = false;
        b.target(100.0, 0.0, 0.0);
        let mut far = 0.0_f64;
        for _ in 0..240 {
            b.step(1.0 / 120.0);
            far = far.max(b.x);
        }
        assert!(far <= 100.0 + 1e-6, "{far}");
        // Back into the card: never under its size.
        let mut b = Body::at(100.0, 0.0);
        b.spring = MORPH_BACK;
        b.swings = false;
        b.target(0.0, 0.0, 0.0);
        let mut low = 100.0_f64;
        for _ in 0..240 {
            b.step(1.0 / 120.0);
            low = low.min(b.x);
        }
        assert!(low > -1e-6, "{low}");
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
}
