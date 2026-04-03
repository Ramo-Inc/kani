//! KvmEngine — Reusable KVM processing core extracted from server.rs.
//!
//! Handles: CursorStateMachine, VirtualCursor, BorderLinkResolver,
//! Platform capture/inject lifecycle, clipboard sync, heartbeat/disconnect,
//! and modifier remapping between platforms.

use crate::server::{build_border_links, build_topologies};
use kani_clipboard::monitor::ClipboardMonitor;
use kani_core::crossing::BorderLinkResolver;
use kani_core::cursor::{MoveResult, VirtualCursor};
use kani_core::state::{CursorState, CursorStateMachine};
use kani_proto::config::KaniConfig;
use kani_proto::event::*;
use kani_proto::modifier_remap::OsType;
use kani_proto::topology::HostTopology;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);
const ACK_TIMEOUT: Duration = Duration::from_millis(500);

/// Per-peer connection state tracked by KvmEngine.
pub struct PeerState {
    pub addr: SocketAddr,
    pub last_heard: Instant,
}

/// Actions that KvmEngine returns to the caller for execution.
#[derive(Debug)]
pub enum KvmAction {
    /// Send event to a known peer (HostId -> SocketAddr resolved by engine internally).
    SendEvent { to: HostId, event: InputEvent },
    /// Send event to a raw address (for Pong to unregistered senders).
    SendToAddr { addr: SocketAddr, event: InputEvent },
    /// A peer was disconnected due to heartbeat timeout.
    PeerDisconnected(HostId),
    /// Cursor was returned to the local host from a disconnected peer.
    CursorReturnedToHost(HostId),
    /// Informational status message.
    StatusMessage(String),
}

/// Result of calling `KvmEngine::start()`.
pub struct KvmStartResult {
    pub platform_rx: tokio::sync::mpsc::Receiver<EventType>,
    pub clipboard_rx: tokio::sync::mpsc::Receiver<kani_proto::clipboard::ClipboardMessage>,
    pub clipboard_receiver: Option<kani_transport::tls_tcp::ClipboardReceiver>,
}

/// Reusable KVM processing core.
///
/// Both Host (UnifiedServer) and Client (ClientAgent) instantiate this
/// when KVM starts. The engine owns all KVM state and produces `KvmAction`s
/// that the caller sends over the network.
pub struct KvmEngine {
    // Identity
    pub(crate) this_host_id: HostId,
    #[allow(dead_code)]
    config: KaniConfig,

    // State machine & cursor
    pub(crate) state_machine: CursorStateMachine,
    cursor: VirtualCursor,
    resolver: BorderLinkResolver,
    server_topo: HostTopology,
    #[allow(dead_code)]
    topologies: Vec<HostTopology>,

    // Peers
    pub(crate) peers: HashMap<HostId, PeerState>,

    // Transition timeout
    pub(crate) transition_deadline: Option<tokio::time::Instant>,

    // Platform
    platform: Option<Box<dyn kani_platform::Platform>>,

    // Modifier remapping
    peer_platforms: HashMap<HostId, OsType>,
    local_os: OsType,

    // Clipboard
    clipboard_monitor: Option<Box<dyn kani_clipboard::monitor::ClipboardMonitor>>,
    pub(crate) clipboard_sync: kani_clipboard::sync::ClipboardSync,
    clipboard_port: u16,
    peer_clipboard_addrs: HashMap<HostId, SocketAddr>,
    pub(crate) known_peer_ips: std::collections::HashSet<std::net::IpAddr>,

    // Mouse sensitivity (client-only, 1.0 = no adjustment)
    pub mouse_sensitivity: f64,
}

impl KvmEngine {
    /// Create engine from config. Builds topology, state machine, border resolver.
    pub fn new(
        config: KaniConfig,
        this_host_id: HostId,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let topologies = build_topologies(&config);
        let border_links = build_border_links(&config, &topologies);

        let server_topo = topologies
            .iter()
            .find(|t| t.host_id == this_host_id)
            .cloned()
            .unwrap_or_else(|| HostTopology::new(this_host_id, vec![]));

        let resolver = BorderLinkResolver::new(border_links);

        let state_machine = CursorStateMachine::new(this_host_id);

        // Initialize cursor at center of first display, or (0,0) if no displays
        let (cx, cy, cd) = if let Some(d) = server_topo.displays.first() {
            let r = d.logical_rect();
            (r.x + r.w / 2.0, r.y + r.h / 2.0, d.id)
        } else {
            (0.0, 0.0, 0)
        };
        let cursor = VirtualCursor::new(cx, cy, cd);

        // Build peer_platforms from config hosts
        let mut peer_platforms = HashMap::new();
        for host in &config.hosts {
            if host.host_id != this_host_id {
                if let Some(os) = OsType::from_platform_str(&host.platform) {
                    peer_platforms.insert(host.host_id, os);
                }
            }
        }

        let local_os = config
            .hosts
            .iter()
            .find(|h| h.host_id == this_host_id)
            .and_then(|h| OsType::from_platform_str(&h.platform))
            .unwrap_or_else(|| {
                OsType::from_platform_str(kani_proto::config::current_platform())
                    .unwrap_or(OsType::Windows)
            });

        let clipboard_port = config
            .server
            .clipboard_port
            .unwrap_or(config.server.bind_port + 1);

        Ok(Self {
            this_host_id,
            config,
            state_machine,
            cursor,
            resolver,
            server_topo,
            topologies,
            peers: HashMap::new(),
            transition_deadline: None,
            platform: None,
            peer_platforms,
            local_os,
            clipboard_monitor: None,
            clipboard_sync: kani_clipboard::sync::ClipboardSync::new(),
            clipboard_port,
            peer_clipboard_addrs: HashMap::new(),
            known_peer_ips: std::collections::HashSet::new(),
            mouse_sensitivity: 1.0,
        })
    }

    /// Add or update a peer's address (upsert semantics).
    ///
    /// NOTE: This updates the peer's network address but does NOT rebuild
    /// topology/BorderLinkResolver. If a peer reconnects with changed displays
    /// while KVM is active, cursor crossing uses stale topology data.
    /// Display changes during active KVM require a stop/restart cycle.
    pub fn add_peer(&mut self, host_id: HostId, addr: SocketAddr) {
        let clipboard_addr = SocketAddr::new(addr.ip(), self.clipboard_port);
        match self.peers.get_mut(&host_id) {
            Some(peer) => {
                peer.addr = addr;
                peer.last_heard = Instant::now();
            }
            None => {
                self.peers.insert(
                    host_id,
                    PeerState {
                        addr,
                        last_heard: Instant::now(),
                    },
                );
            }
        }
        self.peer_clipboard_addrs.insert(host_id, clipboard_addr);
        self.known_peer_ips.insert(addr.ip());
    }

    /// Remove a peer. Returns CursorReturnedToHost if cursor was on that peer.
    /// Idempotent -- removing a nonexistent peer returns empty actions.
    pub fn remove_peer(&mut self, host_id: HostId) -> Vec<KvmAction> {
        let mut actions = Vec::new();
        if self.peers.remove(&host_id).is_some() {
            if self.state_machine.active_host() == host_id {
                self.force_return_cursor_to_local();
                actions.push(KvmAction::CursorReturnedToHost(host_id));
            }
            actions.push(KvmAction::PeerDisconnected(host_id));
        }
        actions
    }

    /// Record that a peer is alive (called when management-level Ping is received).
    pub fn record_peer_alive(&mut self, host_id: HostId) {
        if let Some(peer) = self.peers.get_mut(&host_id) {
            peer.last_heard = Instant::now();
        }
    }

    /// Check heartbeat timeouts. If a peer with cursor is disconnected, returns
    /// CursorReturnedToHost (calls emergency_reset + restore_local_input internally).
    pub fn handle_heartbeat(&mut self) -> Vec<KvmAction> {
        let mut actions = Vec::new();
        let now = Instant::now();
        let timed_out: Vec<HostId> = self
            .peers
            .iter()
            .filter(|(_, p)| now.duration_since(p.last_heard) > HEARTBEAT_TIMEOUT)
            .map(|(id, _)| *id)
            .collect();

        for host_id in timed_out {
            if self.state_machine.active_host() == host_id {
                self.force_return_cursor_to_local();
                actions.push(KvmAction::CursorReturnedToHost(host_id));
            }
            self.peers.remove(&host_id);
            actions.push(KvmAction::PeerDisconnected(host_id));
        }
        actions
    }

    /// Get the current transition deadline (for select! timeout arm).
    pub fn transition_deadline(&self) -> Option<tokio::time::Instant> {
        self.transition_deadline
    }

    /// Access the platform (for server-level QueryDisplays handling).
    pub fn platform(&self) -> &Option<Box<dyn kani_platform::Platform>> {
        &self.platform
    }

    /// Iterate over all known peers and their addresses.
    pub fn peer_addresses(&self) -> impl Iterator<Item = (HostId, SocketAddr)> + '_ {
        self.peers.iter().map(|(id, state)| (*id, state.addr))
    }

    /// Iterate over all known peers and their clipboard addresses.
    pub fn clipboard_peer_addresses(&self) -> impl Iterator<Item = (HostId, SocketAddr)> + '_ {
        self.peer_clipboard_addrs
            .iter()
            .map(|(id, addr)| (*id, *addr))
    }

    /// Resolve HostId to SocketAddr for event routing.
    pub fn peer_addr(&self, host_id: &HostId) -> Option<SocketAddr> {
        self.peers.get(host_id).map(|p| p.addr)
    }

    /// Handle transition timeout (Enter not Ack'd within 500ms).
    pub fn handle_transition_timeout(&mut self) -> Vec<KvmAction> {
        self.state_machine.timeout();
        self.transition_deadline = None;
        restore_local_input(&self.platform);
        vec![KvmAction::StatusMessage(
            "Transition timed out, cursor reverted".into(),
        )]
    }

    /// Force cursor back to local host (emergency reset).
    fn force_return_cursor_to_local(&mut self) {
        self.state_machine.emergency_reset(self.this_host_id);
        self.transition_deadline = None;
        restore_local_input(&self.platform);
    }

    /// Start platform capture + clipboard. Returns KvmStartResult.
    /// Caller MUST hold the receivers separately (not through &mut self) for select! compatibility.
    pub async fn start(
        &mut self,
    ) -> Result<KvmStartResult, Box<dyn std::error::Error + Send + Sync>> {
        let platform = kani_platform::create_platform();
        if !platform.check_permissions() {
            platform.request_permissions();
            return Err("Permissions not granted".into());
        }

        // Sync cursor to physical position
        let (cx, cy) = platform.cursor_position();
        self.cursor.x = cx;
        self.cursor.y = cy;
        if let Some(d) = self
            .server_topo
            .displays
            .iter()
            .find(|d| d.logical_rect().contains(cx, cy))
        {
            self.cursor.current_display = d.id;
        }

        let platform_rx = platform.start_capture();
        self.platform = Some(platform);

        // Start clipboard monitor
        let clipboard_rx;
        #[cfg(target_os = "macos")]
        {
            let m = Box::new(kani_clipboard::monitor::macos::MacOSClipboardMonitor::new());
            clipboard_rx = m.start();
            self.clipboard_monitor = Some(m);
        }
        #[cfg(target_os = "windows")]
        {
            let m = Box::new(kani_clipboard::monitor::windows::WindowsClipboardMonitor::new());
            clipboard_rx = m.start();
            self.clipboard_monitor = Some(m);
        }

        // Bind clipboard receiver
        let clipboard_receiver = match kani_transport::tls_tcp::ClipboardReceiver::bind(&format!(
            "0.0.0.0:{}",
            self.clipboard_port
        ))
        .await
        {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!(error = %e, "Failed to bind clipboard receiver");
                None
            }
        };

        Ok(KvmStartResult {
            platform_rx,
            clipboard_rx,
            clipboard_receiver,
        })
    }

    /// Stop platform capture + clipboard. Unconditionally calls restore_local_input()
    /// (show_cursor + ungrab_input) regardless of CursorStateMachine state.
    pub fn stop(&mut self) {
        // Unconditionally restore input
        if let Some(ref platform) = self.platform {
            platform.show_cursor();
            platform.ungrab_input();
        }

        // Stop capture
        if let Some(ref platform) = self.platform {
            platform.stop_capture();
        }
        self.platform = None;

        // Stop clipboard
        if let Some(ref monitor) = self.clipboard_monitor {
            monitor.stop();
        }
        self.clipboard_monitor = None;
        self.clipboard_sync.clear();
        self.transition_deadline = None;
    }

    /// Process a local clipboard change detected by the clipboard monitor.
    /// Returns `true` if the change is genuine (should be broadcast to peers),
    /// or `false` if it is an echo of a recent remote write (should be suppressed).
    pub fn handle_clipboard_local_change(
        &mut self,
        msg: &kani_proto::clipboard::ClipboardMessage,
    ) -> bool {
        self.clipboard_sync.on_local_change(msg.clone())
    }

    /// Process a remote clipboard message received via TCP.
    /// Returns `true` if the message is from a known peer (should be written to local clipboard),
    /// or `false` if the sender IP is unknown (should be ignored).
    /// When returning true, also records the write for echo suppression.
    pub fn handle_clipboard_remote_receive(
        &mut self,
        msg: &kani_proto::clipboard::ClipboardMessage,
        from_ip: std::net::IpAddr,
    ) -> bool {
        if !self.known_peer_ips.contains(&from_ip) {
            tracing::warn!(%from_ip, "Clipboard from unknown peer, ignoring");
            return false;
        }
        self.clipboard_sync.record_remote_write(msg);
        true
    }

    /// Process a locally captured platform event (mouse/keyboard).
    pub fn handle_platform_event(&mut self, event_type: EventType) -> Vec<KvmAction> {
        let mut actions = Vec::new();
        let this_host_id = self.this_host_id;

        match self.state_machine.state().clone() {
            CursorState::Idle { active } if active == this_host_id => {
                // We are active host -- process locally
                match &event_type {
                    EventType::MouseMove { dx, dy } => {
                        // Resync VirtualCursor to actual physical cursor position
                        if let Some(ref platform) = self.platform {
                            let (px, py) = platform.cursor_position();
                            self.cursor.x = px;
                            self.cursor.y = py;
                            if let Some(d) = self
                                .server_topo
                                .displays
                                .iter()
                                .find(|d| d.logical_rect().contains(px, py))
                            {
                                self.cursor.current_display = d.id;
                            }
                        }
                        let result = self.cursor.apply_delta(*dx, *dy, &self.server_topo);
                        match &result {
                            MoveResult::ExitedEdge {
                                display_id,
                                edge,
                                along_edge,
                                ..
                            } => {
                                if let Some(target) = self.resolver.resolve(
                                    this_host_id,
                                    *display_id,
                                    *edge,
                                    *along_edge,
                                ) {
                                    match self.state_machine.begin_transition(target.target_host) {
                                        Ok(tid) => {
                                            let modifiers = self
                                                .platform
                                                .as_ref()
                                                .map(|p| p.modifier_state())
                                                .unwrap_or_default();
                                            let enter = InputEvent::new(
                                                this_host_id,
                                                EventType::Enter {
                                                    target_host: target.target_host,
                                                    landing_x: target.landing_x,
                                                    landing_y: target.landing_y,
                                                    landing_display: target.target_display,
                                                    modifiers,
                                                    transition_id: tid,
                                                },
                                            );
                                            actions.push(KvmAction::SendEvent {
                                                to: target.target_host,
                                                event: enter,
                                            });
                                            self.transition_deadline =
                                                Some(tokio::time::Instant::now() + ACK_TIMEOUT);
                                            suppress_local_input(&self.platform);
                                        }
                                        Err(e) => {
                                            tracing::warn!(error = %e, "Transition blocked");
                                        }
                                    }
                                } else {
                                    self.cursor.clamp_to(&self.server_topo);
                                }
                            }
                            MoveResult::InBounds => {}
                        }
                    }
                    _ => {
                        // Other local events while we're the active host -- no forwarding needed
                    }
                }
            }
            CursorState::Idle { active } => {
                // Cursor on a remote host -- forward captured event to that host
                let evt = InputEvent::new(this_host_id, event_type);
                actions.push(KvmAction::SendEvent {
                    to: active,
                    event: evt,
                });
            }
            CursorState::Transitioning { .. } => {
                tracing::trace!(event = %event_type, "Event dropped during transition");
            }
        }
        actions
    }

    /// Process a network event from a peer. Updates peer address mapping on each call.
    pub fn handle_network_event(
        &mut self,
        from: HostId,
        from_addr: SocketAddr,
        event: &InputEvent,
    ) -> Vec<KvmAction> {
        // Update peer addr and last_heard on every received packet
        if let Some(peer) = self.peers.get_mut(&from) {
            peer.addr = from_addr;
            peer.last_heard = Instant::now();
        }

        let mut actions = Vec::new();
        let this_host_id = self.this_host_id;

        match &event.event_type {
            EventType::MouseMove { dx, dy } => {
                if let CursorState::Idle { active } = self.state_machine.state() {
                    if *active == this_host_id {
                        // We're active -- check edge crossing
                        let sdx = dx * self.mouse_sensitivity;
                        let sdy = dy * self.mouse_sensitivity;
                        let result = self.cursor.apply_delta(sdx, sdy, &self.server_topo);
                        match result {
                            MoveResult::ExitedEdge {
                                display_id,
                                edge,
                                along_edge,
                                ..
                            } => {
                                if let Some(target) = self.resolver.resolve(
                                    this_host_id,
                                    display_id,
                                    edge,
                                    along_edge,
                                ) {
                                    match self.state_machine.begin_transition(target.target_host) {
                                        Ok(tid) => {
                                            let enter = InputEvent::new(
                                                this_host_id,
                                                EventType::Enter {
                                                    target_host: target.target_host,
                                                    landing_x: target.landing_x,
                                                    landing_y: target.landing_y,
                                                    landing_display: target.target_display,
                                                    modifiers: ModifierState::default(),
                                                    transition_id: tid,
                                                },
                                            );
                                            actions.push(KvmAction::SendEvent {
                                                to: target.target_host,
                                                event: enter,
                                            });
                                            self.transition_deadline =
                                                Some(tokio::time::Instant::now() + ACK_TIMEOUT);
                                            suppress_local_input(&self.platform);
                                        }
                                        Err(e) => {
                                            tracing::debug!(error = %e, "Transition blocked");
                                        }
                                    }
                                } else {
                                    self.cursor.clamp_to(&self.server_topo);
                                }
                            }
                            MoveResult::InBounds => {
                                // Warp cursor to new position
                                if let Some(ref platform) = self.platform {
                                    platform.warp_cursor(self.cursor.x, self.cursor.y);
                                }
                            }
                        }
                    } else {
                        // Forward to active host
                        actions.push(KvmAction::SendEvent {
                            to: *active,
                            event: event.clone(),
                        });
                    }
                }
            }

            // Remote host is sending cursor to us (we are the target of an Enter)
            EventType::Enter {
                target_host,
                landing_x,
                landing_y,
                landing_display,
                transition_id,
                ..
            } if *target_host == this_host_id => {
                if let Some(ref platform) = self.platform {
                    platform.wake_display();
                    platform.warp_cursor(*landing_x, *landing_y);
                }
                restore_local_input(&self.platform);
                // Update cursor position
                self.cursor.x = *landing_x;
                self.cursor.y = *landing_y;
                self.cursor.current_display = *landing_display;
                // Reset state machine so we are active
                self.state_machine.emergency_reset(this_host_id);
                self.transition_deadline = None;
                // Send Ack back
                let ack = InputEvent::new(
                    this_host_id,
                    EventType::Ack {
                        acked_seq: event.seq,
                        transition_id: *transition_id,
                    },
                );
                actions.push(KvmAction::SendToAddr {
                    addr: from_addr,
                    event: ack,
                });
                tracing::info!(
                    from = %event.host_id,
                    tid = transition_id,
                    "Received Enter targeting us, warped cursor, sent Ack"
                );
            }

            EventType::Ack { transition_id, .. } => {
                self.state_machine
                    .ack_received(event.host_id, *transition_id);
                self.transition_deadline = None;
                tracing::info!(
                    from = %event.host_id,
                    tid = transition_id,
                    "Ack received -- transition complete"
                );
            }

            EventType::Pong { .. } => {
                // Already updated last_heard above
                tracing::debug!(from = %event.host_id, "Pong received");
            }

            EventType::Ping { .. } => {
                // Already updated last_heard above
                // Reply with Pong
                let pong = InputEvent::new(this_host_id, EventType::Pong { payload: 0 });
                actions.push(KvmAction::SendToAddr {
                    addr: from_addr,
                    event: pong,
                });
            }

            // Forwarded input events from a remote host (we are the active host)
            EventType::KeyPress { .. }
            | EventType::MouseClick { .. }
            | EventType::MouseScroll { .. } => {
                if let CursorState::Idle { active } = self.state_machine.state() {
                    if *active == this_host_id {
                        // We are the active host -- inject the event locally
                        // Apply modifier remapping for cross-platform Ctrl<->Cmd
                        if let Some(ref platform) = self.platform {
                            let remapped = match self.peer_platforms.get(&event.host_id) {
                                Some(&src_os) => kani_proto::modifier_remap::remap_event(
                                    &event.event_type,
                                    src_os,
                                    self.local_os,
                                ),
                                None => event.event_type.clone(),
                            };
                            platform.inject_event(&remapped);
                        }
                    }
                }
            }

            EventType::RegisterClient { ref platform, .. } => {
                if let Some(os) = OsType::from_platform_str(platform) {
                    self.peer_platforms.insert(event.host_id, os);
                }
            }

            _ => {
                tracing::debug!(event_type = ?event.event_type, "Unhandled event type in KvmEngine");
            }
        }

        actions
    }
}

/// Suppress local input when cursor transitions to remote host.
fn suppress_local_input(active_platform: &Option<Box<dyn kani_platform::Platform>>) {
    if let Some(ref platform) = active_platform {
        platform.hide_cursor();
        platform.grab_input();
    }
}

/// Restore local input when cursor returns to local host.
fn restore_local_input(active_platform: &Option<Box<dyn kani_platform::Platform>>) {
    if let Some(ref platform) = active_platform {
        platform.show_cursor();
        platform.ungrab_input();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kani_proto::config::*;
    use uuid::Uuid;

    fn test_config() -> (KaniConfig, HostId) {
        let host_a = Uuid::from_u128(1);
        let host_b = Uuid::from_u128(2);
        let config = KaniConfig {
            server: ServerConfig {
                host_id: host_a,
                bind_port: 24900,
                clipboard_port: None,
            },
            gui: GuiConfig::default(),
            hosts: vec![
                HostConfig {
                    host_id: host_a,
                    name: "host-a".into(),
                    address: "127.0.0.1".into(),
                    platform: "windows".into(),
                    world_offset: None,
                    displays: vec![],
                },
                HostConfig {
                    host_id: host_b,
                    name: "host-b".into(),
                    address: "127.0.0.2".into(),
                    platform: "macos".into(),
                    world_offset: None,
                    displays: vec![],
                },
            ],
            border_links: vec![],
            trusted_peers: std::collections::HashMap::new(),
        };
        (config, host_a)
    }

    #[test]
    fn test_kvm_engine_new() {
        let (config, host_id) = test_config();
        let engine = KvmEngine::new(config, host_id);
        assert!(engine.is_ok());
    }

    #[test]
    fn test_add_peer_upsert() {
        let (config, host_id) = test_config();
        let mut engine = KvmEngine::new(config, host_id).unwrap();
        let peer = Uuid::from_u128(2);
        let addr1: SocketAddr = "127.0.0.2:12345".parse().unwrap();
        let addr2: SocketAddr = "127.0.0.2:54321".parse().unwrap();
        engine.add_peer(peer, addr1);
        assert_eq!(engine.peer_addr(&peer), Some(addr1));
        engine.add_peer(peer, addr2);
        assert_eq!(engine.peer_addr(&peer), Some(addr2));
    }

    #[test]
    fn test_heartbeat_alive_no_disconnect() {
        let (config, host_id) = test_config();
        let mut engine = KvmEngine::new(config, host_id).unwrap();
        let peer = Uuid::from_u128(2);
        engine.add_peer(peer, "127.0.0.2:24900".parse().unwrap());
        let actions = engine.handle_heartbeat();
        assert!(actions.is_empty());
    }

    #[test]
    fn test_heartbeat_disconnect_returns_cursor() {
        let (config, host_id) = test_config();
        let mut engine = KvmEngine::new(config, host_id).unwrap();
        let peer = Uuid::from_u128(2);
        engine.add_peer(peer, "127.0.0.2:24900".parse().unwrap());
        engine.state_machine.emergency_reset(peer);
        engine.peers.get_mut(&peer).unwrap().last_heard = Instant::now() - Duration::from_secs(6);
        let actions = engine.handle_heartbeat();
        assert!(actions
            .iter()
            .any(|a| matches!(a, KvmAction::CursorReturnedToHost(id) if *id == peer)));
        assert!(actions
            .iter()
            .any(|a| matches!(a, KvmAction::PeerDisconnected(id) if *id == peer)));
        assert_eq!(engine.state_machine.active_host(), host_id);
    }

    #[test]
    fn test_record_peer_alive_prevents_disconnect() {
        let (config, host_id) = test_config();
        let mut engine = KvmEngine::new(config, host_id).unwrap();
        let peer = Uuid::from_u128(2);
        engine.add_peer(peer, "127.0.0.2:24900".parse().unwrap());
        engine.peers.get_mut(&peer).unwrap().last_heard = Instant::now() - Duration::from_secs(2);
        engine.record_peer_alive(peer);
        assert!(engine.handle_heartbeat().is_empty());
    }

    #[test]
    fn test_remove_peer_idempotent() {
        let (config, host_id) = test_config();
        let mut engine = KvmEngine::new(config, host_id).unwrap();
        assert!(engine.remove_peer(Uuid::from_u128(99)).is_empty());
    }

    #[test]
    fn test_peer_addresses_iterator() {
        let (config, host_id) = test_config();
        let mut engine = KvmEngine::new(config, host_id).unwrap();
        let peer_a = Uuid::from_u128(2);
        let peer_b = Uuid::from_u128(3);
        let addr_a: SocketAddr = "127.0.0.2:24900".parse().unwrap();
        let addr_b: SocketAddr = "127.0.0.3:24900".parse().unwrap();
        engine.add_peer(peer_a, addr_a);
        engine.add_peer(peer_b, addr_b);
        let addrs: Vec<(HostId, SocketAddr)> = engine.peer_addresses().collect();
        assert_eq!(addrs.len(), 2);
        assert!(addrs.contains(&(peer_a, addr_a)));
        assert!(addrs.contains(&(peer_b, addr_b)));
    }

    #[test]
    fn test_clipboard_peer_addresses() {
        let (config, host_id) = test_config();
        let mut engine = KvmEngine::new(config, host_id).unwrap();
        let peer = Uuid::from_u128(2);
        let addr: SocketAddr = "127.0.0.2:24901".parse().unwrap();
        engine.peer_clipboard_addrs.insert(peer, addr);
        let addrs: Vec<(HostId, SocketAddr)> = engine.clipboard_peer_addresses().collect();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0], (peer, addr));
    }

    #[test]
    fn test_handle_clipboard_local_change_echo_suppression() {
        let (config, host_id) = test_config();
        let mut engine = KvmEngine::new(config, host_id).unwrap();
        let msg = kani_proto::clipboard::ClipboardMessage::Text("hello".into());
        // Genuine local change -> true
        assert!(engine.handle_clipboard_local_change(&msg));
        // Simulate remote write of same content (echo scenario)
        engine.clipboard_sync.record_remote_write(&msg);
        // Now same content from local monitor -> echo -> false
        assert!(!engine.handle_clipboard_local_change(&msg));
    }

    #[test]
    fn test_add_peer_populates_known_ips_and_clipboard_addr() {
        let (config, host_id) = test_config();
        let mut engine = KvmEngine::new(config, host_id).unwrap();
        let peer = Uuid::from_u128(2);
        let addr: SocketAddr = "192.168.1.20:24900".parse().unwrap();
        engine.add_peer(peer, addr);
        assert!(engine
            .known_peer_ips
            .contains(&"192.168.1.20".parse().unwrap()));
        let clip_addr = engine.peer_clipboard_addrs.get(&peer).unwrap();
        assert_eq!(clip_addr.port(), 24901);
        assert_eq!(clip_addr.ip(), addr.ip());
    }

    #[test]
    fn test_transition_timeout_reverts() {
        let (config, host_id) = test_config();
        let mut engine = KvmEngine::new(config, host_id).unwrap();
        let peer = Uuid::from_u128(2);
        engine.add_peer(peer, "127.0.0.2:24900".parse().unwrap());
        // Begin transition
        let _tid = engine.state_machine.begin_transition(peer).unwrap();
        engine.transition_deadline = Some(tokio::time::Instant::now());
        // Timeout
        engine.handle_transition_timeout();
        assert_eq!(engine.state_machine.active_host(), host_id);
        assert!(engine.transition_deadline.is_none());
    }

    // ---- RecordingPlatform mock (migrated from server.rs) ----

    use std::sync::{Arc, Mutex};

    struct RecordingPlatform {
        warp_calls: Arc<Mutex<Vec<(f64, f64)>>>,
        hide_count: Arc<Mutex<u32>>,
        show_count: Arc<Mutex<u32>>,
        grab_count: Arc<Mutex<u32>>,
        ungrab_count: Arc<Mutex<u32>>,
    }

    impl RecordingPlatform {
        #[allow(clippy::new_ret_no_self, clippy::type_complexity)]
        fn new() -> (
            Box<dyn kani_platform::Platform>,
            Arc<Mutex<Vec<(f64, f64)>>>,
            Arc<Mutex<u32>>,
            Arc<Mutex<u32>>,
            Arc<Mutex<u32>>,
            Arc<Mutex<u32>>,
        ) {
            let warp = Arc::new(Mutex::new(Vec::new()));
            let hide = Arc::new(Mutex::new(0u32));
            let show = Arc::new(Mutex::new(0u32));
            let grab = Arc::new(Mutex::new(0u32));
            let ungrab = Arc::new(Mutex::new(0u32));
            let p = Self {
                warp_calls: warp.clone(),
                hide_count: hide.clone(),
                show_count: show.clone(),
                grab_count: grab.clone(),
                ungrab_count: ungrab.clone(),
            };
            (Box::new(p), warp, hide, show, grab, ungrab)
        }
    }

    impl kani_platform::Platform for RecordingPlatform {
        fn enumerate_displays(&self) -> Vec<kani_platform::types::DisplayInfo> {
            vec![]
        }

        fn start_capture(&self) -> tokio::sync::mpsc::Receiver<EventType> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            rx
        }

        fn stop_capture(&self) {}

        fn inject_event(&self, _event: &EventType) {}

        fn warp_cursor(&self, x: f64, y: f64) {
            self.warp_calls.lock().unwrap().push((x, y));
        }

        fn cursor_position(&self) -> (f64, f64) {
            (0.0, 0.0)
        }

        fn show_cursor(&self) {
            *self.show_count.lock().unwrap() += 1;
        }

        fn hide_cursor(&self) {
            *self.hide_count.lock().unwrap() += 1;
        }

        fn grab_input(&self) {
            *self.grab_count.lock().unwrap() += 1;
        }

        fn ungrab_input(&self) {
            *self.ungrab_count.lock().unwrap() += 1;
        }

        fn modifier_state(&self) -> ModifierState {
            ModifierState::default()
        }

        fn check_permissions(&self) -> bool {
            true
        }

        fn request_permissions(&self) {}
    }

    // ---- Migrated tests from server.rs ----

    #[test]
    fn test_handle_network_event_mouse_move_warps_cursor() {
        use kani_proto::topology::Orientation;

        let host_id = Uuid::from_u128(1);
        let remote = Uuid::from_u128(2);
        let config = KaniConfig {
            server: ServerConfig {
                host_id,
                bind_port: 24900,
                clipboard_port: None,
            },
            gui: GuiConfig::default(),
            hosts: vec![HostConfig {
                host_id,
                name: "server".into(),
                address: "127.0.0.1".into(),
                platform: "windows".into(),
                world_offset: None,
                displays: vec![DisplayConfig {
                    id: 0,
                    origin: [0, 0],
                    resolution: [1920, 1080],
                    scale_factor: 1.0,
                    orientation: Orientation::Normal,
                }],
            }],
            border_links: vec![],
            trusted_peers: std::collections::HashMap::new(),
        };
        let mut engine = KvmEngine::new(config, host_id).unwrap();
        let from_addr: SocketAddr = "127.0.0.2:24900".parse().unwrap();
        engine.add_peer(remote, from_addr);
        // Set cursor to known position and inject mock platform
        engine.cursor = VirtualCursor::new(100.0, 100.0, 0);
        let (platform, warp_calls, _, _, _, _) = RecordingPlatform::new();
        engine.platform = Some(platform);

        let event = InputEvent::new(remote, EventType::MouseMove { dx: 50.0, dy: 30.0 });
        let _actions = engine.handle_network_event(remote, from_addr, &event);

        let calls = warp_calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "warp_cursor should be called exactly once");
        assert!(
            (calls[0].0 - 150.0).abs() < 0.01,
            "warp x={}, expected 150.0",
            calls[0].0
        );
        assert!(
            (calls[0].1 - 130.0).abs() < 0.01,
            "warp y={}, expected 130.0",
            calls[0].1
        );
    }

    #[test]
    fn test_handle_network_event_enter_restores_input() {
        let (config, host_id) = test_config();
        let mut engine = KvmEngine::new(config, host_id).unwrap();
        let remote = Uuid::from_u128(2);
        let from_addr: SocketAddr = "127.0.0.2:24900".parse().unwrap();
        engine.add_peer(remote, from_addr);
        let (platform, _, _, show, _, ungrab) = RecordingPlatform::new();
        engine.platform = Some(platform);
        // Simulate cursor on remote: transition and ack
        let tid = engine.state_machine.begin_transition(remote).unwrap();
        engine.state_machine.ack_received(remote, tid);

        let enter = InputEvent::new(
            remote,
            EventType::Enter {
                target_host: host_id,
                landing_x: 100.0,
                landing_y: 200.0,
                landing_display: 0,
                modifiers: ModifierState::default(),
                transition_id: 99,
            },
        );
        let _actions = engine.handle_network_event(remote, from_addr, &enter);

        assert!(*show.lock().unwrap() > 0, "show_cursor must be called");
        assert!(*ungrab.lock().unwrap() > 0, "ungrab_input must be called");
        assert_eq!(engine.state_machine.active_host(), host_id);
    }

    #[test]
    fn test_handle_clipboard_remote_receive_rejects_unknown_ip() {
        let (config, host_id) = test_config();
        let mut engine = KvmEngine::new(config, host_id).unwrap();
        let peer = Uuid::from_u128(2);
        engine.add_peer(peer, "192.168.1.20:24900".parse().unwrap());
        let msg = kani_proto::clipboard::ClipboardMessage::Text("hello".into());
        // Unknown IP should be rejected
        let unknown_ip: std::net::IpAddr = "10.0.0.99".parse().unwrap();
        assert!(
            !engine.handle_clipboard_remote_receive(&msg, unknown_ip),
            "Unknown peer IP should be rejected"
        );
    }

    #[test]
    fn test_handle_clipboard_remote_receive_accepts_known_ip_and_suppresses_echo() {
        let (config, host_id) = test_config();
        let mut engine = KvmEngine::new(config, host_id).unwrap();
        let peer = Uuid::from_u128(2);
        engine.add_peer(peer, "192.168.1.20:24900".parse().unwrap());
        let msg = kani_proto::clipboard::ClipboardMessage::Text("hello".into());
        let known_ip: std::net::IpAddr = "192.168.1.20".parse().unwrap();
        // Known IP should be accepted
        assert!(
            engine.handle_clipboard_remote_receive(&msg, known_ip),
            "Known peer IP should be accepted"
        );
        // After remote receive, local change with same content should be suppressed (echo)
        assert!(
            !engine.handle_clipboard_local_change(&msg),
            "Echo of remote clipboard should be suppressed"
        );
    }

    #[test]
    fn test_mouse_sensitivity_scales_network_mouse_move() {
        use kani_proto::topology::Orientation;

        let host_id = Uuid::from_u128(1);
        let remote = Uuid::from_u128(2);
        let config = KaniConfig {
            server: ServerConfig {
                host_id,
                bind_port: 24900,
                clipboard_port: None,
            },
            gui: GuiConfig::default(),
            hosts: vec![HostConfig {
                host_id,
                name: "server".into(),
                address: "127.0.0.1".into(),
                platform: "windows".into(),
                world_offset: None,
                displays: vec![DisplayConfig {
                    id: 0,
                    origin: [0, 0],
                    resolution: [1920, 1080],
                    scale_factor: 1.0,
                    orientation: Orientation::Normal,
                }],
            }],
            border_links: vec![],
            trusted_peers: std::collections::HashMap::new(),
        };
        let mut engine = KvmEngine::new(config, host_id).unwrap();
        let from_addr: SocketAddr = "127.0.0.2:24900".parse().unwrap();
        engine.add_peer(remote, from_addr);
        engine.cursor = VirtualCursor::new(100.0, 100.0, 0);
        let (platform, warp_calls, _, _, _, _) = RecordingPlatform::new();
        engine.platform = Some(platform);

        // Set sensitivity to 0.5 (half speed)
        engine.mouse_sensitivity = 0.5;

        let event = InputEvent::new(remote, EventType::MouseMove { dx: 50.0, dy: 30.0 });
        let _actions = engine.handle_network_event(remote, from_addr, &event);

        let calls = warp_calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "warp_cursor should be called exactly once");
        // 100 + 50*0.5 = 125, 100 + 30*0.5 = 115
        assert!(
            (calls[0].0 - 125.0).abs() < 0.01,
            "warp x={}, expected 125.0",
            calls[0].0
        );
        assert!(
            (calls[0].1 - 115.0).abs() < 0.01,
            "warp y={}, expected 115.0",
            calls[0].1
        );
    }
}
