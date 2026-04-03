//! MVP Acceptance Test
//! Simulates the full Kani flow in-process:
//! 1. Config load + validation
//! 2. Topology build from config
//! 3. Edge-crossing: cursor exits Host A -> enters Host B
//! 4. Enter/Ack state machine over UDP
//! 5. Clipboard text sync over TCP
//! 6. Heartbeat disconnect -> emergency cursor recovery
//! 7. Emergency hotkey reset

use kani_core::crossing::BorderLinkResolver;
use kani_core::cursor::{MoveResult, VirtualCursor};
use kani_core::state::{CursorState, CursorStateMachine};
use kani_proto::clipboard::ClipboardMessage;
use kani_proto::config::KaniConfig;
use kani_proto::event::*;
use kani_proto::topology::*;
use kani_transport::connection::ConnectionManager;
use kani_transport::tls_tcp::{ClipboardReceiver, ClipboardSender};
use kani_transport::udp::UdpTransport;
use std::time::Duration;
use uuid::Uuid;

const TEST_CONFIG: &str = r#"
[server]
host_id = "11111111-1111-1111-1111-111111111111"
bind_port = 24900

[[hosts]]
host_id = "11111111-1111-1111-1111-111111111111"
name = "host-a"
address = "127.0.0.1"

[[hosts.displays]]
id = 0
origin = [0, 0]
resolution = [1920, 1080]
scale_factor = 1.0
orientation = "Normal"

[[hosts]]
host_id = "22222222-2222-2222-2222-222222222222"
name = "host-b"
address = "127.0.0.2"

[[hosts.displays]]
id = 0
origin = [0, 0]
resolution = [2560, 1440]
scale_factor = 1.0
orientation = "Normal"

[[border_links]]
from_host = "11111111-1111-1111-1111-111111111111"
from_display = 0
from_edge = "Right"
from_range = [0.0, 1080.0]
to_host = "22222222-2222-2222-2222-222222222222"
to_display = 0
to_edge = "Left"
to_range = [0.0, 1440.0]
mapping = "Linear"
"#;

fn host_a() -> HostId {
    Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap()
}

fn host_b() -> HostId {
    Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap()
}

fn build_topologies(config: &KaniConfig) -> Vec<HostTopology> {
    config
        .hosts
        .iter()
        .map(|h| {
            let displays = h
                .displays
                .iter()
                .map(|d| Display {
                    id: d.id,
                    origin: (d.origin[0], d.origin[1]),
                    resolution: (d.resolution[0], d.resolution[1]),
                    scale_factor: d.scale_factor,
                    logical_size: (
                        d.resolution[0] as f64 / d.scale_factor,
                        d.resolution[1] as f64 / d.scale_factor,
                    ),
                    orientation: d.orientation,
                })
                .collect();
            HostTopology::new(h.host_id, displays)
        })
        .collect()
}

fn build_border_links(config: &KaniConfig, topologies: &[HostTopology]) -> Vec<BorderLink> {
    config
        .border_links
        .iter()
        .map(|bl| {
            let from_ec = bl.from_edge_coord.unwrap_or_else(|| {
                let topo = topologies
                    .iter()
                    .find(|t| t.host_id == bl.from_host)
                    .unwrap();
                let d = topo
                    .displays
                    .iter()
                    .find(|d| d.id == bl.from_display)
                    .unwrap();
                let r = d.logical_rect();
                match bl.from_edge {
                    Edge::Right => r.x + r.w,
                    Edge::Left => r.x,
                    Edge::Bottom => r.y + r.h,
                    Edge::Top => r.y,
                }
            });
            let to_ec = bl.to_edge_coord.unwrap_or_else(|| {
                let topo = topologies.iter().find(|t| t.host_id == bl.to_host).unwrap();
                let d = topo
                    .displays
                    .iter()
                    .find(|d| d.id == bl.to_display)
                    .unwrap();
                let r = d.logical_rect();
                match bl.to_edge {
                    Edge::Left => r.x,
                    Edge::Right => r.x + r.w,
                    Edge::Top => r.y,
                    Edge::Bottom => r.y + r.h,
                }
            });
            BorderLink {
                from: EdgeSegment {
                    host_id: bl.from_host,
                    display_id: bl.from_display,
                    edge: bl.from_edge,
                    range_start: bl.from_range[0],
                    range_end: bl.from_range[1],
                    edge_coord: from_ec,
                },
                to: EdgeSegment {
                    host_id: bl.to_host,
                    display_id: bl.to_display,
                    edge: bl.to_edge,
                    range_start: bl.to_range[0],
                    range_end: bl.to_range[1],
                    edge_coord: to_ec,
                },
                mapping: bl.mapping,
            }
        })
        .collect()
}

/// Phase 1: Config load + validation
#[test]
fn mvp_phase1_config() {
    let config: KaniConfig = toml::from_str(TEST_CONFIG).unwrap();
    assert!(config.validate().is_ok());
    assert_eq!(config.hosts.len(), 2);
    assert_eq!(config.border_links.len(), 1);
    assert_eq!(config.server.bind_port, 24900);
}

/// Phase 2: Topology build + border link resolution
#[test]
fn mvp_phase2_topology() {
    let config: KaniConfig = toml::from_str(TEST_CONFIG).unwrap();
    let topologies = build_topologies(&config);

    assert_eq!(topologies.len(), 2);
    assert_eq!(topologies[0].bounding_box.w, 1920.0);
    assert_eq!(topologies[1].bounding_box.w, 2560.0);
}

/// Phase 3: Full edge-crossing + Enter/Ack + cursor transfer
#[test]
fn mvp_phase3_edge_crossing_and_transition() {
    let config: KaniConfig = toml::from_str(TEST_CONFIG).unwrap();
    let topologies = build_topologies(&config);
    let border_links = build_border_links(&config, &topologies);
    let resolver = BorderLinkResolver::new(border_links);
    let topo_a = &topologies[0];

    // Cursor starts near right edge of Host A
    let mut cursor = VirtualCursor::new(1919.0, 540.0, 0);
    let mut sm = CursorStateMachine::new(host_a());

    // Move cursor past right edge
    let result = cursor.apply_delta(10.0, 0.0, topo_a);

    // Should exit right edge
    let (display_id, edge, along_edge) = match result {
        MoveResult::ExitedEdge {
            display_id,
            edge,
            along_edge,
            ..
        } => (display_id, edge, along_edge),
        other => panic!("Expected ExitedEdge, got {:?}", other),
    };

    // Resolve target
    let target = resolver
        .resolve(host_a(), display_id, edge, along_edge)
        .expect("BorderLink should match");
    assert_eq!(target.target_host, host_b());
    assert!((target.landing_x - 1.0).abs() < 0.01); // Left edge of B + 1px inward

    // Begin transition
    let tid = sm.begin_transition(host_b()).unwrap();

    // Simulate Ack
    sm.ack_received(host_b(), tid);
    assert!(matches!(
        sm.state(),
        CursorState::Idle { active } if *active == host_b()
    ));
}

/// Phase 4: Clipboard text sync over TCP
#[tokio::test]
async fn mvp_phase4_clipboard_sync() {
    let receiver = ClipboardReceiver::bind("127.0.0.1:0").await.unwrap();
    let addr = receiver.local_addr().unwrap();

    let text = "Copied on Host A, pasted on Host B".to_string();
    let expected = text.clone();

    let send_handle = tokio::spawn(async move {
        let mut sender = ClipboardSender::connect(&addr.to_string()).await.unwrap();
        sender.send(&ClipboardMessage::Text(text)).await.unwrap();
    });

    let (msg, _addr) = receiver.accept_and_recv().await.unwrap();
    assert_eq!(msg, ClipboardMessage::Text(expected));
    send_handle.await.unwrap();
}

/// Phase 5: Heartbeat disconnect -> emergency cursor recovery
#[test]
fn mvp_phase5_disconnect_recovery() {
    let mut sm = CursorStateMachine::new(host_a());
    let mut conn_mgr = ConnectionManager::new();
    conn_mgr.register_peer(host_b());

    // Transition to Host B
    let tid = sm.begin_transition(host_b()).unwrap();
    sm.ack_received(host_b(), tid);
    assert!(matches!(
        sm.state(),
        CursorState::Idle { active } if *active == host_b()
    ));

    // Simulate heartbeat timeout with a fresh manager using short timeout
    let mut conn_mgr2 = ConnectionManager::new();
    conn_mgr2.heartbeat_timeout = Duration::from_millis(10);
    conn_mgr2.register_peer(host_b());
    std::thread::sleep(Duration::from_millis(20));

    let disconnected = conn_mgr2.check_heartbeats();
    assert!(disconnected.contains(&host_b()));

    // Emergency reset (simulating what server does on heartbeat timeout)
    sm.emergency_reset(host_a());
    assert!(matches!(
        sm.state(),
        CursorState::Idle { active } if *active == host_a()
    ));
}

/// Phase 6: Emergency hotkey reset from any state
#[test]
fn mvp_phase6_emergency_hotkey() {
    let mut sm = CursorStateMachine::new(host_a());

    // Transition to B, then emergency reset
    let tid = sm.begin_transition(host_b()).unwrap();
    sm.ack_received(host_b(), tid);

    // Emergency hotkey fires
    sm.emergency_reset(host_a());
    assert!(matches!(
        sm.state(),
        CursorState::Idle { active } if *active == host_a()
    ));

    // Also works during transition
    let _tid2 = sm.begin_transition(host_b()).unwrap();
    sm.emergency_reset(host_a());
    assert!(matches!(
        sm.state(),
        CursorState::Idle { active } if *active == host_a()
    ));
}

/// Phase 7: Full Enter/Ack flow over real UDP
#[tokio::test]
async fn mvp_phase7_udp_enter_ack() {
    let server = UdpTransport::bind("127.0.0.1:0").await.unwrap();
    let client = UdpTransport::bind("127.0.0.1:0").await.unwrap();
    let server_addr = server.local_addr().unwrap();
    let client_addr = client.local_addr().unwrap();

    let mut sm = CursorStateMachine::new(host_a());
    let tid = sm.begin_transition(host_b()).unwrap();

    // Server sends Enter
    let enter = InputEvent::new(
        host_a(),
        EventType::Enter {
            target_host: host_b(),
            landing_x: 0.0,
            landing_y: 720.0,
            landing_display: 0,
            modifiers: ModifierState::default(),
            transition_id: tid,
        },
    );
    server.send_event(&enter, client_addr).await.unwrap();

    // Client receives and Acks
    let (recv, _) = client.recv_event().await.unwrap();
    let recv_tid = match &recv.event_type {
        EventType::Enter { transition_id, .. } => *transition_id,
        _ => panic!("Expected Enter"),
    };
    let ack = InputEvent::new(
        host_b(),
        EventType::Ack {
            acked_seq: recv.seq,
            transition_id: recv_tid,
        },
    );
    client.send_event(&ack, server_addr).await.unwrap();

    // Server processes Ack
    let (ack_recv, _) = server.recv_event().await.unwrap();
    if let EventType::Ack { transition_id, .. } = &ack_recv.event_type {
        sm.ack_received(ack_recv.host_id, *transition_id);
    }
    assert!(matches!(
        sm.state(),
        CursorState::Idle { active } if *active == host_b()
    ));
}
