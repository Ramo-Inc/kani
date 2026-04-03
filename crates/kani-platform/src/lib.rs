pub mod types;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod windows;

use kani_proto::event::{EventType, ModifierState};
use types::DisplayInfo;

/// Platform-specific input/display operations.
pub trait Platform: Send + Sync {
    /// Get all connected displays with their layout info.
    fn enumerate_displays(&self) -> Vec<DisplayInfo>;

    /// Start capturing input events. Returns a receiver.
    fn start_capture(&self) -> tokio::sync::mpsc::Receiver<EventType>;

    /// Stop capturing.
    fn stop_capture(&self);

    /// Inject an input event on this machine.
    fn inject_event(&self, event: &EventType);

    /// Warp cursor to absolute position (logical coordinates).
    fn warp_cursor(&self, x: f64, y: f64);

    /// Get current absolute cursor position.
    /// Windows: physical pixels. macOS: Quartz logical coordinates.
    fn cursor_position(&self) -> (f64, f64);

    /// Show the system cursor.
    fn show_cursor(&self);

    /// Hide the system cursor.
    fn hide_cursor(&self);

    /// Suppress local OS input processing (mouse + keyboard).
    /// Call when cursor transitions to a remote host.
    fn grab_input(&self);

    /// Restore local OS input processing.
    /// Call when cursor returns to local host.
    fn ungrab_input(&self);

    /// Get current modifier key state.
    fn modifier_state(&self) -> ModifierState;

    /// Check platform permissions. Returns true if all OK.
    fn check_permissions(&self) -> bool;

    /// Request missing permissions.
    fn request_permissions(&self);

    /// Wake the display from sleep (e.g., when cursor enters from a remote host).
    /// Default: no-op. macOS overrides with IOPMAssertionDeclareUserActivity.
    fn wake_display(&self) {}
}

/// Create the platform implementation for the current OS.
pub fn create_platform() -> Box<dyn Platform> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacOSPlatform::new())
    }

    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WindowsPlatform::new())
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        compile_error!("Unsupported platform")
    }
}
