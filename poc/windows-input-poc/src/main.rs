//! Windows Input Capture + Injection PoC
//!
//! Validates Raw Input (capture), SendInput (injection), multi-monitor enumeration,
//! and UIPI behavior on Windows.
//!
//! Subcommands: capture, inject, displays, uipi, all

// Design Note: Windows Raw Input requires that each device class (mouse, keyboard, HID)
// has exactly one registered window per process. RegisterRawInputDevices called twice for
// the same usUsagePage + usUsage silently replaces the first registration.

use std::env;
use std::mem;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE,
    RAWINPUTHEADER, RIDEV_INPUTSINK, RID_INPUT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetSystemMetrics, PeekMessageW,
    RegisterClassExW, TranslateMessage, HWND_MESSAGE, MSG, PM_REMOVE, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_DESTROY, WM_INPUT, WNDCLASSEXW,
};

// Atomic counters for capture stats
static MOUSE_EVENT_COUNT: AtomicU64 = AtomicU64::new(0);
static KEY_EVENT_COUNT: AtomicU64 = AtomicU64::new(0);
static OTHER_EVENT_COUNT: AtomicU64 = AtomicU64::new(0);

// RIM_TYPE constants (not always exported as named constants)
const RIM_TYPEMOUSE: u32 = 0;
const RIM_TYPEKEYBOARD: u32 = 1;

fn main() {
    let args: Vec<String> = env::args().collect();
    let subcommand = args.get(1).map(|s| s.as_str()).unwrap_or("all");

    match subcommand {
        "capture" => run_capture(),
        "inject" => run_inject(),
        "displays" => run_displays(),
        "uipi" => run_uipi(),
        "all" => {
            run_displays();
            println!();
            run_inject();
            println!();
            run_uipi();
            println!();
            run_capture();
        }
        other => {
            eprintln!("Unknown subcommand: {}", other);
            eprintln!("Usage: windows-input-poc [capture|inject|displays|uipi|all]");
            std::process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// SendInput normalization — CRITICAL: denominator is (size - 1)
// ---------------------------------------------------------------------------

/// Convert desktop pixel coordinates to SendInput's 0–65535 normalized space.
///
/// The virtual desktop may have a negative origin (e.g., monitor to the left of
/// primary). We subtract the virtual origin and divide by (size − 1) because
/// the 65535 value must map to the last pixel, not one past it.
fn to_normalized(x: f64, y: f64) -> (i32, i32) {
    let virt_left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) } as f64;
    let virt_top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) } as f64;
    let virt_width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) } as f64;
    let virt_height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) } as f64;
    let nx = (((x - virt_left) * 65535.0) / (virt_width - 1.0)).clamp(0.0, 65535.0) as i32;
    let ny = (((y - virt_top) * 65535.0) / (virt_height - 1.0)).clamp(0.0, 65535.0) as i32;
    (nx, ny)
}

// ---------------------------------------------------------------------------
// Subcommand: displays
// ---------------------------------------------------------------------------

fn run_displays() {
    println!("=== Display Enumeration ===");

    // Collect monitors via EnumDisplayMonitors callback
    let mut monitors: Vec<HMONITOR> = Vec::new();
    let monitors_ptr: *mut Vec<HMONITOR> = &mut monitors;

    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(enum_monitor_callback),
            LPARAM(monitors_ptr as isize),
        );
    }

    println!("[displays] Found {} monitor(s)", monitors.len());

    for (i, &hmon) in monitors.iter().enumerate() {
        let mut info = MONITORINFO {
            cbSize: mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };

        let ok = unsafe { GetMonitorInfoW(hmon, &mut info as *mut MONITORINFO) };
        if !ok.as_bool() {
            eprintln!("[displays] GetMonitorInfoW failed for monitor #{}", i);
            continue;
        }

        let rc = info.rcMonitor;
        let work = info.rcWork;
        let primary = if info.dwFlags & 1 != 0 {
            " (primary)"
        } else {
            ""
        };

        println!(
            "[displays] Monitor #{}{}:",
            i, primary
        );
        println!(
            "           rcMonitor: ({}, {}) - ({}, {})  [{}x{}]",
            rc.left,
            rc.top,
            rc.right,
            rc.bottom,
            rc.right - rc.left,
            rc.bottom - rc.top,
        );
        println!(
            "           rcWork:    ({}, {}) - ({}, {})",
            work.left, work.top, work.right, work.bottom,
        );

        // Attempt DPI query. GetDpiForMonitor requires shcore.dll and is
        // not always available in the windows crate features. We call it
        // via raw FFI to avoid a hard dependency.
        attempt_dpi_query(hmon, i);
    }

    // Print virtual desktop metrics
    let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    println!(
        "[displays] Virtual desktop: origin=({}, {}), size={}x{}",
        vx, vy, vw, vh
    );
}

/// Callback for EnumDisplayMonitors — collects HMONITOR handles.
unsafe extern "system" fn enum_monitor_callback(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _lprect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let monitors = &mut *(lparam.0 as *mut Vec<HMONITOR>);
    monitors.push(hmonitor);
    BOOL::from(true)
}

/// Try to call GetDpiForMonitor via LoadLibrary/GetProcAddress.
fn attempt_dpi_query(hmon: HMONITOR, index: usize) {
    // GetDpiForMonitor is in shcore.dll. We load it dynamically so this
    // binary still runs on systems where shcore may not be present (pre-8.1).
    type GetDpiForMonitorFn = unsafe extern "system" fn(
        HMONITOR,
        u32,   // MONITOR_DPI_TYPE: MDT_EFFECTIVE_DPI = 0
        *mut u32,
        *mut u32,
    ) -> i32;

    let lib_name: Vec<u16> = "shcore.dll\0".encode_utf16().collect();
    let fn_name = b"GetDpiForMonitor\0";

    unsafe {
        let hlib = windows::Win32::System::LibraryLoader::LoadLibraryW(
            PCWSTR(lib_name.as_ptr()),
        );
        let hlib = match hlib {
            Ok(h) => h,
            Err(_) => {
                println!("           DPI: shcore.dll not available");
                return;
            }
        };

        let proc = windows::Win32::System::LibraryLoader::GetProcAddress(
            hlib,
            windows::core::PCSTR(fn_name.as_ptr()),
        );
        if let Some(func) = proc {
            let get_dpi: GetDpiForMonitorFn = mem::transmute(func);
            let mut dpi_x: u32 = 0;
            let mut dpi_y: u32 = 0;
            let hr = get_dpi(hmon, 0, &mut dpi_x, &mut dpi_y);
            if hr == 0 {
                println!(
                    "           DPI: {}x{} (scale {:.0}%)",
                    dpi_x,
                    dpi_y,
                    dpi_x as f64 / 96.0 * 100.0,
                );
            } else {
                println!("           DPI: GetDpiForMonitor failed (hr=0x{:08x})", hr);
            }
        } else {
            println!("           DPI: GetDpiForMonitor not found in shcore.dll");
        }

        let _ = windows::Win32::Foundation::FreeLibrary(hlib);
    }
}

// ---------------------------------------------------------------------------
// Subcommand: inject
// ---------------------------------------------------------------------------

fn run_inject() {
    println!("=== Event Injection (SendInput) ===");

    // 1. Mouse: move to absolute position (500, 400) on virtual desktop
    let target_x = 500.0_f64;
    let target_y = 400.0_f64;
    let (nx, ny) = to_normalized(target_x, target_y);
    println!(
        "[inject] Moving mouse to ({}, {}) → normalized ({}, {})",
        target_x, target_y, nx, ny
    );

    let mouse_input = MOUSEINPUT {
        dx: nx,
        dy: ny,
        mouseData: 0,
        dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        time: 0,
        dwExtraInfo: 0,
    };

    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: mouse_input },
    };

    let sent = unsafe { SendInput(&[input], mem::size_of::<INPUT>() as i32) };
    println!("[inject] SendInput (mouse move): returned {}", sent);
    if sent == 0 {
        eprintln!("[inject] SendInput failed for mouse move!");
    }

    std::thread::sleep(Duration::from_millis(100));

    // 2. Keyboard: inject 'A' key via scan code
    //    Scan code for 'A' is 0x1E on standard US keyboard
    let scan_code_a: u16 = 0x1E;
    println!(
        "[inject] Injecting key press: scan code 0x{:02X} ('A')",
        scan_code_a
    );

    let key_down = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0), // 0 = use scan code
                wScan: scan_code_a,
                dwFlags: KEYEVENTF_SCANCODE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    let key_up = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: scan_code_a,
                dwFlags: KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    let sent = unsafe { SendInput(&[key_down, key_up], mem::size_of::<INPUT>() as i32) };
    println!("[inject] SendInput (key A down+up): returned {}", sent);
    if sent == 0 {
        eprintln!("[inject] SendInput failed for key injection!");
    }

    println!("[inject] Done.");
}

// ---------------------------------------------------------------------------
// Subcommand: capture
// ---------------------------------------------------------------------------

fn run_capture() {
    println!("=== Raw Input Capture (10 seconds) ===");

    // Reset counters
    MOUSE_EVENT_COUNT.store(0, Ordering::Relaxed);
    KEY_EVENT_COUNT.store(0, Ordering::Relaxed);
    OTHER_EVENT_COUNT.store(0, Ordering::Relaxed);

    // 1. Register a message-only window class
    let class_name = w!("KaniRawInputSink");

    let wc = WNDCLASSEXW {
        cbSize: mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(raw_input_wnd_proc),
        lpszClassName: class_name,
        ..Default::default()
    };

    let atom = unsafe { RegisterClassExW(&wc) };
    if atom == 0 {
        eprintln!("[capture] RegisterClassExW failed");
        return;
    }

    // 2. Create a hidden message-only window (HWND_MESSAGE parent)
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class_name,
            w!("KaniRawInput"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )
    };

    let hwnd = match hwnd {
        Ok(h) => h,
        Err(e) => {
            eprintln!("[capture] CreateWindowExW failed: {}", e);
            return;
        }
    };

    println!("[capture] Created message-only window: {:?}", hwnd);

    // 3. Register Raw Input for mouse and keyboard with RIDEV_INPUTSINK
    let devices = [
        RAWINPUTDEVICE {
            usUsagePage: 1, // HID_USAGE_PAGE_GENERIC
            usUsage: 2,     // HID_USAGE_GENERIC_MOUSE
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        },
        RAWINPUTDEVICE {
            usUsagePage: 1, // HID_USAGE_PAGE_GENERIC
            usUsage: 6,     // HID_USAGE_GENERIC_KEYBOARD
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        },
    ];

    let result = unsafe {
        RegisterRawInputDevices(&devices, mem::size_of::<RAWINPUTDEVICE>() as u32)
    };

    if let Err(e) = result {
        eprintln!("[capture] RegisterRawInputDevices failed: {}", e);
        return;
    }
    println!("[capture] Registered raw input for mouse + keyboard (RIDEV_INPUTSINK)");
    println!("[capture] Capturing events for 10 seconds...");

    // 4. Message pump with 10-second timeout
    let start = Instant::now();
    let duration = Duration::from_secs(10);

    loop {
        if start.elapsed() >= duration {
            break;
        }

        let mut msg = MSG::default();
        let has_msg = unsafe { PeekMessageW(&mut msg as *mut MSG, Some(hwnd), 0, 0, PM_REMOVE) };

        if has_msg.as_bool() {
            if msg.message == WM_DESTROY {
                break;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        } else {
            // No messages pending — sleep briefly to avoid busy-wait
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    // 5. Summary
    println!();
    println!("[capture] === Summary ===");
    println!(
        "[capture] Mouse events:    {}",
        MOUSE_EVENT_COUNT.load(Ordering::Relaxed)
    );
    println!(
        "[capture] Keyboard events: {}",
        KEY_EVENT_COUNT.load(Ordering::Relaxed)
    );
    println!(
        "[capture] Other events:    {}",
        OTHER_EVENT_COUNT.load(Ordering::Relaxed)
    );
}

/// Window procedure for the raw input message-only window.
unsafe extern "system" fn raw_input_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_INPUT {
        process_raw_input(HRAWINPUT(lparam.0 as *mut _));
        return LRESULT(0);
    }

    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Decode a WM_INPUT message and log the event.
unsafe fn process_raw_input(hraw: HRAWINPUT) {
    let mut size: u32 = 0;
    let header_size = mem::size_of::<RAWINPUTHEADER>() as u32;

    // First call: get required buffer size
    GetRawInputData(
        hraw,
        RID_INPUT,
        None,
        &mut size,
        header_size,
    );

    if size == 0 {
        return;
    }

    // Allocate buffer and retrieve the data
    let mut buffer = vec![0u8; size as usize];
    let copied = GetRawInputData(
        hraw,
        RID_INPUT,
        Some(buffer.as_mut_ptr() as *mut _),
        &mut size,
        header_size,
    );

    if copied == u32::MAX {
        eprintln!("[capture] GetRawInputData failed");
        return;
    }

    let raw = &*(buffer.as_ptr() as *const RAWINPUT);

    match raw.header.dwType {
        RIM_TYPEMOUSE => {
            let mouse = &raw.data.mouse;
            let count = MOUSE_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
            // Log every 50th mouse event to avoid flooding
            if count % 50 == 0 {
                println!(
                    "[capture] Mouse #{}: dx={}, dy={}, flags=0x{:04x}",
                    count, mouse.lLastX, mouse.lLastY, mouse.usFlags.0,
                );
            }
        }
        RIM_TYPEKEYBOARD => {
            let kb = &raw.data.keyboard;
            KEY_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
            println!(
                "[capture] Key: vk=0x{:04X}, scan=0x{:04X}, flags=0x{:04X}, msg=0x{:04X}",
                kb.VKey, kb.MakeCode, kb.Flags, kb.Message,
            );
        }
        _ => {
            OTHER_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------------------
// Subcommand: uipi
// ---------------------------------------------------------------------------

fn run_uipi() {
    println!("=== UIPI (User Interface Privilege Isolation) Test ===");

    // UIPI prevents a lower-integrity process from sending input to a
    // higher-integrity (elevated) process. We test this by:
    // 1. Attempting SendInput (which synthesizes input globally)
    // 2. Checking if our process is elevated
    // 3. Reporting the results

    // Check if we're running elevated
    let elevated = is_elevated();
    println!("[uipi] Process elevated: {}", elevated);

    // Try to inject a benign key event (VK_F24 — rarely used, won't cause side effects)
    let vk_f24: u16 = 0x87; // VK_F24
    let key_down = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk_f24),
                wScan: 0,
                dwFlags: KEYBD_EVENT_FLAGS(0),
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    let key_up = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk_f24),
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    let sent = unsafe { SendInput(&[key_down, key_up], mem::size_of::<INPUT>() as i32) };
    println!("[uipi] SendInput (VK_F24 down+up): returned {}", sent);

    if sent == 2 {
        println!("[uipi] SendInput succeeded — no UIPI block detected");
        if !elevated {
            println!(
                "[uipi] Note: To test UIPI blocking, run an elevated (admin) window \
                 in the foreground and try again from a non-elevated process."
            );
        }
    } else {
        println!("[uipi] SendInput returned {} (expected 2) — possible UIPI block", sent);
        let err = std::io::Error::last_os_error();
        println!("[uipi] Last error: {}", err);
    }

    // Also check the key state to see if the injected event was received
    let state = unsafe { GetKeyState(vk_f24 as i32) };
    println!("[uipi] GetKeyState(VK_F24) after injection: 0x{:04X}", state);
}

/// Check if the current process is running with elevated privileges.
fn is_elevated() -> bool {
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = HANDLE::default();
        let ok = OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token);
        if ok.is_err() {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        );

        let _ = windows::Win32::Foundation::CloseHandle(token);

        ok.is_ok() && elevation.TokenIsElevated != 0
    }
}
