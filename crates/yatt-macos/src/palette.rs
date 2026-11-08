//! The new-task capsule's colour popover: the card colours as a grid and the
//! setting «перекрашивать карточку в цвет первого тега».

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
    recolor: Retained<NSButton>,
    _retained: Vec<Retained<AnyObject>>,
}

impl Palette {
    /// `on_pick` gets the chosen colour, `on_recolor` the setting's new state.
    pub fn new(
        mtm: MainThreadMarker,
        on_pick: impl Fn(usize) + 'static,
        on_recolor: impl Fn(bool) + 'static,
    ) -> Self {
        let on_pick = Rc::new(on_pick);
        let mut retained: Vec<Retained<AnyObject>> = Vec::new();
        let rows = style::HUE_COUNT.div_ceil(PER_ROW) as f64;
        let width = PAD * 2.0 + CELL * PER_ROW as f64;
        let grid_h = rows * CELL;
        let height = PAD * 2.0 + grid_h + 34.0;
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
        let target = {
            let slot: Rc<Cell<Option<*const NSButton>>> = Rc::default();
            let read = slot.clone();
            let t = ActionTarget::new(mtm, move || {
                if let Some(p) = read.get() {
                    // SAFETY: the button is kept alive by the popover's view for as
                    // long as this target (retained next to it) can fire.
                    let on = unsafe { &*p }.state() == NSControlStateValueOn;
                    on_recolor(on);
                }
            });
            (t, slot)
        };
        let recolor = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("Цвет карточки по первому тегу"),
                Some(&target.0),
                Some(ActionTarget::sel()),
                mtm,
            )
        };
        target.1.set(Some(&*recolor as *const NSButton));
        recolor.setFrame(NSRect::new(
            NSPoint::new(PAD, PAD + grid_h + 6.0),
            NSSize::new(width - 2.0 * PAD, 22.0),
        ));
        view.addSubview(&recolor);
        retained.push(Retained::into_super(Retained::into_super(target.0)));

        let controller = NSViewController::new(mtm);
        controller.setView(&view);
        let popover = NSPopover::new(mtm);
        popover.setContentViewController(Some(&controller));
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setContentSize(NSSize::new(width, height));
        Self {
            popover,
            swatches,
            recolor,
            _retained: retained,
        }
    }

    /// Marks the chosen colour and the setting's state.
    pub fn show_state(&self, selected: usize, recolor: bool) {
        for (slot, b) in self.swatches.iter().enumerate() {
            let on = HUE_SWATCH_ORDER[slot] == selected;
            b.setImage(symbol(if on { "checkmark.circle.fill" } else { "circle.fill" }).as_deref());
        }
        self.recolor.setState(if recolor {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
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
