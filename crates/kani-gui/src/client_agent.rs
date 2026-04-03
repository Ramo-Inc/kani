//! ClientAgent — tokio-based client that handles BOTH management
//! (RegisterClient, LayoutSync, Ping) and KVM (via KvmEngine) on a single UDP socket.
//!
//! Replaces `client_thread.rs`. Always runs when role == Client.

use kani_proto::codec::{decode, encode, MAX_LAYOUT_SIZE};
use kani_proto::config::KaniConfig;
use kani_proto::event::*;
use kani_server::kvm_engine::{KvmAction, KvmEngine};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Handle for the GUI thread to communicate with the ClientAgent background thread.
pub struct ClientAgentHandle {
    pub event_rx: std::sync::mpsc::Receiver<ClientEvent>,
    pub cancel: CancellationToken,
    thread: Option<std::thread::JoinHandle<()>>,
    pub sensitivity: Arc<AtomicU64>,
}

/// Events sent from ClientAgent to GUI.
#[derive(Debug)]
pub enum ClientEvent {
    LayoutReceived(HostId, Vec<HostLayout>, Vec<BorderLinkLayout>),
    KvmStarted,
    KvmStopped,
    Disconnected,
    Error(String),
}

// ---------------------------------------------------------------------------
// ClientAgentHandle methods
// ---------------------------------------------------------------------------

impl ClientAgentHandle {
    /// Non-blocking poll for the next client event.
    pub fn try_recv_event(&self) -> Option<ClientEvent> {
        self.event_rx.try_recv().ok()
    }

    /// Update mouse sensitivity (called from GUI thread, read by engine on next event).
    pub fn set_mouse_sensitivity(&self, value: f64) {
        self.sensitivity.store(value.to_bits(), Ordering::Relaxed);
    }

    /// Stop the agent: cancel and join the thread.
    #[allow(dead_code)] // used in tests
    pub fn stop(&mut self) {
        self.cancel.cancel();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ClientAgentHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

// ---------------------------------------------------------------------------
// start_client_agent
// ---------------------------------------------------------------------------

/// Start the ClientAgent on a background thread with its own tokio runtime.
pub fn start_client_agent(
    host_addr: SocketAddr,
    client_id: HostId,
    client_name: String,
    client_displays: Vec<DisplaySnapshot>,
    mouse_sensitivity: f64,
) -> Result<ClientAgentHandle, String> {
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    let sensitivity = Arc::new(AtomicU64::new(mouse_sensitivity.to_bits()));
    let sensitivity_clone = sensitivity.clone();

    let (evt_tx, evt_rx) = std::sync::mpsc::channel::<ClientEvent>();

    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        rt.block_on(run_client(
            host_addr,
            client_id,
            client_name,
            client_displays,
            evt_tx,
            cancel_clone,
            sensitivity_clone,
        ));
    });

    Ok(ClientAgentHandle {
        event_rx: evt_rx,
        cancel,
        thread: Some(thread),
        sensitivity,
    })
}

// ---------------------------------------------------------------------------
// Async client loop
// ---------------------------------------------------------------------------

async fn run_client(
    host_addr: SocketAddr,
    client_id: HostId,
    client_name: String,
    client_displays: Vec<DisplaySnapshot>,
    evt_tx: std::sync::mpsc::Sender<ClientEvent>,
    cancel: CancellationToken,
    sensitivity: Arc<AtomicU64>,
) {
    let socket = match UdpSocket::bind("0.0.0.0:0").await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to bind client socket");
            let _ = evt_tx.send(ClientEvent::Error(format!("Bind failed: {e}")));
            return;
        }
    };

    let mut buf = [0u8; MAX_LAYOUT_SIZE];

    // Send initial RegisterClient
    send_register(
        &socket,
        host_addr,
        client_id,
        &client_name,
        &client_displays,
    )
    .await;

    let mut kvm: Option<KvmEngine> = None;
    let mut platform_rx: Option<tokio::sync::mpsc::Receiver<EventType>> = None;
    let mut clipboard_rx: Option<
        tokio::sync::mpsc::Receiver<kani_proto::clipboard::ClipboardMessage>,
    > = None;
    let mut clipboard_receiver: Option<kani_transport::tls_tcp::ClipboardReceiver> = None;
    let mut last_layout: Option<(Vec<HostLayout>, Vec<BorderLinkLayout>)> = None;
    let mut pending_kvm_start = false;
    let mut host_kvm_id: Option<HostId> = None;

    let mut ping_interval = tokio::time::interval(Duration::from_secs(3));
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut register_retry_interval = tokio::time::interval(Duration::from_secs(6));
    register_retry_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(1));
    heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    tracing::info!(%host_addr, "ClientAgent started");

    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled() => break,

            // ---------- Receive from Host ----------
            result = socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, from_addr)) => {
                        if let Ok(event) = decode(&buf[..len]) {
                            match &event.event_type {
                                // === LayoutSync ===
                                EventType::LayoutSync { hosts, border_links } => {
                                    tracing::info!(
                                        hosts = hosts.len(),
                                        links = border_links.len(),
                                        "LayoutSync received from host"
                                    );
                                    last_layout = Some((hosts.clone(), border_links.clone()));
                                    let _ = evt_tx.send(ClientEvent::LayoutReceived(
                                        event.host_id,
                                        hosts.clone(),
                                        border_links.clone(),
                                    ));
                                    // If StartKvm was received before LayoutSync, start now
                                    if pending_kvm_start && kvm.is_none() {
                                        start_client_kvm(
                                            &mut kvm,
                                            &mut platform_rx,
                                            &mut clipboard_rx,
                                            &mut clipboard_receiver,
                                            client_id,
                                            last_layout.as_ref().unwrap(),
                                            host_kvm_id.unwrap_or(event.host_id),
                                            host_addr,
                                            &evt_tx,
                                            &sensitivity,
                                        )
                                        .await;
                                        pending_kvm_start = false;
                                    }
                                }

                                // === StartKvm ===
                                EventType::StartKvm => {
                                    if kvm.is_none() {
                                        if let Some(ref layout) = last_layout {
                                            let layout_clone = layout.clone();
                                            start_client_kvm(
                                                &mut kvm,
                                                &mut platform_rx,
                                                &mut clipboard_rx,
                                                &mut clipboard_receiver,
                                                client_id,
                                                &layout_clone,
                                                event.host_id,
                                                host_addr,
                                                &evt_tx,
                                                &sensitivity,
                                            )
                                            .await;
                                        } else {
                                            // LayoutSync not yet received — defer KVM start
                                            tracing::info!("StartKvm received before LayoutSync, setting pending flag");
                                            pending_kvm_start = true;
                                            host_kvm_id = Some(event.host_id);
                                        }
                                    }
                                }

                                // === StopKvm ===
                                EventType::StopKvm => {
                                    if let Some(mut engine) = kvm.take() {
                                        engine.stop();
                                    }
                                    platform_rx = None;
                                    clipboard_rx = None;
                                    clipboard_receiver = None;
                                    pending_kvm_start = false;
                                    let _ = evt_tx.send(ClientEvent::KvmStopped);
                                    tracing::info!("KVM stopped by host command");
                                }

                                // === KVM events (only when engine active) ===
                                _ if kvm.is_some() => {
                                    let engine = kvm.as_mut().unwrap();
                                    engine.mouse_sensitivity = f64::from_bits(sensitivity.load(Ordering::Relaxed));
                                    let actions = engine.handle_network_event(event.host_id, from_addr, &event);
                                    execute_actions(&socket, &kvm, &actions).await;
                                }

                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "UDP recv error");
                    }
                }
            }

            // ---------- Platform input (separate receiver) ----------
            Some(evt) = async {
                match platform_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(ref mut engine) = kvm {
                    let actions = engine.handle_platform_event(evt);
                    execute_actions(&socket, &kvm, &actions).await;
                }
            }

            // ---------- Local clipboard changes ----------
            Some(clip_msg) = async {
                match clipboard_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(ref mut engine) = kvm {
                    if engine.handle_clipboard_local_change(&clip_msg) {
                        let addrs: Vec<(HostId, SocketAddr)> =
                            engine.clipboard_peer_addresses().collect();
                        tracing::info!(peer_count = addrs.len(), "Local clipboard changed, syncing to peers");
                        for (peer_id, addr) in addrs {
                            let msg = clip_msg.clone();
                            tokio::spawn(async move {
                                let connect_result = tokio::time::timeout(
                                    Duration::from_secs(2),
                                    kani_transport::tls_tcp::ClipboardSender::connect(
                                        &addr.to_string(),
                                    ),
                                )
                                .await;
                                match connect_result {
                                    Ok(Ok(mut sender)) => match sender.send(&msg).await {
                                        Ok(()) => tracing::info!(%peer_id, %addr, "Clipboard sent to peer"),
                                        Err(e) => tracing::warn!(%peer_id, error = %e, "Clipboard send failed"),
                                    },
                                    Ok(Err(e)) => tracing::warn!(%peer_id, %addr, error = %e, "Clipboard TCP connect failed"),
                                    Err(_) => tracing::warn!(%peer_id, %addr, "Clipboard TCP connect timed out (2s)"),
                                }
                            });
                        }
                    } else {
                        tracing::debug!("Clipboard change suppressed by echo filter");
                    }
                }
            }

            // ---------- Remote clipboard ----------
            result = async {
                match clipboard_receiver.as_ref() {
                    Some(rx) => rx.accept_and_recv().await,
                    None => std::future::pending().await,
                }
            }, if clipboard_receiver.is_some() => {
                match result {
                    Ok((msg, from_addr)) => {
                        if let Some(ref mut engine) = kvm {
                            if engine.handle_clipboard_remote_receive(&msg, from_addr.ip()) {
                                tracing::info!(%from_addr, "Remote clipboard received, writing locally");
                                let kani_proto::clipboard::ClipboardMessage::Text(ref text) = msg;
                                match tokio::time::timeout(
                                    Duration::from_secs(2),
                                    kani_clipboard::write_to_clipboard(text),
                                )
                                .await
                                {
                                    Ok(Ok(())) => {}
                                    Ok(Err(e)) => tracing::warn!(error = %e, "Failed to write clipboard locally"),
                                    Err(_) => tracing::warn!("Clipboard write timed out (2s)"),
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Clipboard recv error");
                    }
                }
            }

            // ---------- Ping every 3s ----------
            _ = ping_interval.tick() => {
                let ping = InputEvent::new(client_id, EventType::Ping { payload: 0 });
                if let Ok(bytes) = encode(&ping) {
                    let _ = socket.send_to(&bytes, host_addr).await;
                }
            }

            // ---------- Re-register if no layout for 6s ----------
            _ = register_retry_interval.tick(), if last_layout.is_none() => {
                tracing::info!("No LayoutSync received, re-sending RegisterClient");
                send_register(&socket, host_addr, client_id, &client_name, &client_displays).await;
            }

            // ---------- Heartbeat check (KVM active) ----------
            _ = heartbeat_interval.tick(), if kvm.is_some() => {
                let actions = kvm.as_mut().unwrap().handle_heartbeat();
                // Process ALL actions before checking if we should stop
                let should_stop = actions.iter().any(|a| matches!(a, KvmAction::PeerDisconnected(_)));
                execute_actions(&socket, &kvm, &actions).await;
                if should_stop {
                    if let Some(mut engine) = kvm.take() {
                        engine.stop();
                    }
                    platform_rx = None;
                    clipboard_rx = None;
                    clipboard_receiver = None;
                    let _ = evt_tx.send(ClientEvent::KvmStopped);
                    let _ = evt_tx.send(ClientEvent::Disconnected);
                    last_layout = None; // Will retry registration
                    tracing::info!("Host disconnected, KVM stopped, will re-register");
                }
            }

            // ---------- Transition timeout ----------
            _ = async {
                match kvm.as_ref().and_then(|e| e.transition_deadline()) {
                    Some(dl) => tokio::time::sleep_until(dl).await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(ref mut engine) = kvm {
                    let actions = engine.handle_transition_timeout();
                    execute_actions(&socket, &kvm, &actions).await;
                }
            }
        }
    }

    // Cleanup: send Leave to host so it can disconnect immediately
    let leave = InputEvent::new(client_id, EventType::Leave);
    if let Ok(bytes) = encode(&leave) {
        let _ = socket.send_to(&bytes, host_addr).await;
    }
    if let Some(mut engine) = kvm.take() {
        engine.stop();
    }
    tracing::info!("ClientAgent stopped");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Send a RegisterClient message to the host.
async fn send_register(
    socket: &UdpSocket,
    host_addr: SocketAddr,
    client_id: HostId,
    client_name: &str,
    client_displays: &[DisplaySnapshot],
) {
    let reg = InputEvent::new(
        client_id,
        EventType::RegisterClient {
            client_name: client_name.to_string(),
            platform: kani_proto::config::current_platform().into(),
            displays: client_displays.to_vec(),
        },
    );
    if let Ok(bytes) = encode(&reg) {
        match socket.send_to(&bytes, host_addr).await {
            Ok(_) => tracing::info!(%host_addr, "RegisterClient sent"),
            Err(e) => tracing::warn!(error = %e, %host_addr, "Failed to send RegisterClient"),
        }
    }
}

/// Convert a border link range from world coordinates to host-local coordinates
/// by subtracting the host's world_offset.
fn subtract_world_offset(
    edge: kani_proto::topology::Edge,
    range: [f64; 2],
    hosts: &[HostLayout],
    host_id: HostId,
) -> [f64; 2] {
    use kani_proto::topology::Edge;
    let (offset_x, offset_y) = hosts
        .iter()
        .find(|h| h.host_id == host_id)
        .map(|h| (h.world_offset_x, h.world_offset_y))
        .unwrap_or((0.0, 0.0));
    match edge {
        Edge::Left | Edge::Right => [range[0] - offset_y, range[1] - offset_y],
        Edge::Top | Edge::Bottom => [range[0] - offset_x, range[1] - offset_x],
    }
}

/// Build a KaniConfig from LayoutSync data.
fn build_config_from_layout(
    client_id: HostId,
    hosts: &[HostLayout],
    border_links: &[BorderLinkLayout],
    host_addr: SocketAddr,
) -> KaniConfig {
    use kani_proto::config::*;
    use kani_proto::topology::Orientation;

    let host_configs = hosts
        .iter()
        .map(|h| HostConfig {
            host_id: h.host_id,
            name: h.name.clone(),
            address: if h.host_id == client_id {
                "127.0.0.1".into()
            } else {
                host_addr.ip().to_string()
            },
            platform: h.platform.clone(),
            world_offset: Some([h.world_offset_x, h.world_offset_y]),
            displays: h
                .displays
                .iter()
                .map(|d| DisplayConfig {
                    id: d.id,
                    origin: [d.origin_x as i32, d.origin_y as i32],
                    resolution: [d.width_pixels, d.height_pixels],
                    scale_factor: d.scale_factor,
                    orientation: Orientation::Normal,
                })
                .collect(),
        })
        .collect();

    let border_configs = border_links
        .iter()
        .map(|l| {
            // LayoutSync carries ranges in world coordinates.
            // Convert to host-local coordinates by subtracting each host's world_offset.
            let from_range = subtract_world_offset(l.from_edge, l.from_range, hosts, l.from_host);
            let to_range = subtract_world_offset(l.to_edge, l.to_range, hosts, l.to_host);
            BorderLinkConfig {
                from_host: l.from_host,
                from_display: l.from_display,
                from_edge: l.from_edge,
                from_range,
                from_edge_coord: None,
                to_host: l.to_host,
                to_display: l.to_display,
                to_edge: l.to_edge,
                to_range,
                to_edge_coord: None,
                mapping: l.mapping,
            }
        })
        .collect();

    let clipboard_port = host_addr.port() + 1;

    KaniConfig {
        server: ServerConfig {
            host_id: client_id,
            bind_port: 0,
            clipboard_port: Some(clipboard_port),
        },
        gui: GuiConfig::default(),
        hosts: host_configs,
        border_links: border_configs,
        trusted_peers: HashMap::new(),
    }
}

/// Start KVM engine on the client side.
///
/// This is called when both a layout and StartKvm are available.
/// Errors are handled gracefully — emits ClientEvent::Error instead of panicking.
#[allow(clippy::too_many_arguments)]
async fn start_client_kvm(
    kvm: &mut Option<KvmEngine>,
    platform_rx: &mut Option<tokio::sync::mpsc::Receiver<EventType>>,
    clipboard_rx: &mut Option<tokio::sync::mpsc::Receiver<kani_proto::clipboard::ClipboardMessage>>,
    clipboard_receiver: &mut Option<kani_transport::tls_tcp::ClipboardReceiver>,
    client_id: HostId,
    layout: &(Vec<HostLayout>, Vec<BorderLinkLayout>),
    host_id: HostId,
    host_addr: SocketAddr,
    evt_tx: &std::sync::mpsc::Sender<ClientEvent>,
    sensitivity: &Arc<AtomicU64>,
) {
    let config = build_config_from_layout(client_id, &layout.0, &layout.1, host_addr);
    let mut engine = match KvmEngine::new(config, client_id) {
        Ok(e) => e,
        Err(e) => {
            let msg = format!("Failed to create KVM engine: {e}");
            tracing::warn!("{}", msg);
            let _ = evt_tx.send(ClientEvent::Error(msg));
            return;
        }
    };
    engine.add_peer(host_id, host_addr);
    engine.mouse_sensitivity = f64::from_bits(sensitivity.load(Ordering::Relaxed));
    match engine.start().await {
        Ok(result) => {
            *platform_rx = Some(result.platform_rx);
            *clipboard_rx = Some(result.clipboard_rx);
            *clipboard_receiver = result.clipboard_receiver;
            *kvm = Some(engine);
            let _ = evt_tx.send(ClientEvent::KvmStarted);
            tracing::info!("Client KVM started");
        }
        Err(e) => {
            let msg = format!("Failed to start KVM: {e}");
            tracing::warn!("{}", msg);
            let _ = evt_tx.send(ClientEvent::Error(msg));
        }
    }
}

/// Execute KvmActions by sending events over the socket.
async fn execute_actions(socket: &UdpSocket, kvm: &Option<KvmEngine>, actions: &[KvmAction]) {
    for action in actions {
        match action {
            KvmAction::SendEvent { to, event } => {
                if let Some(ref engine) = kvm {
                    if let Some(addr) = engine.peer_addr(to) {
                        if let Ok(bytes) = encode(event) {
                            let _ = socket.send_to(&bytes, addr).await;
                        }
                    }
                }
            }
            KvmAction::SendToAddr { addr, event } => {
                if let Ok(bytes) = encode(event) {
                    let _ = socket.send_to(&bytes, *addr).await;
                }
            }
            KvmAction::PeerDisconnected(id) => {
                tracing::info!(peer = %id, "Peer disconnected");
            }
            KvmAction::CursorReturnedToHost(id) => {
                tracing::info!(peer = %id, "Cursor returned to host");
            }
            KvmAction::StatusMessage(msg) => {
                tracing::info!(msg = %msg, "KVM status");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use kani_proto::codec::{decode, encode, MAX_LAYOUT_SIZE};
    use kani_proto::event::{DisplaySnapshot, EventType, HostId, HostLayout, InputEvent};
    use std::net::UdpSocket;
    use std::time::Duration;

    #[test]
    fn test_client_agent_start_stop() {
        // Start agent pointing at a nonexistent host (random port)
        let fake_addr: SocketAddr = "127.0.0.1:19999".parse().unwrap();
        let mut handle = start_client_agent(
            fake_addr,
            HostId::new_v4(),
            "test-client".into(),
            vec![],
            1.0,
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        handle.stop();
    }

    #[test]
    fn test_client_agent_receives_layout() {
        // Create a fake host UDP socket
        let host_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        host_socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let host_port = host_socket.local_addr().unwrap().port();
        let host_id = HostId::new_v4();
        let host_addr: SocketAddr = format!("127.0.0.1:{host_port}").parse().unwrap();

        let client_id = HostId::new_v4();
        let handle =
            start_client_agent(host_addr, client_id, "test-client".into(), vec![], 1.0).unwrap();

        // Host receives RegisterClient
        let mut buf = [0u8; MAX_LAYOUT_SIZE];
        let (len, client_addr) = host_socket.recv_from(&mut buf).unwrap();
        let event = decode(&buf[..len]).unwrap();
        assert!(
            matches!(event.event_type, EventType::RegisterClient { .. }),
            "Expected RegisterClient, got: {}",
            event.event_type
        );

        // Host sends LayoutSync back
        let layout = InputEvent::new(
            host_id,
            EventType::LayoutSync {
                hosts: vec![HostLayout {
                    host_id,
                    name: "host".into(),
                    address: "192.168.1.10".into(),
                    platform: "macos".into(),
                    world_offset_x: 0.0,
                    world_offset_y: 0.0,
                    displays: vec![],
                }],
                border_links: vec![],
            },
        );
        let bytes = encode(&layout).unwrap();
        host_socket.send_to(&bytes, client_addr).unwrap();

        // Wait for client agent to process
        std::thread::sleep(Duration::from_millis(300));

        // Check event_rx received LayoutReceived
        let event = handle.try_recv_event();
        assert!(event.is_some(), "Expected LayoutReceived event");
        match event.unwrap() {
            ClientEvent::LayoutReceived(_host_id, hosts, _links) => {
                assert_eq!(hosts.len(), 1);
                assert_eq!(hosts[0].name, "host");
            }
            other => panic!("Expected LayoutReceived, got: {other:?}"),
        }

        drop(handle);
    }

    #[test]
    fn test_client_agent_start_kvm_before_layout() {
        // Create fake host
        let host_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        host_socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let host_port = host_socket.local_addr().unwrap().port();
        let host_id = HostId::new_v4();
        let host_addr: SocketAddr = format!("127.0.0.1:{host_port}").parse().unwrap();

        let client_id = HostId::new_v4();
        let handle = start_client_agent(
            host_addr,
            client_id,
            "test-client".into(),
            vec![DisplaySnapshot {
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
            1.0,
        )
        .unwrap();

        // Host receives RegisterClient
        let mut buf = [0u8; MAX_LAYOUT_SIZE];
        let (len, client_addr) = host_socket.recv_from(&mut buf).unwrap();
        let event = decode(&buf[..len]).unwrap();
        assert!(matches!(event.event_type, EventType::RegisterClient { .. }));

        // Send StartKvm FIRST (before LayoutSync) — tests pending_kvm_start flag
        let start_kvm = InputEvent::new(host_id, EventType::StartKvm);
        let bytes = encode(&start_kvm).unwrap();
        host_socket.send_to(&bytes, client_addr).unwrap();

        // Brief pause to let StartKvm arrive
        std::thread::sleep(Duration::from_millis(100));

        // Then send LayoutSync with both host and client
        let layout = InputEvent::new(
            host_id,
            EventType::LayoutSync {
                hosts: vec![
                    HostLayout {
                        host_id,
                        name: "host".into(),
                        address: "192.168.1.10".into(),
                        platform: "windows".into(),
                        world_offset_x: 0.0,
                        world_offset_y: 0.0,
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
                    HostLayout {
                        host_id: client_id,
                        name: "test-client".into(),
                        address: "127.0.0.1".into(),
                        platform: kani_proto::config::current_platform().into(),
                        world_offset_x: 1920.0,
                        world_offset_y: 0.0,
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
                ],
                border_links: vec![],
            },
        );
        let bytes = encode(&layout).unwrap();
        host_socket.send_to(&bytes, client_addr).unwrap();

        // Wait for processing
        std::thread::sleep(Duration::from_millis(500));

        // Verify ClientEvent::LayoutReceived then ClientEvent::KvmStarted
        // (KVM auto-starts because pending flag was set)
        let mut saw_layout = false;
        let mut saw_kvm_started = false;
        let mut saw_error = false;
        for _ in 0..10 {
            while let Some(event) = handle.try_recv_event() {
                match event {
                    ClientEvent::LayoutReceived(..) => saw_layout = true,
                    ClientEvent::KvmStarted => saw_kvm_started = true,
                    ClientEvent::Error(e) => {
                        // On CI or restricted environments, KVM start may fail
                        // due to permissions — this is acceptable
                        tracing::warn!("KVM start error (may be expected in test): {e}");
                        saw_error = true;
                    }
                    _ => {}
                }
            }
            if saw_layout && (saw_kvm_started || saw_error) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        assert!(saw_layout, "Expected LayoutReceived event");
        // KVM start may fail in test environments (no display/permissions),
        // but the pending flag mechanism was exercised: either KvmStarted or Error
        assert!(
            saw_kvm_started || saw_error,
            "Expected KvmStarted or Error event after pending_kvm_start"
        );

        drop(handle);
    }

    #[test]
    fn test_build_config_from_layout() {
        let client_id = HostId::new_v4();
        let host_id = HostId::new_v4();
        let host_addr: SocketAddr = "192.168.1.10:24900".parse().unwrap();

        let hosts = vec![
            HostLayout {
                host_id,
                name: "host".into(),
                address: "192.168.1.10".into(),
                platform: "windows".into(),
                world_offset_x: 0.0,
                world_offset_y: 0.0,
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
            HostLayout {
                host_id: client_id,
                name: "client".into(),
                address: "127.0.0.1".into(),
                platform: "macos".into(),
                world_offset_x: 1920.0,
                world_offset_y: 0.0,
                displays: vec![DisplaySnapshot {
                    id: 0,
                    origin_x: 0.0,
                    origin_y: 0.0,
                    width: 2560.0,
                    height: 1440.0,
                    width_pixels: 2560,
                    height_pixels: 1440,
                    scale_factor: 2.0,
                    is_primary: true,
                }],
            },
        ];

        let config = build_config_from_layout(client_id, &hosts, &[], host_addr);

        // server.host_id should be client_id
        assert_eq!(config.server.host_id, client_id);
        assert_eq!(config.server.bind_port, 0);

        // 2 hosts
        assert_eq!(config.hosts.len(), 2);

        // Host's address should be the host IP
        let host_cfg = config.hosts.iter().find(|h| h.host_id == host_id).unwrap();
        assert_eq!(host_cfg.address, "192.168.1.10");

        // Client's address should be 127.0.0.1
        let client_cfg = config
            .hosts
            .iter()
            .find(|h| h.host_id == client_id)
            .unwrap();
        assert_eq!(client_cfg.address, "127.0.0.1");

        // Displays should be mapped correctly
        assert_eq!(host_cfg.displays.len(), 1);
        assert_eq!(host_cfg.displays[0].resolution, [1920, 1080]);
        assert_eq!(client_cfg.displays.len(), 1);
        assert_eq!(client_cfg.displays[0].resolution, [2560, 1440]);
        assert!((client_cfg.displays[0].scale_factor - 2.0).abs() < 0.01);
    }

    /// Border link ranges in LayoutSync are in WORLD coordinates.
    /// build_config_from_layout must convert them to host-LOCAL coordinates
    /// (subtract each host's world_offset) so KvmEngine/BorderLinkResolver
    /// can match along_edge values from VirtualCursor (which uses local coords).
    ///
    /// Real scenario: Mac (portrait 1080×1920) at world_offset (-1085, -491),
    /// Host (3440×1440) at world_offset (0, 0).
    /// World-Y overlap = [0, 1429]. In Mac-local coords this should be [491, 1920].
    #[test]
    fn test_build_config_from_layout_converts_border_link_ranges_to_local() {
        use kani_proto::topology::{CoordinateMapping, Edge};

        let host_id = HostId::new_v4();
        let mac_id = HostId::new_v4();
        let host_addr: SocketAddr = "192.168.1.10:24900".parse().unwrap();

        let hosts = vec![
            HostLayout {
                host_id,
                name: "host".into(),
                address: "192.168.1.10".into(),
                platform: "windows".into(),
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
            },
            HostLayout {
                host_id: mac_id,
                name: "mac".into(),
                address: "192.168.1.111".into(),
                platform: "macos".into(),
                world_offset_x: -1086.0,
                world_offset_y: -491.0,
                displays: vec![DisplaySnapshot {
                    id: 2,
                    origin_x: 0.0,
                    origin_y: 0.0,
                    width: 1080.0,
                    height: 1920.0,
                    width_pixels: 1080,
                    height_pixels: 1920,
                    scale_factor: 1.0,
                    is_primary: true,
                }],
            },
        ];

        // World-coordinate border links (as sent in LayoutSync):
        // Mac Right → Host Left: world Y overlap = [0, 1429]
        let border_links = vec![
            BorderLinkLayout {
                from_host: mac_id,
                from_display: 2,
                from_edge: Edge::Right,
                from_range: [0.0, 1429.0], // world coords
                to_host: host_id,
                to_display: 0,
                to_edge: Edge::Left,
                to_range: [0.0, 1429.0], // world coords
                mapping: CoordinateMapping::Linear,
            },
            BorderLinkLayout {
                from_host: host_id,
                from_display: 0,
                from_edge: Edge::Left,
                from_range: [0.0, 1429.0],
                to_host: mac_id,
                to_display: 2,
                to_edge: Edge::Right,
                to_range: [0.0, 1429.0],
                mapping: CoordinateMapping::Linear,
            },
        ];

        let config = build_config_from_layout(mac_id, &hosts, &border_links, host_addr);

        // Border link 0: Mac Right → Host Left
        // Mac world_offset_y = -491, so from_range should be [0 - (-491), 1429 - (-491)] = [491, 1920]
        // Host world_offset_y = 0, so to_range stays [0, 1429]
        let bl0 = &config.border_links[0];
        assert!(
            (bl0.from_range[0] - 491.0).abs() < 1.0,
            "Mac from_range[0] should be ~491 (local), got {}",
            bl0.from_range[0]
        );
        assert!(
            (bl0.from_range[1] - 1920.0).abs() < 1.0,
            "Mac from_range[1] should be ~1920 (local), got {}",
            bl0.from_range[1]
        );
        assert!(
            (bl0.to_range[0] - 0.0).abs() < 1.0,
            "Host to_range[0] should be ~0 (local), got {}",
            bl0.to_range[0]
        );
        assert!(
            (bl0.to_range[1] - 1429.0).abs() < 1.0,
            "Host to_range[1] should be ~1429 (local), got {}",
            bl0.to_range[1]
        );

        // Border link 1: Host Left → Mac Right
        // Host world_offset_y = 0, so from_range stays [0, 1429]
        // Mac world_offset_y = -491, so to_range = [491, 1920]
        let bl1 = &config.border_links[1];
        assert!(
            (bl1.from_range[0] - 0.0).abs() < 1.0,
            "Host from_range[0] should be ~0 (local), got {}",
            bl1.from_range[0]
        );
        assert!(
            (bl1.to_range[0] - 491.0).abs() < 1.0,
            "Mac to_range[0] should be ~491 (local), got {}",
            bl1.to_range[0]
        );
        assert!(
            (bl1.to_range[1] - 1920.0).abs() < 1.0,
            "Mac to_range[1] should be ~1920 (local), got {}",
            bl1.to_range[1]
        );
    }

    #[test]
    fn test_build_config_from_layout_clipboard_port_matches_host() {
        let host_id = HostId::new_v4();
        let client_id = HostId::new_v4();
        let host_addr: SocketAddr = "192.168.1.10:24900".parse().unwrap();

        let hosts = vec![HostLayout {
            host_id,
            name: "host".into(),
            address: "192.168.1.10".into(),
            platform: "windows".into(),
            world_offset_x: 0.0,
            world_offset_y: 0.0,
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
        }];

        let config = build_config_from_layout(client_id, &hosts, &[], host_addr);

        // clipboard_port must be host_port + 1 (= 24901), NOT 1
        let expected_clipboard_port = host_addr.port() + 1; // 24901
        assert_eq!(
            config.server.clipboard_port,
            Some(expected_clipboard_port),
            "Client clipboard_port should be host_port+1 ({}), got {:?}",
            expected_clipboard_port,
            config.server.clipboard_port
        );
    }
}
