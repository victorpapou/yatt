# Measured motion: the swatch fan

Clips by the same author: `event-stack-shifting.mp4` (in the repo,
https://x.com/artntek/status/2100247096993485129) and
https://x.com/artntek/status/2092930171347046747 (9.8 s, 60 fps, fetched with
yt-dlp; not committed). The second is a fan kept open while new cards are
dragged in one by one, every card re-laying itself out each time, then folded
into a stack at 9.0 s.

Method (no eyeballing): `track.py` cuts the clip at 60 fps, measures motion
energy in the fan's area per frame (mean absolute change between frames) to
find each re-layout and its time to rest, and follows one card (the orange
one, by hue) with its centroid and principal-axis angle. Per-frame values are
in `swatch-fill-curves.csv`.

## A card joins the fan (all cards re-lay out)

| Start | Motion lasts (energy) | Orange card Δ | Overshoot | Zero crossings | Within 5% of rest |
|---|---|---|---|---|---|
| 4.12 s | 700 ms | angle −87°, y −49 | 6–7% | 1 | 350–430 ms |
| 5.05 s | 717 ms | x +22, angle +22° | 6–17% | 1–3 | 520–630 ms |
| 6.23 s | 767 ms | x +10, angle +23° | 1–19% | 1–3 | 320–650 ms |
| (0.6–3.8 s, 3 more) | 700–783 ms | — | — | — | — |

Delay from the fan starting to move to this card moving: 0–17 ms, every time.

## The fan folds into a stack (9.02 s)

Motion 733 ms (energy, incl. the stack's own settle); the orange card: angle
+20°, x −28, no overshoot, within 5% of rest after 300 ms.

## What follows for our model

- Fan open and re-layout: one soft spring with a small overshoot: ζ ≈ 0.58
  (≈ 10%, one or two crossings), k ≈ 90 (within 5% after ≈ 0.55 s). Was k 220,
  ζ 0.4 (25% overshoot): too bouncy and too quick.
- Every card reacts at once: the wave between cards is cut to ≤ 5 ms a card
  (was up to 35 ms).
- Fold: critically damped, ≈ 0.3 s: k ≈ 250 (was 800, too quick).

Still unmeasured: per-card scale (the clip's cards do not visibly scale) and
the active card's own path as it is dropped in (it is hidden behind the fan
for most of the way).

## The other clips

- https://x.com/artntek/status/2092930171347046747 holds one video only:
  `/video/1` is the same 9.8 s clip analysed above.
- `event-stack-shifting.mp4` (15.3 s): the camera zooms and pans, so positions
  are not comparable between frames; card angles are (zoom and pan keep
  them). `track_angles.py` follows four cards by hue (`stack-shifting-curves.csv`).
  - Fan opening (10.3 s), the yellow card: +19° → −36°, past by 3° (5%),
    within 5% after about 0.2 s, at rest after about 0.47 s.
  - Re-layout (11.4 s): every card turns at once, within one 67 ms sample of
    each other; small give (under 10%), at rest in about 0.4 s.
  - Back toward the stack (12.9 s): yellow +6° → −33°, no overshoot, 0.33 s.

## The fan as a whole when a card joins (`spread.py`)

The 90th-percentile distance of the cards' coloured pixels from their centroid,
in the second clip: 46 → 50 → 49, 49 → 54, 54 → 52 → 56 → 55 (360 px frames).
The fan grows with each new card and overshoots its new size a little; once
(6.2 s) it first pulls in toward its centre by about 4% before spreading out:
the "rake". The cards go round the centre by one slot each, on the spiral,
not across it.

## The model now (crates/yatt-macos/src/physics.rs, `Body` with an orbit)

A fanned card is a loose body on the spiral, not a spring to a fixed point:
firm across the spiral (k 110, ζ 0.55), loose along it (k 70, ζ 0.6), turning
as it goes round; neighbours closer than half their gap shove apart; a card
dropped into an open fan gives the others an inward kick (120 pt/s next to it,
falling off by e every 3 slots), and they spring back out. Fold: critically
damped, k 250. The cards start together (at most 5 ms apart).

## Ours against the reference, after this change

`YATT_DEMO_COMPARE=1`, 1/60 s steps (`compare/ours.csv`, `metrics.py`):

| Action | Reference | Ours |
|---|---|---|
| card joins, a moved card | past 6–19%, 1–3 crossings, rest 0.35–0.65 s | past 4–14%, 1 crossing, rest 0.52–0.68 s |
| card joins, a deep card | (same) | past 8–11%, 0–1 crossing, rest 0.58–0.68 s |
| fan opens | past ≈5% in angle, rest ≈0.47 s | past 11–13% in position, rest 0.43–0.77 s |
| fold | no overshoot, ≈0.3 s | no overshoot, 0.25–0.3 s |

Still different: ours rests up to about 30 ms later than the slowest reference
case; the opening overshoots more in position than the reference does in angle
(the clip gives no positions there). The reference shows 1–3 crossings; ours 1.

## Opening: one flow, not two halves

Each card used to take the short way round from the pile to its slot, so the
cards whose slots lie more than half a turn from the pile went the other way:
two groups met in the middle ("glued from two halves"). Now a card leaving the
pile always goes clockwise, the way the fan winds, counting its way round
continuously (`Body::lap`), so the fan spills out of the stack in one flow,
older cards further round. Test: `a_fan_spills_out_one_way_round` (eight
cards: under 20% of each card's travel goes back, all of it the end's spring
back; no frame jumps more than 40 pt). The resting places now carry a small
fixed stray per card (`fan_jitter`).

## Opening, again: one blob

The clockwise flow above was wrong. In the first clip (10.1–10.9 s, 20 fps
sheets) the top card stays where the pile was and the rest slide out from
under it, each straight toward its place, all at once, keeping their order:
the pile swells into the rosette as one mass, with no card going round the
centre. So a card leaving the pile now goes straight to its place (`spill`,
spring k 110, ζ 0.55) and only then lies loose on the spiral. Ours: angle
settled in 0.2 s (4% past), position 14–20% past, at rest in 0.52–0.67 s.
Test: `a_fan_opens_as_one_blob` (eight cards, all moving from the first
frame, none off its straight line by more than 5% of its way).
