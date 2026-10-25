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

The timer row is a native `NSToolbar` item, unified with the titlebar: a progress
ring (a donut that fills clockwise from the top) before the digits, then the
phase over the tasks, the controls on the right. Minimal mode hides the toolbar
and shows the same row, larger, as the whole window.

## Card material

A flat tint of the card's colour at 55% (white cards 65%) over a 4 pt blur of
what is behind it with a saturation boost (CSS `backdrop-filter: blur(4px)
saturate(1.2)`), so text behind is only a soft patch. Continuous corners, no
gradients. `YATT_GLASS=1` swaps the blur for the system's own Liquid Glass
(`NSGlassEffectView`), to compare look and cost.

The edge is exactly two layers (`style::Edges`), and nothing else: no stroke, no
glow, no layer shadow.

1. Outside, the outline: a black shadow at 30%, 1 pt blur, no offset, only
   outside the card.
2. Inside, a white inner shadow about 2 pt deep along the edge, only inside it.

All sizes are points: the bitmaps are drawn at 2 px per point and shown at that
scale, never multiplied by the screen's backing factor.

Both are drawn once at start-up as 9-slice bitmaps on the card's continuous
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
- A dragged group: the lead follows the pointer; the first follower rides exactly
  under it with no lag; the rest chase the lead with a lag that grows along an
  ease-out curve over the index (big steps right behind the head, smaller ones
  towards the end). The bigger the group, the less the whole tail lags. While
  they ride under the lead they shed their own blur (the lead's covers them) and
  are drawn from a cached bitmap. The selection ring shows while choosing, not
  while dragging, and the column opens one card's room for the whole group: it
  lands as a stack.
- A fan is the reference rosette; past eight cards the rest wrap onto an inner
  ring, and such a big fan keeps clear of the window's sides.
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
