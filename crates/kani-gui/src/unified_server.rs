//! UnifiedServer — tokio-based server that handles BOTH management
//! (RegisterClient, LayoutSync, Ping) and KVM (via KvmEngine) on a single UDP socket.
//!
//! Replaces `listener.rs`. Always runs when role == Host.
//!
//! NOTE: This module is not yet wired into the GUI (Task 6).

use kani_proto::codec::{decode, encode, MAX_LAYOUT_SIZE};
use kani_proto::config::KaniConfig;
use kani_proto::event::*;
use kani_server::kvm_engine::{KvmAction, KvmEngine};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Handle for the GUI thread to communicate with the UnifiedServer background thread.
pub struct UnifiedServerHandle {
    pub command_tx: std::sync::mpsc::Sender<ServerCommand>,
    pub event_rx: std::sync::mpsc::Receiver<ServerEvent>,
    pub cancel: CancellationToken,
    #[allow(dead_code)] // used in tests
    pub local_port: u16,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Commands sent from GUI to server.
pub enum ServerCommand {
    BroadcastLayout(Vec<u8>),
    SetDragging(bool),
    StartKvm(KaniConfig),
    StopKvm,
}

/// Events sent from server to GUI.
#[derive(Debug)]
pub enum ServerEvent {
    ClientRegistered(ClientInfo),
    ClientDisconnected(HostId),
    KvmStarted,
    KvmStopped,
    KvmError(String),
    CursorReturnedToHost(HostId),
}

/// Information about a registered client.
#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub host_id: HostId,
    pub name: String,
    pub address: SocketAddr,
    pub platform: String,
    pub displays: Vec<DisplaySnapshot>,
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

struct ClientState {
    info: ClientInfo,
    last_heard: Instant,
}

// ---------------------------------------------------------------------------
// UnifiedServerHandle methods
// ---------------------------------------------------------------------------

impl UnifiedServerHandle {
    /// Non-blocking poll for the next server event.
    pub fn try_recv_event(&self) -> Option<ServerEvent> {
        self.event_rx.try_recv().ok()
    }

    /// Send a command to the server.
    pub fn send_command(&self, cmd: ServerCommand) {
        let _ = self.command_tx.send(cmd);
    }

    /// Stop the server: cancel and join the thread.
    #[allow(dead_code)] // used in tests
    pub fn stop(&mut self) {
        self.cancel.cancel();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for UnifiedServerHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

// ---------------------------------------------------------------------------
// start_unified_server
// ---------------------------------------------------------------------------

/// Start the UnifiedServer on a background thread with its own tokio runtime.
///
/// `port` may be 0 for OS-assigned. Returns a handle with the actual `local_port`.
pub fn start_unified_server(
    port: u16,
    host_id: HostId,
    host_name: String,
    host_displays: Vec<DisplaySnapshot>,
    initial_layout: Vec<u8>,
) -> Result<UnifiedServerHandle, String> {
    // Bind synchronously so we can report the port before spawning.
    let std_socket = std::net::UdpSocket::bind(format!("0.0.0.0:{port}"))
        .map_err(|e| format!("Failed to bind UDP port {port}: {e}"))?;
    let local_port = std_socket
        .local_addr()
        .map_err(|e| format!("local_addr: {e}"))?
        .port();
    std_socket
        .set_nonblocking(true)
        .map_err(|e| format!("set_nonblocking: {e}"))?;

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    // std::sync::mpsc channels for GUI <-> server communication
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<ServerCommand>();
    let (evt_tx, evt_rx) = std::sync::mpsc::channel::<ServerEvent>();

    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        rt.block_on(run_server(
            std_socket,
            host_id,
            host_name,
            host_displays,
            initial_layout,
            cmd_rx,
            evt_tx,
            cancel_clone,
        ));
    });

    Ok(UnifiedServerHandle {
        command_tx: cmd_tx,
        event_rx: evt_rx,
        cancel,
        local_port,
        thread: Some(thread),
    })
}

// ---------------------------------------------------------------------------
// Async server loop
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn run_server(
    std_socket: std::net::UdpSocket,
    host_id: HostId,
    host_name: String,
    host_displays: Vec<DisplaySnapshot>,
    initial_layout: Vec<u8>,
    cmd_rx: std::sync::mpsc::Receiver<ServerCommand>,
    evt_tx: std::sync::mpsc::Sender<ServerEvent>,
    cancel: CancellationToken,
) {
    let socket = UdpSocket::from_std(std_socket).expect("from_std failed");

    // Bridge std::sync::mpsc -> tokio::sync::mpsc so we can use it in select!
    let (tokio_cmd_tx, mut tokio_cmd_rx) = tokio::sync::mpsc::unbounded_channel::<ServerCommand>();
    // Spawn a bridging task that forwards commands from std channel to tokio channel
    let bridge_cancel = cancel.clone();
    tokio::spawn(async move {
        loop {
            // Check every 50ms for commands from the std channel
            tokio::select! {
                _ = bridge_cancel.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    while let Ok(cmd) = cmd_rx.try_recv() {
                        if tokio_cmd_tx.send(cmd).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    });

    let mut buf = [0u8; MAX_LAYOUT_SIZE];
    let mut clients: HashMap<HostId, ClientState> = HashMap::new();
    let mut kvm: Option<KvmEngine> = None;
    let mut platform_rx: Option<tokio::sync::mpsc::Receiver<EventType>> = None;
    let mut clipboard_rx: Option<
        tokio::sync::mpsc::Receiver<kani_proto::clipboard::ClipboardMessage>,
    > = None;
    let mut clipboard_receiver: Option<kani_transport::tls_tcp::ClipboardReceiver> = None;
    let mut last_layout_bytes: Option<Vec<u8>> = if initial_layout.is_empty() {
        None
    } else {
        Some(initial_layout)
    };
    let mut is_dragging = false;

    let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(1));
    heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut resend_interval = tokio::time::interval(Duration::from_secs(2));
    resend_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut prune_interval = tokio::time::interval(Duration::from_secs(5));
    prune_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    tracing::info!("UnifiedServer started");

    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled() => break,

            // ---------- Receive UDP packets ----------
            result = socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, from_addr)) => {
                        if let Ok(event) = decode(&buf[..len]) {
                            match &event.event_type {
                                // === Management: RegisterClient ===
                                EventType::RegisterClient {
                                    client_name,
                                    platform,
                                    displays,
                                } => {
                                    tracing::info!(%from_addr, %client_name, "Client registered");
                                    let info = ClientInfo {
                                        host_id: event.host_id,
                                        name: client_name.clone(),
                                        address: from_addr,
                                        platform: platform.clone(),
                                        displays: displays.clone(),
                                    };
                                    // Upsert client
                                    match clients.get_mut(&event.host_id) {
                                        Some(c) => {
                                            c.info = info.clone();
                                            c.last_heard = Instant::now();
                                        }
                                        None => {
                                            clients.insert(event.host_id, ClientState {
                                                info: info.clone(),
                                                last_heard: Instant::now(),
                                            });
                                        }
                                    }
                                    let _ = evt_tx.send(ServerEvent::ClientRegistered(info));
                                    // Send current layout to new client
                                    if let Some(ref layout) = last_layout_bytes {
                                        let _ = socket.send_to(layout, from_addr).await;
                                    }
                                    // If KVM active, send StartKvm and add peer
                                    if let Some(ref mut engine) = kvm {
                                        let start = InputEvent::new(host_id, EventType::StartKvm);
                                        if let Ok(bytes) = encode(&start) {
                                            let _ = socket.send_to(&bytes, from_addr).await;
                                        }
                                        engine.add_peer(event.host_id, from_addr);
                                    }
                                }

                                // === Management: Ping ===
                                EventType::Ping { .. } => {
                                    // Update client heartbeat
                                    if let Some(cid) = identify_host_by_addr(&clients, from_addr) {
                                        if let Some(c) = clients.get_mut(&cid) {
                                            c.last_heard = Instant::now();
                                        }
                                        if let Some(ref mut engine) = kvm {
                                            engine.record_peer_alive(cid);
                                        }
                                    }

                                    // Reply with Pong so client's KvmEngine heartbeat stays alive
                                    let pong = InputEvent::new(host_id, EventType::Pong { payload: 0 });
                                    if let Ok(bytes) = encode(&pong) {
                                        let _ = socket.send_to(&bytes, from_addr).await;
                                    }
                                }

                                // === Management: Pong (explicit match) ===
                                EventType::Pong { .. } => {
                                    if let Some(cid) = identify_host_by_addr(&clients, from_addr) {
                                        if let Some(c) = clients.get_mut(&cid) {
                                            c.last_heard = Instant::now();
                                        }
                                        if let Some(ref mut engine) = kvm {
                                            engine.record_peer_alive(cid);
                                        }
                                    }
                                }

                                // === Management: QueryDisplays (backward compat) ===
                                EventType::QueryDisplays => {
                                    let response = InputEvent::new(
                                        host_id,
                                        EventType::DisplaysResponse {
                                            host_name: host_name.clone(),
                                            displays: host_displays.clone(),
                                        },
                                    );
                                    if let Ok(bytes) = encode(&response) {
                                        let _ = socket.send_to(&bytes, from_addr).await;
                                    }
                                }

                                // === Leave: client disconnecting gracefully ===
                                EventType::Leave => {
                                    let leaving_id = identify_host_by_addr(&clients, from_addr)
                                        .unwrap_or(event.host_id);
                                    tracing::info!(%leaving_id, "Client sent Leave, disconnecting immediately");
                                    clients.remove(&leaving_id);
                                    if let Some(ref mut engine) = kvm {
                                        let actions = engine.remove_peer(leaving_id);
                                        execute_actions(&socket, &kvm, &actions, &evt_tx).await;
                                    }
                                    let _ = evt_tx.send(ServerEvent::ClientDisconnected(leaving_id));
                                }

                                // === KVM events (only when engine active) ===
                                _ if kvm.is_some() => {
                                    let from_host = identify_host_by_addr(&clients, from_addr)
                                        .unwrap_or(event.host_id);
                                    let actions = kvm.as_mut().unwrap()
                                        .handle_network_event(from_host, from_addr, &event);
                                    execute_actions(&socket, &kvm, &actions, &evt_tx).await;
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

            // ---------- GUI commands ----------
            Some(cmd) = tokio_cmd_rx.recv() => {
                match cmd {
                    ServerCommand::StartKvm(config) => {
                        match KvmEngine::new(config, host_id) {
                            Ok(mut engine) => {
                                // Add all connected clients as peers
                                for (cid, cstate) in &clients {
                                    engine.add_peer(*cid, cstate.info.address);
                                }
                                match engine.start().await {
                                    Ok(result) => {
                                        platform_rx = Some(result.platform_rx);
                                        clipboard_rx = Some(result.clipboard_rx);
                                        clipboard_receiver = result.clipboard_receiver;
                                        // Send StartKvm to all connected clients
                                        let start = InputEvent::new(host_id, EventType::StartKvm);
                                        if let Ok(bytes) = encode(&start) {
                                            for client in clients.values() {
                                                let _ = socket.send_to(&bytes, client.info.address).await;
                                            }
                                        }
                                        kvm = Some(engine);
                                        let _ = evt_tx.send(ServerEvent::KvmStarted);
                                    }
                                    Err(e) => {
                                        let _ = evt_tx.send(ServerEvent::KvmError(format!("Failed to start KVM: {e}")));
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = evt_tx.send(ServerEvent::KvmError(format!("Failed to create KVM engine: {e}")));
                            }
                        }
                    }
                    ServerCommand::StopKvm => {
                        // Send StopKvm to all clients
                        let stop_evt = InputEvent::new(host_id, EventType::StopKvm);
                        if let Ok(bytes) = encode(&stop_evt) {
                            for client in clients.values() {
                                let _ = socket.send_to(&bytes, client.info.address).await;
                            }
                        }
                        if let Some(mut engine) = kvm.take() {
                            engine.stop();
                        }
                        platform_rx = None;
                        clipboard_rx = None;
                        clipboard_receiver = None;
                        let _ = evt_tx.send(ServerEvent::KvmStopped);
                    }
                    ServerCommand::BroadcastLayout(bytes) => {
                        for client in clients.values() {
                            let _ = socket.send_to(&bytes, client.info.address).await;
                        }
                        last_layout_bytes = Some(bytes);
                    }
                    ServerCommand::SetDragging(d) => {
                        is_dragging = d;
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
                    execute_actions(&socket, &kvm, &actions, &evt_tx).await;
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

            // ---------- Heartbeat (1s) ----------
            _ = heartbeat_interval.tick(), if kvm.is_some() => {
                if let Some(ref mut engine) = kvm {
                    let actions = engine.handle_heartbeat();
                    execute_actions(&socket, &kvm, &actions, &evt_tx).await;
                }

                // Send Ping to all connected clients so their KvmEngine heartbeat stays alive
                let ping = InputEvent::new(host_id, EventType::Ping { payload: 0 });
                if let Ok(bytes) = encode(&ping) {
                    for client in clients.values() {
                        let _ = socket.send_to(&bytes, client.info.address).await;
                    }
                }
            }

            // ---------- Periodic layout resend (2s, skip during drag) ----------
            _ = resend_interval.tick(), if !is_dragging => {
                if let Some(ref layout) = last_layout_bytes {
                    for client in clients.values() {
                        let _ = socket.send_to(layout, client.info.address).await;
                    }
                }
            }

            // ---------- Prune disconnected clients (5s check, 10s timeout) ----------
            _ = prune_interval.tick() => {
                let timeout = Duration::from_secs(10);
                let pruned: Vec<HostId> = clients
                    .iter()
                    .filter(|(_, c)| c.last_heard.elapsed() > timeout)
                    .map(|(id, _)| *id)
                    .collect();
                for host_id_pruned in pruned {
                    clients.remove(&host_id_pruned);
                    if let Some(ref mut engine) = kvm {
                        let actions = engine.remove_peer(host_id_pruned);
                        execute_actions(&socket, &kvm, &actions, &evt_tx).await;
                    }
                    let _ = evt_tx.send(ServerEvent::ClientDisconnected(host_id_pruned));
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
                    execute_actions(&socket, &kvm, &actions, &evt_tx).await;
                }
            }
        }
    }

    // Cleanup
    if let Some(mut engine) = kvm.take() {
        engine.stop();
    }
    tracing::info!("UnifiedServer stopped");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Find a client HostId by socket address.
fn identify_host_by_addr(
    clients: &HashMap<HostId, ClientState>,
    addr: SocketAddr,
) -> Option<HostId> {
    clients
        .iter()
        .find(|(_, c)| c.info.address == addr)
        .map(|(id, _)| *id)
}

/// Execute KvmActions by sending events over the socket.
async fn execute_actions(
    socket: &UdpSocket,
    kvm: &Option<KvmEngine>,
    actions: &[KvmAction],
    evt_tx: &std::sync::mpsc::Sender<ServerEvent>,
) {
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
                let _ = evt_tx.send(ServerEvent::ClientDisconnected(*id));
            }
            KvmAction::CursorReturnedToHost(id) => {
                let _ = evt_tx.send(ServerEvent::CursorReturnedToHost(*id));
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
    use kani_proto::event::{DisplaySnapshot, EventType, HostId, InputEvent};
    use std::net::UdpSocket;
    use std::time::Duration;

    #[test]
    fn test_start_and_stop_unified_server() {
        let host_id = HostId::new_v4();
        let mut handle = start_unified_server(0, host_id, "test".into(), vec![], vec![]).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(handle.local_port > 0);
        handle.stop();
    }

    #[test]
    fn test_register_client_received() {
        let host_id = HostId::new_v4();
        let handle = start_unified_server(0, host_id, "host".into(), vec![], vec![]).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        // Send RegisterClient from a client socket
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        let client_id = HostId::new_v4();
        let reg = InputEvent::new(
            client_id,
            EventType::RegisterClient {
                client_name: "test-client".into(),
                platform: "test".into(),
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
        let bytes = encode(&reg).unwrap();
        client
            .send_to(&bytes, format!("127.0.0.1:{}", handle.local_port))
            .unwrap();

        std::thread::sleep(Duration::from_millis(300));

        let event = handle.try_recv_event();
        assert!(event.is_some(), "Expected ClientRegistered event");
        match event.unwrap() {
            ServerEvent::ClientRegistered(info) => {
                assert_eq!(info.name, "test-client");
                assert_eq!(info.host_id, client_id);
                assert_eq!(info.displays.len(), 1);
            }
            other => panic!("Expected ClientRegistered, got: {other:?}"),
        }

        drop(handle);
    }

    #[test]
    fn test_broadcast_layout() {
        let host_id = HostId::new_v4();
        let handle = start_unified_server(0, host_id, "host".into(), vec![], vec![]).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        // Register a client first
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let reg = InputEvent::new(
            HostId::new_v4(),
            EventType::RegisterClient {
                client_name: "c".into(),
                platform: "test".into(),
                displays: vec![],
            },
        );
        client
            .send_to(
                &encode(&reg).unwrap(),
                format!("127.0.0.1:{}", handle.local_port),
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let _ = handle.try_recv_event(); // consume ClientRegistered

        // Drain any LayoutSync that might have been sent on registration
        let mut drain_buf = [0u8; MAX_LAYOUT_SIZE];
        client
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        while client.recv_from(&mut drain_buf).is_ok() {}
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        // Broadcast layout
        let layout_event = InputEvent::new(
            host_id,
            EventType::LayoutSync {
                hosts: vec![],
                border_links: vec![],
            },
        );
        let layout_bytes = encode(&layout_event).unwrap();
        handle.send_command(ServerCommand::BroadcastLayout(layout_bytes));

        // Wait for broadcast
        std::thread::sleep(Duration::from_millis(200));

        // Client should receive LayoutSync
        let mut buf = [0u8; MAX_LAYOUT_SIZE];
        let (len, _) = client.recv_from(&mut buf).unwrap();
        let event = decode(&buf[..len]).unwrap();
        assert!(matches!(event.event_type, EventType::LayoutSync { .. }));

        drop(handle);
    }

    #[test]
    fn test_ping_receives_pong() {
        let host_id = HostId::new_v4();
        let handle = start_unified_server(0, host_id, "host".into(), vec![], vec![]).unwrap();
        std::thread::sleep(Duration::from_millis(200));

        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        // Send Ping
        let ping = InputEvent::new(HostId::new_v4(), EventType::Ping { payload: 42 });
        let bytes = encode(&ping).unwrap();
        client
            .send_to(&bytes, format!("127.0.0.1:{}", handle.local_port))
            .unwrap();

        // Should receive Pong
        let mut buf = [0u8; MAX_LAYOUT_SIZE];
        let (len, _) = client.recv_from(&mut buf).unwrap();
        let response = decode(&buf[..len]).unwrap();
        assert!(matches!(response.event_type, EventType::Pong { .. }));
        assert_eq!(response.host_id, host_id);

        drop(handle);
    }

    #[test]
    fn test_query_displays_backward_compat() {
        let host_id = HostId::new_v4();
        let displays = vec![DisplaySnapshot {
            id: 0,
            origin_x: 0.0,
            origin_y: 0.0,
            width: 1920.0,
            height: 1080.0,
            width_pixels: 1920,
            height_pixels: 1080,
            scale_factor: 1.0,
            is_primary: true,
        }];
        let handle =
            start_unified_server(0, host_id, "test-host".into(), displays, vec![]).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        // Send QueryDisplays
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let query = InputEvent {
            seq: 0,
            host_id: HostId::nil(),
            timestamp_us: 0,
            event_type: EventType::QueryDisplays,
        };
        let bytes = encode(&query).unwrap();
        client
            .send_to(&bytes, format!("127.0.0.1:{}", handle.local_port))
            .unwrap();

        let mut buf = [0u8; MAX_LAYOUT_SIZE];
        let (len, _) = client.recv_from(&mut buf).unwrap();
        let response = decode(&buf[..len]).unwrap();
        assert_eq!(response.host_id, host_id);
        match response.event_type {
            EventType::DisplaysResponse {
                host_name,
                displays,
            } => {
                assert_eq!(host_name, "test-host");
                assert_eq!(displays.len(), 1);
                assert_eq!(displays[0].width_pixels, 1920);
            }
            other => panic!("Expected DisplaysResponse, got: {other}"),
        }

        drop(handle);
    }
}
