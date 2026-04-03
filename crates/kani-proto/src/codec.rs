use crate::event::InputEvent;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("encode error: {0}")]
    Encode(#[from] bincode::Error),
    #[error("decode error: {0}")]
    // No #[from] — only one variant can auto-convert from bincode::Error
    Decode(bincode::Error),
}

pub fn encode(event: &InputEvent) -> Result<Vec<u8>, CodecError> {
    bincode::serialize(event).map_err(CodecError::Encode)
}

pub fn decode(bytes: &[u8]) -> Result<InputEvent, CodecError> {
    bincode::deserialize(bytes).map_err(CodecError::Decode)
}

pub const MAX_EVENT_SIZE: usize = 512;
pub const MAX_LAYOUT_SIZE: usize = 4096;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::*;
    use proptest::prelude::*;

    #[test]
    fn test_roundtrip_mouse_move() {
        let evt = InputEvent::new(
            HostId::new_v4(),
            EventType::MouseMove {
                dx: 42.0,
                dy: -13.5,
            },
        );
        let bytes = encode(&evt).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(evt.event_type, decoded.event_type);
        assert_eq!(evt.seq, decoded.seq);
    }

    #[test]
    fn test_roundtrip_enter_with_transition_id() {
        let evt = InputEvent::new(
            HostId::new_v4(),
            EventType::Enter {
                target_host: HostId::new_v4(),
                landing_x: 100.0,
                landing_y: 200.0,
                landing_display: 1,
                modifiers: ModifierState {
                    shift: true,
                    ..Default::default()
                },
                transition_id: 42,
            },
        );
        let bytes = encode(&evt).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(evt.event_type, decoded.event_type);
    }

    #[test]
    fn test_roundtrip_ack_with_transition_id() {
        let evt = InputEvent::new(
            HostId::new_v4(),
            EventType::Ack {
                acked_seq: 10,
                transition_id: 42,
            },
        );
        let bytes = encode(&evt).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(evt.event_type, decoded.event_type);
    }

    #[test]
    fn test_encoded_size_within_limit() {
        let evt = InputEvent::new(
            HostId::new_v4(),
            EventType::Enter {
                target_host: HostId::new_v4(),
                landing_x: 9999.99,
                landing_y: 9999.99,
                landing_display: u32::MAX,
                modifiers: ModifierState {
                    shift: true,
                    ctrl: true,
                    alt: true,
                    meta: true,
                    caps_lock: true,
                },
                transition_id: u64::MAX,
            },
        );
        let bytes = encode(&evt).unwrap();
        assert!(
            bytes.len() <= MAX_EVENT_SIZE,
            "Event size {} exceeds MAX_EVENT_SIZE {}",
            bytes.len(),
            MAX_EVENT_SIZE
        );
    }

    #[test]
    fn test_max_layout_size_is_larger_than_event_size() {
        // Runtime check (not const) to verify configuration
        let layout = MAX_LAYOUT_SIZE;
        let event = MAX_EVENT_SIZE;
        assert!(layout >= event);
        assert_eq!(layout, 4096);
    }

    proptest! {
        #[test]
        fn test_roundtrip_arbitrary_mouse_move(dx in -10000.0f64..10000.0, dy in -10000.0f64..10000.0) {
            let evt = InputEvent::new(HostId::new_v4(), EventType::MouseMove { dx, dy });
            let bytes = encode(&evt).unwrap();
            let decoded = decode(&bytes).unwrap();
            prop_assert_eq!(evt.event_type, decoded.event_type);
        }
    }
}
