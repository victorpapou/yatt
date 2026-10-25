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
