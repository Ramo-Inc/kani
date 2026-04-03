//! Windows input grab via Low-level Hooks (WH_MOUSE_LL + WH_KEYBOARD_LL).
//!
//! Hooks are registered once on the Raw Input thread and stay active for
//! the process lifetime. The GRABBING flag controls whether events are
//! consumed (blocked from OS) or passed through.
//!
//! IMPORTANT: When GRABBING is true, keyboard events are captured here
//! (not via Raw Input) because egui/winit supersedes the keyboard Raw Input
//! registration. The hook is guaranteed to fire regardless.

use kani_proto::event::EventType;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, MSLLHOOKSTRUCT,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// When true, hook callbacks consume events (block local OS processing).
static GRABBING: AtomicBool = AtomicBool::new(false);

/// Marker value set in dwExtraInfo by inject_event to identify self-injected events.
pub const KANI_INJECTED: usize = 0x4B414E49; // "KANI" in ASCII

/// Self-tracked modifier state for the keyboard hook.
///
/// When GRABBING is true, consumed keys are NOT reflected in Windows' key state
/// tables (GetKeyState/GetAsyncKeyState both return stale data). We must track
/// modifier state ourselves based on key-down/key-up events seen by the hook.
mod hook_modifiers {
    use std::sync::atomic::{AtomicBool, Ordering};

    static SHIFT: AtomicBool = AtomicBool::new(false);
    static CTRL: AtomicBool = AtomicBool::new(false);
    static ALT: AtomicBool = AtomicBool::new(false);
    static META: AtomicBool = AtomicBool::new(false);

    pub fn update(vkey: u16, pressed: bool) {
        match vkey {
            0xA0 | 0xA1 | 0x10 => SHIFT.store(pressed, Ordering::Relaxed), // VK_LSHIFT/RSHIFT/SHIFT
            0xA2 | 0xA3 | 0x11 => CTRL.store(pressed, Ordering::Relaxed), // VK_LCONTROL/RCONTROL/CONTROL
            0xA4 | 0xA5 | 0x12 => ALT.store(pressed, Ordering::Relaxed),  // VK_LMENU/RMENU/MENU
            0x5B | 0x5C => META.store(pressed, Ordering::Relaxed),        // VK_LWIN/RWIN
            _ => {}
        }
    }

    pub fn get() -> kani_proto::event::ModifierState {
        kani_proto::event::ModifierState {
            shift: SHIFT.load(Ordering::Relaxed),
            ctrl: CTRL.load(Ordering::Relaxed),
            alt: ALT.load(Ordering::Relaxed),
            meta: META.load(Ordering::Relaxed),
            caps_lock: false, // CapsLock toggle state not tracked here; acceptable for remote injection
        }
    }

    pub fn reset() {
        SHIFT.store(false, Ordering::Relaxed);
        CTRL.store(false, Ordering::Relaxed);
        ALT.store(false, Ordering::Relaxed);
        META.store(false, Ordering::Relaxed);
    }
}

thread_local! {
    static MOUSE_HOOK: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
    static KEYBOARD_HOOK: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
    /// Channel sender for keyboard events captured by the hook.
    static KEYBOARD_TX: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub fn grab_input() {
    GRABBING.store(true, Ordering::Relaxed);
}

pub fn ungrab_input() {
    GRABBING.store(false, Ordering::Relaxed);
    hook_modifiers::reset(); // Clear tracked state to avoid stale modifiers on next grab
}

/// Set the keyboard capture channel. Must be called from the hook thread
/// before install_hooks, so the keyboard hook can forward events.
pub fn set_keyboard_tx(tx: &mpsc::Sender<EventType>) {
    KEYBOARD_TX.with(|cell| {
        let tx_box = Box::new(tx.clone());
        cell.set(Box::into_raw(tx_box) as usize);
    });
}

/// Install hooks on the current thread. Must be called from a thread with a message pump.
pub fn install_hooks() {
    unsafe {
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), None, 0)
            .expect("Failed to install mouse hook");
        let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), None, 0)
            .expect("Failed to install keyboard hook");

        MOUSE_HOOK.with(|h| h.set(mouse.0 as isize));
        KEYBOARD_HOOK.with(|h| h.set(keyboard.0 as isize));
    }
}

/// Uninstall hooks. Call from the same thread that installed them.
pub fn uninstall_hooks() {
    MOUSE_HOOK.with(|h| {
        let raw = h.get();
        if raw != 0 {
            unsafe {
                let _ = UnhookWindowsHookEx(HHOOK(raw as *mut _));
            }
            h.set(0);
        }
    });
    KEYBOARD_HOOK.with(|h| {
        let raw = h.get();
        if raw != 0 {
            unsafe {
                let _ = UnhookWindowsHookEx(HHOOK(raw as *mut _));
            }
            h.set(0);
        }
    });
    // Cleanup keyboard TX
    KEYBOARD_TX.with(|cell| {
        let ptr = cell.get();
        if ptr != 0 {
            unsafe {
                let _ = Box::from_raw(ptr as *mut mpsc::Sender<EventType>);
            }
            cell.set(0);
        }
    });
}

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    // Allow self-injected events (from inject_event via SendInput)
    let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
    if info.dwExtraInfo == KANI_INJECTED {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    if GRABBING.load(Ordering::Relaxed) {
        return LRESULT(1); // Consume -- don't pass to OS
    }

    CallNextHookEx(None, code, wparam, lparam)
}

unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    if info.dwExtraInfo == KANI_INJECTED {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    if GRABBING.load(Ordering::Relaxed) {
        // Capture keyboard event via the hook and send to channel.
        // Raw Input keyboard registration is superseded by egui/winit,
        // so the hook is the only reliable way to capture keyboard when grabbing.
        capture_key_from_hook(wparam, info);
        return LRESULT(1);
    }

    CallNextHookEx(None, code, wparam, lparam)
}

/// Convert hook data to EventType::KeyPress and send to the capture channel.
fn capture_key_from_hook(wparam: WPARAM, info: &KBDLLHOOKSTRUCT) {
    let msg = wparam.0 as u32;
    let pressed = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    if msg != WM_KEYDOWN && msg != WM_KEYUP && msg != WM_SYSKEYDOWN && msg != WM_SYSKEYUP {
        return;
    }

    let vkey = info.vkCode as u16;

    // Resolve generic modifier VKeys to left/right-specific VKeys.
    let is_extended = info.flags.0 & 0x01 != 0; // LLKHF_EXTENDED
    let resolved_vkey = match vkey {
        0x10 => {
            // VK_SHIFT — use scan code to distinguish left/right
            use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW, MAPVK_VSC_TO_VK_EX};
            unsafe { MapVirtualKeyW(info.scanCode, MAPVK_VSC_TO_VK_EX) as u16 }
        }
        0x11 => {
            // VK_CONTROL — extended = Right Ctrl
            if is_extended {
                0xA3
            } else {
                0xA2
            }
        }
        0x12 => {
            // VK_MENU (Alt) — extended = Right Alt
            if is_extended {
                0xA5
            } else {
                0xA4
            }
        }
        other => other,
    };

    // Update self-tracked modifier state BEFORE reading it.
    // This ensures the current key's state is reflected in the modifiers.
    hook_modifiers::update(resolved_vkey, pressed);

    // Convert to HID
    let hid = match kani_proto::keymap::vkey_to_hid(resolved_vkey) {
        Some(h) => h,
        None => {
            tracing::debug!(vkey = resolved_vkey, "Hook: unmapped VKey, dropping");
            return;
        }
    };

    // Use self-tracked modifiers (Windows APIs return stale data for consumed keys)
    let modifiers = hook_modifiers::get();

    let evt = EventType::KeyPress {
        hid_usage: hid,
        pressed,
        modifiers,
    };

    KEYBOARD_TX.with(|cell| {
        let ptr = cell.get();
        if ptr == 0 {
            return;
        }
        let tx = unsafe { &*(ptr as *const mpsc::Sender<EventType>) };
        let _ = tx.try_send(evt);
    });
}
