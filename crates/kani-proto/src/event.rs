use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use uuid::Uuid;

pub type HostId = Uuid;
pub type DisplayId = u32;

static SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InputEvent {
    pub seq: u64,
    pub host_id: HostId,
    pub timestamp_us: u64,
    pub event_type: EventType,
}

impl InputEvent {
    pub fn new(host_id: HostId, event_type: EventType) -> Self {
        Self {
            seq: SEQUENCE.fetch_add(1, Ordering::Relaxed),
            host_id,
            timestamp_us: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros() as u64,
            event_type,
        }
    }
}

/// Display info for auto-discovery over the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DisplaySnapshot {
    pub id: DisplayId,
    pub origin_x: f64,
    pub origin_y: f64,
    pub width: f64,
    pub height: f64,
    pub width_pixels: u32,
    pub height_pixels: u32,
    pub scale_factor: f64,
    pub is_primary: bool,
}

/// A host's display positions in world coordinates (for LayoutSync).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HostLayout {
    pub host_id: HostId,
    pub name: String,
    pub address: String,
    pub platform: String,
    pub world_offset_x: f64,
    pub world_offset_y: f64,
    pub displays: Vec<DisplaySnapshot>,
}

/// A border link between two displays (for LayoutSync).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BorderLinkLayout {
    pub from_host: HostId,
    pub from_display: DisplayId,
    pub from_edge: crate::topology::Edge,
    pub from_range: [f64; 2],
    pub to_host: HostId,
    pub to_display: DisplayId,
    pub to_edge: crate::topology::Edge,
    pub to_range: [f64; 2],
    pub mapping: crate::topology::CoordinateMapping,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EventType {
    MouseMove {
        dx: f64,
        dy: f64,
    },
    MouseClick {
        button: MouseButton,
        pressed: bool,
    },
    MouseScroll {
        dx: f64,
        dy: f64,
    },
    KeyPress {
        hid_usage: u32,
        pressed: bool,
        modifiers: ModifierState,
    },
    Enter {
        target_host: HostId,
        landing_x: f64,
        landing_y: f64,
        landing_display: DisplayId,
        modifiers: ModifierState,
        transition_id: u64,
    },
    Ack {
        acked_seq: u64,
        transition_id: u64,
    },
    Release {
        released_host: HostId,
    },
    /// Sent by client on graceful shutdown so host can disconnect immediately.
    Leave,
    Ping {
        payload: u64,
    },
    Pong {
        payload: u64,
    },
    /// Request remote machine's display layout.
    QueryDisplays,
    /// Response with display layout.
    DisplaysResponse {
        host_name: String,
        displays: Vec<DisplaySnapshot>,
    },
    /// Client registers with Host, sending its display info.
    RegisterClient {
        client_name: String,
        platform: String,
        displays: Vec<DisplaySnapshot>,
    },
    /// Host pushes complete layout to connected clients.
    LayoutSync {
        hosts: Vec<HostLayout>,
        border_links: Vec<BorderLinkLayout>,
    },
    /// Host commands Client to start KVM processing.
    StartKvm,
    /// Host commands Client to stop KVM processing.
    StopKvm,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModifierState {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
    pub caps_lock: bool,
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MouseMove { dx, dy } => write!(f, "MouseMove({dx:.1}, {dy:.1})"),
            Self::MouseClick { button, pressed } => write!(
                f,
                "MouseClick({button:?}, {})",
                if *pressed { "down" } else { "up" }
            ),
            Self::MouseScroll { dx, dy } => write!(f, "MouseScroll({dx:.1}, {dy:.1})"),
            Self::KeyPress {
                hid_usage, pressed, ..
            } => write!(
                f,
                "Key(0x{hid_usage:04x}, {})",
                if *pressed { "down" } else { "up" }
            ),
            Self::Enter {
                target_host,
                transition_id,
                ..
            } => {
                let host_str = target_host.to_string();
                write!(f, "Enter(→{}, tid={transition_id})", &host_str[..8])
            }
            Self::Ack { transition_id, .. } => write!(f, "Ack(tid={transition_id})"),
            Self::Release { released_host } => {
                let host_str = released_host.to_string();
                write!(f, "Release({})", &host_str[..8])
            }
            Self::Leave => write!(f, "Leave"),
            Self::Ping { payload } => write!(f, "Ping({payload})"),
            Self::Pong { payload } => write!(f, "Pong({payload})"),
            Self::QueryDisplays => write!(f, "QueryDisplays"),
            Self::DisplaysResponse {
                host_name,
                displays,
            } => {
                write!(
                    f,
                    "DisplaysResponse({}, {} displays)",
                    host_name,
                    displays.len()
                )
            }
            Self::RegisterClient {
                client_name,
                displays,
                ..
            } => {
                write!(
                    f,
                    "RegisterClient({}, {} displays)",
                    client_name,
                    displays.len()
                )
            }
            Self::LayoutSync {
                hosts,
                border_links,
            } => {
                write!(
                    f,
                    "LayoutSync({} hosts, {} links)",
                    hosts.len(),
                    border_links.len()
                )
            }
            Self::StartKvm => write!(f, "StartKvm"),
            Self::StopKvm => write!(f, "StopKvm"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mouse_move_event_creation() {
        let evt = InputEvent::new(HostId::new_v4(), EventType::MouseMove { dx: 1.5, dy: -2.3 });
        assert!(evt.seq > 0);
        assert!(matches!(evt.event_type, EventType::MouseMove { .. }));
    }

    #[test]
    fn test_sequence_increments() {
        let a = InputEvent::new(HostId::new_v4(), EventType::Ping { payload: 1 });
        let b = InputEvent::new(HostId::new_v4(), EventType::Ping { payload: 2 });
        assert!(b.seq > a.seq);
    }

    #[test]
    fn test_event_type_display_mouse_move() {
        let evt = EventType::MouseMove { dx: 1.5, dy: -2.3 };
        assert_eq!(format!("{evt}"), "MouseMove(1.5, -2.3)");
    }

    #[test]
    fn test_event_type_display_enter() {
        let host = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let evt = EventType::Enter {
            target_host: host,
            landing_x: 0.0,
            landing_y: 720.0,
            landing_display: 0,
            modifiers: ModifierState::default(),
            transition_id: 42,
        };
        let s = format!("{evt}");
        assert!(s.contains("Enter"));
        assert!(s.contains("tid=42"));
    }

    #[test]
    fn test_event_type_display_ping_pong() {
        assert_eq!(format!("{}", EventType::Ping { payload: 99 }), "Ping(99)");
        assert_eq!(format!("{}", EventType::Pong { payload: 99 }), "Pong(99)");
    }

    #[test]
    fn test_modifier_state_default() {
        let m = ModifierState::default();
        assert!(!m.shift && !m.ctrl && !m.alt && !m.meta && !m.caps_lock);
    }

    #[test]
    fn test_modifier_state_roundtrip_serialization() {
        use crate::codec::{decode, encode};
        let event = InputEvent::new(
            Uuid::new_v4(),
            EventType::KeyPress {
                hid_usage: 0x04,
                pressed: true,
                modifiers: ModifierState {
                    shift: true,
                    ctrl: false,
                    alt: true,
                    meta: false,
                    caps_lock: false,
                },
            },
        );
        let bytes = encode(&event).unwrap();
        let decoded = decode(&bytes).unwrap();
        if let EventType::KeyPress { modifiers, .. } = &decoded.event_type {
            assert!(modifiers.shift);
            assert!(!modifiers.ctrl);
            assert!(modifiers.alt);
        } else {
            panic!("Expected KeyPress");
        }
    }

    #[test]
    fn test_displays_response_fits_in_packet() {
        use crate::codec::{encode, MAX_EVENT_SIZE};
        let event = InputEvent {
            seq: 1,
            host_id: uuid::Uuid::nil(),
            timestamp_us: 0,
            event_type: EventType::DisplaysResponse {
                host_name: "long-hostname-test".into(),
                displays: (0..6)
                    .map(|i| DisplaySnapshot {
                        id: i,
                        origin_x: -1920.0,
                        origin_y: 1440.0,
                        width: 3440.0,
                        height: 1440.0,
                        width_pixels: 3440,
                        height_pixels: 1440,
                        scale_factor: 2.0,
                        is_primary: i == 0,
                    })
                    .collect(),
            },
        };
        let bytes = encode(&event).unwrap();
        assert!(
            bytes.len() <= MAX_EVENT_SIZE,
            "DisplaysResponse too large: {} bytes > {}",
            bytes.len(),
            MAX_EVENT_SIZE
        );
    }

    #[test]
    fn test_query_displays_roundtrip() {
        use crate::codec::{decode, encode};
        let event = InputEvent {
            seq: 42,
            host_id: uuid::Uuid::new_v4(),
            timestamp_us: 12345,
            event_type: EventType::QueryDisplays,
        };
        let bytes = encode(&event).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.seq, 42);
        assert_eq!(decoded.event_type, EventType::QueryDisplays);
    }

    #[test]
    fn test_register_client_roundtrip() {
        use crate::codec::{decode, encode};
        let event = InputEvent::new(
            Uuid::new_v4(),
            EventType::RegisterClient {
                client_name: "test-client".into(),
                platform: "windows".into(),
                displays: vec![DisplaySnapshot {
                    id: 0,
                    origin_x: 0.0,
                    origin_y: 0.0,
                    width: 1920.0,
                    height: 1080.0,
                    width_pixels: 1920,
                    height_pixels: 1080,
                    scale_factor: 1.0,
                    is_primary: true,
                }],
            },
        );
        let bytes = encode(&event).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.event_type, event.event_type);
    }

    #[test]
    fn test_layout_sync_roundtrip() {
        use crate::codec::{decode, encode};
        use crate::topology::{CoordinateMapping, Edge};
        let event = InputEvent::new(
            Uuid::new_v4(),
            EventType::LayoutSync {
                hosts: vec![HostLayout {
                    host_id: Uuid::new_v4(),
                    name: "host".into(),
                    address: "192.168.1.10".into(),
                    platform: "macos".into(),
                    world_offset_x: 0.0,
                    world_offset_y: 0.0,
                    displays: vec![DisplaySnapshot {
                        id: 0,
                        origin_x: 0.0,
                        origin_y: 0.0,
                        width: 3440.0,
                        height: 1440.0,
                        width_pixels: 3440,
                        height_pixels: 1440,
                        scale_factor: 1.0,
                        is_primary: true,
                    }],
                }],
                border_links: vec![BorderLinkLayout {
                    from_host: Uuid::new_v4(),
                    from_display: 0,
                    from_edge: Edge::Right,
                    from_range: [0.0, 1080.0],
                    to_host: Uuid::new_v4(),
                    to_display: 0,
                    to_edge: Edge::Left,
                    to_range: [0.0, 1080.0],
                    mapping: CoordinateMapping::Linear,
                }],
            },
        );
        let bytes = encode(&event).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.event_type, event.event_type);
    }

    #[test]
    fn test_layout_sync_worst_case_fits_in_max_layout_size() {
        use crate::codec::{encode, MAX_LAYOUT_SIZE};
        use crate::topology::{CoordinateMapping, Edge};
        let host_id_a = Uuid::from_u128(1);
        let host_id_b = Uuid::from_u128(2);
        let make_displays = |count: u32| -> Vec<DisplaySnapshot> {
            (0..count)
                .map(|i| DisplaySnapshot {
                    id: i,
                    origin_x: (i as f64) * 1920.0,
                    origin_y: 0.0,
                    width: 1920.0,
                    height: 1080.0,
                    width_pixels: 1920,
                    height_pixels: 1080,
                    scale_factor: 1.25,
                    is_primary: i == 0,
                })
                .collect()
        };
        let make_links = |count: usize| -> Vec<BorderLinkLayout> {
            (0..count)
                .map(|_| BorderLinkLayout {
                    from_host: host_id_a,
                    from_display: 0,
                    from_edge: Edge::Right,
                    from_range: [0.0, 1080.0],
                    to_host: host_id_b,
                    to_display: 0,
                    to_edge: Edge::Left,
                    to_range: [0.0, 1080.0],
                    mapping: CoordinateMapping::Linear,
                })
                .collect()
        };
        let event = InputEvent::new(
            host_id_a,
            EventType::LayoutSync {
                hosts: vec![
                    HostLayout {
                        host_id: host_id_a,
                        name: "windows-pc".into(),
                        address: "192.168.1.10".into(),
                        platform: "windows".into(),
                        world_offset_x: 0.0,
                        world_offset_y: 0.0,
                        displays: make_displays(5),
                    },
                    HostLayout {
                        host_id: host_id_b,
                        name: "macmini".into(),
                        address: "192.168.1.20".into(),
                        platform: "windows".into(),
                        world_offset_x: 5000.0,
                        world_offset_y: 0.0,
                        displays: make_displays(5),
                    },
                ],
                border_links: make_links(16),
            },
        );
        let bytes = encode(&event).unwrap();
        assert!(
            bytes.len() <= MAX_LAYOUT_SIZE,
            "LayoutSync {} bytes > MAX_LAYOUT_SIZE {}",
            bytes.len(),
            MAX_LAYOUT_SIZE
        );
    }

    #[test]
    fn test_start_kvm_roundtrip() {
        use crate::codec::{decode, encode};
        let event = InputEvent::new(Uuid::new_v4(), EventType::StartKvm);
        let bytes = encode(&event).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.event_type, EventType::StartKvm);
    }

    #[test]
    fn test_stop_kvm_roundtrip() {
        use crate::codec::{decode, encode};
        let event = InputEvent::new(Uuid::new_v4(), EventType::StopKvm);
        let bytes = encode(&event).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.event_type, EventType::StopKvm);
    }

    #[test]
    fn test_start_stop_kvm_display() {
        assert_eq!(format!("{}", EventType::StartKvm), "StartKvm");
        assert_eq!(format!("{}", EventType::StopKvm), "StopKvm");
    }
}
