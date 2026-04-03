//! macOS input capture via CGEventTap and injection via CGEventPost.

use core_foundation::runloop::{kCFRunLoopCommonModes, kCFRunLoopDefaultMode, CFRunLoop};
use core_graphics::event::{
    CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, CGKeyCode, CGMouseButton, EventField,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use kani_proto::event::{EventType, MouseButton};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

/// When true, the event tap callback nullifies events (blocks local OS processing).
static GRABBING: AtomicBool = AtomicBool::new(false);

/// Marker value set on injected events so the event tap can skip them.
pub(super) const KANI_EVENT_MARKER: i64 = 0x4B414E49; // "KANI" in ASCII

/// Set the grabbing state. When true, captured events are consumed (not delivered to OS).
pub fn set_grabbing(grab: bool) {
    GRABBING.store(grab, Ordering::Relaxed);
}

// FFI for re-enabling a tap that macOS auto-disabled due to timeout.
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapEnable(tap: *mut std::ffi::c_void, enable: bool);
}

// Store the tap's CFMachPortRef as a raw pointer so the callback can re-enable the tap.
thread_local! {
    static TAP_MACH_PORT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Start capturing input events via a CGEventTap running on a dedicated thread.
///
/// Returns a receiver for captured events and a join handle for the capture thread.
/// Set `running` to false to stop the capture loop.
pub fn start_capture(
    running: Arc<AtomicBool>,
) -> (mpsc::Receiver<EventType>, std::thread::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(256);

    let handle = std::thread::spawn(move || {
        run_event_tap(tx, running);
    });

    (rx, handle)
}

fn run_event_tap(tx: mpsc::Sender<EventType>, running: Arc<AtomicBool>) {
    let event_types = vec![
        CGEventType::MouseMoved,
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::LeftMouseDragged,
        CGEventType::RightMouseDragged,
        CGEventType::ScrollWheel,
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
    ];

    // We need to leak the sender into a static so the callback can access it.
    // This is safe because we only create one tap at a time and the thread
    // owns the lifecycle.
    let tx_box = Box::new(tx);
    let tx_ptr = Box::into_raw(tx_box);

    // Store the sender pointer in a thread-local so the callback can access it.
    CAPTURE_TX.with(|cell| {
        cell.set(tx_ptr as usize);
    });

    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        event_types,
        event_tap_callback,
    );

    let tap = match tap {
        Ok(tap) => tap,
        Err(_) => {
            error!(
                "CGEventTapCreate returned NULL. Input Monitoring permission likely denied. \
                 Enable in System Settings > Privacy & Security > Input Monitoring."
            );
            // Reclaim the sender
            unsafe {
                let _ = Box::from_raw(tx_ptr);
            }
            return;
        }
    };

    // Store the mach port reference so the callback can re-enable the tap
    // if macOS auto-disables it due to a TapDisabledByTimeout event.
    use core_foundation::base::TCFType;
    TAP_MACH_PORT.with(|cell| {
        cell.set(tap.mach_port.as_concrete_TypeRef() as usize);
    });

    let loop_source = tap
        .mach_port
        .create_runloop_source(0)
        .expect("Failed to create run loop source from event tap mach port");

    let run_loop = CFRunLoop::get_current();
    unsafe {
        run_loop.add_source(&loop_source, kCFRunLoopCommonModes);
    }
    tap.enable();

    debug!("macOS event tap started");

    // Run in short increments so we can check the running flag.
    while running.load(Ordering::Relaxed) {
        CFRunLoop::run_in_mode(
            unsafe { kCFRunLoopDefaultMode },
            std::time::Duration::from_millis(100),
            false,
        );
    }

    debug!("macOS event tap stopping");

    // Reclaim the sender to drop it, which closes the channel.
    unsafe {
        let _ = Box::from_raw(tx_ptr);
    }
    CAPTURE_TX.with(|cell| {
        cell.set(0);
    });
    TAP_MACH_PORT.with(|cell| {
        cell.set(0);
    });
}

thread_local! {
    static CAPTURE_TX: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn event_tap_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: &CGEvent,
) -> Option<CGEvent> {
    // Handle tap auto-disabled by macOS timeout (slow callback detection).
    if matches!(event_type, CGEventType::TapDisabledByTimeout) {
        warn!("Event tap disabled by timeout, re-enabling");
        TAP_MACH_PORT.with(|cell| {
            let port = cell.get();
            if port != 0 {
                unsafe {
                    CGEventTapEnable(port as *mut _, true);
                }
            }
        });
        return None;
    }

    // Skip self-injected events (prevent feedback loop).
    let user_data = event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA);
    if user_data == KANI_EVENT_MARKER {
        return None; // Pass through to OS without re-capturing
    }

    // Always capture events for forwarding (before grab check).
    CAPTURE_TX.with(|cell| {
        let ptr = cell.get();
        if ptr == 0 {
            return;
        }
        let tx = unsafe { &*(ptr as *const mpsc::Sender<EventType>) };

        if let Some(evt) = convert_cg_event(event_type, event) {
            if let Err(e) = tx.try_send(evt) {
                warn!("Failed to send captured event: {}", e);
            }
        }
    });

    if GRABBING.load(Ordering::Relaxed) {
        // In Default mode, returning None passes the original event through.
        // Set the event type to Null so macOS ignores it instead.
        event.set_type(CGEventType::Null);
        return None;
    }

    // Default mode: None returns original event unchanged (pass through).
    None
}

fn convert_cg_event(event_type: CGEventType, event: &CGEvent) -> Option<EventType> {
    let location = event.location();

    match event_type {
        CGEventType::MouseMoved
        | CGEventType::LeftMouseDragged
        | CGEventType::RightMouseDragged => {
            // Use delta values for relative mouse movement.
            let dx = event.get_double_value_field(EventField::MOUSE_EVENT_DELTA_X);
            let dy = event.get_double_value_field(EventField::MOUSE_EVENT_DELTA_Y);
            Some(EventType::MouseMove { dx, dy })
        }
        CGEventType::LeftMouseDown => Some(EventType::MouseClick {
            button: MouseButton::Left,
            pressed: true,
        }),
        CGEventType::LeftMouseUp => Some(EventType::MouseClick {
            button: MouseButton::Left,
            pressed: false,
        }),
        CGEventType::RightMouseDown => Some(EventType::MouseClick {
            button: MouseButton::Right,
            pressed: true,
        }),
        CGEventType::RightMouseUp => Some(EventType::MouseClick {
            button: MouseButton::Right,
            pressed: false,
        }),
        CGEventType::ScrollWheel => {
            // scrollingDeltaX/Y gives the scroll amount.
            let dx = event.get_double_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2);
            let dy = event.get_double_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1);
            Some(EventType::MouseScroll { dx, dy })
        }
        CGEventType::KeyDown => {
            let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
            let modifiers = super::get_modifier_state_from_flags(event.get_flags());
            let hid = match kani_proto::keymap::cgkeycode_to_hid(keycode) {
                Some(h) => h,
                None => {
                    debug!(keycode, "Unmapped CGKeyCode, dropping key event");
                    return None;
                }
            };
            Some(EventType::KeyPress {
                hid_usage: hid,
                pressed: true,
                modifiers,
            })
        }
        CGEventType::KeyUp => {
            let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
            let modifiers = super::get_modifier_state_from_flags(event.get_flags());
            let hid = match kani_proto::keymap::cgkeycode_to_hid(keycode) {
                Some(h) => h,
                None => {
                    debug!(keycode, "Unmapped CGKeyCode, dropping key event");
                    return None;
                }
            };
            Some(EventType::KeyPress {
                hid_usage: hid,
                pressed: false,
                modifiers,
            })
        }
        CGEventType::FlagsChanged => {
            let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
            let new_flags = super::get_modifier_state_from_flags(event.get_flags());

            // Determine pressed state: FlagsChanged fires for both press and release.
            // The keycode tells us WHICH modifier, the flags tell us the NEW state.
            let pressed = match keycode {
                // Anomalous macOS event: keycode=0 with flags=0 (documented in
                // ShortcutRecorder issue #129). Ignore silently.
                0x00 => return None,
                0x37 | 0x36 => new_flags.meta, // kVK_Command / kVK_RightCommand
                0x3B | 0x3E => new_flags.ctrl, // kVK_Control / kVK_RightControl
                0x38 | 0x3C => new_flags.shift, // kVK_Shift / kVK_RightShift
                0x3A | 0x3D => new_flags.alt,  // kVK_Option / kVK_RightOption
                // CapsLock: macOS only fires on toggle transitions (no physical key-up event).
                // This is correct for KVM toggle semantics — each press toggles the state.
                0x39 => new_flags.caps_lock, // kVK_CapsLock
                _ => {
                    debug!(keycode, "Unknown FlagsChanged keycode");
                    return None;
                }
            };

            let hid = match kani_proto::keymap::cgkeycode_to_hid(keycode) {
                Some(h) => h,
                None => {
                    debug!(keycode, "Unmapped FlagsChanged CGKeyCode");
                    return None;
                }
            };

            Some(EventType::KeyPress {
                hid_usage: hid,
                pressed,
                modifiers: new_flags,
            })
        }
        _ => {
            debug!(
                ?event_type,
                x = location.x,
                y = location.y,
                "Unhandled event type"
            );
            None
        }
    }
}

/// Inject a mouse or keyboard event via CGEventPost.
pub fn inject_event(event: &EventType) {
    let source = match CGEventSource::new(CGEventSourceStateID::CombinedSessionState) {
        Ok(s) => s,
        Err(_) => {
            error!("Failed to create CGEventSource");
            return;
        }
    };

    match event {
        EventType::MouseMove { .. } => {
            // MouseMove injection not supported — use Platform::warp_cursor() instead.
            // CGEventPost(HID) feeds back into our own CGEventTap capture loop.
        }
        EventType::MouseClick { button, pressed } => {
            let (event_type, cg_button) = match (button, pressed) {
                (MouseButton::Left, true) => (CGEventType::LeftMouseDown, CGMouseButton::Left),
                (MouseButton::Left, false) => (CGEventType::LeftMouseUp, CGMouseButton::Left),
                (MouseButton::Right, true) => (CGEventType::RightMouseDown, CGMouseButton::Right),
                (MouseButton::Right, false) => (CGEventType::RightMouseUp, CGMouseButton::Right),
                // Map other buttons to left for now.
                (_, true) => (CGEventType::LeftMouseDown, CGMouseButton::Left),
                (_, false) => (CGEventType::LeftMouseUp, CGMouseButton::Left),
            };
            // Get current cursor position instead of hardcoded (0, 0)
            let location = match CGEvent::new(source.clone()) {
                Ok(e) => e.location(),
                Err(_) => {
                    error!("Failed to create event to query cursor position");
                    return;
                }
            };
            let cg_event = CGEvent::new_mouse_event(source, event_type, location, cg_button);
            if let Ok(evt) = cg_event {
                evt.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, KANI_EVENT_MARKER);
                evt.post(CGEventTapLocation::HID);
            } else {
                error!("Failed to create mouse click event");
            }
        }
        EventType::MouseScroll { dx, dy } => {
            // CGEventCreateScrollWheelEvent is not directly in the crate,
            // so we create a generic event and set scroll fields.
            if let Ok(evt) = CGEvent::new(source) {
                evt.set_type(CGEventType::ScrollWheel);
                evt.set_double_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1, *dy);
                evt.set_double_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2, *dx);
                evt.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, KANI_EVENT_MARKER);
                evt.post(CGEventTapLocation::HID);
            }
        }
        EventType::KeyPress {
            hid_usage,
            pressed,
            modifiers,
        } => {
            let keycode = match kani_proto::keymap::hid_to_cgkeycode(*hid_usage) {
                Some(c) => c as CGKeyCode,
                None => {
                    error!(hid_usage, "Unmapped HID code, dropping key injection");
                    return;
                }
            };
            let cg_event = CGEvent::new_keyboard_event(source, keycode, *pressed);
            if let Ok(evt) = cg_event {
                use core_graphics::event::CGEventFlags;
                let mut flags = CGEventFlags::CGEventFlagNull;
                if modifiers.shift {
                    flags |= CGEventFlags::CGEventFlagShift;
                }
                if modifiers.ctrl {
                    flags |= CGEventFlags::CGEventFlagControl;
                }
                if modifiers.alt {
                    flags |= CGEventFlags::CGEventFlagAlternate;
                }
                if modifiers.meta {
                    flags |= CGEventFlags::CGEventFlagCommand;
                }
                if modifiers.caps_lock {
                    flags |= CGEventFlags::CGEventFlagAlphaShift;
                }
                evt.set_flags(flags);
                evt.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, KANI_EVENT_MARKER);
                tracing::info!(
                    hid_usage,
                    cgkeycode = keycode,
                    pressed,
                    "Injecting keyboard via CGEventPost"
                );
                evt.post(CGEventTapLocation::HID);
                tracing::info!("CGEventPost completed for keyboard event");
            } else {
                error!(keycode, "Failed to create keyboard event");
            }
        }
        _ => {
            debug!(?event, "inject_event: ignoring non-input event type");
        }
    }
}
