# Design notes

## Seamlessness (бесшовность)

The guiding rule for every transition: the same element morphs, keeping its
content and colour. Nothing is cross-faded or swapped for a separate view.

- New task: the new-task capsule, typed text and all, grows into the card on its
  way to the column; the text becomes the card's title.
- Card to modal: the card grows into the task panel in its own colour and
  material, and shrinks back into the card on close.
- Stack to fan: the stacked cards themselves spread out and fold back.
- One card to a group drag: the pressed card leads and the selected cards trail
  it as they are.

## Card material

A flat tint of the card's colour at 55% (white cards 65%) over a 4 pt blur of
what is behind it with a slight saturation boost (CSS `backdrop-filter: blur(4px)
saturate(1.08)`), so text behind is only a soft patch; then a plain 1 px light rim
(white, 60%) with continuous corners and a thin soft white glow. No gradients, and
no dark shadow: on overlapping cards it would add up into a smudge.

## Nothing jumps

Seamlessness goes down to single properties: shadows, opacity, colour, borders,
hover, selection rings, chips, the column highlight and view-mode switches all
change over at least 100 ms. Motion uses springs by default, critically damped
with no bounce (`physics.rs`). Where a spring makes no sense (a colour, a ring, a
shadow, a label's text) the change eases over 150 ms through `motion::soften`,
because AppKit turns off implicit animations on a view's layer and a plain
setter would jump.

## No artefacts

Seamless also means nothing flickers: no flash or fade when a card moves to
another column or onto the drag layer (views move between parents with Core
Animation's implicit actions off, `motion::quietly`), no shadows piling up where
cards overlap, and no part of a card drawing outside its shape when it scales
(the blur and the tint are sized by hand; only the text scales).
