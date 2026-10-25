# Reference: "Event Stack Shifting"

Source: https://x.com/artntek/status/2100247096993485129 (15.3 s, 1442×1080, 60 fps).
The mp4 is not committed (`docs/reference/*.mp4` is ignored); key frames are below.
Measurements come from frames cut with ffmpeg at 60 fps. Sizes are given relative to the
card width `W` (the clip zooms in and out, so absolute pixels are meaningless).

| Frame | What it shows |
| --- | --- |
| `rest-stack.png` (3.05 s) | Collapsed stack at rest: one card on top, the rest peeking out |
| `fan.png` (3.75 s) | Hover: the stack fanned out like a swatch book |
| `pickup-collapse.png` (4.40 s) | Shift-drag: the fan folds back into a pile and lifts |
| `drag-train.png` (4.75 s) | The group dragged: glass lead with target label, others trailing |
| `drop-trail.png` (5.45 s) | Drop: lead lands, the train catches up |
| `settled-stack.png` (5.75 s) | Settled pile, glass top card with "3 hidden" |
| `pull-single.png` (12.55 s) | Plain drag pulls one card out of the fan |
| `single-glass-label.png` (13.10 s) | The pulled card alone: glass, blurred content, target label |

## Cards

- Size about `W × 0.44W`, corner radius `0.072W` (16 pt at W = 230).
- Pastel fills, slightly translucent with a lighter rim: blue `#7F9CF5`, pink `#F28B9B`,
  purple `#B45EF0`, yellow `#EEF58C`, green `#8FD0A2`. Text is a dark shade of the fill.
- Layout: filled glyph top-left (`0.06W` inset), `···` top-right, then at the bottom a small
  semibold line (the time) above a bold title.
- Soft shadow: about `y +2`, blur `6`, opacity ~0.12.

## Stacks

- Cards sharing a slot form one stack. The top card is frosted glass: it blurs and
  saturates the cards behind it, so it takes their colour. A capsule badge reads "N hidden".
- The rest peek out around it, with no rotation: behind-right `(+0.10W, −0.23H)`, then
  behind-left and lower `(−0.06W, +0.18H)`, `(−0.10W, +0.62H)`, `(−0.07W, +0.82H)`.
- Hover fans the stack out like a swatch book. Every card turns around a shared pivot near
  the stack's bottom-left corner. Angles run from about +20° (bottom card, clockwise) to
  about −45° (top card) in equal steps of about 13°. The hovered card turns to glass.
- Fan-out takes about 250 ms (frames 3.40–3.65 s) with a small overshoot that settles by
  about 300 ms. Collapsing on mouse-out takes the same time.

## Dragging

- Plain drag on a card pulls that card alone. Shift-drag (the ⇧ overlay at 4.0–5.0 s)
  takes the whole stack.
- Pickup: the fan folds into the pile in about 130–200 ms. The lead card turns to glass:
  its own content blurs (about 6 pt), a big target label appears centred (about 0.11W,
  semibold), and it tilts 2–4° with the horizontal velocity.
- Train: the followers trail the lead on springs. At rest their offset is a few points
  each (`≈ (−7, +5)`). Moving fast stretches them out behind the lead by up to 1–2 card
  widths, each tilting in the direction of motion. The further back a card is, the more it
  lags and tilts.
- Drop: the lead snaps into the slot within about 120 ms. The followers catch up over about
  350 ms, then collapse into the pile. The glass on the lead turns back into the stack's top
  card with a crossfade of about 150 ms.
- Spring fit (unit mass): lead `k ≈ 1200, ζ ≈ 0.85`; followers `k ≈ 420 / (1 + 0.15·i),
  ζ ≈ 0.7`; fan and pile `k ≈ 320, ζ ≈ 0.75`; tilt `≈ 0.012° per pt/s`, clamped to 4°
  (lead) and 12° (followers).

## Mapping to yatt

- A slot becomes a board column. Each column is one stack, top card = first task.
- The target label shows the column name ("В работе") instead of a time.
- The time line on a card shows tracked time (`0:12:03`) instead of the event time.
- Glyph and colour are derived from the task id until projects and tags have colours.
