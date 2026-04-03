//! Windows cursor control via SetCursorPos and ShowCursor.
//!
//! show_cursor/hide_cursor are idempotent — safe to call multiple times.
//! An AtomicBool tracks state to prevent ShowCursor counter drift.

use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos, ShowCursor};

static CURSOR_HIDDEN: AtomicBool = AtomicBool::new(false);

/// Warp the mouse cursor to an absolute screen position (physical pixels).
pub fn warp_cursor(x: f64, y: f64) {
    unsafe {
        let _ = SetCursorPos(x as i32, y as i32);
    }
}

/// Show the system cursor. Idempotent — no-op if already visible.
pub fn show_cursor() {
    if CURSOR_HIDDEN.swap(false, Ordering::SeqCst) {
        unsafe {
            ShowCursor(true);
        }
    }
}

/// Hide the system cursor. Idempotent — no-op if already hidden.
pub fn hide_cursor() {
    if !CURSOR_HIDDEN.swap(true, Ordering::SeqCst) {
        unsafe {
            ShowCursor(false);
        }
    }
}

/// Get the current cursor position in physical screen coordinates.
/// Calls ensure_dpi_awareness() to guarantee correct coordinates.
pub fn get_cursor_position() -> (f64, f64) {
    super::display::ensure_dpi_awareness();
    let mut point = POINT::default();
    unsafe {
        if let Err(e) = GetCursorPos(&mut point) {
            tracing::warn!(error = %e, "GetCursorPos failed, defaulting to (0, 0)");
        }
    }
    (point.x as f64, point.y as f64)
}
