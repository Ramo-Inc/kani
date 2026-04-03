//! TCP transport for clipboard messages.
//! MVP: plain TCP. TLS can be layered on top later.

use kani_proto::clipboard::{self, ClipboardCodecError, ClipboardMessage};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Error)]
pub enum TcpTransportError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("clipboard codec error: {0}")]
    Codec(#[from] ClipboardCodecError),
    #[error("connection closed")]
    ConnectionClosed,
}

/// TCP clipboard sender (client side).
pub struct ClipboardSender {
    stream: TcpStream,
}

impl ClipboardSender {
    pub async fn connect(addr: &str) -> Result<Self, TcpTransportError> {
        let stream = TcpStream::connect(addr).await?;
        Ok(Self { stream })
    }

    pub async fn send(&mut self, msg: &ClipboardMessage) -> Result<(), TcpTransportError> {
        let bytes = clipboard::encode_clipboard(msg)?;
        self.stream.write_all(&bytes).await?;
        self.stream.flush().await?;
        Ok(())
    }
}

/// TCP clipboard receiver (server side).
pub struct ClipboardReceiver {
    listener: TcpListener,
}

impl ClipboardReceiver {
    pub async fn bind(addr: &str) -> Result<Self, TcpTransportError> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self { listener })
    }

    pub fn local_addr(&self) -> Result<std::net::SocketAddr, TcpTransportError> {
        Ok(self.listener.local_addr()?)
    }

    /// Accept one connection and read one clipboard message.
    /// Returns (message, source_address). 5-second read timeout prevents slow-loris.
    pub async fn accept_and_recv(
        &self,
    ) -> Result<(ClipboardMessage, std::net::SocketAddr), TcpTransportError> {
        let (mut stream, addr) = self.listener.accept().await?;
        let msg =
            tokio::time::timeout(std::time::Duration::from_secs(5), recv_message(&mut stream))
                .await
                .map_err(|_| {
                    TcpTransportError::Io(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "clipboard recv timeout (5s)",
                    ))
                })??;
        Ok((msg, addr))
    }
}

/// Read a single framed clipboard message from a TCP stream.
async fn recv_message(stream: &mut TcpStream) -> Result<ClipboardMessage, TcpTransportError> {
    // Read 4-byte length header
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len == 0 {
        return Err(TcpTransportError::ConnectionClosed);
    }

    // Reject oversized messages before allocating
    if len > clipboard::MAX_CLIPBOARD_SIZE + 1 {
        return Err(TcpTransportError::Codec(
            clipboard::ClipboardCodecError::TooLarge {
                size: len,
                max: clipboard::MAX_CLIPBOARD_SIZE,
            },
        ));
    }

    // Read type + payload
    let mut payload_buf = vec![0u8; len];
    stream.read_exact(&mut payload_buf).await?;

    // Reassemble full frame for decode
    let mut full = Vec::with_capacity(4 + len);
    full.extend_from_slice(&len_buf);
    full.extend_from_slice(&payload_buf);

    let (msg, _) = clipboard::decode_clipboard(&full)?;
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kani_proto::clipboard::ClipboardMessage;

    #[tokio::test]
    async fn test_tcp_clipboard_roundtrip() {
        let receiver = ClipboardReceiver::bind("127.0.0.1:0").await.unwrap();
        let addr = receiver.local_addr().unwrap();

        let send_handle = tokio::spawn(async move {
            let mut sender = ClipboardSender::connect(&addr.to_string()).await.unwrap();
            sender
                .send(&ClipboardMessage::Text("Hello from clipboard!".to_string()))
                .await
                .unwrap();
        });

        let (msg, _addr) = receiver.accept_and_recv().await.unwrap();
        assert_eq!(
            msg,
            ClipboardMessage::Text("Hello from clipboard!".to_string())
        );

        send_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_tcp_clipboard_unicode() {
        let receiver = ClipboardReceiver::bind("127.0.0.1:0").await.unwrap();
        let addr = receiver.local_addr().unwrap();

        let send_handle = tokio::spawn(async move {
            let mut sender = ClipboardSender::connect(&addr.to_string()).await.unwrap();
            sender
                .send(&ClipboardMessage::Text("日本語テスト 🎉".to_string()))
                .await
                .unwrap();
        });

        let (msg, _addr) = receiver.accept_and_recv().await.unwrap();
        assert_eq!(msg, ClipboardMessage::Text("日本語テスト 🎉".to_string()));

        send_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_recv_rejects_oversized_length() {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let send_handle = tokio::spawn(async move {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            // Send a length header claiming 20MB (exceeds MAX_CLIPBOARD_SIZE)
            let fake_len: u32 = 20 * 1024 * 1024;
            stream.write_all(&fake_len.to_be_bytes()).await.unwrap();
        });

        let (mut stream, _) = listener.accept().await.unwrap();
        let result = recv_message(&mut stream).await;
        assert!(result.is_err());

        send_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_tcp_clipboard_large_text() {
        let receiver = ClipboardReceiver::bind("127.0.0.1:0").await.unwrap();
        let addr = receiver.local_addr().unwrap();

        let large_text = "x".repeat(100_000); // 100KB
        let expected = large_text.clone();

        let send_handle = tokio::spawn(async move {
            let mut sender = ClipboardSender::connect(&addr.to_string()).await.unwrap();
            sender
                .send(&ClipboardMessage::Text(large_text))
                .await
                .unwrap();
        });

        let (msg, _addr) = receiver.accept_and_recv().await.unwrap();
        assert_eq!(msg, ClipboardMessage::Text(expected));

        send_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_accept_returns_source_addr() {
        let receiver = ClipboardReceiver::bind("127.0.0.1:0").await.unwrap();
        let addr = receiver.local_addr().unwrap();

        tokio::spawn(async move {
            let mut sender = ClipboardSender::connect(&addr.to_string()).await.unwrap();
            sender
                .send(&ClipboardMessage::Text("test".into()))
                .await
                .unwrap();
        });

        let (_msg, from_addr) = receiver.accept_and_recv().await.unwrap();
        assert_eq!(from_addr.ip(), std::net::IpAddr::from([127, 0, 0, 1]));
    }

    #[tokio::test]
    async fn test_accept_timeout_on_slow_sender() {
        let receiver = ClipboardReceiver::bind("127.0.0.1:0").await.unwrap();
        let addr = receiver.local_addr().unwrap();

        // Connect but send nothing — should timeout after 5s
        let _conn = tokio::net::TcpStream::connect(addr).await.unwrap();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(7),
            receiver.accept_and_recv(),
        )
        .await;
        assert!(result.is_ok()); // outer timeout didn't fire
        assert!(result.unwrap().is_err()); // inner 5s timeout fired
    }
}
