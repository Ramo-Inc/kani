//! Loopback test: two hosts on localhost, simulated input events.
//! Tests the full flow: capture -> edge detect -> Enter -> Ack -> forward.

use kani_core::crossing::BorderLinkResolver;
use kani_core::cursor::{MoveResult, VirtualCursor};
use kani_core::state::{CursorState, CursorStateMachine};
use kani_proto::event::*;
use kani_proto::topology::*;
use kani_transport::udp::UdpTransport;
use uuid::Uuid;

fn host_a() -> HostId {
    Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap()
}

fn host_b() -> HostId {
    Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap()
}

fn make_display(id: DisplayId, origin: (i32, i32), res: (u32, u32)) -> Display {
    Display {
        id,
        origin,
        resolution: res,
        scale_factor: 1.0,
        logical_size: (res.0 as f64, res.1 as f64),
        orientation: Orientation::Normal,
    }
}

/// Full loopback: Host A captures mouse -> exits edge -> sends Enter to Host B ->
/// Host B Acks -> Host A forwards subsequent mouse events to Host B
#[tokio::test]
async fn test_full_loopback_flow() {
    let topo_a = HostTopology::new(host_a(), vec![make_display(0, (0, 0), (1920, 1080))]);
    let _topo_b = HostTopology::new(host_b(), vec![make_display(0, (0, 0), (2560, 1440))]);

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

    // Setup UDP transports on localhost
    let transport_a = UdpTransport::bind("127.0.0.1:0").await.unwrap();
    let transport_b = UdpTransport::bind("127.0.0.1:0").await.unwrap();
    let addr_a = transport_a.local_addr().unwrap();
    let addr_b = transport_b.local_addr().unwrap();

    // Host A state
    let mut sm_a = CursorStateMachine::new(host_a());
    let mut cursor_a = VirtualCursor::new(960.0, 540.0, 0);

    // === Phase 1: Simulate mouse moving to right edge ===
    cursor_a.x = 1919.0;
    cursor_a.y = 540.0;

    // Apply delta that pushes past edge
    let result = cursor_a.apply_delta(10.0, 0.0, &topo_a);
    let (display_id, edge, along_edge) = match result {
        MoveResult::ExitedEdge {
            display_id,
            edge,
            along_edge,
            ..
        } => (display_id, edge, along_edge),
        other => panic!("Expected ExitedEdge, got {:?}", other),
    };

    // Resolve border link
    let target = resolver
        .resolve(host_a(), display_id, edge, along_edge)
        .unwrap();
    assert_eq!(target.target_host, host_b());

    // Begin transition
    let tid = sm_a.begin_transition(host_b()).unwrap();

    // === Phase 2: Send Enter over UDP ===
    let enter = InputEvent::new(
        host_a(),
        EventType::Enter {
            target_host: host_b(),
            landing_x: target.landing_x,
            landing_y: target.landing_y,
            landing_display: target.target_display,
            modifiers: ModifierState::default(),
            transition_id: tid,
        },
    );
    transport_a.send_event(&enter, addr_b).await.unwrap();

    // Host B receives Enter
    let (recv_enter, from) = transport_b.recv_event().await.unwrap();
    assert_eq!(from, addr_a);
    let recv_tid = match &recv_enter.event_type {
        EventType::Enter {
            transition_id,
            landing_x,
            landing_y,
            ..
        } => {
            assert!((*landing_x - 1.0).abs() < 0.01); // left edge of B + 1px inward
            assert!(
                (*landing_y - 720.0).abs() < 1.0,
                "Expected ~720.0, got {}",
                landing_y
            ); // 540/1080 * 1440 = 720
            *transition_id
        }
        _ => panic!("Expected Enter"),
    };

    // === Phase 3: Host B sends Ack ===
    let ack = InputEvent::new(
        host_b(),
        EventType::Ack {
            acked_seq: recv_enter.seq,
            transition_id: recv_tid,
        },
    );
    transport_b.send_event(&ack, addr_a).await.unwrap();

    // Host A processes Ack
    let (recv_ack, _) = transport_a.recv_event().await.unwrap();
    if let EventType::Ack { transition_id, .. } = &recv_ack.event_type {
        sm_a.ack_received(recv_ack.host_id, *transition_id);
    }
    assert!(
        matches!(sm_a.state(), CursorState::Idle { active } if *active == host_b()),
        "Expected idle on host_b, got {:?}",
        sm_a.state()
    );

    // === Phase 4: Forward mouse events to Host B ===
    let mouse_move = InputEvent::new(host_a(), EventType::MouseMove { dx: 5.0, dy: 3.0 });
    transport_a.send_event(&mouse_move, addr_b).await.unwrap();

    let (recv_move, _) = transport_b.recv_event().await.unwrap();
    assert!(
        matches!(recv_move.event_type, EventType::MouseMove { dx, dy } if dx == 5.0 && dy == 3.0)
    );

    // Forward key press
    let key = InputEvent::new(
        host_a(),
        EventType::KeyPress {
            hid_usage: 0x04, // 'a'
            pressed: true,
            modifiers: ModifierState::default(),
        },
    );
    transport_a.send_event(&key, addr_b).await.unwrap();

    let (recv_key, _) = transport_b.recv_event().await.unwrap();
    assert!(matches!(
        recv_key.event_type,
        EventType::KeyPress {
            hid_usage: 0x04,
            pressed: true,
            ..
        }
    ));

    // === Phase 5: Ping/Pong heartbeat ===
    let ping = InputEvent::new(host_a(), EventType::Ping { payload: 42 });
    transport_a.send_event(&ping, addr_b).await.unwrap();

    let (recv_ping, _) = transport_b.recv_event().await.unwrap();
    assert!(matches!(
        recv_ping.event_type,
        EventType::Ping { payload: 42 }
    ));

    let pong = InputEvent::new(host_b(), EventType::Pong { payload: 42 });
    transport_b.send_event(&pong, addr_a).await.unwrap();

    let (recv_pong, _) = transport_a.recv_event().await.unwrap();
    assert!(matches!(
        recv_pong.event_type,
        EventType::Pong { payload: 42 }
    ));
}

/// Test: clipboard text sent over TCP during transition
#[tokio::test]
async fn test_clipboard_during_transition() {
    use kani_proto::clipboard::ClipboardMessage;
    use kani_transport::tls_tcp::{ClipboardReceiver, ClipboardSender};

    // Host B starts clipboard receiver
    let receiver = ClipboardReceiver::bind("127.0.0.1:0").await.unwrap();
    let recv_addr = receiver.local_addr().unwrap();

    // Host A sends clipboard during transition
    let clipboard_text = "Hello from Host A clipboard!".to_string();
    let expected = clipboard_text.clone();

    let send_handle = tokio::spawn(async move {
        let mut sender = ClipboardSender::connect(&recv_addr.to_string())
            .await
            .unwrap();
        sender
            .send(&ClipboardMessage::Text(clipboard_text))
            .await
            .unwrap();
    });

    let (msg, _addr) = receiver.accept_and_recv().await.unwrap();
    assert_eq!(msg, ClipboardMessage::Text(expected));
    send_handle.await.unwrap();
}

/// Test: emergency reset via hotkey during active remote session
#[tokio::test]
async fn test_emergency_reset_during_remote() {
    let mut sm = CursorStateMachine::new(host_a());

    // Transition to B
    let tid = sm.begin_transition(host_b()).unwrap();
    sm.ack_received(host_b(), tid);
    assert!(matches!(sm.state(), CursorState::Idle { active } if *active == host_b()));

    // Emergency hotkey fires -> reset to server
    sm.emergency_reset(host_a());
    assert!(matches!(sm.state(), CursorState::Idle { active } if *active == host_a()));

    // Can transition again after reset
    let tid2 = sm.begin_transition(host_b()).unwrap();
    assert_ne!(tid, tid2); // unique transition IDs
}

/// Test: bidirectional border links (A->B right edge, B->A left edge)
#[tokio::test]
async fn test_bidirectional_crossing() {
    let topo_a = HostTopology::new(host_a(), vec![make_display(0, (0, 0), (1920, 1080))]);
    // topo_b is defined to document the test setup (B has a 2560x1440 display)
    let _topo_b = HostTopology::new(host_b(), vec![make_display(0, (0, 0), (2560, 1440))]);

    let links = vec![
        BorderLink {
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
        },
        BorderLink {
            from: EdgeSegment {
                host_id: host_b(),
                display_id: 0,
                edge: Edge::Left,
                range_start: 0.0,
                range_end: 1440.0,
                edge_coord: 0.0,
            },
            to: EdgeSegment {
                host_id: host_a(),
                display_id: 0,
                edge: Edge::Right,
                range_start: 0.0,
                range_end: 1080.0,
                edge_coord: 1920.0,
            },
            mapping: CoordinateMapping::Linear,
        },
    ];
    let resolver = BorderLinkResolver::new(links);

    // A exits right edge
    let mut cursor_a = VirtualCursor::new(1919.0, 540.0, 0);
    let result = cursor_a.apply_delta(10.0, 0.0, &topo_a);
    assert!(matches!(
        result,
        MoveResult::ExitedEdge {
            edge: Edge::Right,
            ..
        }
    ));

    let target = resolver.resolve(host_a(), 0, Edge::Right, 540.0).unwrap();
    assert_eq!(target.target_host, host_b());
    assert!((target.landing_x - 1.0).abs() < 0.01);
    assert!((target.landing_y - 720.0).abs() < 1.0);

    // B exits left edge (going back to A)
    let target_back = resolver.resolve(host_b(), 0, Edge::Left, 720.0).unwrap();
    assert_eq!(target_back.target_host, host_a());
    assert!((target_back.landing_x - 1919.0).abs() < 0.01);
    assert!((target_back.landing_y - 540.0).abs() < 1.0);
}

/// Test: state machine prevents double transition
#[tokio::test]
async fn test_no_double_transition() {
    let mut sm = CursorStateMachine::new(host_a());

    let _tid = sm.begin_transition(host_b()).unwrap();
    // Attempting another transition while one is in flight should fail
    let result = sm.begin_transition(host_b());
    assert!(result.is_err());
}
