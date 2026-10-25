# Card motion, measured from the reference clip

Source: `docs/reference/event-stack-shifting.mp4` (the clip behind
https://x.com/artntek/status/2100247096993485129, 60 fps), cut at 20 fps:

- `pull-20fps.png`: 12.30–13.85 s, one card pulled out of a fan and dropped.
- `drag-20fps.png`: 4.30–6.05 s, a stack shift-dragged as a group and dropped.

## What the frames show

- A carried card hangs a little: it tilts against its motion, about 5–8° at a
  brisk drag, never much more. When the pointer stops it swings back past
  level by about 2–3° once and is still within about 0.2–0.3 s. Read as a
  damped pendulum: period about 0.45 s, damping ratio about 0.45.
- Its position is not springy: it stays with the pointer, no overshoot.
- A dragged group is a chain, not a delay curve: each card hangs off the one
  ahead. At speed the chain stretches into a diagonal staircase about a card
  apart (4.95–5.15 s); when the lead slows the chain reels in, six cards in
  about 0.6 s, so about 0.1 s of lag per link.
- After the drop the lead lands within about 0.12 s; the chain catches up and
  folds into the pile.

## Constants used (crates/yatt-macos/src/physics.rs)

- Swing: period 0.45 s, ζ 0.45, lever 150 pt (about 8° of tilt at 4000 pt/s² of
  sideways acceleration), at most 12°.
- Position: critically damped springs everywhere.
- Chain: each follower chases the card ahead on a critically damped spring
  with a time constant of half the group's step (80 / 60 / 40 ms for 2 / 3 / 4+
  cards), so each link lags about one step at a steady speed.
