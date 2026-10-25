//! Card colours, glyphs and sizes, matched to the reference clip
//! (docs/reference/card-motion.md).

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2::{AnyThread, msg_send};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSColor, NSFontAttributeName, NSForegroundColorAttributeName,
    NSGlassEffectView, NSGlassEffectViewStyle, NSImage, NSImageSymbolConfiguration,
    NSKernAttributeName, NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSTextField,
    NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
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
pub const CARD_SATURATE: f64 = 1.5;

type Rgb = (u8, u8, u8);

/// (fill, text) as sRGB: the clip's blue, pink, purple, yellow and green with a
/// quarter more saturation, and white.
const HUES: [(Rgb, Rgb); 6] = [
    ((0x62, 0x86, 0xF5), (0x22, 0x31, 0x6B)),
    ((0xF2, 0x71, 0x85), (0x6B, 0x1E, 0x2A)),
    ((0xA5, 0x3A, 0xF0), (0x3E, 0x11, 0x60)),
    ((0xEC, 0xF5, 0x72), (0x4F, 0x54, 0x1A)),
    ((0x7F, 0xD0, 0x96), (0x22, 0x54, 0x3A)),
    // The "white" card: a light neutral grey, so it reads as a card of its own.
    ((0xE4, 0xE4, 0xE8), (0x2E, 0x2E, 0x34)),
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
    // Over the system's glass the tint is lighter, so less of the frost shows
    // through as colour mud; over the plain blur it is denser.
    let white = hue % HUES.len() == HUES.len() - 1;
    // The grey card is denser than the colours: a pale, thin grey over other
    // cards reads as a white halo round the card in front, not as a card.
    let alpha = match (real_glass(), white) {
        (true, true) => 0.75,
        (true, false) => 0.45,
        (false, true) => 0.85,
        (false, false) => 0.55,
    };
    rgb(HUES[hue % HUES.len()].0, alpha)
}

pub fn text(hue: usize) -> Retained<NSColor> {
    rgb(HUES[hue % HUES.len()].1, 1.0)
}

/// Each column's colour, for its header chip: backlog, in progress, done.
pub fn column_color(i: usize) -> Retained<NSColor> {
    match i {
        0 => NSColor::systemGrayColor(),
        // «В работе»: yellow, a shade deep enough for its text to read.
        1 => NSColor::colorWithSRGBRed_green_blue_alpha(0.86, 0.62, 0.0, 1.0),
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
        // The public style with the least frost.
        glass.setStyle(NSGlassEffectViewStyle::Clear);
        tune_glass(&glass);
        // The glass has no saturation of its own: under it, a backdrop that only
        // saturates (no blur), so it shows the same 1.2 boost as the plain path.
        let holder = FlippedView::new(mtm, NSRect::ZERO);
        let fill = NSAutoresizingMaskOptions::ViewWidthSizable
            | NSAutoresizingMaskOptions::ViewHeightSizable;
        if let Some(layer) = backdrop_layer(0.0) {
            round(&layer, CARD_RADIUS);
            layer.setMasksToBounds(true);
            let saturate = FlippedView::new(mtm, NSRect::ZERO);
            saturate.setLayer(Some(&layer));
            saturate.setWantsLayer(true);
            saturate.setAutoresizingMask(fill);
            holder.addSubview(&saturate);
        }
        glass.setAutoresizingMask(fill);
        holder.addSubview(&glass);
        return Retained::into_super(holder);
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

/// The cards' backdrop: a plain backdrop blur of exactly [`CARD_BLUR`] points
/// with the saturation boost, the lightest frost there is. `YATT_GLASS=1` puts
/// the system's Liquid Glass (`NSGlassEffectView`) there instead, where the
/// system has it; it has no blur radius to turn down, so it stays an option.
pub fn real_glass() -> bool {
    thread_local! {
        static ON: bool = std::env::var_os("YATT_GLASS").is_some_and(|v| v != "0")
            && AnyClass::get(c"NSGlassEffectView").is_some();
    }
    ON.with(|on| *on)
}

/// `NSGlassEffectView` has no public blur radius. What it exposes beyond `style`
/// and `tintColor` are private integer properties (`_variant`, `_subvariant`,
/// `_scrimState`, `_subduedState`, `_contentLensing`, `_adaptiveAppearance`;
/// there is no "splay"). Their values are undocumented, so they stay at the
/// system's defaults unless set for a try with `YATT_GLASS_VARIANT`,
/// `YATT_GLASS_SCRIM`, `YATT_GLASS_SUBDUED`, `YATT_GLASS_LENSING`,
/// `YATT_GLASS_ADAPTIVE` (integers).
fn tune_glass(glass: &NSGlassEffectView) {
    for (key, var) in [
        ("_variant", "YATT_GLASS_VARIANT"),
        ("_scrimState", "YATT_GLASS_SCRIM"),
        ("_subduedState", "YATT_GLASS_SUBDUED"),
        ("_contentLensing", "YATT_GLASS_LENSING"),
        ("_adaptiveAppearance", "YATT_GLASS_ADAPTIVE"),
    ] {
        let Some(value) = std::env::var(var)
            .ok()
            .and_then(|v| v.parse::<isize>().ok())
        else {
            continue;
        };
        let number = NSNumber::new_isize(value);
        unsafe { glass.setValue_forKey(Some(&number), &NSString::from_str(key)) };
    }
}

/// A `CABackdropLayer` blurring what is behind it by `radius` points, if the
/// running system has the classes.
fn backdrop_layer(radius: f64) -> Option<Retained<CALayer>> {
    let backdrop = AnyClass::get(c"CABackdropLayer")?;
    let filter_class = AnyClass::get(c"CAFilter")?;
    let filter = |kind: &NSString| -> Option<Retained<NSObject>> {
        unsafe { msg_send![filter_class, filterWithType: kind] }
    };
    // Like CSS `backdrop-filter: blur(<radius>px) saturate(1.5)`.
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
    /// The shadow of a carried card, hidden until it is lifted.
    pub lift: Retained<NSView>,
    lift_layer: Retained<CALayer>,
    pub outer: Retained<NSView>,
    pub inner: Retained<NSView>,
    /// The selection: a blue ring outside the outline, never on the card itself.
    pub ring: Retained<NSView>,
    ring_layer: Retained<CALayer>,
}

impl Edges {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let layer_view = |image: Option<&CGImage>, inset: f64| {
            // A layer-hosting view: AppKit leaves the layer's contents alone.
            let view = FlippedView::new(mtm, NSRect::ZERO);
            view.setLayer(Some(&edge_layer(image, inset)));
            view.setWantsLayer(true);
            Retained::into_super(view)
        };
        let ring_layer = selection_ring_layer();
        ring_layer.setOpacity(0.0);
        let ring = FlippedView::new(mtm, NSRect::ZERO);
        ring.setLayer(Some(&ring_layer));
        ring.setWantsLayer(true);
        let lift_layer = lift_image().map_or_else(CALayer::new, |i| edge_layer(Some(&i), LIFT_PAD));
        lift_layer.setOpacity(0.0);
        let lift = FlippedView::new(mtm, NSRect::ZERO);
        lift.setLayer(Some(&lift_layer));
        lift.setWantsLayer(true);
        with_edge_images(|outer, inner| Self {
            lift: Retained::into_super(lift),
            lift_layer,
            outer: layer_view(outer, EDGE_PAD),
            inner: layer_view(inner, 0.0),
            ring: Retained::into_super(ring),
            ring_layer,
        })
    }

    /// Fits both layers to a `w` × `h` card at the origin of its view.
    pub fn layout(&self, w: f64, h: f64) {
        self.layout_scaled(w, h, 1.0);
    }

    /// The same for a card shown at `scale` of its size: the edges' corners,
    /// blur and depth and the ring's gap, width and corners shrink with it. The
    /// edge bitmaps are shown at a matching density, so their 9-slice corners
    /// cover `scale` times the points.
    pub fn layout_scaled(&self, w: f64, h: f64, scale: f64) {
        let s = scale.max(0.05);
        let lp = LIFT_PAD * s;
        self.lift.setFrame(NSRect::new(
            NSPoint::new(-lp, -lp),
            NSSize::new(w + 2.0 * lp, h + 2.0 * lp),
        ));
        self.lift_layer.setContentsScale(EDGE_SCALE / s);
        let pad = EDGE_PAD * s;
        self.outer.setFrame(NSRect::new(
            NSPoint::new(-pad, -pad),
            NSSize::new(w + 2.0 * pad, h + 2.0 * pad),
        ));
        self.inner
            .setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(w, h)));
        for v in [&self.outer, &self.inner] {
            if let Some(layer) = v.layer() {
                layer.setContentsScale(EDGE_SCALE / s);
            }
        }
        let o = (RING_GAP + RING_WIDTH) * s;
        self.ring.setFrame(NSRect::new(
            NSPoint::new(-o, -o),
            NSSize::new(w + 2.0 * o, h + 2.0 * o),
        ));
        self.ring_layer.setCornerRadius(CARD_RADIUS * s + o);
        self.ring_layer.setBorderWidth(RING_WIDTH * s);
    }

    /// Shows the carried card's shadow, or puts it away, easing over.
    pub fn set_lifted(&self, on: bool) {
        let layer = self.lift_layer.clone();
        crate::motion::eased(crate::motion::GLASS_SECS, move || {
            layer.setOpacity(if on { 1.0 } else { 0.0 })
        });
    }

    /// Shows or hides the selection ring, easing over.
    pub fn set_selected(&self, on: bool) {
        let layer = self.ring_layer.clone();
        crate::motion::eased(crate::motion::SOFTEN_SECS, move || {
            layer.setOpacity(if on { 1.0 } else { 0.0 })
        });
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

/// The carried card's shadow as a 9-slice bitmap, drawn once: only outside the
/// card, soft and dropped a little.
fn lift_image() -> Option<CFRetained<CGImage>> {
    thread_local! {
        static IMAGE: Option<CFRetained<CGImage>> = edge_image(LIFT_PAD, |cx, card, side| {
            let all = CGMutablePath::new();
            unsafe {
                CGMutablePath::add_rect(Some(&all), std::ptr::null(), CGRect::new(CGPoint::ZERO, CGSize::new(side, side)));
                CGMutablePath::add_path(Some(&all), std::ptr::null(), Some(card));
            }
            CGContext::add_path(Some(cx), Some(&all));
            CGContext::eo_clip(Some(cx));
            let black = CGColor::new_srgb(0.0, 0.0, 0.0, LIFT_ALPHA);
            // Shadow offsets and blur are in device pixels; y runs up here.
            CGContext::set_shadow_with_color(
                Some(cx),
                CGSize::new(0.0, -LIFT_DROP * EDGE_SCALE),
                LIFT_BLUR * EDGE_SCALE,
                Some(&black),
            );
            CGContext::set_fill_color_with_color(Some(cx), Some(&CGColor::new_srgb(0.0, 0.0, 0.0, 1.0)));
            CGContext::add_path(Some(cx), Some(card));
            CGContext::fill_path(Some(cx));
        });
    }
    IMAGE.with(|i| i.clone())
}

/// The outline and inner-edge bitmaps, drawn once per thread.
fn with_edge_images<R>(f: impl FnOnce(Option<&CGImage>, Option<&CGImage>) -> R) -> R {
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
                // Shadow blur is in device pixels whatever the scale: in points it is this.
                CGContext::set_shadow_with_color(Some(cx), CGSize::new(0.0, 0.0), OUTLINE_BLUR * EDGE_SCALE, Some(&black));
                CGContext::set_fill_color_with_color(Some(cx), Some(&CGColor::new_srgb(0.0, 0.0, 0.0, 1.0)));
                CGContext::add_path(Some(cx), Some(card));
                CGContext::fill_path(Some(cx));
            }),
            edge_image(0.0, |cx, card, _side| {
                // Clip to the card and stroke its outline again and again, each
                // time wider and faint: the strokes pile up at the edge and thin
                // out inwards, a white band fading over INSET_DEPTH points.
                // (A shadow would be blurred in device pixels, not points, and
                // reads at only half its alpha right at the edge.)
                CGContext::add_path(Some(cx), Some(card));
                CGContext::clip(Some(cx));
                let n = INSET_STEPS as f64;
                let a = 1.0 - (1.0 - INSET_ALPHA).powf(1.0 / n);
                let white = CGColor::new_srgb(1.0, 1.0, 1.0, a);
                CGContext::set_stroke_color_with_color(Some(cx), Some(&white));
                for k in 0..INSET_STEPS {
                    // The stroke straddles the outline: half of it lies inside.
                    // Half-widths of DEPTH·(1 − √(k/n)) stack up to a glow that
                    // fades as (1 − d/DEPTH)², soft all the way, no line.
                    let half = INSET_DEPTH * (1.0 - (k as f64 / n).sqrt());
                    CGContext::set_line_width(Some(cx), 2.0 * half);
                    CGContext::add_path(Some(cx), Some(card));
                    CGContext::stroke_path(Some(cx));
                }
            }),
        );
    }
    IMAGES.with(|(outer, inner)| f(outer.as_deref(), inner.as_deref()))
}

/// Where the selection ring sits: `RING_GAP` points outside the card (clear of
/// the 1 pt outline), `RING_WIDTH` wide.
const RING_GAP: f64 = 1.5;
const RING_WIDTH: f64 = 2.0;

/// The selection ring's layer: an accent stroke on the inside of its bounds, with
/// continuous corners whose radius is the card's plus the outset, so it runs
/// parallel to the card's edge. No fill, no inset, no white line.
fn selection_ring_layer() -> Retained<CALayer> {
    let layer = CALayer::new();
    round(&layer, CARD_RADIUS + RING_GAP + RING_WIDTH);
    layer.setBorderWidth(RING_WIDTH);
    layer.setBorderColor(Some(&NSColor::controlAccentColor().CGColor()));
    layer
}

/// A 9-slice layer showing an edge bitmap whose card shape sits `inset` points
/// in from its sides.
fn edge_layer(image: Option<&CGImage>, inset: f64) -> Retained<CALayer> {
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
    layer
}

/// The outline: black at 50%, blurred 1 pt.
const OUTLINE_ALPHA: f64 = 0.5;
const OUTLINE_BLUR: f64 = 1.0;
/// The inner edge: a soft white glow, 51% at the edge, fading out on an ease-out
/// curve over 3.75 pt, drawn as stacked strokes.
const INSET_ALPHA: f64 = 0.51;
const INSET_DEPTH: f64 = 3.75;
const INSET_STEPS: usize = 24;

/// The shadow under a carried card: 20% black, 24 pt blur, 8 pt down.
const LIFT_PAD: f64 = 40.0;
const LIFT_ALPHA: f64 = 0.2;
const LIFT_BLUR: f64 = 24.0;
const LIFT_DROP: f64 = 8.0;

/// The cards' backdrop (blur and saturation) for a small rounded surface, such
/// as a column chip, `radius` round at the corners.
pub fn chip_blur(mtm: MainThreadMarker, radius: f64) -> Retained<NSView> {
    let view = FlippedView::new(mtm, NSRect::ZERO);
    if let Some(layer) = backdrop_layer(CARD_BLUR) {
        round(&layer, radius);
        layer.setMasksToBounds(true);
        view.setLayer(Some(&layer));
    }
    view.setWantsLayer(true);
    Retained::into_super(view)
}

/// Card glyph bitmaps by symbol name and hue.
type GlyphCache = std::collections::HashMap<(String, usize), Option<Retained<CGImage>>>;

/// Gives a card's backdrop (as made by [`card_blur`]) the corner radius of a card
/// at `scale` of its size.
pub fn set_blur_corners(view: &NSView, scale: f64) {
    let r = CARD_RADIUS * scale;
    if let Some(glass) = view.downcast_ref::<NSGlassEffectView>() {
        glass.setCornerRadius(r);
    } else if let Some(layer) = view.layer() {
        layer.setCornerRadius(r);
    }
    for sub in view.subviews().iter() {
        set_blur_corners(&sub, scale);
    }
}

/// A card's glyph (an SF Symbol) in the card's ink, drawn once per symbol and
/// hue and shown as a layer's contents.
/// A card's glyph as a bitmap in the card's ink, cached per symbol and hue.
pub fn glyph_image(symbol: &str, hue: usize) -> Option<Retained<CGImage>> {
    thread_local! {
        static CACHE: std::cell::RefCell<GlyphCache> =
            Default::default();
    }
    CACHE.with(|cache| {
        cache
            .borrow_mut()
            .entry((symbol.to_string(), hue))
            .or_insert_with(|| {
                let ink = text(hue).colorWithAlphaComponent(0.8);
                let base = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(symbol),
                    None,
                )?;
                let config = NSImageSymbolConfiguration::configurationWithPaletteColors(
                    &NSArray::from_retained_slice(&[ink]),
                );
                let tinted = base.imageWithSymbolConfiguration(&config)?;
                let size = tinted.size();
                // Fit the symbol into 16 pt, at 2 px per point.
                let k = 16.0 / size.width.max(size.height).max(1.0);
                let mut rect = NSRect::new(
                    NSPoint::ZERO,
                    NSSize::new(size.width * k * EDGE_SCALE, size.height * k * EDGE_SCALE),
                );
                unsafe { tinted.CGImageForProposedRect_context_hints(&mut rect, None, None) }
            })
            .clone()
    })
}

pub fn glyph_view(mtm: MainThreadMarker, symbol: &str, hue: usize) -> Retained<NSView> {
    let image = glyph_image(symbol, hue);
    let layer = CALayer::new();
    if let Some(image) = image.as_deref() {
        let image: &AnyObject = unsafe { &*(image as *const CGImage).cast::<AnyObject>() };
        unsafe { layer.setContents(Some(image)) };
    }
    layer.setContentsGravity(unsafe { objc2_quartz_core::kCAGravityResizeAspect });
    layer.setContentsScale(EDGE_SCALE);
    let view = FlippedView::new(mtm, NSRect::ZERO);
    view.setLayer(Some(&layer));
    view.setWantsLayer(true);
    Retained::into_super(view)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders a card's plain layers (tint, then the inner edge; no backdrop) into
    /// a bitmap at 2 px per point and returns its BGRA pixels and row length.
    fn render_card(tint: (f64, f64, f64)) -> (Vec<u8>, usize) {
        render_card_at(tint, 1.0)
    }

    /// The same for a card shown at `scale`, set up the way `Edges` and the tint
    /// scale theirs.
    fn render_card_at(tint: (f64, f64, f64), scale: f64) -> (Vec<u8>, usize) {
        let (w, h) = (CARD_W * scale, CARD_H * scale);
        let root = CALayer::new();
        root.setFrame(CGRect::new(CGPoint::ZERO, CGSize::new(w, h)));
        let face = CALayer::new();
        face.setFrame(root.bounds());
        face.setBackgroundColor(Some(&CGColor::new_srgb(tint.0, tint.1, tint.2, 1.0)));
        round(&face, CARD_RADIUS * scale);
        let inner = with_edge_images(|_, inner| edge_layer(inner, 0.0));
        inner.setContentsScale(EDGE_SCALE / scale);
        inner.setFrame(root.bounds());
        root.addSublayer(&face);
        root.addSublayer(&inner);

        let (pw, ph) = ((w * EDGE_SCALE) as usize, (h * EDGE_SCALE) as usize);
        let mut px = vec![0u8; pw * ph * 4];
        let space = CGColorSpace::new_device_rgb().unwrap();
        let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
        let cx = unsafe {
            CGBitmapContextCreate(
                px.as_mut_ptr().cast(),
                pw,
                ph,
                8,
                pw * 4,
                Some(&space),
                info,
            )
        }
        .unwrap();
        CGContext::scale_ctm(Some(&cx), EDGE_SCALE, EDGE_SCALE);
        root.renderInContext(&cx);
        drop(cx);
        (px, pw)
    }

    /// Lightness (sum of R, G, B) at a point `x` points in from the left edge, on
    /// the card's middle row.
    fn light(px: &[u8], pw: usize, x: f64) -> u32 {
        let (col, row) = (
            (x * EDGE_SCALE) as usize,
            (CARD_H * EDGE_SCALE / 2.0) as usize,
        );
        let i = (row * pw + col) * 4;
        px[i] as u32 + px[i + 1] as u32 + px[i + 2] as u32
    }

    #[test]
    fn the_white_inset_shows_inside_the_edge_on_a_coloured_card() {
        // The purple of Тима's screenshot.
        let (px, pw) = render_card((0.78, 0.58, 0.95));
        let centre = light(&px, pw, CARD_W / 2.0);
        let at = |x| light(&px, pw, x);
        eprintln!(
            "lightness (R+G+B): 0.25 pt in {}, 1 pt {}, 1.5 pt {}, 2 pt {}, 3 pt {}, centre {centre}",
            at(0.25),
            at(1.0),
            at(1.5),
            at(2.0),
            at(3.0)
        );
        // Right inside the edge about a third of the way to white (the glow is
        // 42% white there), fading softly, gone by about 4 pt.
        let white = 3 * 255;
        assert!(
            (at(0.25) - centre) * 100 > 30 * (white - centre),
            "edge {} vs centre {centre}",
            at(0.25)
        );
        assert!(
            (at(1.0) - centre) * 100 > 8 * (white - centre),
            "1 pt {}",
            at(1.0)
        );
        assert!(at(1.0) > at(1.5) && at(1.5) >= at(2.0));
        assert!(
            at(4.5) <= centre + 3,
            "4.5 pt {} vs centre {centre}",
            at(4.5)
        );
    }

    #[test]
    fn every_card_glyph_draws() {
        for symbol in GLYPHS {
            let image = glyph_image(symbol, 0).unwrap_or_else(|| panic!("{symbol}: no image"));
            let (w, h) = (CGImage::width(Some(&image)), CGImage::height(Some(&image)));
            assert!(w >= 16 && h >= 16, "{symbol}: {w}×{h}");
            // Some ink in it: draw it and look for opaque pixels.
            let mut px = vec![0u8; w * h * 4];
            let space = CGColorSpace::new_device_rgb().unwrap();
            let info =
                CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
            let cx = unsafe {
                CGBitmapContextCreate(px.as_mut_ptr().cast(), w, h, 8, w * 4, Some(&space), info)
            }
            .unwrap();
            CGContext::draw_image(
                Some(&cx),
                CGRect::new(CGPoint::ZERO, CGSize::new(w as f64, h as f64)),
                Some(&image),
            );
            drop(cx);
            let opaque = px.chunks(4).filter(|p| p[3] > 128).count();
            assert!(opaque > w * h / 20, "{symbol}: only {opaque} opaque pixels");
        }
    }

    #[test]
    fn a_card_at_0_8_has_its_edge_and_corners_at_0_8() {
        let tint = (0.78, 0.58, 0.95);
        let (full, fw) = render_card_at(tint, 1.0);
        let (small, sw) = render_card_at(tint, 0.8);
        let light = |px: &[u8], pw: usize, x: f64, y: f64| {
            let i = (((y * EDGE_SCALE) as usize) * pw + (x * EDGE_SCALE) as usize) * 4;
            px[i] as i32 + px[i + 1] as i32 + px[i + 2] as i32
        };
        // The white inner edge: what the full card shows d points in, the small
        // one shows 0.8 d in.
        for d in [0.5, 1.0, 1.5, 2.5] {
            let a = light(&full, fw, d, CARD_H / 2.0);
            let b = light(&small, sw, d * 0.8, CARD_H * 0.8 / 2.0);
            // Within what half a pixel of the 2 px/pt grid makes up.
            assert!((a - b).abs() <= 30, "{d} pt in: full {a}, at 0.8 {b}");
        }
        // The corner: a point 0.25 radii in from the corner along the diagonal
        // lies outside the full card's curve, and at 0.8 both corners shrink alike.
        let alpha = |px: &[u8], pw: usize, x: f64| {
            let i = (((x * EDGE_SCALE) as usize) * pw + (x * EDGE_SCALE) as usize) * 4;
            px[i + 3]
        };
        let p = 0.2 * CARD_RADIUS;
        assert_eq!(alpha(&full, fw, p), alpha(&small, sw, p * 0.8));
        assert!(alpha(&full, fw, p) < 40, "corner {}", alpha(&full, fw, p));
        assert!(alpha(&full, fw, 0.5 * CARD_RADIUS) > 200);
        assert!(alpha(&small, sw, 0.5 * CARD_RADIUS * 0.8) > 200);
    }

    /// Renders a selected card the way `Edges` stacks it (ring, outline, tint,
    /// inner edge) over white, 8 pt of margin all round, and returns the pixels
    /// along the middle row as (R, G, B) per point offset from the card's edge.
    fn selected_cross_section(tint: (f64, f64, f64), offsets: &[f64]) -> Vec<(u8, u8, u8)> {
        let m = 8.0;
        let (w, h) = (CARD_W + 2.0 * m, CARD_H + 2.0 * m);
        let root = CALayer::new();
        root.setFrame(CGRect::new(CGPoint::ZERO, CGSize::new(w, h)));
        root.setBackgroundColor(Some(&CGColor::new_srgb(1.0, 1.0, 1.0, 1.0)));
        let card = CGRect::new(CGPoint::new(m, m), CGSize::new(CARD_W, CARD_H));
        let around = |d: f64| {
            CGRect::new(
                CGPoint::new(m - d, m - d),
                CGSize::new(CARD_W + 2.0 * d, CARD_H + 2.0 * d),
            )
        };
        let ring = selection_ring_layer();
        ring.setFrame(around(RING_GAP + RING_WIDTH));
        let (outer, inner) = with_edge_images(|o, i| (edge_layer(o, EDGE_PAD), edge_layer(i, 0.0)));
        outer.setFrame(around(EDGE_PAD));
        inner.setFrame(card);
        let face = CALayer::new();
        face.setFrame(card);
        face.setBackgroundColor(Some(&CGColor::new_srgb(tint.0, tint.1, tint.2, 1.0)));
        round(&face, CARD_RADIUS);
        for l in [&ring, &outer, &face, &inner] {
            root.addSublayer(l);
        }
        let (pw, ph) = ((w * EDGE_SCALE) as usize, (h * EDGE_SCALE) as usize);
        let mut px = vec![0u8; pw * ph * 4];
        let space = CGColorSpace::new_device_rgb().unwrap();
        let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
        let cx = unsafe {
            CGBitmapContextCreate(
                px.as_mut_ptr().cast(),
                pw,
                ph,
                8,
                pw * 4,
                Some(&space),
                info,
            )
        }
        .unwrap();
        CGContext::scale_ctm(Some(&cx), EDGE_SCALE, EDGE_SCALE);
        root.renderInContext(&cx);
        drop(cx);
        let row = (h * EDGE_SCALE / 2.0) as usize;
        offsets
            .iter()
            .map(|d| {
                let col = ((m + d) * EDGE_SCALE) as usize;
                let i = (row * pw + col) * 4;
                (px[i + 2], px[i + 1], px[i])
            })
            .collect()
    }

    #[test]
    fn a_selected_card_reads_ring_outline_inset_tint_from_outside_in() {
        let tint = (0.78, 0.58, 0.95);
        // Offsets from the card's left edge, in points (negative is outside).
        let at = [-2.5, -0.5, 0.25, CARD_W / 2.0];
        let px = selected_cross_section(tint, &at);
        eprintln!("selected card, R G B at {at:?} pt from the edge: {px:?}");
        let sum = |p: (u8, u8, u8)| p.0 as u32 + p.1 as u32 + p.2 as u32;
        let (ring, outline, inset, centre) = (px[0], px[1], px[2], px[3]);
        // The ring: blue, outside the card.
        let c = |v: u8| v as u32;
        assert!(
            c(ring.2) > c(ring.0) + 60 && c(ring.2) > c(ring.1) + 30,
            "ring {ring:?}"
        );
        // The outline: darker than the white page, not blue.
        assert!(
            sum(outline) < 3 * 255 - 30 && c(outline.2) < c(outline.0) + 40,
            "outline {outline:?}"
        );
        // The inset: lighter than the tint; the centre is the tint.
        assert!(
            sum(inset) > sum(centre) + 30,
            "inset {inset:?} vs centre {centre:?}"
        );
    }
}
