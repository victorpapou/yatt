//! Card colours, glyphs and sizes, matched to the reference clip
//! (docs/reference/card-motion.md).

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAutoresizingMaskOptions,
    NSColor, NSGlassEffectView, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView,
};
use objc2_foundation::NSRect;
use objc2_quartz_core::{CALayer, kCACornerCurveContinuous};
use yatt_core::TaskId;

use crate::classes::FlippedView;

/// Card size; the height keeps the clip's 2.27 : 1 proportion.
pub const CARD_W: f64 = 230.0;
pub const CARD_H: f64 = 101.0;
pub const CARD_RADIUS: f64 = 20.0;

type Rgb = (u8, u8, u8);

/// (fill, text) as sRGB, from the clip: blue, pink, purple, yellow, green, white.
const HUES: [(Rgb, Rgb); 6] = [
    ((0x7F, 0x9C, 0xF5), (0x22, 0x31, 0x6B)),
    ((0xF2, 0x8B, 0x9B), (0x6B, 0x1E, 0x2A)),
    ((0xB4, 0x5E, 0xF0), (0x3E, 0x11, 0x60)),
    ((0xEE, 0xF5, 0x8C), (0x4F, 0x54, 0x1A)),
    ((0x8F, 0xD0, 0xA2), (0x22, 0x54, 0x3A)),
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

/// The card's tint: translucent, so the blur behind it shows through as glass.
pub fn fill(hue: usize) -> Retained<NSColor> {
    rgb(HUES[hue % HUES.len()].0, 0.9)
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

/// The blur under a card's tint: whatever is behind it in the window, frosted
/// light in any system appearance.
pub fn card_blur(mtm: MainThreadMarker) -> Retained<NSVisualEffectView> {
    let glass = NSVisualEffectView::new(mtm);
    glass.setMaterial(NSVisualEffectMaterial::Popover);
    glass.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
    glass.setState(NSVisualEffectState::Active);
    glass.setAppearance(NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }).as_deref());
    glass.setWantsLayer(true);
    if let Some(layer) = glass.layer() {
        round(&layer, CARD_RADIUS);
        layer.setMasksToBounds(true);
    }
    glass
}

/// Rounds a layer's corners as a continuous squircle, the way system controls are.
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
