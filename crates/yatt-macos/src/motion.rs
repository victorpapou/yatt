//! Every duration and curve the UI animates with, in one place. Card motion is
//! spring-driven instead; its constants live in `physics.rs`.

use objc2_app_kit::NSWorkspace;

/// New cards fading in, removed cards fading out.
pub const APPEAR_SECS: f64 = 0.22;
/// A card's look changing (blurred content and target label while leading a drag).
pub const GLASS_SECS: f64 = 0.15;
/// Any other visual property that changes in place: fill, ring, shadow, text.
pub const SOFTEN_SECS: f64 = 0.15;
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

/// Makes the next property changes on `view`'s layer (colour, border, shadow, text)
/// ease in over [`SOFTEN_SECS`] instead of jumping. AppKit disables implicit layer
/// animations on views, so this adds an explicit fade of the layer's own render.
pub fn soften(view: &objc2_app_kit::NSView) {
    let Some(layer) = view.layer() else { return };
    let t = objc2_quartz_core::CATransition::animation();
    unsafe {
        t.setType(objc2_quartz_core::kCATransitionFade);
        t.setTimingFunction(Some(
            &objc2_quartz_core::CAMediaTimingFunction::functionWithName(
                objc2_quartz_core::kCAMediaTimingFunctionEaseOut,
            ),
        ));
    }
    objc2_quartz_core::CAMediaTiming::setDuration(&*t, SOFTEN_SECS);
    layer.addAnimation_forKey(&t, Some(objc2_foundation::ns_string!("soften")));
}

/// Runs `f` with Core Animation's implicit actions off, so moving a card's view
/// between parents or reordering siblings shows no fade or flash.
pub fn quietly(f: impl FnOnce()) {
    objc2_quartz_core::CATransaction::begin();
    objc2_quartz_core::CATransaction::setDisableActions(true);
    f();
    objc2_quartz_core::CATransaction::commit();
}

/// System setting "Reduce motion": cross-fade instead of moving things around.
pub fn reduce_motion() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}
