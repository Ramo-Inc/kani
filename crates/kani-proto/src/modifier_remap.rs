//! Cross-platform modifier remapping for Ctrl ↔ Cmd.

use crate::event::{EventType, ModifierState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsType {
    Windows,
    MacOS,
}

impl OsType {
    pub fn from_platform_str(s: &str) -> Option<Self> {
        match s {
            "windows" => Some(Self::Windows),
            "macos" => Some(Self::MacOS),
            _ => None,
        }
    }
}

fn swap_ctrl_meta(modifiers: &ModifierState) -> ModifierState {
    let mut m = *modifiers;
    std::mem::swap(&mut m.ctrl, &mut m.meta);
    m
}

pub fn remap_event(event: &EventType, source_os: OsType, target_os: OsType) -> EventType {
    if source_os == target_os {
        return event.clone();
    }

    match event {
        EventType::KeyPress {
            hid_usage,
            pressed,
            modifiers,
        } => {
            let new_modifiers = swap_ctrl_meta(modifiers);
            let new_hid = match *hid_usage {
                0xE0 => 0xE3, // Left Control → Left GUI (Cmd)
                0xE4 => 0xE7, // Right Control → Right GUI (Cmd)
                0xE3 => 0xE0, // Left GUI (Cmd) → Left Control
                0xE7 => 0xE4, // Right GUI (Cmd) → Right Control
                other => other,
            };
            EventType::KeyPress {
                hid_usage: new_hid,
                pressed: *pressed,
                modifiers: new_modifiers,
            }
        }
        EventType::Enter {
            target_host,
            landing_x,
            landing_y,
            landing_display,
            modifiers,
            transition_id,
        } => EventType::Enter {
            target_host: *target_host,
            landing_x: *landing_x,
            landing_y: *landing_y,
            landing_display: *landing_display,
            modifiers: swap_ctrl_meta(modifiers),
            transition_id: *transition_id,
        },
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, meta: bool) -> ModifierState {
        ModifierState {
            shift: false,
            ctrl,
            alt: false,
            meta,
            caps_lock: false,
        }
    }

    #[test]
    fn test_remap_ctrl_to_cmd_win_to_mac() {
        let event = EventType::KeyPress {
            hid_usage: 0xE0, // Left Control
            pressed: true,
            modifiers: mods(true, false),
        };
        let result = remap_event(&event, OsType::Windows, OsType::MacOS);
        assert_eq!(
            result,
            EventType::KeyPress {
                hid_usage: 0xE3, // Left GUI
                pressed: true,
                modifiers: mods(false, true),
            }
        );
    }

    #[test]
    fn test_remap_cmd_to_ctrl_mac_to_win() {
        let event = EventType::KeyPress {
            hid_usage: 0xE3, // Left GUI
            pressed: true,
            modifiers: mods(false, true),
        };
        let result = remap_event(&event, OsType::MacOS, OsType::Windows);
        assert_eq!(
            result,
            EventType::KeyPress {
                hid_usage: 0xE0, // Left Control
                pressed: true,
                modifiers: mods(true, false),
            }
        );
    }

    #[test]
    fn test_remap_right_side_keys() {
        let event = EventType::KeyPress {
            hid_usage: 0xE4, // Right Control
            pressed: true,
            modifiers: mods(true, false),
        };
        let result = remap_event(&event, OsType::Windows, OsType::MacOS);
        assert_eq!(
            result,
            EventType::KeyPress {
                hid_usage: 0xE7, // Right GUI
                pressed: true,
                modifiers: mods(false, true),
            }
        );
    }

    #[test]
    fn test_same_os_no_remap() {
        let event = EventType::KeyPress {
            hid_usage: 0xE0,
            pressed: true,
            modifiers: mods(true, false),
        };
        let result = remap_event(&event, OsType::Windows, OsType::Windows);
        assert_eq!(result, event);
    }

    #[test]
    fn test_non_modifier_key_flags_swap() {
        // Ctrl+C: the C key itself (HID 0x06) should not change, but modifier flags swap
        let event = EventType::KeyPress {
            hid_usage: 0x06, // C key
            pressed: true,
            modifiers: mods(true, false),
        };
        let result = remap_event(&event, OsType::Windows, OsType::MacOS);
        assert_eq!(
            result,
            EventType::KeyPress {
                hid_usage: 0x06, // C key unchanged
                pressed: true,
                modifiers: mods(false, true), // ctrl→meta
            }
        );
    }

    #[test]
    fn test_enter_event_remap() {
        let event = EventType::Enter {
            target_host: uuid::Uuid::nil(),
            landing_x: 100.0,
            landing_y: 200.0,
            landing_display: 0,
            modifiers: mods(true, false),
            transition_id: 42,
        };
        let result = remap_event(&event, OsType::Windows, OsType::MacOS);
        if let EventType::Enter { modifiers, .. } = result {
            assert!(!modifiers.ctrl);
            assert!(modifiers.meta);
        } else {
            panic!("Expected Enter event");
        }
    }

    #[test]
    fn test_mouse_event_passthrough() {
        let event = EventType::MouseMove { dx: 1.0, dy: 2.0 };
        let result = remap_event(&event, OsType::Windows, OsType::MacOS);
        assert_eq!(result, event);
    }

    #[test]
    fn test_os_type_parsing() {
        assert_eq!(OsType::from_platform_str("windows"), Some(OsType::Windows));
        assert_eq!(OsType::from_platform_str("macos"), Some(OsType::MacOS));
        assert_eq!(OsType::from_platform_str("unknown"), None);
        assert_eq!(OsType::from_platform_str("linux"), None);
    }
}
