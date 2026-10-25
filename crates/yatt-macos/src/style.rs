//! Card colours, glyphs and sizes, matched to the reference clip
//! (docs/reference/card-motion.md).

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2::{AnyThread, msg_send};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSColor, NSFontAttributeName, NSForegroundColorAttributeName,
    NSGlassEffectView, NSKernAttributeName, NSMutableParagraphStyle, NSParagraphStyleAttributeName,
    NSTextField, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSVisualEffectView,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGColor, CGColorSpace, CGContext, CGImage,
    CGImageAlphaInfo, CGImageByteOrderInfo, CGMutablePath,
};
use objc2_core_image::CIFilter;
use objc2_foundation::{
    NSArray, NSAttributedString, NSAttributedStringKey, NSDictionary, NSNumber,
    NSObjectNSKeyValueCoding, NSPoint, NSRect, NSSize, NSString, ns_string,
};
use objc2_quartz_core::{CALayer, kCACornerCurveContinuous};
use yatt_core::TaskId;

use crate::classes::FlippedView;

/// Card size; the height keeps the clip's 2.27 : 1 proportion.
pub const CARD_W: f64 = 230.0;
pub const CARD_H: f64 = 101.0;
pub const CARD_RADIUS: f64 = 20.0;
/// Blur of what shows through a card, in points.
pub const CARD_BLUR: f64 = 4.0;
/// Saturation boost of what shows through a card.
pub const CARD_SATURATE: f64 = 1.2;

type Rgb = (u8, u8, u8);

/// (fill, text) as sRGB: the clip's blue, pink, purple, yellow and green with a
/// quarter more saturation, and white.
const HUES: [(Rgb, Rgb); 6] = [
    ((0x62, 0x86, 0xF5), (0x22, 0x31, 0x6B)),
    ((0xF2, 0x71, 0x85), (0x6B, 0x1E, 0x2A)),
    ((0xA5, 0x3A, 0xF0), (0x3E, 0x11, 0x60)),
    ((0xEC, 0xF5, 0x72), (0x4F, 0x54, 0x1A)),
    ((0x7F, 0xD0, 0x96), (0x22, 0x54, 0x3A)),
    ((0xF6, 0xF6, 0xF8), (0x2E, 0x2E, 0x34)),
];

/// Filled SF Symbols in the spirit of the clip's event glyphs.
const GLYPHS: [&str; 6] = [
    "paperplane.fill",
    "flag.fill",
    "bubble.left.fill",
    "sun.max.fill",
    "calendar",
    "doc.fill",
];

/// A stable colour index for a task, until projects and tags carry colours.
pub fn hue_of(id: TaskId) -> usize {
    (fold(id) % HUES.len() as u64) as usize
}

pub fn glyph_of(id: TaskId) -> &'static str {
    GLYPHS[((fold(id) / 7) % GLYPHS.len() as u64) as usize]
}

fn fold(id: TaskId) -> u64 {
    // UUIDv7 starts with a timestamp; the tail is random, so use that.
    let b = id.0.as_bytes();
    u64::from_le_bytes([b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]])
}

fn rgb((r, g, b): Rgb, alpha: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        r as f64 / 255.0,
        g as f64 / 255.0,
        b as f64 / 255.0,
        alpha,
    )
}

/// The card's tint: translucent, so the cards and board behind it show through
/// the blur.
pub fn fill(hue: usize) -> Retained<NSColor> {
    // Dense enough that text behind is only a soft patch under the light blur.
    let white = hue % HUES.len() == HUES.len() - 1;
    rgb(HUES[hue % HUES.len()].0, if white { 0.65 } else { 0.55 })
}

pub fn text(hue: usize) -> Retained<NSColor> {
    rgb(HUES[hue % HUES.len()].1, 1.0)
}

/// Each column's colour, for its header chip: backlog, in progress, done.
pub fn column_color(i: usize) -> Retained<NSColor> {
    match i {
        0 => NSColor::systemGrayColor(),
        1 => NSColor::systemBlueColor(),
        _ => NSColor::systemGreenColor(),
    }
}

/// The card's text colour as CSS, for the notes editor.
pub fn text_css(hue: usize) -> String {
    let (r, g, b) = HUES[hue % HUES.len()].1;
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// The blur under a card's tint: what is behind it in the window (other cards,
/// text, grid lines), blurred by [`CARD_BLUR`] points with no material colour of
/// its own. A backdrop layer with a Gaussian blur filter, the way the system's own
/// blur views work inside (private Core Animation classes, looked up at run time);
/// a system within-window blur where those are missing.
pub fn card_blur(mtm: MainThreadMarker) -> Retained<NSView> {
    if real_glass() {
        let glass = NSGlassEffectView::new(mtm);
        glass.setCornerRadius(CARD_RADIUS);
        return Retained::into_super(glass);
    }
    if let Some(layer) = backdrop_layer(CARD_BLUR) {
        round(&layer, CARD_RADIUS);
        layer.setMasksToBounds(true);
        // A layer-hosting view: AppKit keeps the layer sized to the view.
        let view = FlippedView::new(mtm, NSRect::ZERO);
        view.setLayer(Some(&layer));
        view.setWantsLayer(true);
        return Retained::into_super(view);
    }
    let view = NSVisualEffectView::new(mtm);
    view.setMaterial(NSVisualEffectMaterial::Popover);
    view.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
    view.setState(NSVisualEffectState::Active);
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        round(&layer, CARD_RADIUS);
        layer.setMasksToBounds(true);
    }
    Retained::into_super(view)
}

/// `YATT_GLASS=1` puts the system's own Liquid Glass (`NSGlassEffectView`) under
/// the cards instead of the plain backdrop blur, where the system has it: an
/// option to compare looks and cost.
pub fn real_glass() -> bool {
    thread_local! {
        static ON: bool = std::env::var_os("YATT_GLASS").is_some_and(|v| v != "0")
            && AnyClass::get(c"NSGlassEffectView").is_some();
    }
    ON.with(|on| *on)
}

/// A `CABackdropLayer` blurring what is behind it by `radius` points, if the
/// running system has the classes.
fn backdrop_layer(radius: f64) -> Option<Retained<CALayer>> {
    let backdrop = AnyClass::get(c"CABackdropLayer")?;
    let filter_class = AnyClass::get(c"CAFilter")?;
    let filter = |kind: &NSString| -> Option<Retained<NSObject>> {
        unsafe { msg_send![filter_class, filterWithType: kind] }
    };
    // Like CSS `backdrop-filter: blur(4px) saturate(1.08)`.
    let blur = filter(ns_string!("gaussianBlur"))?;
    let saturate = filter(ns_string!("colorSaturate"))?;
    let layer: Option<Retained<CALayer>> = unsafe { msg_send![backdrop, layer] };
    let layer = layer?;
    unsafe {
        blur.setValue_forKey(Some(&NSNumber::new_f64(radius)), ns_string!("inputRadius"));
        blur.setValue_forKey(
            Some(&NSNumber::new_bool(true)),
            ns_string!("inputNormalizeEdges"),
        );
        saturate.setValue_forKey(
            Some(&NSNumber::new_f64(CARD_SATURATE)),
            ns_string!("inputAmount"),
        );
        let filters: Retained<NSArray> =
            Retained::cast_unchecked(NSArray::from_retained_slice(&[blur, saturate]));
        layer.setFilters(Some(&filters));
    }
    Some(layer)
}

/// A Gaussian blur filter of `radius` points.
pub fn gaussian(radius: f64) -> Option<Retained<CIFilter>> {
    let filter = unsafe { CIFilter::filterWithName(ns_string!("CIGaussianBlur")) }?;
    unsafe {
        filter.setDefaults();
        filter.setValue_forKey(Some(&NSNumber::new_f64(radius)), ns_string!("inputRadius"));
    }
    Some(filter)
}

/// Rounds a layer's corners as a continuous squircle, the way system controls are.
/// Adds a rounded rect with continuous corners (the curve Core Animation's
/// `kCACornerCurveContinuous` draws) to `path`. The corner starts 1.53 radii
/// from each edge's end; a rect too small for that gets a smaller radius.
pub fn continuous_rect(path: &CGMutablePath, x: f64, y: f64, w: f64, h: f64, r: f64) {
    const L: f64 = 1.528_664_83;
    let r = r.min(w.min(h) / 2.0 / L).max(0.0);
    // One corner as offsets from the corner point along its two edges, from the
    // first edge to the second: (along the first edge back, along the second in).
    const CURVES: [[(f64, f64); 3]; 3] = [
        [
            (1.088_493_23, 0.0),
            (0.868_407_01, 0.0),
            (0.631_493_99, 0.074_911_00),
        ],
        [
            (0.372_823_92, 0.169_058_99),
            (0.169_058_99, 0.372_823_92),
            (0.074_911_00, 0.631_493_99),
        ],
        [(0.0, 0.868_407_01), (0.0, 1.088_493_23), (0.0, L)],
    ];
    // Corner points and the unit directions of "back along the first edge" and
    // "in along the second", clockwise in a y-down frame from the top right.
    let corners = [
        ((x + w, y), (-1.0, 0.0), (0.0, 1.0)),
        ((x + w, y + h), (0.0, -1.0), (-1.0, 0.0)),
        ((x, y + h), (1.0, 0.0), (0.0, -1.0)),
        ((x, y), (0.0, 1.0), (1.0, 0.0)),
    ];
    let at =
        |(cx, cy): (f64, f64), (ax, ay): (f64, f64), (bx, by): (f64, f64), (u, v): (f64, f64)| {
            (cx + (ax * u + bx * v) * r, cy + (ay * u + by * v) * r)
        };
    unsafe {
        let (sx, sy) = at(corners[3].0, corners[3].1, corners[3].2, (0.0, L));
        CGMutablePath::move_to_point(Some(path), std::ptr::null(), sx, sy);
        for (c, a, b) in corners {
            let (lx, ly) = at(c, a, b, (L, 0.0));
            CGMutablePath::add_line_to_point(Some(path), std::ptr::null(), lx, ly);
            for [p1, p2, p3] in CURVES {
                let ((x1, y1), (x2, y2), (x3, y3)) =
                    (at(c, a, b, p1), at(c, a, b, p2), at(c, a, b, p3));
                CGMutablePath::add_curve_to_point(
                    Some(path),
                    std::ptr::null(),
                    x1,
                    y1,
                    x2,
                    y2,
                    x3,
                    y3,
                );
            }
        }
    }
    CGMutablePath::close_subpath(Some(path));
}

/// Room around a card for its outer shadow, in points.
const EDGE_PAD: f64 = 4.0;
/// Pixels per point the edge bitmaps are drawn at.
const EDGE_SCALE: f64 = 2.0;
/// Size of a card corner's region in an edge bitmap, in points.
const EDGE_CORNER: f64 = 32.0;

/// Draws one edge bitmap: a card-shaped hole `inset` points in from the sides,
/// handed to `draw` with the context scaled to points, as a 9-slice source.
fn edge_image(
    inset: f64,
    draw: impl Fn(&CGContext, &CGMutablePath, f64),
) -> Option<CFRetained<CGImage>> {
    let side = 2.0 * (EDGE_CORNER + inset) + 1.0;
    let px = (side * EDGE_SCALE).ceil() as usize;
    let space = CGColorSpace::new_device_rgb()?;
    let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
    let cx =
        unsafe { CGBitmapContextCreate(std::ptr::null_mut(), px, px, 8, 0, Some(&space), info)? };
    CGContext::scale_ctm(Some(&cx), EDGE_SCALE, EDGE_SCALE);
    let card = CGMutablePath::new();
    let inner = side - 2.0 * inset;
    continuous_rect(&card, inset, inset, inner, inner, CARD_RADIUS);
    draw(&cx, &card, side);
    CGBitmapContextCreateImage(Some(&cx))
}

/// The card's two edge layers, drawn once as 9-slice bitmaps so they cost no
/// more than a picture however many cards move:
/// - outside, the outline: a black 1 px shadow at 30%, 1 px blur, no offset,
///   only outside the card;
/// - inside, a white 2 px inner shadow along the edge, only inside it.
///   Nothing else draws at the edge: no stroke, no glow.
pub struct Edges {
    pub outer: Retained<NSView>,
    pub inner: Retained<NSView>,
}

impl Edges {
    pub fn new(mtm: MainThreadMarker) -> Self {
        thread_local! {
            static IMAGES: (Option<CFRetained<CGImage>>, Option<CFRetained<CGImage>>) = (
                edge_image(EDGE_PAD, |cx, card, side| {
                    // Clip to outside the card, then let a filled card cast its
                    // shadow: only the part outside remains.
                    let all = CGMutablePath::new();
                    unsafe {
                        CGMutablePath::add_rect(Some(&all), std::ptr::null(), CGRect::new(CGPoint::ZERO, CGSize::new(side, side)));
                        CGMutablePath::add_path(Some(&all), std::ptr::null(), Some(card));
                    }
                    CGContext::add_path(Some(cx), Some(&all));
                    CGContext::eo_clip(Some(cx));
                    let black = CGColor::new_srgb(0.0, 0.0, 0.0, OUTLINE_ALPHA);
                    CGContext::set_shadow_with_color(Some(cx), CGSize::new(0.0, 0.0), OUTLINE_BLUR, Some(&black));
                    CGContext::set_fill_color_with_color(Some(cx), Some(&CGColor::new_srgb(0.0, 0.0, 0.0, 1.0)));
                    CGContext::add_path(Some(cx), Some(card));
                    CGContext::fill_path(Some(cx));
                }),
                edge_image(0.0, |cx, card, side| {
                    // Clip to the card, then let everything outside it cast a white
                    // shadow inwards.
                    CGContext::add_path(Some(cx), Some(card));
                    CGContext::clip(Some(cx));
                    let white = CGColor::new_srgb(1.0, 1.0, 1.0, INSET_ALPHA);
                    CGContext::set_shadow_with_color(Some(cx), CGSize::new(0.0, 0.0), INSET_BLUR, Some(&white));
                    CGContext::set_fill_color_with_color(Some(cx), Some(&CGColor::new_srgb(1.0, 1.0, 1.0, 1.0)));
                    let ring = CGMutablePath::new();
                    unsafe {
                        CGMutablePath::add_rect(Some(&ring), std::ptr::null(), CGRect::new(CGPoint::new(-20.0, -20.0), CGSize::new(side + 40.0, side + 40.0)));
                        CGMutablePath::add_path(Some(&ring), std::ptr::null(), Some(card));
                    }
                    CGContext::add_path(Some(cx), Some(&ring));
                    CGContext::eo_fill_path(Some(cx));
                }),
            );
        }
        let layer_view = |image: Option<&CGImage>, inset: f64| {
            let layer = CALayer::new();
            if let Some(image) = image {
                let image: &AnyObject = unsafe { &*(image as *const CGImage).cast::<AnyObject>() };
                unsafe { layer.setContents(Some(image)) };
            }
            let side = 2.0 * (EDGE_CORNER + inset) + 1.0;
            let c = (EDGE_CORNER + inset) / side;
            layer.setContentsCenter(CGRect::new(
                CGPoint::new(c, c),
                CGSize::new(1.0 / side, 1.0 / side),
            ));
            layer.setContentsScale(EDGE_SCALE);
            // A layer-hosting view: AppKit leaves the layer's contents alone.
            let view = FlippedView::new(mtm, NSRect::ZERO);
            view.setLayer(Some(&layer));
            view.setWantsLayer(true);
            Retained::into_super(view)
        };
        IMAGES.with(|(outer, inner)| Self {
            outer: layer_view(outer.as_deref(), EDGE_PAD),
            inner: layer_view(inner.as_deref(), 0.0),
        })
    }

    /// Fits both layers to a `w` × `h` card at the origin of its view.
    pub fn layout(&self, w: f64, h: f64) {
        self.outer.setFrame(NSRect::new(
            NSPoint::new(-EDGE_PAD, -EDGE_PAD),
            NSSize::new(w + 2.0 * EDGE_PAD, h + 2.0 * EDGE_PAD),
        ));
        self.inner
            .setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(w, h)));
    }

    /// Lets both layers follow their view's size from now on.
    pub fn follow(&self, w: f64, h: f64) {
        self.layout(w, h);
        let mask = NSAutoresizingMaskOptions::ViewWidthSizable
            | NSAutoresizingMaskOptions::ViewHeightSizable;
        self.outer.setAutoresizingMask(mask);
        self.inner.setAutoresizingMask(mask);
    }
}

/// The outline: black at 30%, blurred 1 pt.
const OUTLINE_ALPHA: f64 = 0.3;
const OUTLINE_BLUR: f64 = 1.0;
/// The inner shadow: white, about 2 pt.
const INSET_ALPHA: f64 = 0.7;
const INSET_BLUR: f64 = 2.0;

/// Sets a label's text with `kern` points of tracking, in the label's own font,
/// colour and truncation (an attributed string brings its own and would otherwise
/// fall back to the defaults).
pub fn set_tracked(label: &NSTextField, text: &str, kern: f64) {
    if label.stringValue().to_string() == text {
        return;
    }
    let style = NSMutableParagraphStyle::new();
    style.setLineBreakMode(label.lineBreakMode());
    style.setAlignment(label.alignment());
    let mut keys: Vec<&NSAttributedStringKey> = Vec::new();
    let mut values: Vec<Retained<AnyObject>> = Vec::new();
    unsafe {
        keys.push(NSKernAttributeName);
        values.push(Retained::into_super(Retained::into_super(
            Retained::into_super(NSNumber::new_f64(kern)),
        )));
        keys.push(NSParagraphStyleAttributeName);
        values.push(Retained::into_super(Retained::into_super(
            Retained::into_super(style),
        )));
        if let Some(font) = label.font() {
            keys.push(NSFontAttributeName);
            values.push(Retained::into_super(Retained::into_super(font)));
        }
        if let Some(color) = label.textColor() {
            keys.push(NSForegroundColorAttributeName);
            values.push(Retained::into_super(Retained::into_super(color)));
        }
    }
    let attrs = NSDictionary::from_retained_objects(&keys, &values);
    let string = unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(text),
            Some(&attrs),
        )
    };
    label.setAttributedStringValue(&string);
}

pub fn round(layer: &CALayer, radius: f64) {
    layer.setCornerRadius(radius);
    layer.setCornerCurve(unsafe { kCACornerCurveContinuous });
}

/// A native glass surface: `NSGlassEffectView` where the system has it, a
/// within-window blur with a hairline rim otherwise. Put content into the
/// returned flipped view; it tracks the glass's size.
pub fn glass(mtm: MainThreadMarker, radius: f64) -> (Retained<NSView>, Retained<FlippedView>) {
    let content = FlippedView::new(mtm, NSRect::ZERO);
    content.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    if AnyClass::get(c"NSGlassEffectView").is_some() {
        let glass = NSGlassEffectView::new(mtm);
        glass.setCornerRadius(radius);
        glass.setContentView(Some(&content));
        return (Retained::into_super(glass), content);
    }
    let glass = NSVisualEffectView::new(mtm);
    glass.setMaterial(NSVisualEffectMaterial::Popover);
    glass.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
    glass.setState(NSVisualEffectState::Active);
    glass.setWantsLayer(true);
    if let Some(layer) = glass.layer() {
        round(&layer, radius);
        layer.setMasksToBounds(true);
        layer.setBorderWidth(0.5);
        layer.setBorderColor(Some(&NSColor::separatorColor().CGColor()));
    }
    glass.addSubview(&content);
    (Retained::into_super(glass), content)
}
