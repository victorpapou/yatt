//! Every duration and curve the UI animates with, in one place.

use objc2_app_kit::NSWorkspace;

/// Cards sliding to a new place in a column or across columns.
pub const MOVE_SECS: f64 = 0.32;
/// A card growing or shrinking when it expands or collapses.
pub const EXPAND_SECS: f64 = 0.38;
/// New cards fading and scaling in, removed cards fading out.
pub const APPEAR_SECS: f64 = 0.22;
/// Neighbours moving apart to open a gap under a dragged card.
pub const GAP_SECS: f64 = 0.18;
/// A timer digit rolling to its next value.
pub const DIGIT_SECS: f64 = 0.25;
/// Scale a new card starts from.
pub const APPEAR_SCALE: f64 = 0.92;

/// Cubic bezier control points for moves and resizes: fast start, soft landing
/// with a hint of overshoot-free spring.
pub const SPRING_CURVE: [f32; 4] = [0.2, 0.9, 0.25, 1.0];

/// System setting "Reduce motion": cross-fade instead of moving things around.
pub fn reduce_motion() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}
