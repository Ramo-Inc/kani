//! Clipboard message types.
//! Wire format: [4-byte length (big-endian)][1-byte type][payload]
//! Type 0x01 = Text/UTF-8
//! Type 0x02 = Image/PNG (post-MVP)
//! Type 0x03 = File metadata (post-MVP)
//! Type 0x04 = File chunk (post-MVP)

use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum ClipboardMessage {
    Text(String),
    // Future: Image(Vec<u8>), FileMetadata { name, size }, FileChunk { offset, data }
}

#[derive(Debug, Error)]
pub enum ClipboardCodecError {
    #[error("buffer too short: need {need}, got {got}")]
    BufferTooShort { need: usize, got: usize },
    #[error("unknown message type: 0x{0:02x}")]
    UnknownType(u8),
    #[error("invalid UTF-8: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    #[error("message too large: {size} bytes (max {max})")]
    TooLarge { size: usize, max: usize },
}

pub const MSG_TYPE_TEXT: u8 = 0x01;
pub const MAX_CLIPBOARD_SIZE: usize = 10 * 1024 * 1024; // 10MB

/// Encode a clipboard message to wire format: [4-byte len][1-byte type][payload]
pub fn encode_clipboard(msg: &ClipboardMessage) -> Result<Vec<u8>, ClipboardCodecError> {
    match msg {
        ClipboardMessage::Text(text) => {
            let payload = text.as_bytes();
            if payload.len() > MAX_CLIPBOARD_SIZE {
                return Err(ClipboardCodecError::TooLarge {
                    size: payload.len(),
                    max: MAX_CLIPBOARD_SIZE,
                });
            }
            let total_len = 1 + payload.len(); // type byte + payload
            let mut buf = Vec::with_capacity(4 + total_len);
            buf.extend_from_slice(&(total_len as u32).to_be_bytes());
            buf.push(MSG_TYPE_TEXT);
            buf.extend_from_slice(payload);
            Ok(buf)
        }
    }
}

/// Decode a clipboard message from wire format.
/// Returns (message, bytes_consumed).
pub fn decode_clipboard(data: &[u8]) -> Result<(ClipboardMessage, usize), ClipboardCodecError> {
    if data.len() < 5 {
        return Err(ClipboardCodecError::BufferTooShort {
            need: 5,
            got: data.len(),
        });
    }
    let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let total = 4 + len;
    if data.len() < total {
        return Err(ClipboardCodecError::BufferTooShort {
            need: total,
            got: data.len(),
        });
    }
    let msg_type = data[4];
    let payload = &data[5..total];

    let msg = match msg_type {
        MSG_TYPE_TEXT => ClipboardMessage::Text(String::from_utf8(payload.to_vec())?),
        other => return Err(ClipboardCodecError::UnknownType(other)),
    };

    Ok((msg, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_roundtrip() {
        let msg = ClipboardMessage::Text("Hello, world!".to_string());
        let bytes = encode_clipboard(&msg).unwrap();
        let (decoded, consumed) = decode_clipboard(&bytes).unwrap();
        assert_eq!(decoded, msg);
        assert_eq!(consumed, bytes.len());
    }

    #[test]
    fn test_text_unicode() {
        let msg = ClipboardMessage::Text("こんにちは世界 🌍".to_string());
        let bytes = encode_clipboard(&msg).unwrap();
        let (decoded, _) = decode_clipboard(&bytes).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_empty_text() {
        let msg = ClipboardMessage::Text(String::new());
        let bytes = encode_clipboard(&msg).unwrap();
        let (decoded, _) = decode_clipboard(&bytes).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_wire_format() {
        let msg = ClipboardMessage::Text("Hi".to_string());
        let bytes = encode_clipboard(&msg).unwrap();
        // Length = 1 (type) + 2 (payload) = 3
        assert_eq!(&bytes[0..4], &[0, 0, 0, 3]); // big-endian length
        assert_eq!(bytes[4], MSG_TYPE_TEXT);
        assert_eq!(&bytes[5..], b"Hi");
    }

    #[test]
    fn test_decode_too_short() {
        assert!(decode_clipboard(&[0, 0]).is_err());
    }

    #[test]
    fn test_decode_unknown_type() {
        let bytes = [0, 0, 0, 1, 0xFF]; // unknown type 0xFF
        assert!(matches!(
            decode_clipboard(&bytes),
            Err(ClipboardCodecError::UnknownType(0xFF))
        ));
    }

    #[test]
    fn test_too_large() {
        let big = "x".repeat(MAX_CLIPBOARD_SIZE + 1);
        assert!(matches!(
            encode_clipboard(&ClipboardMessage::Text(big)),
            Err(ClipboardCodecError::TooLarge { .. })
        ));
    }
}
