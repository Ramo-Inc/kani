//! macOS cursor warping and visibility control.
//!
//! show_cursor/hide_cursor are idempotent — safe to call multiple times.
//! An AtomicBool tracks state to prevent CGDisplay counter drift.

use core_graphics::display::{CGDirectDisplayID, CGDisplay, CGPoint};
use core_graphics::event::CGEventTapLocation;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, error};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWarpMouseCursorPosition(newCursorPosition: CGPoint) -> i32;
    fn CGDisplayShowCursor(display: CGDirectDisplayID) -> i32;
    fn CGDisplayHideCursor(display: CGDirectDisplayID) -> i32;
}

static CURSOR_HIDDEN: AtomicBool = AtomicBool::new(false);

/// Warp the mouse cursor to an absolute position in Quartz (logical) coordinates.
/// Also posts a synthetic mouse event so applications receive move notifications.
/// CGWarpMouseCursorPosition alone does NOT generate events, which breaks
/// window dragging and other move-dependent operations.
pub fn warp_cursor(x: f64, y: f64) {
    let err = unsafe { CGWarpMouseCursorPosition(CGPoint::new(x, y)) };
    if err != 0 {
        error!(x, y, error = err, "CGWarpMouseCursorPosition failed");
        return;
    }
    debug!(x, y, "Warped cursor");
    post_synthetic_move(x, y);
}

/// Post a synthetic mouse event at the given position so applications
/// receive move/drag notifications after a cursor warp.
fn post_synthetic_move(x: f64, y: f64) {
    use core_graphics::event::{CGEvent, CGEventType, CGMouseButton, EventField};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let source = match CGEventSource::new(CGEventSourceStateID::CombinedSessionState) {
        Ok(s) => s,
        Err(_) => return,
    };

    // Check if left or right mouse button is currently held for drag events.
    let event_type = if is_mouse_button_pressed(0) {
        CGEventType::LeftMouseDragged
    } else if is_mouse_button_pressed(1) {
        CGEventType::RightMouseDragged
    } else {
        CGEventType::MouseMoved
    };

    let point = CGPoint::new(x, y);
    if let Ok(evt) = CGEvent::new_mouse_event(source, event_type, point, CGMouseButton::Left) {
        evt.set_integer_value_field(
            EventField::EVENT_SOURCE_USER_DATA,
            super::input::KANI_EVENT_MARKER,
        );
        evt.post(CGEventTapLocation::HID);
    }
}

/// Check if a mouse button is currently pressed using CGEventSourceButtonState.
fn is_mouse_button_pressed(button: u32) -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceButtonState(stateID: i32, button: u32) -> bool;
    }
    // CGEventSourceStateID::CombinedSessionState = 0
    unsafe { CGEventSourceButtonState(0, button) }
}

/// Show the system cursor on the main display. Idempotent — no-op if already visible.
pub fn show_cursor() {
    if CURSOR_HIDDEN.swap(false, Ordering::SeqCst) {
        let main_display = CGDisplay::main().id;
        let err = unsafe { CGDisplayShowCursor(main_display) };
        if err != 0 {
            error!(error = err, "CGDisplayShowCursor failed");
        }
    }
}

/// Hide the system cursor on the main display. Idempotent — no-op if already hidden.
pub fn hide_cursor() {
    if !CURSOR_HIDDEN.swap(true, Ordering::SeqCst) {
        let main_display = CGDisplay::main().id;
        let err = unsafe { CGDisplayHideCursor(main_display) };
        if err != 0 {
            error!(error = err, "CGDisplayHideCursor failed");
        }
    }
}

/// Get the current cursor position in Quartz logical coordinates.
pub fn get_cursor_position() -> (f64, f64) {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    if let Ok(source) = CGEventSource::new(CGEventSourceStateID::CombinedSessionState) {
        if let Ok(event) = CGEvent::new(source) {
            let point = event.location();
            return (point.x, point.y);
        }
    }
    tracing::warn!("CGEvent cursor position query failed, defaulting to (0, 0)");
    (0.0, 0.0)
}
