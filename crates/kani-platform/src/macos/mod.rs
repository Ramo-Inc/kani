//! macOS platform implementation using Core Graphics event taps.

pub mod cursor;
pub mod display;
pub mod input;
pub mod permissions;
pub mod wake;

use crate::types::DisplayInfo;
use crate::Platform;
use core_graphics::event::CGEventFlags;
use kani_proto::event::{EventType, ModifierState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::debug;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGAssociateMouseAndMouseCursorPosition(connected: bool) -> i32;
}

/// macOS platform implementation.
pub struct MacOSPlatform {
    /// Whether the capture event tap is currently running.
    capture_running: Arc<AtomicBool>,
    /// Join handle for the capture thread, if active.
    capture_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Default for MacOSPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl MacOSPlatform {
    pub fn new() -> Self {
        Self {
            capture_running: Arc::new(AtomicBool::new(false)),
            capture_handle: Mutex::new(None),
        }
    }
}

impl Drop for MacOSPlatform {
    fn drop(&mut self) {
        // Stop capture if running.
        self.stop_capture();
    }
}

impl Platform for MacOSPlatform {
    fn enumerate_displays(&self) -> Vec<DisplayInfo> {
        display::enumerate_displays()
    }

    fn start_capture(&self) -> tokio::sync::mpsc::Receiver<EventType> {
        // Stop any existing capture first.
        self.stop_capture();

        self.capture_running.store(true, Ordering::Relaxed);
        let (rx, handle) = input::start_capture(Arc::clone(&self.capture_running));

        *self.capture_handle.lock().unwrap() = Some(handle);
        rx
    }

    fn stop_capture(&self) {
        self.capture_running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.capture_handle.lock().unwrap().take() {
            debug!("Waiting for capture thread to finish");
            let _ = handle.join();
        }
    }

    fn inject_event(&self, event: &EventType) {
        input::inject_event(event);
    }

    fn warp_cursor(&self, x: f64, y: f64) {
        cursor::warp_cursor(x, y);
    }

    fn cursor_position(&self) -> (f64, f64) {
        cursor::get_cursor_position()
    }

    fn show_cursor(&self) {
        cursor::show_cursor();
    }

    fn hide_cursor(&self) {
        cursor::hide_cursor();
    }

    fn grab_input(&self) {
        input::set_grabbing(true);
        unsafe {
            CGAssociateMouseAndMouseCursorPosition(false);
        }
    }

    fn ungrab_input(&self) {
        input::set_grabbing(false);
        unsafe {
            CGAssociateMouseAndMouseCursorPosition(true);
        }
    }

    fn modifier_state(&self) -> ModifierState {
        // Use CGEventSourceFlagsState to query current modifier flags
        // without creating an event source or event.
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGEventSourceFlagsState(stateID: i32) -> u64;
        }
        // CGEventSourceStateID::HIDSystemState = 1
        let raw_flags = unsafe { CGEventSourceFlagsState(1) };

        // CGEventFlags bit masks (from CGEventTypes.h):
        const ALPHA_SHIFT: u64 = 0x00010000; // caps lock
        const SHIFT: u64 = 0x00020000;
        const CONTROL: u64 = 0x00040000;
        const ALTERNATE: u64 = 0x00080000;
        const COMMAND: u64 = 0x00100000;

        ModifierState {
            shift: raw_flags & SHIFT != 0,
            ctrl: raw_flags & CONTROL != 0,
            alt: raw_flags & ALTERNATE != 0,
            meta: raw_flags & COMMAND != 0,
            caps_lock: raw_flags & ALPHA_SHIFT != 0,
        }
    }

    fn check_permissions(&self) -> bool {
        permissions::check_permissions().all_granted()
    }

    fn request_permissions(&self) {
        permissions::request_permissions();
    }

    fn wake_display(&self) {
        wake::wake_display();
    }
}

/// Convert CGEventFlags to our ModifierState.
pub(crate) fn get_modifier_state_from_flags(flags: CGEventFlags) -> ModifierState {
    ModifierState {
        shift: flags.contains(CGEventFlags::CGEventFlagShift),
        ctrl: flags.contains(CGEventFlags::CGEventFlagControl),
        alt: flags.contains(CGEventFlags::CGEventFlagAlternate),
        meta: flags.contains(CGEventFlags::CGEventFlagCommand),
        caps_lock: flags.contains(CGEventFlags::CGEventFlagAlphaShift),
    }
}

// Mark MacOSPlatform as Send + Sync.
// Safety: All mutable state is behind Mutex or AtomicBool.
unsafe impl Send for MacOSPlatform {}
unsafe impl Sync for MacOSPlatform {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_platform() {
        let platform = crate::create_platform();
        // On headless macOS, displays may be empty
        let displays = platform.enumerate_displays();
        // Just verify it doesn't panic
        println!("Found {} displays", displays.len());
    }

    #[test]
    fn test_permissions_check() {
        let platform = MacOSPlatform::new();
        // On CI/headless, permissions will be false -- that's OK
        let ok = platform.check_permissions();
        println!("Permissions OK: {}", ok);
        // Don't assert true -- we're testing it doesn't panic
    }

    #[test]
    fn test_modifier_state() {
        // Test the FFI call directly to avoid any MacOSPlatform lifecycle issues.
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGEventSourceFlagsState(stateID: i32) -> u64;
        }
        // CGEventSourceStateID::CombinedSessionState = 0 (more reliable in test env)
        let raw_flags = unsafe { CGEventSourceFlagsState(0) };
        // Just verify it returns without hanging/panicking
        println!("Raw modifier flags: 0x{:x}", raw_flags);
    }

    #[test]
    fn test_modifier_state_from_flags() {
        let mods = get_modifier_state_from_flags(CGEventFlags::CGEventFlagNull);
        assert!(!mods.shift);
        assert!(!mods.ctrl);
        assert!(!mods.alt);
        assert!(!mods.meta);
        assert!(!mods.caps_lock);

        let mods = get_modifier_state_from_flags(
            CGEventFlags::CGEventFlagShift | CGEventFlags::CGEventFlagControl,
        );
        assert!(mods.shift);
        assert!(mods.ctrl);
        assert!(!mods.alt);
    }

    #[test]
    fn test_display_info_clone() {
        let info = DisplayInfo {
            id: 1,
            origin_x: 0.0,
            origin_y: 0.0,
            width_logical: 1920.0,
            height_logical: 1080.0,
            width_pixels: 3840,
            height_pixels: 2160,
            scale_factor: 2.0,
            is_primary: true,
        };
        let cloned = info.clone();
        assert_eq!(cloned.id, 1);
        assert_eq!(cloned.scale_factor, 2.0);
        assert!(cloned.is_primary);
    }
}
