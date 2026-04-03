//! Integration test: simulates server+client edge-crossing flow in-process.
//! Uses UdpTransport on localhost. No platform permissions needed.

use kani_core::crossing::BorderLinkResolver;
use kani_core::cursor::{MoveResult, VirtualCursor};
use kani_core::state::CursorStateMachine;
use kani_proto::event::*;
use kani_proto::topology::*;
use uuid::Uuid;

fn host_a() -> HostId {
    Uuid::from_u128(1)
}

fn host_b() -> HostId {
    Uuid::from_u128(2)
}

fn make_display(id: DisplayId, origin: (i32, i32), res: (u32, u32), scale: f64) -> Display {
    Display {
        id,
        origin,
        resolution: res,
        scale_factor: scale,
        logical_size: (res.0 as f64 / scale, res.1 as f64 / scale),
        orientation: Orientation::Normal,
    }
}

/// Full edge-crossing simulation: cursor on Host A exits right -> enters Host B left.
#[test]
fn test_edge_crossing_flow() {
    // Setup: Host A (1920x1080) on the left, Host B (2560x1440) on the right
    let topo_a = HostTopology::new(host_a(), vec![make_display(0, (0, 0), (1920, 1080), 1.0)]);
    let _topo_b = HostTopology::new(host_b(), vec![make_display(0, (0, 0), (2560, 1440), 1.0)]);

    // Border link: A's right edge -> B's left edge (Linear mapping)
    let links = vec![BorderLink {
        from: EdgeSegment {
            host_id: host_a(),
            display_id: 0,
            edge: Edge::Right,
            range_start: 0.0,
            range_end: 1080.0,
            edge_coord: 1920.0,
        },
        to: EdgeSegment {
            host_id: host_b(),
            display_id: 0,
            edge: Edge::Left,
            range_start: 0.0,
            range_end: 1440.0,
            edge_coord: 0.0,
        },
        mapping: CoordinateMapping::Linear,
    }];
    let resolver = BorderLinkResolver::new(links);

    // State machine starts on Host A
    let mut sm = CursorStateMachine::new(host_a());
    let mut cursor = VirtualCursor::new(1919.0, 540.0, 0);

    // Move cursor past right edge
    let result = cursor.apply_delta(10.0, 0.0, &topo_a);

    // Should exit right edge
    match result {
        MoveResult::ExitedEdge {
            display_id,
            edge,
            along_edge,
            ..
        } => {
            assert_eq!(display_id, 0);
            assert_eq!(edge, Edge::Right);

            // Resolve crossing target
            let target = resolver
                .resolve(host_a(), display_id, edge, along_edge)
                .unwrap();
            assert_eq!(target.target_host, host_b());
            assert!((target.landing_x - 1.0).abs() < 0.01); // Left edge + 1px inward
                                                            // along_edge ~ 540, mapped linearly: 540/1080 * 1440 = 720
            assert!(
                (target.landing_y - 720.0).abs() < 1.0,
                "landing_y was {}",
                target.landing_y
            );

            // Begin transition
            let tid = sm.begin_transition(host_b()).unwrap();
            assert!(matches!(
                sm.state(),
                kani_core::state::CursorState::Transitioning { .. }
            ));

            // Simulate Ack from Host B
            sm.ack_received(host_b(), tid);
            assert!(matches!(
                sm.state(),
                kani_core::state::CursorState::Idle { active } if *active == host_b()
            ));
        }
        _ => panic!("Expected ExitedEdge, got {:?}", result),
    }
}

/// Test: cursor exits but no border link defined -> clamp.
#[test]
fn test_no_border_link_clamps() {
    let topo = HostTopology::new(host_a(), vec![make_display(0, (0, 0), (1920, 1080), 1.0)]);
    let resolver = BorderLinkResolver::new(vec![]); // No links

    let mut cursor = VirtualCursor::new(1919.0, 540.0, 0);
    let result = cursor.apply_delta(10.0, 0.0, &topo);

    match result {
        MoveResult::ExitedEdge {
            display_id,
            edge,
            along_edge,
            ..
        } => {
            assert!(resolver
                .resolve(host_a(), display_id, edge, along_edge)
                .is_none());
            cursor.clamp_to(&topo);
            assert!(cursor.x < 1920.0);
        }
        _ => panic!("Expected ExitedEdge"),
    }
}

/// Test: transition timeout reverts to previous host.
#[test]
fn test_transition_timeout_reverts() {
    let mut sm = CursorStateMachine::new(host_a());
    let _tid = sm.begin_transition(host_b()).unwrap();

    // Simulate timeout (no Ack received)
    sm.timeout();
    assert!(matches!(
        sm.state(),
        kani_core::state::CursorState::Idle { active } if *active == host_a()
    ));
}

/// Test: emergency hotkey resets cursor to server.
#[test]
fn test_emergency_reset() {
    let mut sm = CursorStateMachine::new(host_a());
    let tid = sm.begin_transition(host_b()).unwrap();
    sm.ack_received(host_b(), tid);
    // Cursor now on Host B
    assert!(matches!(
        sm.state(),
        kani_core::state::CursorState::Idle { active } if *active == host_b()
    ));

    // Emergency reset
    sm.emergency_reset(host_a());
    assert!(matches!(
        sm.state(),
        kani_core::state::CursorState::Idle { active } if *active == host_a()
    ));
}

/// UDP transport integration: send InputEvent over localhost.
#[tokio::test]
async fn test_udp_event_roundtrip() {
    use kani_transport::udp::UdpTransport;

    let server = UdpTransport::bind("127.0.0.1:0").await.unwrap();
    let client = UdpTransport::bind("127.0.0.1:0").await.unwrap();
    let server_addr = server.local_addr().unwrap();

    // Send Enter event
    let enter = InputEvent::new(
        host_a(),
        EventType::Enter {
            target_host: host_b(),
            landing_x: 0.0,
            landing_y: 720.0,
            landing_display: 0,
            modifiers: ModifierState::default(),
            transition_id: 42,
        },
    );
    client.send_event(&enter, server_addr).await.unwrap();

    // Receive and verify
    let (received, _) = server.recv_event().await.unwrap();
    match &received.event_type {
        EventType::Enter {
            target_host,
            transition_id,
            landing_y,
            ..
        } => {
            assert_eq!(*target_host, host_b());
            assert_eq!(*transition_id, 42);
            assert!((landing_y - 720.0).abs() < 0.01);
        }
        _ => panic!("Expected Enter event"),
    }
}

/// Full flow with UDP: server sends Enter, client responds Ack.
#[tokio::test]
async fn test_enter_ack_flow_over_udp() {
    use kani_transport::udp::UdpTransport;

    let server_transport = UdpTransport::bind("127.0.0.1:0").await.unwrap();
    let client_transport = UdpTransport::bind("127.0.0.1:0").await.unwrap();
    let server_addr = server_transport.local_addr().unwrap();
    let client_addr = client_transport.local_addr().unwrap();

    let mut sm = CursorStateMachine::new(host_a());
    let tid = sm.begin_transition(host_b()).unwrap();

    // Server sends Enter to client
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
    server_transport
        .send_event(&enter, client_addr)
        .await
        .unwrap();

    // Client receives Enter
    let (received, from) = client_transport.recv_event().await.unwrap();
    assert_eq!(from, server_addr);

    // Client sends Ack
    let ack_tid = match &received.event_type {
        EventType::Enter { transition_id, .. } => *transition_id,
        _ => panic!("Expected Enter"),
    };
    let ack = InputEvent::new(
        host_b(),
        EventType::Ack {
            acked_seq: received.seq,
            transition_id: ack_tid,
        },
    );
    client_transport
        .send_event(&ack, server_addr)
        .await
        .unwrap();

    // Server receives Ack
    let (ack_event, _) = server_transport.recv_event().await.unwrap();
    if let EventType::Ack { transition_id, .. } = &ack_event.event_type {
        sm.ack_received(ack_event.host_id, *transition_id);
    }

    // Transition complete
    assert!(matches!(
        sm.state(),
        kani_core::state::CursorState::Idle { active } if *active == host_b()
    ));
}
