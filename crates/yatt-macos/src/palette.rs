//! The new-task capsule's colour: a native menu of round swatches with the
//! colours' names, a tick on the current one.

use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSControlStateValueOff, NSControlStateValueOn, NSMenu, NSMenuItem, NSView,
};
use objc2_foundation::{NSPoint, NSString, ns_string};

use crate::board::HUE_NAMES;
use crate::classes::ActionTarget;
use crate::picker::HUE_SWATCH_ORDER;
use crate::settings::swatch_image;

pub struct Palette {
    menu: Retained<NSMenu>,
    /// Menu items in `HUE_SWATCH_ORDER`.
    items: Vec<Retained<NSMenuItem>>,
    _retained: Vec<Retained<AnyObject>>,
}

impl Palette {
    /// `on_pick` gets the chosen colour.
    pub fn new(mtm: MainThreadMarker, on_pick: impl Fn(usize) + 'static) -> Self {
        let on_pick = Rc::new(on_pick);
        let menu = NSMenu::new(mtm);
        let mut items = Vec::new();
        let mut retained: Vec<Retained<AnyObject>> = Vec::new();
        for hue in HUE_SWATCH_ORDER {
            let pick = on_pick.clone();
            let target = ActionTarget::new(mtm, move || pick(hue));
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(HUE_NAMES[hue]),
                    Some(ActionTarget::sel()),
                    ns_string!(""),
                )
            };
            unsafe { item.setTarget(Some(&target)) };
            item.setImage(swatch_image(hue).as_deref());
            menu.addItem(&item);
            items.push(item);
            retained.push(Retained::into_super(Retained::into_super(target)));
        }
        Self {
            menu,
            items,
            _retained: retained,
        }
    }

    /// Ticks the chosen colour.
    pub fn show_state(&self, selected: usize) {
        for (slot, item) in self.items.iter().enumerate() {
            item.setState(if HUE_SWATCH_ORDER[slot] == selected {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
    }

    /// Opens the menu under `anchor` (blocks until it closes).
    pub fn toggle(&self, anchor: &NSView) {
        let at = NSPoint::new(0.0, anchor.bounds().size.height + 4.0);
        self.menu
            .popUpMenuPositioningItem_atLocation_inView(None, at, Some(anchor));
    }
}
