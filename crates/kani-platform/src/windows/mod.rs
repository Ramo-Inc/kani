//! Windows platform implementation using Win32 APIs.

pub mod cursor;
pub mod display;
pub mod grab;
pub mod input;

use crate::types::DisplayInfo;
use crate::Platform;
use input::SendHwnd;
use kani_proto::event::{EventType, ModifierState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{LPARAM, WPARAM};

/// Windows platform implementation.
pub struct WindowsPlatform {
    capture_running: Arc<AtomicBool>,
    capture_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// HWND of the message-only window used by the capture thread.
    /// Stored so that `stop_capture()` can post `WM_CLOSE` to unblock `GetMessageW`.
    capture_hwnd: Arc<Mutex<Option<SendHwnd>>>,
}

impl Default for WindowsPlatform {
    fn default() -> Self {
        Self {
            capture_running: Arc::new(AtomicBool::new(false)),
            capture_handle: Mutex::new(None),
            capture_hwnd: Arc::new(Mutex::new(None)),
        }
    }
}

impl WindowsPlatform {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Drop for WindowsPlatform {
    fn drop(&mut self) {
        self.stop_capture();
    }
}

impl Platform for WindowsPlatform {
    fn enumerate_displays(&self) -> Vec<DisplayInfo> {
        display::enumerate_displays()
    }

    fn start_capture(&self) -> tokio::sync::mpsc::Receiver<EventType> {
        self.stop_capture();
        self.capture_running.store(true, Ordering::Relaxed);
        let (rx, handle) = input::start_capture(
            Arc::clone(&self.capture_running),
            Arc::clone(&self.capture_hwnd),
        );
        *self.capture_handle.lock().unwrap() = Some(handle);
        rx
    }

    fn stop_capture(&self) {
        // Post WM_CLOSE to the capture window to unblock GetMessageW.
        // The window proc handles WM_CLOSE -> DestroyWindow -> WM_DESTROY -> PostQuitMessage,
        // which causes GetMessageW to return 0 and the message loop to exit.
        if let Some(send_hwnd) = self.capture_hwnd.lock().unwrap().take() {
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
                let _ = PostMessageW(Some(send_hwnd.0), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        self.capture_running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.capture_handle.lock().unwrap().take() {
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
        grab::grab_input();
    }

    fn ungrab_input(&self) {
        grab::ungrab_input();
    }

    fn modifier_state(&self) -> ModifierState {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            VK_CAPITAL, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
        };

        let pressed = |vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| -> bool {
            unsafe { GetAsyncKeyState(vk.0 as i32) & (0x8000u16 as i16) != 0 }
        };

        ModifierState {
            shift: pressed(VK_SHIFT),
            ctrl: pressed(VK_CONTROL),
            alt: pressed(VK_MENU),
            meta: pressed(VK_LWIN) || pressed(VK_RWIN),
            caps_lock: unsafe { GetAsyncKeyState(VK_CAPITAL.0 as i32) & 1 != 0 },
        }
    }

    fn check_permissions(&self) -> bool {
        // Windows doesn't require explicit input permissions like macOS.
        // Running as a standard user is sufficient for most input operations.
        // UAC elevation may be needed for certain scenarios but we don't check here.
        true
    }

    fn request_permissions(&self) {
        // No-op on Windows; permissions are handled by the OS via UAC if needed.
    }
}
