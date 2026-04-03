//! macOS Input Capture + Injection PoC
//!
//! Validates CGEventTap (capture), CGEventPost (injection), and multi-display
//! coordinate handling on macOS, including the permission model.

use core_foundation::runloop::{kCFRunLoopCommonModes, kCFRunLoopDefaultMode, CFRunLoop};
use core_graphics::display::{CGDirectDisplayID, CGDisplay, CGPoint, CGRect};
use core_graphics::event::{
    CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, CGKeyCode, CGMouseButton, EventField,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

// --- Raw FFI bindings for APIs not exposed by the core-graphics crate ---

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
    fn CGPreflightPostEventAccess() -> bool;
    fn CGRequestPostEventAccess() -> bool;
    fn CGAssociateMouseAndMouseCursorPosition(connected: bool) -> i32;
    fn CGWarpMouseCursorPosition(newCursorPosition: CGPoint) -> i32;
    fn CGGetActiveDisplayList(
        max_displays: u32,
        active_displays: *mut CGDirectDisplayID,
        display_count: *mut u32,
    ) -> i32;
    fn CGDisplayBounds(display: CGDirectDisplayID) -> CGRect;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

// Atomic counters for event capture stats
static MOUSE_MOVE_COUNT: AtomicU64 = AtomicU64::new(0);
static MOUSE_CLICK_COUNT: AtomicU64 = AtomicU64::new(0);
static KEY_DOWN_COUNT: AtomicU64 = AtomicU64::new(0);
static KEY_UP_COUNT: AtomicU64 = AtomicU64::new(0);
static OTHER_COUNT: AtomicU64 = AtomicU64::new(0);

fn main() {
    let args: Vec<String> = env::args().collect();
    let subcommand = args.get(1).map(|s| s.as_str()).unwrap_or("all");

    match subcommand {
        "permissions" => {
            check_permissions(true);
        }
        "capture" => {
            check_permissions(false);
            run_capture();
        }
        "inject" => {
            check_permissions(false);
            run_inject();
        }
        "displays" => {
            check_permissions(true);
            println!();
            run_displays();
        }
        "all" => {
            check_permissions(false);
            println!();
            run_displays();
            println!();
            run_inject();
            println!();
            run_capture();
        }
        other => {
            eprintln!("Unknown subcommand: {}", other);
            eprintln!("Usage: macos-input-poc [permissions|capture|inject|displays|all]");
            std::process::exit(1);
        }
    }
}

/// Check and report macOS input permissions.
/// If `report_only` is true, just print status and return.
/// Otherwise, request missing permissions and exit if all are denied.
fn check_permissions(report_only: bool) {
    let listen = unsafe { CGPreflightListenEventAccess() };
    let post = unsafe { CGPreflightPostEventAccess() };
    let accessibility = unsafe { AXIsProcessTrusted() };

    let check = |v: bool| if v { "\u{2713}" } else { "\u{2717}" };

    println!("[permissions] ListenEvent: {}", check(listen));
    println!("[permissions] PostEvent: {}", check(post));
    println!("[permissions] Accessibility: {}", check(accessibility));

    if report_only {
        return;
    }

    if !listen {
        println!();
        println!("[permissions] Requesting ListenEvent access...");
        let requested = unsafe { CGRequestListenEventAccess() };
        if !requested {
            println!(
                "[permissions] ListenEvent access denied. \
                 Enable in System Settings > Privacy & Security > Input Monitoring."
            );
        }
    }

    if !post {
        println!();
        println!("[permissions] Requesting PostEvent access...");
        let requested = unsafe { CGRequestPostEventAccess() };
        if !requested {
            println!(
                "[permissions] PostEvent access denied. \
                 Enable in System Settings > Privacy & Security > Accessibility."
            );
        }
    }

    if !listen && !post && !accessibility {
        println!();
        eprintln!(
            "[permissions] All permissions denied. \
             Please enable this app in System Settings > Privacy & Security \
             (Input Monitoring + Accessibility) and re-run."
        );
        std::process::exit(1);
    }
}

/// Enumerate all active displays and print their Quartz coordinates.
fn run_displays() {
    println!("=== Display Enumeration ===");

    // First call to get count
    let mut display_count: u32 = 0;
    let err = unsafe { CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut display_count) };
    if err != 0 {
        eprintln!("[displays] CGGetActiveDisplayList failed (count query): error {}", err);
        return;
    }
    println!("[displays] Found {} active display(s)", display_count);

    if display_count == 0 {
        return;
    }

    // Second call to get IDs
    let mut display_ids = vec![0u32; display_count as usize];
    let err = unsafe {
        CGGetActiveDisplayList(display_count, display_ids.as_mut_ptr(), &mut display_count)
    };
    if err != 0 {
        eprintln!("[displays] CGGetActiveDisplayList failed (list query): error {}", err);
        return;
    }

    for (i, &display_id) in display_ids.iter().enumerate() {
        let bounds: CGRect = unsafe { CGDisplayBounds(display_id) };
        let display = CGDisplay::new(display_id);
        let main = if display_id == CGDisplay::main().id {
            " (main)"
        } else {
            ""
        };

        // CGDisplayPixelsWide/High gives the pixel dimensions.
        // The ratio vs bounds gives the effective scale factor.
        let pixel_w = display.pixels_wide() as f64;
        let point_w = bounds.size.width;
        let scale = if point_w > 0.0 {
            pixel_w / point_w
        } else {
            1.0
        };

        println!(
            "[displays] Display #{} (id=0x{:08x}){}:",
            i, display_id, main
        );
        println!(
            "           origin: ({}, {}), size: {} x {} points",
            bounds.origin.x, bounds.origin.y, bounds.size.width, bounds.size.height
        );
        println!(
            "           pixels: {} x {}, scale factor: {:.1}x",
            display.pixels_wide(),
            display.pixels_high(),
            scale
        );

        // Warp cursor to display center
        let center = CGPoint::new(
            bounds.origin.x + bounds.size.width / 2.0,
            bounds.origin.y + bounds.size.height / 2.0,
        );
        let err = unsafe { CGWarpMouseCursorPosition(center) };
        if err == 0 {
            println!(
                "           Warped cursor to center ({:.0}, {:.0})",
                center.x, center.y
            );
        } else {
            eprintln!("           Failed to warp cursor: error {}", err);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Capture mouse + keyboard events for 10 seconds via CGEventTap.
fn run_capture() {
    println!("=== Event Capture (10 seconds) ===");
    println!("[capture] Creating event tap for mouse + keyboard events...");

    // Reset counters
    MOUSE_MOVE_COUNT.store(0, Ordering::Relaxed);
    MOUSE_CLICK_COUNT.store(0, Ordering::Relaxed);
    KEY_DOWN_COUNT.store(0, Ordering::Relaxed);
    KEY_UP_COUNT.store(0, Ordering::Relaxed);
    OTHER_COUNT.store(0, Ordering::Relaxed);

    let event_types = vec![
        CGEventType::MouseMoved,
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::LeftMouseDragged,
        CGEventType::RightMouseDragged,
        CGEventType::KeyDown,
        CGEventType::KeyUp,
    ];

    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        event_types,
        event_tap_callback,
    );

    let tap = match tap {
        Ok(tap) => tap,
        Err(_) => {
            eprintln!(
                "[capture] CGEventTapCreate returned NULL. \
                 Input Monitoring permission denied."
            );
            eprintln!(
                "[capture] Enable in System Settings > Privacy & Security > Input Monitoring."
            );
            return;
        }
    };

    let loop_source = tap
        .mach_port
        .create_runloop_source(0)
        .expect("Failed to create run loop source");

    let run_loop = CFRunLoop::get_current();
    unsafe {
        run_loop.add_source(&loop_source, kCFRunLoopCommonModes);
    }

    tap.enable();
    println!("[capture] Tap active. Move mouse and press keys...");

    let start = Instant::now();
    let duration = Duration::from_secs(10);

    // Run the event loop in short increments so we can check elapsed time
    while start.elapsed() < duration {
        let remaining = duration.saturating_sub(start.elapsed());
        let run_for = remaining.min(Duration::from_millis(500));
        CFRunLoop::run_in_mode(
            unsafe { kCFRunLoopDefaultMode },
            run_for,
            false,
        );
    }

    println!();
    println!("[capture] === Summary ===");
    println!(
        "[capture] Mouse moves:  {}",
        MOUSE_MOVE_COUNT.load(Ordering::Relaxed)
    );
    println!(
        "[capture] Mouse clicks: {}",
        MOUSE_CLICK_COUNT.load(Ordering::Relaxed)
    );
    println!(
        "[capture] Key downs:    {}",
        KEY_DOWN_COUNT.load(Ordering::Relaxed)
    );
    println!(
        "[capture] Key ups:      {}",
        KEY_UP_COUNT.load(Ordering::Relaxed)
    );
    println!(
        "[capture] Other:        {}",
        OTHER_COUNT.load(Ordering::Relaxed)
    );
}

/// Callback invoked for each captured event.
fn event_tap_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: &CGEvent,
) -> Option<CGEvent> {
    let location = event.location();

    match event_type {
        CGEventType::MouseMoved
        | CGEventType::LeftMouseDragged
        | CGEventType::RightMouseDragged => {
            let count = MOUSE_MOVE_COUNT.fetch_add(1, Ordering::Relaxed);
            // Only log every 50th mouse move to avoid flooding
            if count % 50 == 0 {
                println!(
                    "[capture] MouseMove #{}: ({:.1}, {:.1})",
                    count, location.x, location.y
                );
            }
        }
        CGEventType::LeftMouseDown
        | CGEventType::LeftMouseUp
        | CGEventType::RightMouseDown
        | CGEventType::RightMouseUp => {
            MOUSE_CLICK_COUNT.fetch_add(1, Ordering::Relaxed);
            println!(
                "[capture] MouseClick {:?}: ({:.1}, {:.1})",
                event_type, location.x, location.y
            );
        }
        CGEventType::KeyDown => {
            KEY_DOWN_COUNT.fetch_add(1, Ordering::Relaxed);
            let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            println!(
                "[capture] KeyDown: keycode={}, pos=({:.1}, {:.1})",
                keycode, location.x, location.y
            );
        }
        CGEventType::KeyUp => {
            KEY_UP_COUNT.fetch_add(1, Ordering::Relaxed);
            let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            println!(
                "[capture] KeyUp:   keycode={}, pos=({:.1}, {:.1})",
                keycode, location.x, location.y
            );
        }
        _ => {
            OTHER_COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }

    // ListenOnly tap — return None (we don't modify events)
    None
}

/// Inject synthetic mouse move + keypress via CGEventPost.
fn run_inject() {
    println!("=== Event Injection ===");

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .expect("Failed to create event source");

    // 1. Warp cursor to (100, 100)
    println!("[inject] Warping cursor to (100.0, 100.0)...");
    let err = unsafe { CGWarpMouseCursorPosition(CGPoint::new(100.0, 100.0)) };
    if err != 0 {
        eprintln!("[inject] CGWarpMouseCursorPosition failed: error {}", err);
        return;
    }
    std::thread::sleep(Duration::from_millis(100));

    // 2. Decouple mouse and cursor
    println!("[inject] Decoupling mouse/cursor association...");
    unsafe {
        CGAssociateMouseAndMouseCursorPosition(false);
    }

    // 3. Post synthetic mouse move to (200, 200)
    println!("[inject] Posting synthetic mouse move to (200.0, 200.0)...");
    let move_event = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::MouseMoved,
        CGPoint::new(200.0, 200.0),
        CGMouseButton::Left,
    )
    .expect("Failed to create mouse move event");
    move_event.post(CGEventTapLocation::HID);
    std::thread::sleep(Duration::from_millis(100));

    // 4. Re-associate mouse and cursor (MUST restore)
    println!("[inject] Restoring mouse/cursor association...");
    unsafe {
        CGAssociateMouseAndMouseCursorPosition(true);
    }

    // 5. Post synthetic key press for 'a' (keycode 0)
    //    macOS virtual keycode for 'a' is 0x00
    let keycode_a: CGKeyCode = 0x00;
    println!("[inject] Posting synthetic key press: 'a' (keycode {})...", keycode_a);

    let key_down = CGEvent::new_keyboard_event(source.clone(), keycode_a, true)
        .expect("Failed to create key down event");
    key_down.post(CGEventTapLocation::HID);
    std::thread::sleep(Duration::from_millis(50));

    let key_up = CGEvent::new_keyboard_event(source.clone(), keycode_a, false)
        .expect("Failed to create key up event");
    key_up.post(CGEventTapLocation::HID);

    println!("[inject] Done. Injected: mouse warp to (100,100), mouse move to (200,200), key 'a' down+up.");
}
