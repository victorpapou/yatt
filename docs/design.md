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

A flat tint of the card's colour at about 40% over a blur of what is behind it,
a plain 1 px light rim (white, 60%) with continuous corners, and a soft white glow
over a faint dark shadow. No gradients.
