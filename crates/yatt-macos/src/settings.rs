//! The settings window: a native tab-view controller with a toolbar of tabs, as
//! the system's own settings windows have. Rows are a label on the left and the
//! control on the right (switches, pop-up buttons with colour swatches), and
//! everything that can be set in the interface can be set here too.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::{Retained, Weak as ObjcWeak};
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSColor, NSControlStateValueOff, NSControlStateValueOn, NSImage, NSImageSymbolConfiguration,
    NSNormalWindowLevel, NSPopUpButton, NSSwitch, NSTabViewController, NSTabViewControllerTabStyle,
    NSTabViewItem, NSTextAlignment, NSTextField, NSView, NSViewController, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSCopying, NSPoint, NSRect, NSSize, NSString, NSUserDefaults, ns_string,
};

use crate::board::HUE_NAMES;
use crate::classes::{ActionTarget, Fill, FillView, FlippedView};
use crate::style;

const COLOR_KEY: &str = "defaultTaskColor";
const OPEN_KEY: &str = "openTaskIn";
const RECOLOR_KEY: &str = "recolorByTag";
const GROUPING_KEY: &str = "grouping";
const STACKS_KEY: &str = "yatt.stacked";

/// A new card's colour unless one is picked: yellow (an index into the palette).
pub const DEFAULT_COLOR: usize = 8;

fn defaults() -> Retained<NSUserDefaults> {
    NSUserDefaults::standardUserDefaults()
}

/// The colour new tasks get.
pub fn default_color() -> usize {
    let d = defaults();
    let key = NSString::from_str(COLOR_KEY);
    if d.objectForKey(&key).is_some() {
        (d.integerForKey(&key).max(0) as usize).min(HUE_NAMES.len() - 1)
    } else {
        DEFAULT_COLOR
    }
}

pub fn set_default_color(c: usize) {
    defaults().setInteger_forKey(c as isize, &NSString::from_str(COLOR_KEY));
}

/// Whether a task opens in a window of its own instead of the inspector.
pub fn open_in_window() -> bool {
    // `YATT_OPEN=window|inspector`: whatever was saved (for the demo's captures).
    match std::env::var("YATT_OPEN").as_deref() {
        Ok("window") => return true,
        Ok("inspector") => return false,
        _ => {}
    }
    defaults().integerForKey(&NSString::from_str(OPEN_KEY)) == 1
}

pub fn set_open_in_window(on: bool) {
    defaults().setInteger_forKey(on as isize, &NSString::from_str(OPEN_KEY));
}

/// Whether cards take the colour of their first tag.
pub fn recolor_by_tag() -> bool {
    defaults().boolForKey(&NSString::from_str(RECOLOR_KEY))
}

pub fn set_recolor_by_tag(on: bool) {
    defaults().setBool_forKey(on, &NSString::from_str(RECOLOR_KEY));
}

/// How the board groups cards: 0 none, 1 by tag, 2 by colour.
pub fn grouping() -> usize {
    defaults().integerForKey(&NSString::from_str(GROUPING_KEY)).clamp(0, 2) as usize
}

pub fn set_grouping(g: usize) {
    defaults().setInteger_forKey(g as isize, &NSString::from_str(GROUPING_KEY));
}

/// Whether the columns are stacks instead of lists.
pub fn stacked() -> bool {
    defaults().boolForKey(&NSString::from_str(STACKS_KEY))
}

pub fn set_stacked(on: bool) {
    defaults().setBool_forKey(on, &NSString::from_str(STACKS_KEY));
}

/// A round swatch of a card colour, to stand beside its name.
pub fn swatch_image(hue: usize) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        ns_string!("circle.fill"),
        None,
    )?;
    let config = NSImageSymbolConfiguration::configurationWithPaletteColors(
        &NSArray::from_retained_slice(&[style::swatch(hue)]),
    );
    image.imageWithSymbolConfiguration(&config)
}

const W: f64 = 540.0;
const PAD: f64 = 24.0;
const ROW: f64 = 38.0;

fn label(mtm: MainThreadMarker, text: &str, y: f64) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setAlignment(NSTextAlignment::Left);
    l.setFrame(NSRect::new(NSPoint::new(PAD, y + 6.0), NSSize::new(300.0, 20.0)));
    l
}

/// A pop-up button on the right of a row, with `titles` (and a swatch each when
/// `swatches` is set), `selected` chosen.
fn popup(
    mtm: MainThreadMarker,
    y: f64,
    titles: &[String],
    swatches: bool,
    selected: usize,
) -> Retained<NSPopUpButton> {
    let w = 200.0;
    let p = unsafe {
        NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            NSRect::new(NSPoint::new(W - PAD - w, y + 2.0), NSSize::new(w, 26.0)),
            false,
        )
    };
    let items: Vec<Retained<NSString>> = titles.iter().map(|t| NSString::from_str(t)).collect();
    p.addItemsWithTitles(&NSArray::from_retained_slice(&items));
    if swatches {
        for i in 0..titles.len() {
            if let Some(item) = p.itemAtIndex(i as isize) {
                item.setImage(swatch_image(i).as_deref());
            }
        }
    }
    p.selectItemAtIndex(selected as isize);
    p
}

/// A switch on the right of a row.
fn switch(mtm: MainThreadMarker, y: f64, on: bool) -> Retained<NSSwitch> {
    let s = NSSwitch::new(mtm);
    s.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
    s.setFrame(NSRect::new(NSPoint::new(W - PAD - 38.0, y + 5.0), NSSize::new(38.0, 22.0)));
    s
}

fn on_pick(
    mtm: MainThreadMarker,
    p: &Retained<NSPopUpButton>,
    retained: &mut Vec<Retained<AnyObject>>,
    f: impl Fn(usize) + 'static,
) {
    let weak = ObjcWeak::from_retained(p);
    let t = ActionTarget::new(mtm, move || {
        if let Some(p) = weak.load() {
            f(p.indexOfSelectedItem().max(0) as usize);
        }
    });
    unsafe {
        p.setTarget(Some(&t));
        p.setAction(Some(ActionTarget::sel()));
    }
    retained.push(Retained::into_super(Retained::into_super(t)));
}

fn on_switch(
    mtm: MainThreadMarker,
    s: &Retained<NSSwitch>,
    retained: &mut Vec<Retained<AnyObject>>,
    f: impl Fn(bool) + 'static,
) {
    let weak = ObjcWeak::from_retained(s);
    let t = ActionTarget::new(mtm, move || {
        if let Some(s) = weak.load() {
            f(s.state() == NSControlStateValueOn);
        }
    });
    unsafe {
        s.setTarget(Some(&t));
        s.setAction(Some(ActionTarget::sel()));
    }
    retained.push(Retained::into_super(Retained::into_super(t)));
}

/// A tab: a pane view of `height` points, titled, with a symbol in the toolbar.
fn tab(
    mtm: MainThreadMarker,
    title: &str,
    symbol: &str,
    view: &Retained<FlippedView>,
    height: f64,
    retained: &mut Vec<Retained<AnyObject>>,
) -> Retained<NSTabViewItem> {
    let vc = NSViewController::new(mtm);
    vc.setView(view);
    vc.setTitle(Some(&NSString::from_str(title)));
    vc.setPreferredContentSize(NSSize::new(W, height));
    let item = NSTabViewItem::tabViewItemWithViewController(&vc);
    item.setLabel(&NSString::from_str(title));
    item.setImage(
        NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol),
            Some(&NSString::from_str(title)),
        )
        .as_deref(),
    );
    retained.push(Retained::<AnyObject>::from(Retained::into_super(Retained::into_super(vc))));
    item
}

/// The live demo beside the first-tag switch: a small card that eases from the
/// default colour to its tag's colour and back.
struct DemoCard {
    card: Retained<FlippedView>,
}

impl DemoCard {
    fn new(mtm: MainThreadMarker, y: f64) -> Self {
        let card = FlippedView::new(mtm, NSRect::new(NSPoint::new(PAD, y), NSSize::new(170.0, 66.0)));
        card.setWantsLayer(true);
        if let Some(layer) = card.layer() {
            layer.setCornerRadius(14.0);
        }
        let title = NSTextField::labelWithString(ns_string!("Задача"), mtm);
        title.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize_weight(13.0, unsafe {
            objc2_app_kit::NSFontWeightSemibold
        })));
        title.setFrame(NSRect::new(NSPoint::new(14.0, 10.0), NSSize::new(130.0, 18.0)));
        card.addSubview(&title);
        let tag = FlippedView::new(mtm, NSRect::new(NSPoint::new(14.0, 36.0), NSSize::new(64.0, 18.0)));
        tag.setWantsLayer(true);
        if let Some(layer) = tag.layer() {
            layer.setCornerRadius(9.0);
            layer.setBackgroundColor(Some(
                &style::swatch(style::tag_hue("дизайн")).colorWithAlphaComponent(0.55).CGColor(),
            ));
        }
        let tag_text = NSTextField::labelWithString(ns_string!("дизайн"), mtm);
        tag_text.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize(11.0)));
        tag_text.setAlignment(NSTextAlignment::Center);
        tag_text.setFrame(NSRect::new(NSPoint::new(0.0, 1.0), NSSize::new(64.0, 16.0)));
        tag.addSubview(&tag_text);
        card.addSubview(&tag);
        let this = Self { card };
        this.paint(false, false);
        this
    }

    fn paint(&self, by_tag: bool, eased: bool) {
        let hue = if by_tag {
            style::tag_hue("дизайн")
        } else {
            default_color()
        };
        if eased {
            crate::motion::soften(&self.card);
        }
        if let Some(layer) = self.card.layer() {
            layer.setBackgroundColor(Some(&style::swatch(hue).colorWithAlphaComponent(0.7).CGColor()));
        }
    }

    /// Eases to the card's colour with or without the tag's.
    fn show(&self, by_tag: bool) {
        self.paint(by_tag, true);
    }
}

/// The settings window and the controls that must follow changes made elsewhere.
pub struct SettingsWindow {
    window: Retained<NSWindow>,
    refresh: Rc<RefCell<Vec<Box<dyn Fn()>>>>,
    _retained: Vec<Retained<AnyObject>>,
}

impl SettingsWindow {
    /// `changed` is called after any setting changed (the board reads them again).
    pub fn new(mtm: MainThreadMarker, changed: Rc<dyn Fn()>) -> Self {
        let mut retained: Vec<Retained<AnyObject>> = Vec::new();
        let refresh: Rc<RefCell<Vec<Box<dyn Fn()>>>> = Rc::default();

        // ---- Задачи ----
        let tasks = FlippedView::new(mtm, NSRect::new(NSPoint::ZERO, NSSize::new(W, 250.0)));
        tasks.addSubview(&label(mtm, "Цвет новой задачи", 16.0));
        let names: Vec<String> = HUE_NAMES.iter().map(|n| n.to_string()).collect();
        let colors = popup(mtm, 16.0, &names, true, default_color());
        let demo = Rc::new(DemoCard::new(mtm, 136.0));
        {
            let (changed, demo) = (changed.clone(), demo.clone());
            on_pick(mtm, &colors, &mut retained, move |i| {
                set_default_color(i);
                demo.show(recolor_by_tag());
                changed();
            });
        }
        tasks.addSubview(&colors);
        tasks.addSubview(&label(mtm, "Открывать задачу", 16.0 + ROW));
        let open = popup(
            mtm,
            16.0 + ROW,
            &["В инспекторе".to_string(), "В отдельном окне".to_string()],
            false,
            open_in_window() as usize,
        );
        {
            let changed = changed.clone();
            on_pick(mtm, &open, &mut retained, move |i| {
                set_open_in_window(i == 1);
                changed();
            });
        }
        tasks.addSubview(&open);
        tasks.addSubview(&label(mtm, "Красить карточку в цвет первого тега", 16.0 + 2.0 * ROW));
        let recolor = switch(mtm, 16.0 + 2.0 * ROW, recolor_by_tag());
        {
            let (changed, demo) = (changed.clone(), demo.clone());
            on_switch(mtm, &recolor, &mut retained, move |on| {
                set_recolor_by_tag(on);
                demo.show(on);
                changed();
            });
        }
        tasks.addSubview(&recolor);
        tasks.addSubview(&demo.card);
        demo.show(recolor_by_tag());
        {
            let (colors, open, recolor, demo) =
                (colors.clone(), open.clone(), recolor.clone(), demo.clone());
            refresh.borrow_mut().push(Box::new(move || {
                colors.selectItemAtIndex(default_color() as isize);
                open.selectItemAtIndex(open_in_window() as isize);
                recolor.setState(if recolor_by_tag() {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
                demo.show(recolor_by_tag());
            }));
        }
        let tasks_tab = tab(mtm, "Задачи", "checklist", &tasks, 250.0, &mut retained);

        // ---- Вид ----
        let view = FlippedView::new(mtm, NSRect::new(NSPoint::ZERO, NSSize::new(W, 130.0)));
        view.addSubview(&label(mtm, "Группировка карточек", 16.0));
        let grouping_pop = popup(
            mtm,
            16.0,
            &["Без группировки".to_string(), "По тегу".to_string(), "По цвету".to_string()],
            false,
            grouping(),
        );
        {
            let changed = changed.clone();
            on_pick(mtm, &grouping_pop, &mut retained, move |i| {
                set_grouping(i);
                changed();
            });
        }
        view.addSubview(&grouping_pop);
        view.addSubview(&label(mtm, "Колонки стопками", 16.0 + ROW));
        let stacks = switch(mtm, 16.0 + ROW, stacked());
        {
            let changed = changed.clone();
            on_switch(mtm, &stacks, &mut retained, move |on| {
                set_stacked(on);
                changed();
            });
        }
        view.addSubview(&stacks);
        {
            let (grouping_pop, stacks) = (grouping_pop.clone(), stacks.clone());
            refresh.borrow_mut().push(Box::new(move || {
                grouping_pop.selectItemAtIndex(grouping() as isize);
                stacks.setState(if stacked() {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
            }));
        }
        let view_tab = tab(mtm, "Вид", "rectangle.3.group", &view, 130.0, &mut retained);

        let tabs = NSTabViewController::new(mtm);
        tabs.setTabStyle(NSTabViewControllerTabStyle::Toolbar);
        tabs.addTabViewItem(&tasks_tab);
        tabs.addTabViewItem(&view_tab);
        let window = NSWindow::windowWithContentViewController(&tabs);
        window.setTitle(ns_string!("Настройки"));
        window.setStyleMask(NSWindowStyleMask::Titled | NSWindowStyleMask::Closable);
        unsafe { window.setReleasedWhenClosed(false) };
        window.setLevel(NSNormalWindowLevel);
        window.center();
        retained.push(Retained::<AnyObject>::from(Retained::into_super(Retained::into_super(tabs))));
        retained.push(Retained::<AnyObject>::from(Retained::into_super(tasks_tab)));
        retained.push(Retained::<AnyObject>::from(Retained::into_super(view_tab)));
        let _ = (NSColor::clearColor(), NSView::new(mtm), ns_string!("").copy());
        Self {
            window,
            refresh,
            _retained: retained,
        }
    }

    pub fn show(&self) {
        // The toolbar and the board may have changed things since it was last open.
        for f in self.refresh.borrow().iter() {
            f();
        }
        self.window.makeKeyAndOrderFront(None);
    }

    /// Development aid: saves the settings window as a PNG.
    pub fn capture_png(&self, path: &str) -> bool {
        let Some(content) = self.window.contentView() else {
            return false;
        };
        let view = unsafe { content.superview() }.unwrap_or(content);
        let bounds = view.bounds();
        let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(bounds) else {
            return false;
        };
        view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        let props = objc2_foundation::NSDictionary::new();
        let Some(data) = (unsafe {
            rep.representationUsingType_properties(objc2_app_kit::NSBitmapImageFileType::PNG, &props)
        }) else {
            return false;
        };
        data.writeToFile_atomically(&NSString::from_str(path), true)
    }
}
