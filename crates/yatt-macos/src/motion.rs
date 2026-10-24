//! Every duration and curve the UI animates with, in one place. Card motion is
//! spring-driven instead; its constants live in `physics.rs`.

use objc2_app_kit::NSWorkspace;

/// New cards fading in, removed cards fading out.
pub const APPEAR_SECS: f64 = 0.22;
/// A card turning into frosted glass and back (the clip's crossfade on drop).
pub const GLASS_SECS: f64 = 0.15;
/// The column under a dragged card tinting in or out.
pub const COLUMN_TINT_SECS: f64 = 0.15;
/// A card morphing into the task modal, and back.
pub const MODAL_OPEN_SECS: f64 = 0.42;
pub const MODAL_CLOSE_SECS: f64 = 0.34;
/// The task modal's size when the window has room for it.
pub const MODAL_SIZE: (f64, f64) = (600.0, 480.0);
/// A timer digit rolling to its next value.
pub const DIGIT_SECS: f64 = 0.25;

/// Cubic bezier control points for moves and resizes: fast start, soft landing
/// with a hint of overshoot-free spring.
pub const SPRING_CURVE: [f32; 4] = [0.2, 0.9, 0.25, 1.0];

/// System setting "Reduce motion": cross-fade instead of moving things around.
pub fn reduce_motion() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}
