//! Windows input capture via Raw Input and injection via SendInput.
//!
//! Design note: Windows Raw Input has a single-window-per-device-class constraint.
//! Only one window can receive WM_INPUT messages for a given device class
//! (mouse, keyboard, etc.) with RIDEV_INPUTSINK at a time. If another application
//! registers for the same device class, our registration is superseded.
//! This is a fundamental Windows limitation that cannot be worked around.

use kani_proto::event::{EventType, MouseButton};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};

/// Wrapper around HWND that is Send + Sync.
///
/// Windows HWND values are plain handles (opaque pointers) that are valid across threads.
/// The `windows` crate does not implement `Send`/`Sync` for `HWND` because raw pointers
/// are `!Send` by default, but HWNDs are safe to pass between threads in practice —
/// `PostMessageW` is explicitly designed for cross-thread message posting.
#[derive(Clone, Copy)]
pub(super) struct SendHwnd(pub HWND);

// SAFETY: HWND is a Windows handle (opaque integer) that can safely be used from any thread.
// PostMessageW, which is the only operation we perform from another thread, is thread-safe.
unsafe impl Send for SendHwnd {}
unsafe impl Sync for SendHwnd {}

use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT,
};
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RIDEV_INPUTSINK, RID_INPUT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostQuitMessage,
    RegisterClassW, TranslateMessage, CS_HREDRAW, CS_VREDRAW, HWND_MESSAGE, MSG, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_DESTROY, WM_INPUT, WNDCLASSW,
};

/// Start capturing input events via Raw Input on a dedicated thread.
///
/// The `hwnd_out` parameter receives the HWND of the message-only window created
/// by the capture thread. This allows the caller to post `WM_CLOSE` to unblock
/// `GetMessageW` when stopping capture.
pub(super) fn start_capture(
    running: Arc<AtomicBool>,
    hwnd_out: Arc<Mutex<Option<SendHwnd>>>,
) -> (mpsc::Receiver<EventType>, std::thread::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(256);

    let handle = std::thread::spawn(move || {
        run_raw_input_loop(tx, running, hwnd_out);
    });

    (rx, handle)
}

fn run_raw_input_loop(
    tx: mpsc::Sender<EventType>,
    running: Arc<AtomicBool>,
    hwnd_out: Arc<Mutex<Option<SendHwnd>>>,
) {
    unsafe {
        // Register a minimal window class for the message-only window.
        let class_name = wide_string("KaniRawInputClass");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(raw_input_wnd_proc),
            hInstance: windows::Win32::Foundation::HINSTANCE::default(),
            lpszClassName: windows::core::PCWSTR(class_name.as_ptr()),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            windows::core::PCWSTR(class_name.as_ptr()),
            windows::core::PCWSTR::null(),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(wc.hInstance),
            None,
        );

        let hwnd = match hwnd {
            Ok(h) => h,
            Err(_) => return,
        };

        // Publish the HWND so stop_capture() can post WM_CLOSE to unblock GetMessageW.
        *hwnd_out.lock().unwrap() = Some(SendHwnd(hwnd));

        // Register for raw mouse + keyboard input.
        let devices = [
            RAWINPUTDEVICE {
                usUsagePage: 0x01, // HID_USAGE_PAGE_GENERIC
                usUsage: 0x02,     // HID_USAGE_GENERIC_MOUSE
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: hwnd,
            },
            RAWINPUTDEVICE {
                usUsagePage: 0x01,
                usUsage: 0x06, // HID_USAGE_GENERIC_KEYBOARD
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: hwnd,
            },
        ];

        let _ = RegisterRawInputDevices(&devices, std::mem::size_of::<RAWINPUTDEVICE>() as u32);

        // Store sender in thread-local for the window proc.
        RAW_INPUT_TX.with(|cell| {
            let tx_box = Box::new(tx.clone());
            cell.set(Box::into_raw(tx_box) as usize);
        });

        // Share the channel with the keyboard hook so it can capture keys
        // when GRABBING is true. Raw Input keyboard registration is superseded
        // by egui/winit's own registration, so the hook is the only reliable
        // keyboard capture path.
        super::grab::set_keyboard_tx(&tx);

        // Install low-level hooks for input grab on this thread (has message pump).
        super::grab::install_hooks();

        // Message loop.
        let mut msg = MSG::default();
        while running.load(Ordering::Relaxed) {
            let ret = GetMessageW(&mut msg, None, 0, 0);
            if ret.0 <= 0 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Uninstall low-level hooks before cleanup.
        super::grab::uninstall_hooks();

        // Cleanup sender.
        RAW_INPUT_TX.with(|cell| {
            let ptr = cell.get();
            if ptr != 0 {
                let _ = Box::from_raw(ptr as *mut mpsc::Sender<EventType>);
                cell.set(0);
            }
        });
    }
}

thread_local! {
    static RAW_INPUT_TX: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

unsafe extern "system" fn raw_input_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_INPUT => {
            process_raw_input(HRAWINPUT(lparam.0 as _));
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe fn process_raw_input(hraw: HRAWINPUT) {
    let mut size: u32 = 0;
    let header_size = std::mem::size_of::<RAWINPUTHEADER>() as u32;

    let _ = GetRawInputData(hraw, RID_INPUT, None, &mut size, header_size);

    if size == 0 {
        return;
    }

    let mut buffer = vec![0u8; size as usize];
    let copied = GetRawInputData(
        hraw,
        RID_INPUT,
        Some(buffer.as_mut_ptr() as *mut _),
        &mut size,
        header_size,
    );

    if copied == u32::MAX {
        return;
    }

    let raw = &*(buffer.as_ptr() as *const RAWINPUT);

    RAW_INPUT_TX.with(|cell| {
        let ptr = cell.get();
        if ptr == 0 {
            return;
        }
        let tx = &*(ptr as *const mpsc::Sender<EventType>);

        let event = convert_raw_input(raw);
        if let Some(evt) = event {
            let _ = tx.try_send(evt);
        }
    });
}

unsafe fn convert_raw_input(raw: &RAWINPUT) -> Option<EventType> {
    match raw.header.dwType {
        0 => {
            // RIM_TYPEMOUSE
            let mouse = raw.data.mouse;
            let dx = mouse.lLastX as f64;
            let dy = mouse.lLastY as f64;

            let flags = mouse.Anonymous.Anonymous.usButtonFlags;
            if flags & 0x0001 != 0 {
                // RI_MOUSE_LEFT_BUTTON_DOWN
                return Some(EventType::MouseClick {
                    button: MouseButton::Left,
                    pressed: true,
                });
            }
            if flags & 0x0002 != 0 {
                // RI_MOUSE_LEFT_BUTTON_UP
                return Some(EventType::MouseClick {
                    button: MouseButton::Left,
                    pressed: false,
                });
            }
            if flags & 0x0004 != 0 {
                // RI_MOUSE_RIGHT_BUTTON_DOWN
                return Some(EventType::MouseClick {
                    button: MouseButton::Right,
                    pressed: true,
                });
            }
            if flags & 0x0008 != 0 {
                // RI_MOUSE_RIGHT_BUTTON_UP
                return Some(EventType::MouseClick {
                    button: MouseButton::Right,
                    pressed: false,
                });
            }
            // RI_MOUSE_WHEEL — vertical scroll
            if flags & 0x0400 != 0 {
                let delta = mouse.Anonymous.Anonymous.usButtonData as i16;
                return Some(EventType::MouseScroll {
                    dx: 0.0,
                    dy: delta as f64 / 120.0, // WHEEL_DELTA = 120 per notch
                });
            }
            // RI_MOUSE_HWHEEL — horizontal scroll
            if flags & 0x0800 != 0 {
                let delta = mouse.Anonymous.Anonymous.usButtonData as i16;
                return Some(EventType::MouseScroll {
                    dx: delta as f64 / 120.0,
                    dy: 0.0,
                });
            }

            // Regular mouse move.
            if dx != 0.0 || dy != 0.0 {
                return Some(EventType::MouseMove { dx, dy });
            }

            None
        }
        1 => {
            // RIM_TYPEKEYBOARD
            let keyboard = raw.data.keyboard;
            let pressed = keyboard.Flags & 1 == 0; // RI_KEY_MAKE = 0, RI_KEY_BREAK = 1
            let is_e0 = keyboard.Flags & 2 != 0; // RI_KEY_E0

            // Resolve generic modifier VKeys to left/right-specific VKeys.
            // Raw Input reports VK_SHIFT (0x10), VK_CONTROL (0x11), VK_MENU (0x12)
            // but the keymap table uses VK_LSHIFT (0xA0), VK_RSHIFT (0xA1), etc.
            let vkey = match keyboard.VKey {
                0x10 => {
                    // VK_SHIFT — cannot use E0 flag (both sides report E0=0).
                    // Use MapVirtualKeyW with the scan code to distinguish.
                    use windows::Win32::UI::Input::KeyboardAndMouse::{
                        MapVirtualKeyW, MAPVK_VSC_TO_VK_EX,
                    };
                    MapVirtualKeyW(keyboard.MakeCode as u32, MAPVK_VSC_TO_VK_EX) as u16
                }
                0x11 => {
                    // VK_CONTROL — E0 = Right Ctrl
                    if is_e0 {
                        0xA3
                    } else {
                        0xA2
                    }
                }
                0x12 => {
                    // VK_MENU (Alt) — E0 = Right Alt
                    if is_e0 {
                        0xA5
                    } else {
                        0xA4
                    }
                }
                other => other,
            };

            // Convert Windows VKey to USB HID Usage Code for cross-platform wire format
            let hid = match kani_proto::keymap::vkey_to_hid(vkey) {
                Some(h) => h,
                None => {
                    tracing::debug!(vkey, "Unmapped VKey, dropping keyboard event");
                    return None;
                }
            };

            Some(EventType::KeyPress {
                hid_usage: hid,
                pressed,
                modifiers: get_current_modifiers(),
            })
        }
        _ => None,
    }
}

/// Query current modifier key state via GetAsyncKeyState.
///
/// Uses GetAsyncKeyState (physical key state) instead of GetKeyState
/// (message-queue state) because the low-level keyboard hook consumes
/// keys before they reach the message queue. GetKeyState would return
/// false for grabbed modifier keys, causing modifier flags to be lost
/// when forwarding keyboard events to remote hosts.
pub(super) fn get_current_modifiers() -> kani_proto::event::ModifierState {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetKeyState};
    let pressed = |vk: i32| -> bool { unsafe { GetAsyncKeyState(vk) < 0 } };
    kani_proto::event::ModifierState {
        shift: pressed(0x10),                             // VK_SHIFT
        ctrl: pressed(0x11),                              // VK_CONTROL
        alt: pressed(0x12),                               // VK_MENU
        meta: pressed(0x5B) || pressed(0x5C),             // VK_LWIN || VK_RWIN
        caps_lock: unsafe { GetKeyState(0x14) & 1 != 0 }, // VK_CAPITAL toggle (toggle state, not async)
    }
}

/// Inject a mouse or keyboard event via SendInput.
pub fn inject_event(event: &EventType) {
    match event {
        EventType::MouseMove { .. } => {
            // MouseMove injection not supported — use Platform::warp_cursor() instead.
            // SendInput with MOUSEEVENTF_MOVE generates Raw Input events that feed
            // back into our own capture loop.
        }
        EventType::MouseClick { button, pressed } => {
            let flags = match (button, pressed) {
                (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
                (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
                (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
                (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
                _ => MOUSEEVENTF_LEFTDOWN,
            };
            let input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: 0,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: super::grab::KANI_INJECTED,
                    },
                },
            };
            unsafe {
                SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
            }
        }
        EventType::KeyPress {
            hid_usage, pressed, ..
        } => {
            // Convert USB HID Usage Code back to Windows VKey for SendInput
            let vkey = match kani_proto::keymap::hid_to_vkey(*hid_usage) {
                Some(v) => v,
                None => {
                    tracing::debug!(hid_usage, "Unmapped HID code, dropping key injection");
                    return;
                }
            };
            let flags = if *pressed {
                Default::default()
            } else {
                KEYEVENTF_KEYUP
            };
            let input = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(vkey),
                        wScan: 0,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: super::grab::KANI_INJECTED,
                    },
                },
            };
            unsafe {
                SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
            }
        }
        EventType::MouseScroll { dx, dy } => {
            // Vertical scroll
            if *dy != 0.0 {
                let input = INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: 0,
                            dy: 0,
                            mouseData: (*dy * 120.0) as i32 as u32,
                            dwFlags: MOUSEEVENTF_WHEEL,
                            time: 0,
                            dwExtraInfo: super::grab::KANI_INJECTED,
                        },
                    },
                };
                unsafe {
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                }
            }
            // Horizontal scroll
            if *dx != 0.0 {
                let input = INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: 0,
                            dy: 0,
                            mouseData: (*dx * 120.0) as i32 as u32,
                            dwFlags: MOUSEEVENTF_HWHEEL,
                            time: 0,
                            dwExtraInfo: super::grab::KANI_INJECTED,
                        },
                    },
                };
                unsafe {
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                }
            }
        }
        _ => {}
    }
}

/// Helper to create a null-terminated wide string for Win32 APIs.
fn wide_string(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
