use kani_proto::codec::{self, MAX_EVENT_SIZE};
use kani_proto::event::InputEvent;
use std::net::SocketAddr;
use thiserror::Error;
use tokio::net::UdpSocket;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("codec error: {0}")]
    Codec(#[from] codec::CodecError),
    #[error("packet too large: {size} > {max}")]
    PacketTooLarge { size: usize, max: usize },
}

pub struct UdpTransport {
    socket: UdpSocket,
}

impl UdpTransport {
    pub async fn bind(addr: &str) -> Result<Self, TransportError> {
        let socket = UdpSocket::bind(addr).await?;
        Ok(Self { socket })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        Ok(self.socket.local_addr()?)
    }

    pub async fn send_event(
        &self,
        event: &InputEvent,
        target: SocketAddr,
    ) -> Result<(), TransportError> {
        let bytes = codec::encode(event)?;
        if bytes.len() > MAX_EVENT_SIZE {
            return Err(TransportError::PacketTooLarge {
                size: bytes.len(),
                max: MAX_EVENT_SIZE,
            });
        }
        self.socket.send_to(&bytes, target).await?;
        Ok(())
    }

    pub async fn recv_event(&self) -> Result<(InputEvent, SocketAddr), TransportError> {
        let mut buf = vec![0u8; MAX_EVENT_SIZE];
        let (n, addr) = self.socket.recv_from(&mut buf).await?;
        let event = codec::decode(&buf[..n])?;
        Ok((event, addr))
    }

    pub fn into_socket(self) -> UdpSocket {
        self.socket
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kani_proto::event::*;

    #[tokio::test]
    async fn test_send_recv_event() {
        let server = UdpTransport::bind("127.0.0.1:0").await.unwrap();
        let client = UdpTransport::bind("127.0.0.1:0").await.unwrap();
        let server_addr = server.local_addr().unwrap();

        let evt = InputEvent::new(HostId::new_v4(), EventType::MouseMove { dx: 1.0, dy: 2.0 });
        client.send_event(&evt, server_addr).await.unwrap();

        let (received, from_addr) = server.recv_event().await.unwrap();
        assert_eq!(received.event_type, evt.event_type);
        assert_eq!(received.seq, evt.seq);
        assert_eq!(from_addr, client.local_addr().unwrap());
    }

    #[tokio::test]
    async fn test_multiple_events() {
        let server = UdpTransport::bind("127.0.0.1:0").await.unwrap();
        let client = UdpTransport::bind("127.0.0.1:0").await.unwrap();
        let server_addr = server.local_addr().unwrap();

        for i in 0..10 {
            let evt = InputEvent::new(HostId::new_v4(), EventType::Ping { payload: i });
            client.send_event(&evt, server_addr).await.unwrap();
        }

        for i in 0..10 {
            let (received, _) = server.recv_event().await.unwrap();
            assert!(matches!(received.event_type, EventType::Ping { payload } if payload == i));
        }
    }
}
