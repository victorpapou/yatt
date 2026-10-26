# Our motion against the reference, frame by frame

Reference: `docs/reference/event-stack-shifting.mp4` (60 fps), 50 frames per
action from the times given. Ours: `YATT_DEMO_COMPARE=1 YATT_DEMO=<dir>`, the
same actions scripted on a 6-card board and stepped at a fixed 1/60 s per frame
(the frame timer off, so every run gives the same frames), a Core Animation
render per frame; the lead card's centre, angle and scale per frame are in
`ours.csv`. Each sheet: reference rows on top, ours below.

The reference's angles and timings are read off the frames by eye (no tracker);
ours are exact.

| Action | Reference | Ours | Verdict |
|---|---|---|---|
| fan open (`fan-open.png`) | pile opens in about 8 frames (0.13 s), cards turn into place with one small give | moving until frame 7, angles within 0.3° of final by frame 13, one give of up to 7° | matches; ours turns a little further on the way |
| fan fold (`fan-fold.png`) | folds in about 8–10 frames | moving until frame 6, angle settled by frame 13 | close; ours is slightly quicker |
| single drag (`single-drag.png`) | card leans a few degrees (about 3–5°) with the motion, level again within about 10 frames of stopping | lean −3.6°…+3.0°, settled 17 frames after the pointer stops | matches in size; ours settles a little slower |
| single drop (`single-drop.png`) | lands within about 7 frames | moving until frame 9, angle settled by frame 17 | close |
| group drag (`group-drag.png`) | the train stretches into a diagonal chain about a card apart at speed, reels in when slowing | lean −3.0°…+2.5° on the lead; followers chained, 40 ms a link for four cards | the chain is shorter than the reference's at speed |
| group drop (`group-drop.png`) | lead lands in about 7 frames, the chain catches up over about 0.35 s and folds into the pile | lead moving until frame 9; the group lands together | ours folds the chain faster |

Constants after this pass (crates/yatt-macos/src/physics.rs): grip gain 0.9,
grip period 0.5 s, ζ 0.7, at most 8°; lean 3° per 1000 pt/s, period 0.45 s,
ζ 0.7, at most 7°; slot spring k 1500 (critically damped); chain link 80 / 60 /
40 ms for 2 / 3 / 4+ cards.

Residual differences: the reference's chain at speed is longer than ours (its
links lag more, about 0.1 s each at six cards); ours settles a held card's
lean a little slower after a stop. The clip has no modal expand, hover or
pickup-without-drag to compare.
