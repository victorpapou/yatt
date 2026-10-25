# Design notes

## Seamlessness (бесшовность)

The guiding rule for every transition: the same element morphs, keeping its
content and colour. Nothing is cross-faded or swapped for a separate view.

- New task: the new-task capsule, typed text and all, grows into the card on its
  way to the column; the text becomes the card's title.
- Card to modal: the panel starts as the card itself (same frame, angle, colour
  and text), grows on one spring, and only once it has settled does the card's
  text hand over to the panel's controls. Closing runs it backwards and hands back
  to the card when the panel lies exactly on it.
- Stack to fan: the stacked cards themselves spread out and fold back.
- One card to a group drag: the pressed card leads and the selected cards follow
  right under it, as they are.

## Header

The timer row is a native `NSToolbar`, unified with the titlebar: the progress
ring and the digits as plain text (an item that tells AppKit it wants no glass,
`glassBehavior` 0), then … [🍅 / ⏸ / ▶] [▶ stopwatch] [Пропустить] [Стоп] [view
icons, sharing one bubble]. The toolbar puts neighbouring items into one shared
bubble, so fixed spaces separate them. A changed digit does the system's numeric
transition: the old glyph shrinks to 0.8, blurs to 4 pt and fades up, the new
one grows in from below, sharpening, on a critically damped spring of about 0.3
s; the other digits hold still. Minimal mode hides the toolbar
and shows the same row, larger, as the whole window.

## Card material

A flat tint of the card's colour at 55% (the grey card 65%) over a 4 pt blur
of what is behind it with a saturation boost (CSS `backdrop-filter: blur(4px)
saturate(1.5)`). Continuous corners, no gradients. `YATT_GLASS=1` puts the
system's Liquid Glass (`NSGlassEffectView`, clear style, a lighter tint and the
same saturation boost under it) there instead. It has no blur radius to turn
down: its private knobs (`_variant`, `_scrimState`, `_subduedState`,
`_contentLensing`, `_adaptiveAppearance`; no "splay") are integers without
documented meanings, settable for a try with `YATT_GLASS_VARIANT` and friends.

The edge is exactly two layers (`style::Edges`), and nothing else: no stroke, no
glow, no layer shadow.

1. Outside, the outline: a black shadow at 30%, 1 pt blur, no offset, only
   outside the card.
2. Inside, a white inner edge: 65% white at the edge, fading out over 2 pt,
   only inside the card, over the tint and under the text.

Everything scales with the card (a sliver of a stack, a follower of a drag, a
fan shrunk to fit its column): the corner radius of the tint, the backdrop and
the ring, and the edge bitmaps, which are shown at a matching density.

All sizes are points: the bitmaps are drawn at 2 px per point and shown at that
scale, never multiplied by the screen's backing factor. (Core Graphics blurs
shadows in device pixels whatever the scale, so the outline's blur is converted,
and the inner edge is built from strokes, which are in points.) Tests render the
real layers and check the pixels: outside in, ring, outline, inner edge, tint.

Selection is a third layer, outside the card: a 2 pt accent ring 1.5 pt beyond
the edge (clear of the outline), corners concentric with the card's. It never
touches the card's own edge, so a selected card keeps its white inner edge
exactly as it is.

The edges are drawn once at start-up as 9-slice bitmaps on the card's continuous
corner curve, so they cost a picture per card, not a shadow pass per frame. A
layer shadow would also show through the translucent glass, darkening the card
from inside.

Type on a card: the title 15 pt semibold, tracked −0.2, at 92% of the card's
ink; the meta line (time, ● while running, ✎ with notes) 11 pt medium with
tabular digits, tracked +0.15, at 50%.

## Motion

- Nothing jumps: shadows, opacity, colour, borders, hover, selection rings, chips,
  the column highlight and view-mode switches all change over at least 100 ms.
  Where a spring makes no sense (a colour, a ring, a label's text) the change eases
  over 150 ms through `motion::soften`, because AppKit turns off implicit
  animations on a view's layer and a plain setter would jump.
- Cards are firm jelly, the one exception to "no bounce": they move on springs
  of a small engine (`physics.rs`), land with a short underdamped settle (ζ 0.8,
  about 1.5% overshoot, still within half a second) and squash and stretch a
  little with their speed. Panels and the new-task morph stay critically damped:
  their text has to read the whole way.
- A dragged group: the lead follows the pointer on a slightly jelly spring
  (ζ 0.7); up to eight followers replay the lead's own path, each the same
  step later than the one before (the first one too): 80 ms for a pair, 60 ms
  for three, 50 ms for four, 40 ms from five on, each 5% smaller (never below
  0.6), centred under the lead so none shows outside it at rest; the rest hide
  until the drop. All of them squash a little (at most 4.5%) with their speed;
  the followers tilt up to 2°. The selection ring shows while choosing, not while
  dragging. The column opens one card's room for the whole group, and the group
  lands together, keeping the stacking order it flew in until the last card is
  down.
- A fan is the swatch layout of the reference frames
  (docs/reference/tima/swatch): from the newest card, just right of the top and
  turned about −70°, the cards wind clockwise round one centre about 40° apart
  (fewer cards go less far round), each turned about 13° further, to the oldest
  at the upper left at about +38°; newer cards on top. The radius grows with the
  count (about 0.36 card widths for nine). The whole rosette shrinks uniformly to
  fit its column and the board's height (angles unchanged, never below 0.6), and
  draws up to 20 cards; it opens in about 0.3 s and folds in about 0.22 s.
- A click on a fan folds it within 0.22 s (sooner than the system's double-click
  interval, which can be 0.8 s); a double-click on a fanned card opens it. A
  click on empty board folds it at once.
- Scrolling is the system's own: each column is an `NSScrollView`, rubber band
  and momentum included.

## No artefacts

Seamless also means nothing flickers or shows through: no flash or fade when a
card moves to another column or onto the drag layer (views move between parents
with Core Animation's implicit actions off, `motion::quietly`), no shadows piling
up where cards overlap, and no part of a card drawing outside its shape when it
scales (the blur and the tint are sized by hand; only the text scales). Cards
that ride the overlay to a new home land there together, once all of them are at
rest, so a card that settles first never drops under the others.

## Large boards

Cost follows what is on screen, not the board's size: list rows more than half
a screen outside their column's visible area are hidden and let go of their
backdrop; rows out of sight jump to their slots instead of animating; a fan or
a drag draws a bounded number of cards; card glyphs are cached bitmaps; a card
at rest is not touched by the frame timer. With `YATT_PERF=1` and
`YATT_DEMO_PERF=1000`, a 1000-card board scrolls and drags 20 cards at a frame
interval p95 of about 16–18 ms on the plain blur.

## Diagnostics

- `YATT_PERF=1`: frame intervals of every drag, and the time from a stack click
  to its first fan frame, on stderr.
- `YATT_EVENTLOG=1`: every mouse and scroll event (window, hit view, first
  responder) and the board's decisions, in `/tmp/yatt-events.log`.
- `YATT_DEMO=<dir>`: the scripted demo with captures; `YATT_DEMO_PERF=<cards>`,
  `YATT_DEMO_FAN=<cards>` and `YATT_DEMO_TOOLBAR=1` are short variants.
