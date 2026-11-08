//! The new-task capsule's colour popover: the card colours as a grid.

use std::cell::Cell;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSButton, NSControlStateValueOff, NSControlStateValueOn, NSImage, NSPopover, NSPopoverBehavior,
    NSView, NSViewController,
};
use objc2_foundation::{NSPoint, NSRect, NSRectEdge, NSSize, NSString, ns_string};

use crate::classes::{ActionTarget, FlippedView};
use crate::picker::HUE_SWATCH_ORDER;
use crate::style;

const CELL: f64 = 30.0;
const PER_ROW: usize = 8;
const PAD: f64 = 14.0;

fn symbol(name: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)
}

pub struct Palette {
    popover: Retained<NSPopover>,
    swatches: Vec<Retained<NSButton>>,
    _retained: Vec<Retained<AnyObject>>,
}

impl Palette {
    /// `on_pick` gets the chosen colour, `on_recolor` the setting's new state.
    pub fn new(
        mtm: MainThreadMarker,
        on_pick: impl Fn(usize) + 'static,
    ) -> Self {
        let on_pick = Rc::new(on_pick);
        let mut retained: Vec<Retained<AnyObject>> = Vec::new();
        let rows = style::HUE_COUNT.div_ceil(PER_ROW) as f64;
        let width = PAD * 2.0 + CELL * PER_ROW as f64;
        let grid_h = rows * CELL;
        let height = PAD * 2.0 + grid_h;
        let view = FlippedView::new(mtm, NSRect::new(NSPoint::ZERO, NSSize::new(width, height)));
        let mut swatches = Vec::new();
        for (slot, hue) in HUE_SWATCH_ORDER.iter().copied().enumerate() {
            let pick = on_pick.clone();
            let target = ActionTarget::new(mtm, move || pick(hue));
            let b = unsafe {
                NSButton::buttonWithImage_target_action(
                    &symbol("circle.fill").expect("symbol"),
                    Some(&target),
                    Some(ActionTarget::sel()),
                    mtm,
                )
            };
            b.setBordered(false);
            b.setFrame(NSRect::new(
                NSPoint::new(
                    PAD + (slot % PER_ROW) as f64 * CELL,
                    PAD + (slot / PER_ROW) as f64 * CELL,
                ),
                NSSize::new(CELL, CELL),
            ));
            b.setContentTintColor(Some(&style::swatch(hue)));
            b.setToolTip(Some(ns_string!("Цвет новой задачи")));
            view.addSubview(&b);
            swatches.push(b);
            retained.push(Retained::into_super(Retained::into_super(target)));
        }
        let controller = NSViewController::new(mtm);
        controller.setView(&view);
        let popover = NSPopover::new(mtm);
        popover.setContentViewController(Some(&controller));
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setContentSize(NSSize::new(width, height));
        Self {
            popover,
            swatches,
            _retained: retained,
        }
    }

    /// Marks the chosen colour and the setting's state.
    pub fn show_state(&self, selected: usize) {
        for (slot, b) in self.swatches.iter().enumerate() {
            let on = HUE_SWATCH_ORDER[slot] == selected;
            b.setImage(symbol(if on { "checkmark.circle.fill" } else { "circle.fill" }).as_deref());
        }
    }

    pub fn toggle(&self, anchor: &NSView) {
        if self.popover.isShown() {
            self.popover.close();
            return;
        }
        self.popover
            .showRelativeToRect_ofView_preferredEdge(anchor.bounds(), anchor, NSRectEdge::MaxY);
    }
}
