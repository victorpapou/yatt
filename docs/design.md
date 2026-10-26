# Design notes

## Seamlessness (бесшовность)

The guiding rule for every transition: the same element morphs, keeping its
content and colour. Nothing is cross-faded or swapped for a separate view.

- New task: the new-task capsule, typed text and all, grows into the card on its
  way to the column; the text becomes the card's title.
- Card to modal: the panel is the card itself, one element (same frame, angle,
  material, colour and text), growing and shrinking back on one critically
  damped spring (most of the way in about 0.25 s, still in about 0.5 s), so it
  never overshoots or dips under the card before the card takes over. The
  angle rides the same spring from the card's own (a fanned card at −70° turns
  upright all the way, no snap). The dim follows that same progress, so the
  board and panel settle together. The notes editor's view follows the panel,
  never bigger than what shows, but its page keeps the open width (`--page`),
  so the notes do not reflow. The face and the white body sit in one clip of
  the card's shape, so the corners are the card's at every size; the face
  covers the whole card until the body starts to show, so no white strip is
  left under it as it closes. `YATT_DEMO_MODAL=flat` opens an upright card and
  captures every frame of the close: from the hand-over on, the pixels are the
  card's own. `YATT_DEMO_MODAL=1` logs the frame,
  angle and alphas every frame and captures the layers. Its coloured face stays on top and eases from the card's height to the
  header's; below it the same card shows its white body, growing downward as the
  card grows (none of it at card size), so the editor is the card's own content
  area, not a plate that comes in. The title and the editor stay at the card's
  top left; the card's text fades out over the first half of the way, the
  panel's controls fade in over the second, driven by the morph itself. Closing
  runs it backwards and hands back to the card when the panel lies exactly on
  it.
- Stack to fan: the stacked cards themselves spread out and fold back.
- One card to a group drag: the pressed card leads and the selected cards follow
  right under it, as they are.

## Header

The timer row is a native `NSToolbar`, unified with the titlebar: the progress
ring and the digits as plain text (an item that tells AppKit it wants no glass,
`glassBehavior` 0), then … [🍅 / ⏸ / ▶] [▶ stopwatch] [⏭ skip] [■ stop] (icons with tooltips) [view
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

1. Outside, the outline: a black shadow at 50%, 1 pt blur, no offset, only
   outside the card.
2. Inside, a soft white glow along the edge: 61% white at the edge, fading out
   on an ease-out curve over 3.75 pt, only inside the card, over the tint and
   under the text.

While a card is carried (the head of a group only) a lift shadow eases in under
it: 20% black, 24 pt blur, 8 pt down, outside the card only, pre-drawn like the
edges; it eases out again when the card is set down.

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

Type: nothing heavier than medium; the hierarchy comes from size and opacity.
On a card the title is 15 pt medium, tracked −0.2, at 92% of the card's ink; the meta line (time, ● while running, ✎ with notes) 11 pt medium with
tabular digits, tracked +0.15, at 50%.

## Motion

- Nothing jumps: shadows, opacity, colour, borders, hover, selection rings, chips,
  the column highlight and view-mode switches all change over at least 100 ms.
  Where a spring makes no sense (a colour, a ring, a label's text) the change eases
  over 150 ms through `motion::soften`, because AppKit turns off implicit
  animations on a view's layer and a plain setter would jump.
- Cards already in a list that move over to make room, or close a gap, glide on
  a critically damped spring (ease in and out, no overshoot).
- Cards feel weighted (matched to the reference clip,
  docs/reference/tima/motion). A card's position goes to its place on a
  critically damped spring (no overshoot, no jitter); its angle leans into its
  sideways motion, 3° per 1000 pt/s and at most 7°, through a damped response
  (period 0.45 s, ζ 0.7): one slight overshoot, no swinging. One integrator,
  fixed 480 Hz substeps (`Body::step`). The task panel uses it without the lean.
- A carried card is held at the very point it was grabbed: a rigid body hanging
  from the pointer there (`physics::Grip`). Moving it turns it about that point
  as its own inertia makes it: held by a corner it swings round in the direction
  it is moved, held at its centre it mostly slides. It takes a third of a rigid
  body's turn, so it reads as weight, and a damped spring brings it back level
  (period 0.5 s, ζ 0.7, at most 8°), while held and after the drop.
- A dragged group is a chain: the lead is held as above, each follower hangs off
  the card ahead, lagging one link (80 ms for two cards, 60 ms for three, 40 ms
  from four on); up to eight followers draw, each 5% smaller, centred so none
  shows outside the lead at rest. The selection ring is off while dragging. The
  column opens one card's room for the whole group, which lands together,
  keeping the stacking order it flew in until the last card is down.
- A fanned card is a loose body lying on the fan's spiral, not one spring to a
  fixed slot (measured from the reference clips,
  docs/reference/tima/motion/analysis.md): firm across the spiral, like
  gravity onto a floor; loose along it, so cards slide round and turn as they
  go; neighbours pressed together shove apart; a card dropped into an open fan
  rakes the others in toward the centre and they spring back out (about 10%
  past, at rest in about 0.55 s). Opening, the fan swells out of the pile as one
  blob: every card leaves at once (at most 5 ms apart) and goes straight from
  the pile to its place, no sweep round the centre; only there does it lie
  loose on the spiral. Each card strays a little from its exact place (±4° turn, ±4 pt, ±4° round,
  fixed by its id, `FAN_JITTER`); out of the stack each card starts up to 60 ms
  late, on its own spring (±28%), launched up to 9° off and ±20% faster, and the
  blob leans to one side (`FAN_SPILL_VARY`). Nothing fades: hidden cards are
  whole from the first frame under the pile; folding, they are gathered back
  under it the way they spilled out (about 0.3 s, a hint of settle, no
  bounce): each card on its own spring off `FAN_FOLD`, up to 60 ms late, with
  a quarter of the spill's uneven push (`gather_spring`, `gather_launch`),
  and only then leave the render tree.
- Scrolling down over a stack fans it out, scrolling up folds it, and the blob
  follows the fingers: 140 pt of scroll for all the way, each card on the
  straight way from its pile place to its fan place (springs on top, so it
  stays soft); a fling (40 pt in one event) goes all the way at once; left
  part open for 0.25 s it opens or folds, whichever is nearer. The column
  never scrolls under a stack. Any number of stacks can stand fanned. A click on a card, in a stack or a fan, opens it.
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

## Columns

Each column's title and count sit in a small glass chip: the cards' backdrop
(4 pt blur, saturation 1.5) under a light tint of the column's colour, 10 pt at
the ends, the count 6 pt after the title and a little dimmer. The colours:
«Бэклог» grey, «В работе» yellow, «Готово» green; the grey card is denser than
the colours (85%), or its pale tint over other cards reads as a white halo round
the card in front; the drop highlight over a
column is a faint wash of the same colour.

## Light and dark

The app follows the system appearance live (`viewDidChangeEffectiveAppearance`
on the root view): the board, chips, dividers and the panel's body use system
colours; the cards switch palette. In dark, each hue is a deep tint of itself
(denser, +0.2 alpha) with a pale text of the same hue, and the white inner edge
is fainter (20% instead of 61%) so it does not read as a frame. Card glyphs are
cached per appearance; the tracked title and time are rebuilt in the new ink
(their colour lives in an attributed string). Changed fills ease over
(`motion::soften`). The light palette's first six are the original ones,
unchanged; a task with no colour of its own takes one of those six, picked by a
mixed hash of its id (a UUIDv7's low tail bits are a counter, so taken plainly
most new tasks came out the same grey). `YATT_DEMO_FLIP=1` captures a board made
in dark, turned light and back.
`YATT_APPEARANCE=dark|light` pins one for the demo's captures.

## The task panel

The notes editor fills the body between the header's rule and the footer's,
edge to edge; the 24 pt margins round the notes are the page's own padding,
inside what scrolls, so the scroller sits at the panel's edge, the text lines up
with the title and scrolls away under the rules. Opening puts the caret at the end
without scrolling the notes.

No Done or Delete buttons: the panel closes by Esc or a click outside, as
before. A palette button at the header's right (its icon a bitmap like the
cards' glyphs, in the task's ink, aspect kept: the button's own image layer
stretched) opens a native popover
(transient, closes on a click outside): the card colours as swatches, the
status (the three columns, a segmented control), a searchable grid of icons
(48 SF Symbols, eight a row) and «Удалить задачу». Colour and
icon are stored on the task (`color`, `icon`, migration 2), undoable, and every
card follows them at once. The time entries are one quiet line (the latest and
how many); a click opens a menu with «Скопировать всё» and every entry, each
copied exactly as written («25.10 15:32 – 15:36 · 0:04:10»). A menu scrolls by
itself, however many entries there are.

## Stacks in the column

A stack, and its fan, centre in the column between the header and the
new-task capsule. A fan may stick out 36 pt past its column on each side before
it shrinks; the pile's layers step down 3.5% a layer.

## Menu bar

A status item (`menubar.rs`; the mapping is `yatt_platform::menu_bar_look`,
tested): a pomodoro (work or break) shows its time left as a small pie, a
template image so it follows the menu bar's light or dark look, over a faint
disc for the time gone, no outline and no text. The stopwatch has no end, so
no pie: only its minutes as text («<1 мин», «12 мин», «1 ч 05 мин»), never
seconds, so it does not tick. Idle: a still whole disc, dimmed (a three-quarter pie read as a quarter already gone); paused
dims too. It is drawn again only when a degree of the pie, the minutes or the
state change. Its menu: the timer to the second at the top, then the tasks it
counts for (or, with nothing counted, those «В работе»), each with its colour
and time; a click on one brings yatt forward and opens that task's panel
(morphing from its card, or from the middle when the card is out of sight).
Then open yatt, pause or resume, stop.

## Pomodoro lengths

Hold the 🍅 in the toolbar (0.4 s) for a popover: work and break minutes with
steppers (5 min a step) and the presets 25/5, 40/10, 50/15. They are kept with
the timer (stored), and a running interval takes the new length at once,
keeping its time so far, so nothing recorded is lost; a plain click still
starts or pauses. Long breaks keep their own length.

## Timer-only mode

Off for now (`MINIMAL_MODE`): its toolbar button and menu item are gone, the
code stays.

## Motion safety

A fan card's way round its centre is always measured the short way (never
counted up: a path through the centre would add whole turns, and the card
whirled round the board). No card moves faster than 6000 pt/s, and a
non-number lands the card on its place. A fuzz test drives eight cards with
random targets, fans, kicks and 2 s frames.

## Card ↔ panel

The panel grows out of the card and back on a critically damped spring, the
way a card comes to rest: no give, no jelly (`physics::MORPH`).
