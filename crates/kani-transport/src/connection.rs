use kani_proto::event::HostId;
use kani_proto::topology::HostTopology;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConnectionError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("peer not found: {0}")]
    PeerNotFound(HostId),
    #[error("topology exchange timeout")]
    TopologyTimeout,
}

#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub host_id: HostId,
    pub topology: Option<HostTopology>,
    pub last_pong: Instant,
    pub connected: bool,
}

/// Manages connections to peers with heartbeat monitoring.
pub struct ConnectionManager {
    peers: HashMap<HostId, PeerInfo>,
    heartbeat_interval: Duration,
    pub heartbeat_timeout: Duration,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            peers: HashMap::new(),
            heartbeat_interval: Duration::from_secs(1),
            heartbeat_timeout: Duration::from_secs(3),
        }
    }

    pub fn register_peer(&mut self, host_id: HostId) {
        self.peers.insert(
            host_id,
            PeerInfo {
                host_id,
                topology: None,
                last_pong: Instant::now(),
                connected: true,
            },
        );
    }

    pub fn update_topology(&mut self, host_id: HostId, topology: HostTopology) {
        if let Some(peer) = self.peers.get_mut(&host_id) {
            peer.topology = Some(topology);
        }
    }

    pub fn record_pong(&mut self, host_id: HostId) {
        if let Some(peer) = self.peers.get_mut(&host_id) {
            peer.last_pong = Instant::now();
            peer.connected = true;
        }
    }

    /// Check all peers for heartbeat timeout. Returns list of disconnected host IDs.
    pub fn check_heartbeats(&mut self) -> Vec<HostId> {
        let timeout = self.heartbeat_timeout;
        let mut disconnected = Vec::new();
        for peer in self.peers.values_mut() {
            if peer.connected && peer.last_pong.elapsed() > timeout {
                peer.connected = false;
                disconnected.push(peer.host_id);
            }
        }
        disconnected
    }

    pub fn is_connected(&self, host_id: &HostId) -> bool {
        self.peers
            .get(host_id)
            .map(|p| p.connected)
            .unwrap_or(false)
    }

    pub fn get_topology(&self, host_id: &HostId) -> Option<&HostTopology> {
        self.peers.get(host_id).and_then(|p| p.topology.as_ref())
    }

    pub fn connected_peers(&self) -> Vec<HostId> {
        self.peers
            .values()
            .filter(|p| p.connected)
            .map(|p| p.host_id)
            .collect()
    }

    pub fn heartbeat_interval(&self) -> Duration {
        self.heartbeat_interval
    }
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn host_a() -> HostId {
        Uuid::from_u128(1)
    }

    fn host_b() -> HostId {
        Uuid::from_u128(2)
    }

    #[test]
    fn test_register_and_connect() {
        let mut mgr = ConnectionManager::new();
        mgr.register_peer(host_a());
        assert!(mgr.is_connected(&host_a()));
        assert!(!mgr.is_connected(&host_b()));
    }

    #[test]
    fn test_heartbeat_timeout() {
        let mut mgr = ConnectionManager::new();
        mgr.heartbeat_timeout = Duration::from_millis(10);
        mgr.register_peer(host_a());

        // No timeout yet
        assert!(mgr.check_heartbeats().is_empty());

        // Wait for timeout
        std::thread::sleep(Duration::from_millis(20));
        let disconnected = mgr.check_heartbeats();
        assert_eq!(disconnected, vec![host_a()]);
        assert!(!mgr.is_connected(&host_a()));
    }

    #[test]
    fn test_pong_resets_timeout() {
        let mut mgr = ConnectionManager::new();
        mgr.heartbeat_timeout = Duration::from_millis(50);
        mgr.register_peer(host_a());

        std::thread::sleep(Duration::from_millis(30));
        mgr.record_pong(host_a());

        // Should not timeout because pong was received
        std::thread::sleep(Duration::from_millis(30));
        assert!(mgr.check_heartbeats().is_empty());
    }

    #[test]
    fn test_topology_update() {
        let mut mgr = ConnectionManager::new();
        mgr.register_peer(host_a());
        assert!(mgr.get_topology(&host_a()).is_none());

        let topo = HostTopology::new(host_a(), vec![]);
        mgr.update_topology(host_a(), topo);
        assert_eq!(mgr.get_topology(&host_a()).unwrap().host_id, host_a());
    }

    #[test]
    fn test_connected_peers() {
        let mut mgr = ConnectionManager::new();
        mgr.heartbeat_timeout = Duration::from_millis(10);
        mgr.register_peer(host_a());
        mgr.register_peer(host_b());

        assert_eq!(mgr.connected_peers().len(), 2);

        std::thread::sleep(Duration::from_millis(20));
        mgr.record_pong(host_b()); // only B sends pong
        mgr.check_heartbeats();

        let connected = mgr.connected_peers();
        assert_eq!(connected.len(), 1);
        assert_eq!(connected[0], host_b());
    }

    #[test]
    fn test_disconnect_and_reconnect() {
        let mut mgr = ConnectionManager::new();
        mgr.heartbeat_timeout = Duration::from_millis(10);
        mgr.register_peer(host_a());

        std::thread::sleep(Duration::from_millis(20));
        mgr.check_heartbeats();
        assert!(!mgr.is_connected(&host_a()));

        // Pong received again — reconnected
        mgr.record_pong(host_a());
        assert!(mgr.is_connected(&host_a()));
    }
}
