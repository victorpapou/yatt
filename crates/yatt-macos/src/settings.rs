//! The settings window: a native tab-view controller with a toolbar of tabs, as
//! the system's own settings windows have. Rows are a label on the left and the
//! control on the right (switches, pop-up buttons with colour swatches), and
//! everything that can be set in the interface can be set here too.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::{Retained, Weak as ObjcWeak};
use objc2::runtime::ProtocolObject;
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
const TAG_COLORS_KEY: &str = "tagColors";

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

/// The colours picked for tags (name to palette index).
pub fn tag_colors() -> std::collections::HashMap<String, usize> {
    let mut out = std::collections::HashMap::new();
    if let Some(dict) = defaults().dictionaryForKey(&NSString::from_str(TAG_COLORS_KEY)) {
        for key in dict.allKeys().iter() {
            if let Some(v) = dict.objectForKey(&key)
                && let Ok(n) = v.downcast::<objc2_foundation::NSNumber>()
            {
                out.insert(key.to_string(), n.integerValue().max(0) as usize);
            }
        }
    }
    out
}

/// Picks a colour for a tag (`None`: back to the one from its name).
pub fn set_tag_color(name: &str, hue: Option<usize>) {
    let mut map = tag_colors();
    match hue {
        Some(h) => {
            map.insert(name.to_string(), h);
        }
        None => {
            map.remove(name);
        }
    }
    let dict = objc2_foundation::NSMutableDictionary::<NSString, objc2_foundation::NSNumber>::new();
    for (k, v) in &map {
        unsafe {
            dict.setObject_forKey(
                &objc2_foundation::NSNumber::new_isize(*v as isize),
                ProtocolObject::from_ref(&*NSString::from_str(k)),
            );
        }
    }
    unsafe {
        defaults().setObject_forKey(Some(&dict), &NSString::from_str(TAG_COLORS_KEY));
    }
    style::set_tag_colors(map);
}

/// A round swatch of a card colour, to stand beside its name.
pub fn swatch_image_sized(hue: usize, size: f64) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        ns_string!("circle.fill"),
        None,
    )?;
    let config = NSImageSymbolConfiguration::configurationWithPaletteColors(
        &NSArray::from_retained_slice(&[style::swatch(hue)]),
    )
    .configurationByApplyingConfiguration(&NSImageSymbolConfiguration::configurationWithPointSize_weight_scale(
        size,
        0.0,
        objc2_app_kit::NSImageSymbolScale::Medium,
    ));
    image.imageWithSymbolConfiguration(&config)
}

/// A round swatch of a colour as a plain image (not a template: it keeps its colour),
/// a plain filled disc, no outline.
pub fn circle_image(hue: usize, size: f64) -> Retained<NSImage> {
    pie_image(&[hue], size)
}

/// The colours as equal sectors of one circle (one colour: a plain disc).
pub fn pie_image(hues: &[usize], size: f64) -> Retained<NSImage> {
    let colors: Vec<Retained<NSColor>> = hues.iter().map(|h| style::swatch(*h)).collect();
    let block = block2::RcBlock::new(move |rect: NSRect| {
        let path = |r: NSRect| objc2_app_kit::NSBezierPath::bezierPathWithOvalInRect(r);
        let inner = NSRect::new(
            NSPoint::new(rect.origin.x + 0.5, rect.origin.y + 0.5),
            NSSize::new(rect.size.width - 1.0, rect.size.height - 1.0),
        );
        if colors.len() <= 1 {
            if let Some(c) = colors.first() {
                c.setFill();
            }
            path(inner).fill();
        } else {
            let (cx, cy) = (rect.size.width / 2.0, rect.size.height / 2.0);
            let r = inner.size.width / 2.0;
            let n = colors.len() as f64;
            for (i, c) in colors.iter().enumerate() {
                let (a0, a1) = (90.0 - 360.0 * i as f64 / n, 90.0 - 360.0 * (i as f64 + 1.0) / n);
                let p = objc2_app_kit::NSBezierPath::bezierPath();
                p.moveToPoint(NSPoint::new(cx, cy));
                p.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise(
                    NSPoint::new(cx, cy),
                    r,
                    a0,
                    a1,
                    true,
                );
                p.closePath();
                c.setFill();
                p.fill();
            }
        }
        objc2::runtime::Bool::YES
    });
    NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(size, size), false, &block)
}

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

const W: f64 = 560.0;
/// The grouped form's geometry, on the system's grid: groups 20 pt in from the
/// window's edges, rows 44 pt, controls 16 pt in from a group's edge.
const MARGIN: f64 = 20.0;
const BOX_W: f64 = W - 2.0 * MARGIN;
const INNER: f64 = 16.0;
const ROW_H: f64 = 44.0;
const SUB_ROW_H: f64 = 56.0;
const GAP: f64 = 20.0;

/// One row of a group: its height and the views, framed within the row.
struct Row {
    h: f64,
    views: Vec<Retained<NSView>>,
}

fn text_label(
    mtm: MainThreadMarker,
    text: &str,
    secondary: bool,
    frame: NSRect,
) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setAlignment(NSTextAlignment::Left);
    if secondary {
        l.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize(11.0)));
        l.setTextColor(Some(&NSColor::secondaryLabelColor()));
    }
    l.setFrame(frame);
    l
}

/// A row with `title` (and a small grey `subtitle` under it) on the left and
/// `control` on the right, vertically centred.
fn control_row(
    mtm: MainThreadMarker,
    title: &str,
    subtitle: Option<&str>,
    control: &NSView,
) -> Row {
    let h = if subtitle.is_some() { SUB_ROW_H } else { ROW_H };
    let cw = control.frame().size.width;
    let ch = control.frame().size.height;
    control.setFrame(NSRect::new(
        NSPoint::new(BOX_W - INNER - cw, ((h - ch) / 2.0).round()),
        NSSize::new(cw, ch),
    ));
    let text_w = BOX_W - 3.0 * INNER - cw;
    let mut views: Vec<Retained<NSView>> = Vec::new();
    match subtitle {
        Some(sub) => {
            views.push(Retained::into_super(Retained::into_super(text_label(
                mtm,
                title,
                false,
                NSRect::new(NSPoint::new(INNER, 10.0), NSSize::new(text_w, 18.0)),
            ))));
            views.push(Retained::into_super(Retained::into_super(text_label(
                mtm,
                sub,
                true,
                NSRect::new(NSPoint::new(INNER, 29.0), NSSize::new(text_w, 15.0)),
            ))));
        }
        None => views.push(Retained::into_super(Retained::into_super(text_label(
            mtm,
            title,
            false,
            NSRect::new(NSPoint::new(INNER, (h - 18.0) / 2.0), NSSize::new(text_w, 18.0)),
        )))),
    }
    views.push(objc2::Message::retain(control));
    Row { h, views }
}

/// Stacks `rows` in a rounded group at `top`, hairlines between them; adds it to
/// `parent` and returns where the next group may start.
fn group(mtm: MainThreadMarker, parent: &NSView, top: f64, rows: Vec<Row>) -> f64 {
    let total: f64 = rows.iter().map(|r| r.h).sum();
    let frame = NSRect::new(NSPoint::new(MARGIN, top), NSSize::new(BOX_W, total));
    let boxv = objc2_app_kit::NSBox::new(mtm);
    boxv.setFrame(frame);
    boxv.setBoxType(objc2_app_kit::NSBoxType::Custom);
    boxv.setTitlePosition(objc2_app_kit::NSTitlePosition::NoTitle);
    boxv.setBorderWidth(0.0);
    boxv.setCornerRadius(18.0);
    boxv.setContentViewMargins(NSSize::ZERO);
    boxv.setFillColor(&NSColor::quaternarySystemFillColor());
    let content = FlippedView::new(mtm, NSRect::new(NSPoint::ZERO, frame.size));
    boxv.setContentView(Some(&content));
    let boxv = content;
    let mut y = 0.0;
    for (i, row) in rows.iter().enumerate() {
        for v in &row.views {
            let f = v.frame();
            v.setFrame(NSRect::new(
                NSPoint::new(f.origin.x, f.origin.y + y),
                f.size,
            ));
            boxv.addSubview(v);
        }
        y += row.h;
        if i + 1 < rows.len() {
            let line = FillView::new(mtm, Fill::Separator);
            line.setFrame(NSRect::new(
                NSPoint::new(INNER, y - 0.5),
                NSSize::new(BOX_W - INNER, 1.0),
            ));
            boxv.addSubview(&line);
        }
    }
    if let Some(b) = unsafe { boxv.superview() } {
        parent.addSubview(&b);
    }
    top + total + GAP
}

/// A pop-up button with `titles` (a swatch each when `swatches` is set),
/// `selected` chosen.
fn popup(
    mtm: MainThreadMarker,
    titles: &[String],
    swatches: bool,
    selected: usize,
) -> Retained<NSPopUpButton> {
    let p = unsafe {
        NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(190.0, 26.0)),
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

/// A switch for the right of a row.
fn switch(mtm: MainThreadMarker, on: bool) -> Retained<NSSwitch> {
    let s = NSSwitch::new(mtm);
    s.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
    s.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(38.0, 22.0)));
    s
}

fn as_view<T: objc2::Message + std::ops::Deref<Target = NSView>>(v: &Retained<T>) -> Retained<NSView> {
    let view: &NSView = v;
    objc2::Message::retain(view)
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

/// The live demo under the first-tag switch: a mini card of the board's own
/// glass, with a tag capsule, that eases from the default colour to its tag's
/// colour and back as the switch is flipped.
struct DemoCard {
    card: crate::board::CardPreview,
    glass: Retained<NSView>,
}

impl DemoCard {
    fn new(mtm: MainThreadMarker) -> Self {
        let card = crate::board::CardPreview::new(mtm, "Сверстать доску", default_color());
        let glass = Retained::into_super(card.view.clone());
        Self { card, glass }
    }

    fn paint(&self, by_tag: bool, eased: bool) {
        let hue = if by_tag {
            style::tag_hue("дизайн")
        } else {
            default_color()
        };
        self.card.set_hue(hue, eased);
    }

    /// Eases to the card's colour with or without the tag's.
    fn show(&self, by_tag: bool) {
        self.paint(by_tag, true);
    }
}

/// The settings window and the controls that must follow changes made elsewhere.
pub struct SettingsWindow {
    window: Retained<NSWindow>,
    tabs: Retained<NSTabViewController>,
    timer_show: Rc<dyn Fn(&crate::model::TimerSnap)>,
    refresh: Rc<RefCell<Vec<Box<dyn Fn()>>>>,
    _retained: Vec<Retained<AnyObject>>,
}

impl SettingsWindow {
    /// `changed` is called after any setting changed (the board reads them again).
    pub fn new(
        mtm: MainThreadMarker,
        changed: Rc<dyn Fn()>,
        tags_provider: Rc<dyn Fn() -> Vec<String>>,
        sink: crate::window::Sink,
    ) -> Self {
        let mut retained: Vec<Retained<AnyObject>> = Vec::new();
        let refresh: Rc<RefCell<Vec<Box<dyn Fn()>>>> = Rc::default();

        // ---- Задачи ----
        let tasks = FlippedView::new(mtm, NSRect::new(NSPoint::ZERO, NSSize::new(W, 350.0)));
        let names: Vec<String> = crate::picker::HUE_SWATCH_ORDER
            .iter()
            .map(|h| HUE_NAMES[*h].to_string())
            .collect();
        let slot_of = |hue: usize| {
            crate::picker::HUE_SWATCH_ORDER
                .iter()
                .position(|h| *h == hue)
                .unwrap_or(0)
        };
        // The default colour: a button showing it, opening the same colour popover
        // as the capsule's (one component).
        let colors = unsafe {
            objc2_app_kit::NSButton::buttonWithTitle_target_action(ns_string!(""), None, None, mtm)
        };
        colors.setBezelStyle(objc2_app_kit::NSBezelStyle::Push);
        colors.setImagePosition(objc2_app_kit::NSCellImagePosition::ImageLeading);
        colors.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(190.0, 26.0)));
        fn show_color(b: &objc2_app_kit::NSButton, hue: usize) {
            b.setTitle(&NSString::from_str(crate::board::HUE_NAMES[hue % crate::board::HUE_NAMES.len()]));
            b.setImage(swatch_image(hue).as_deref());
        }
        show_color(&colors, default_color());
        let _ = &names;
        let open = popup(
            mtm,
            &["В инспекторе".to_string(), "В отдельном окне".to_string()],
            false,
            open_in_window() as usize,
        );
        let recolor = switch(mtm, recolor_by_tag());
        let demo = Rc::new(DemoCard::new(mtm));
        {
            let (changed, demo) = (changed.clone(), demo.clone());
            let palette = Rc::new(crate::palette::Palette::new(mtm, {
                let (changed, demo, colors) = (changed.clone(), demo.clone(), colors.clone());
                move |hue| {
                    set_default_color(hue);
                    show_color(&colors, hue);
                    demo.show(recolor_by_tag());
                    changed();
                }
            }));
            palette.show_state(default_color());
            let anchor = colors.clone();
            let target = ActionTarget::new(mtm, {
                let palette = palette.clone();
                move || {
                    palette.show_state(default_color());
                    palette.toggle(&anchor);
                }
            });
            unsafe {
                colors.setTarget(Some(&target));
                colors.setAction(Some(ActionTarget::sel()));
            }
            retained.push(Retained::into_super(Retained::into_super(target)));
        }
        {
            let changed = changed.clone();
            on_pick(mtm, &open, &mut retained, move |i| {
                set_open_in_window(i == 1);
                changed();
            });
        }
        {
            let (changed, demo) = (changed.clone(), demo.clone());
            on_switch(mtm, &recolor, &mut retained, move |on| {
                set_recolor_by_tag(on);
                demo.show(on);
                changed();
            });
        }
        // The preview row: the card, centred in the group.
        let preview_h = 141.0;
        let card_frame = demo.glass.frame();
        demo.glass.setFrame(NSRect::new(
            NSPoint::new(((BOX_W - card_frame.size.width) / 2.0).round(), 20.0),
            card_frame.size,
        ));
        let preview = Row {
            h: preview_h,
            views: vec![demo.glass.clone()],
        };
        let next = group(
            mtm,
            &tasks,
            20.0,
            vec![
                control_row(mtm, "Цвет новой задачи", None, &colors),
                control_row(mtm, "Открывать задачу", None, &open),
            ],
        );
        group(
            mtm,
            &tasks,
            next,
            vec![
                preview,
                control_row(
                    mtm,
                    "Красить карточку в цвет тега",
                    Some("Карточка берёт цвет своего первого тега"),
                    &recolor,
                ),
            ],
        );
        demo.show(recolor_by_tag());
        {
            let (colors, open, recolor, demo) =
                (colors.clone(), open.clone(), recolor.clone(), demo.clone());
            refresh.borrow_mut().push(Box::new(move || {
                show_color(&colors, default_color());
                open.selectItemAtIndex(open_in_window() as isize);
                recolor.setState(if recolor_by_tag() {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
                demo.show(recolor_by_tag());
            }));
        }
        let tasks_tab = tab(mtm, "Задачи", "checklist", &tasks, 350.0, &mut retained);

        // ---- Вид ----
        let view = FlippedView::new(mtm, NSRect::new(NSPoint::ZERO, NSSize::new(W, 150.0)));
        let grouping_pop = popup(
            mtm,
            &["Без группировки".to_string(), "По тегу".to_string(), "По цвету".to_string()],
            false,
            grouping(),
        );
        let stacks = switch(mtm, stacked());
        {
            let changed = changed.clone();
            on_pick(mtm, &grouping_pop, &mut retained, move |i| {
                set_grouping(i);
                changed();
            });
        }
        {
            let changed = changed.clone();
            on_switch(mtm, &stacks, &mut retained, move |on| {
                set_stacked(on);
                changed();
            });
        }
        group(
            mtm,
            &view,
            20.0,
            vec![
                control_row(mtm, "Группировка карточек", None, &grouping_pop),
                control_row(
                    mtm,
                    "Колонки стопками",
                    Some("Карточки колонки складываются в стопку"),
                    &stacks,
                ),
            ],
        );
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
        let view_tab = tab(mtm, "Вид", "rectangle.3.group", &view, 150.0, &mut retained);

        // ---- Теги ----
        let tags_pane = FlippedView::new(mtm, NSRect::new(NSPoint::ZERO, NSSize::new(W, 320.0)));
        let scroll = objc2_app_kit::NSScrollView::new(mtm);
        scroll.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(W, 320.0)));
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        let doc = FlippedView::new(mtm, NSRect::new(NSPoint::ZERO, NSSize::new(W, 320.0)));
        scroll.setDocumentView(Some(&doc));
        tags_pane.addSubview(&scroll);
        {
            let (doc, changed, provider) = (doc.clone(), changed.clone(), tags_provider.clone());
            let held: Rc<RefCell<Vec<Retained<AnyObject>>>> = Rc::default();
            let palettes: Rc<RefCell<Vec<Rc<crate::palette::Palette>>>> = Rc::default();
            refresh.borrow_mut().push(Box::new(move || {
                held.borrow_mut().clear();
                palettes.borrow_mut().clear();
                for v in doc.subviews().iter() {
                    v.removeFromSuperview();
                }
                let tags = provider();
                if tags.is_empty() {
                    let l = text_label(
                        mtm,
                        "Тегов пока нет: напишите #тег в заметке задачи",
                        true,
                        NSRect::new(NSPoint::new(MARGIN + INNER, 24.0), NSSize::new(BOX_W - 2.0 * INNER, 18.0)),
                    );
                    doc.addSubview(&l);
                    doc.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(W, 320.0)));
                    return;
                }
                let mut rows = Vec::new();
                for tag in &tags {
                    // The colour as a round button, the palette popover under it.
                    let btn = objc2_app_kit::NSButton::new(mtm);
                    btn.setBordered(true);
                    btn.setBezelStyle(objc2_app_kit::NSBezelStyle::Toolbar);
                    btn.setShowsBorderOnlyWhileMouseInside(true);
                    btn.setImagePosition(objc2_app_kit::NSCellImagePosition::ImageOnly);
                    btn.setImage(Some(&circle_image(style::tag_hue(tag), 18.0)));
                    btn.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(30.0, 26.0)));
                    let palette = {
                        let (name, changed, weak) =
                            (tag.clone(), changed.clone(), ObjcWeak::from_retained(&btn));
                        Rc::new(crate::palette::Palette::for_tag(mtm, move |hue| {
                            set_tag_color(&name, hue);
                            if let Some(b) = weak.load() {
                                b.setImage(Some(&circle_image(style::tag_hue(&name), 18.0)));
                            }
                            changed();
                        }))
                    };
                    let target = {
                        let (palette, weak) = (palette.clone(), ObjcWeak::from_retained(&btn));
                        ActionTarget::new(mtm, move || {
                            if let Some(b) = weak.load() {
                                palette.toggle(&b);
                            }
                        })
                    };
                    unsafe {
                        btn.setTarget(Some(&target));
                        btn.setAction(Some(ActionTarget::sel()));
                    }
                    held.borrow_mut().push(Retained::into_super(Retained::into_super(target)));
                    palettes.borrow_mut().push(palette);
                    let p = btn;
                    // The tag as its capsule, in its colour now, on the left.
                    let mut row = control_row(mtm, "", None, &p);
                    row.views.remove(0);
                    let cap = crate::tags::badge(mtm, tag);
                    cap.setFrame(NSRect::new(
                        NSPoint::new(INNER, (ROW_H - 20.0) / 2.0),
                        cap.frame().size,
                    ));
                    row.views.insert(0, Retained::into_super(cap));
                    rows.push(row);
                }
                let bottom = group(mtm, &doc, 20.0, rows);
                doc.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(W, bottom.max(320.0))));
            }));
        }
        let tags_tab = tab(mtm, "Теги", "tag", &tags_pane, 320.0, &mut retained);

        // ---- Помидор ----
        let timer_pane = FlippedView::new(mtm, NSRect::new(NSPoint::ZERO, NSSize::new(W, 340.0)));
        let state: Rc<std::cell::Cell<(u32, u32, u32, u32, bool, bool)>> =
            Rc::new(std::cell::Cell::new((25, 5, 15, 4, true, false)));
        let send = {
            let (sink, state) = (sink.clone(), state.clone());
            Rc::new(move |p: (u32, u32, u32, u32, bool, bool)| {
                let old = state.get();
                state.set(p);
                if (p.0, p.1) != (old.0, old.1) {
                    sink(crate::model::UiEvent::SetDurations {
                        work_min: p.0,
                        break_min: p.1,
                    });
                }
                if (p.2, p.3, p.4, p.5) != (old.2, old.3, old.4, old.5) {
                    sink(crate::model::UiEvent::SetCycle {
                        long_min: p.2,
                        every: p.3,
                        auto_breaks: p.4,
                        auto_work: p.5,
                    });
                }
            })
        };
        // A pop-up of minutes: the usual lengths plus the current one if unusual.
        fn minute_titles(opts: &[u32], cur: u32) -> (Vec<u32>, Vec<String>) {
            let mut v: Vec<u32> = opts.to_vec();
            if !v.contains(&cur) {
                v.push(cur);
                v.sort_unstable();
            }
            let t = v.iter().map(|m| format!("{m} мин")).collect();
            (v, t)
        }
        const LENGTHS: [(&str, &[u32]); 3] = [
            ("Работа", &[15, 20, 25, 30, 40, 45, 50, 60, 90]),
            ("Короткий перерыв", &[3, 5, 10, 15]),
            ("Длинный перерыв", &[10, 15, 20, 30]),
        ];
        let mut pops: Vec<(Retained<NSPopUpButton>, usize)> = Vec::new();
        let mut length_rows = Vec::new();
        for (field, (title, opts)) in LENGTHS.iter().enumerate() {
            let (_, titles) = minute_titles(opts, [25, 5, 15][field]);
            let p = popup(mtm, &titles, false, 0);
            let (send, state) = (send.clone(), state.clone());
            let slot = ObjcWeak::from_retained(&p);
            let t = ActionTarget::new(mtm, move || {
                let Some(p) = slot.load() else { return };
                let Some(item) = p.titleOfSelectedItem() else { return };
                let Some(minutes) = item
                    .to_string()
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse::<u32>().ok())
                else {
                    return;
                };
                let mut s = state.get();
                match field {
                    0 => s.0 = minutes,
                    1 => s.1 = minutes,
                    _ => s.2 = minutes,
                }
                send(s);
            });
            unsafe {
                p.setTarget(Some(&t));
                p.setAction(Some(ActionTarget::sel()));
            }
            retained.push(Retained::into_super(Retained::into_super(t)));
            length_rows.push(control_row(mtm, title, None, &p));
            pops.push((p, field));
        }
        let every = popup(
            mtm,
            &(2..=8).map(|n| format!("{n}-й")).collect::<Vec<_>>(),
            false,
            2,
        );
        {
            let (send, state) = (send.clone(), state.clone());
            on_pick(mtm, &every, &mut retained, move |i| {
                let mut s = state.get();
                s.3 = (i + 2) as u32;
                send(s);
            });
        }
        length_rows.push(control_row(
            mtm,
            "Длинный перерыв после каждой",
            Some("Через сколько рабочих отрезков"),
            &every,
        ));
        let auto_b = switch(mtm, true);
        let auto_w = switch(mtm, false);
        {
            let (send, state) = (send.clone(), state.clone());
            on_switch(mtm, &auto_b, &mut retained, move |on| {
                let mut s = state.get();
                s.4 = on;
                send(s);
            });
        }
        {
            let (send, state) = (send.clone(), state.clone());
            on_switch(mtm, &auto_w, &mut retained, move |on| {
                let mut s = state.get();
                s.5 = on;
                send(s);
            });
        }
        let next = group(mtm, &timer_pane, 20.0, length_rows);
        group(
            mtm,
            &timer_pane,
            next,
            vec![
                control_row(mtm, "Перерыв стартует сам", None, &auto_b),
                control_row(mtm, "Работа стартует сама", None, &auto_w),
            ],
        );
        let timer_tab = tab(mtm, "Помидор", "timer", &timer_pane, 340.0, &mut retained);
        // The timer's values now in force (from the snapshot), shown without sending.
        let timer_show: Rc<dyn Fn(&crate::model::TimerSnap)> = {
            let (state, every, auto_b, auto_w) =
                (state.clone(), every.clone(), auto_b.clone(), auto_w.clone());
            Rc::new(move |t: &crate::model::TimerSnap| {
                state.set((t.work_min, t.break_min, t.long_min, t.every, t.auto_breaks, t.auto_work));
                for (p, field) in &pops {
                    let cur = [t.work_min, t.break_min, t.long_min][*field];
                    let (vals, titles) = minute_titles(LENGTHS[*field].1, cur);
                    p.removeAllItems();
                    let items: Vec<Retained<NSString>> =
                        titles.iter().map(|x| NSString::from_str(x)).collect();
                    p.addItemsWithTitles(&NSArray::from_retained_slice(&items));
                    p.selectItemAtIndex(vals.iter().position(|m| *m == cur).unwrap_or(0) as isize);
                }
                every.selectItemAtIndex((t.every.clamp(2, 8) - 2) as isize);
                auto_b.setState(if t.auto_breaks { NSControlStateValueOn } else { NSControlStateValueOff });
                auto_w.setState(if t.auto_work { NSControlStateValueOn } else { NSControlStateValueOff });
            })
        };

        let tabs = NSTabViewController::new(mtm);
        tabs.setTabStyle(NSTabViewControllerTabStyle::Toolbar);
        tabs.addTabViewItem(&tasks_tab);
        tabs.addTabViewItem(&tags_tab);
        tabs.addTabViewItem(&timer_tab);
        tabs.addTabViewItem(&view_tab);
        // `YATT_SETTINGS_TAB=n`: opens on that tab (for the demo's captures).
        if let Some(n) = std::env::var("YATT_SETTINGS_TAB").ok().and_then(|v| v.parse::<isize>().ok()) {
            tabs.setSelectedTabViewItemIndex(n);
        }
        let window = NSWindow::windowWithContentViewController(&tabs);
        window.setTitle(ns_string!("Настройки"));
        window.setStyleMask(NSWindowStyleMask::Titled | NSWindowStyleMask::Closable);
        unsafe { window.setReleasedWhenClosed(false) };
        window.setLevel(NSNormalWindowLevel);
        window.center();
        retained.push(Retained::<AnyObject>::from(Retained::into_super(tabs.clone())));
        retained.push(Retained::<AnyObject>::from(Retained::into_super(timer_tab)));
        retained.push(Retained::<AnyObject>::from(Retained::into_super(tasks_tab)));
        retained.push(Retained::<AnyObject>::from(Retained::into_super(view_tab)));
        retained.push(Retained::<AnyObject>::from(Retained::into_super(tags_tab)));
        let _ = (NSColor::clearColor(), NSView::new(mtm), ns_string!("").copy());
        Self {
            window,
            tabs,
            timer_show,
            refresh,
            _retained: retained,
        }
    }

    /// The pomodoro's values now in force (every snapshot).
    pub fn show_timer(&self, t: &crate::model::TimerSnap) {
        (self.timer_show)(t);
    }

    /// Opens the window on a tab: 0 tasks, 1 tags, 2 pomodoro, 3 view.
    pub fn show_tab(&self, index: isize) {
        self.show();
        self.tabs.setSelectedTabViewItemIndex(index);
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
