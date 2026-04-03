use kani_proto::config::KaniConfig;
use kani_proto::event::*;
use kani_proto::topology::*;
use kani_transport::udp::UdpTransport;
use std::net::SocketAddr;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub async fn run(
    config: KaniConfig,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    run_with_stop(config, dry_run, CancellationToken::new()).await
}

pub async fn run_with_stop(
    config: KaniConfig,
    dry_run: bool,
    cancel: CancellationToken,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server_id = config.server.host_id;

    // Build topology info for logging (before engine takes ownership)
    let topologies = build_topologies(&config);
    let border_links = build_border_links(&config, &topologies);
    tracing::info!(count = border_links.len(), "Border links loaded");
    for (i, link) in border_links.iter().enumerate() {
        tracing::info!(
            i,
            from_host = %link.from.host_id,
            from_display = link.from.display_id,
            from_edge = ?link.from.edge,
            from_range_start = link.from.range_start,
            from_range_end = link.from.range_end,
            to_host = %link.to.host_id,
            to_edge = ?link.to.edge,
            "Border link"
        );
    }

    // Extract data from config before engine takes ownership
    let bind_port = config.server.bind_port;
    let config_peer_addrs: std::collections::HashMap<HostId, SocketAddr> = config
        .hosts
        .iter()
        .filter(|h| h.host_id != server_id)
        .map(|h| {
            let addr: SocketAddr = format!("{}:{}", h.address, bind_port).parse()?;
            Ok((h.host_id, addr))
        })
        .collect::<Result<_, Box<dyn std::error::Error + Send + Sync>>>()?;

    let server_name = config
        .hosts
        .iter()
        .find(|h| h.host_id == server_id)
        .map(|h| h.name.clone())
        .unwrap_or_else(|| "unknown".into());

    // Bind transport
    let bind_addr = format!("0.0.0.0:{}", bind_port);
    let transport = UdpTransport::bind(&bind_addr).await?;
    tracing::info!(addr = %transport.local_addr()?, "UDP transport bound");

    // Create engine (takes ownership of config)
    let mut engine = crate::kvm_engine::KvmEngine::new(config, server_id)?;

    for (&host_id, &addr) in &config_peer_addrs {
        engine.add_peer(host_id, addr);
    }

    // Dry-run: print info and exit
    if dry_run {
        tracing::info!("Dry-run mode: skipping platform initialization");
        return Ok(());
    }

    // Start platform + clipboard
    let crate::kvm_engine::KvmStartResult {
        mut platform_rx,
        mut clipboard_rx,
        clipboard_receiver,
    } = engine.start().await?;

    tracing::info!("Server event loop starting");
    let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled() => {
                tracing::info!("KVM stopped by user");
                break;
            }

            Some(event_type) = platform_rx.recv() => {
                let actions = engine.handle_platform_event(event_type);
                execute_actions(&transport, &engine, actions).await;
            }

            result = transport.recv_event() => {
                match result {
                    Ok((event, from_addr)) => {
                        engine.record_peer_alive(event.host_id);

                        // Server-level handlers (not in KvmEngine)
                        match &event.event_type {
                            EventType::QueryDisplays => {
                                if let Some(ref platform) = engine.platform() {
                                    let displays = platform.enumerate_displays();
                                    let snapshots: Vec<DisplaySnapshot> = displays
                                        .iter()
                                        .map(|d| DisplaySnapshot {
                                            id: d.id,
                                            origin_x: d.origin_x,
                                            origin_y: d.origin_y,
                                            width: d.width_logical,
                                            height: d.height_logical,
                                            width_pixels: d.width_pixels,
                                            height_pixels: d.height_pixels,
                                            scale_factor: d.scale_factor,
                                            is_primary: d.is_primary,
                                        })
                                        .collect();
                                    let response = InputEvent::new(
                                        server_id,
                                        EventType::DisplaysResponse {
                                            host_name: server_name.clone(),
                                            displays: snapshots,
                                        },
                                    );
                                    if let Err(e) = transport.send_event(&response, from_addr).await
                                    {
                                        tracing::warn!(error = %e, "Failed to send display response");
                                    }
                                }
                            }
                            EventType::RegisterClient {
                                ref client_name,
                                ref platform,
                                ..
                            } => {
                                tracing::info!(name = %client_name, platform = %platform, from = %from_addr, "Client registered");
                            }
                            _ => {}
                        }

                        let actions =
                            engine.handle_network_event(event.host_id, from_addr, &event);
                        execute_actions(&transport, &engine, actions).await;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Transport recv error");
                    }
                }
            }

            _ = heartbeat_interval.tick() => {
                let ping = InputEvent::new(server_id, EventType::Ping { payload: 0 });
                for addr in config_peer_addrs.values() {
                    let _ = transport.send_event(&ping, *addr).await;
                }
                let actions = engine.handle_heartbeat();
                execute_actions(&transport, &engine, actions).await;
            }

            Some(clip_msg) = clipboard_rx.recv() => {
                if engine.handle_clipboard_local_change(&clip_msg) {
                    tracing::info!("Local clipboard changed, syncing to peers");
                    for (host_id, addr) in engine.clipboard_peer_addresses() {
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
                                    Ok(()) => {
                                        tracing::info!(peer = %host_id, "Clipboard sent")
                                    }
                                    Err(e) => {
                                        tracing::warn!(peer = %host_id, error = %e, "Clipboard send failed")
                                    }
                                },
                                Ok(Err(e)) => {
                                    tracing::warn!(peer = %host_id, error = %e, "Clipboard connect failed")
                                }
                                Err(_) => {
                                    tracing::warn!(peer = %host_id, "Clipboard connect timed out")
                                }
                            }
                        });
                    }
                }
            }

            result = async {
                clipboard_receiver.as_ref().unwrap().accept_and_recv().await
            }, if clipboard_receiver.is_some() => {
                match result {
                    Ok((msg, from_addr)) => {
                        if !engine.known_peer_ips.contains(&from_addr.ip()) {
                            tracing::warn!(from = %from_addr, "Clipboard from unknown peer, ignoring");
                        } else {
                            tracing::info!(from = %from_addr, "Remote clipboard received via TCP");
                            engine.clipboard_sync.record_remote_write(&msg);
                            let kani_proto::clipboard::ClipboardMessage::Text(ref text) = msg;
                            match tokio::time::timeout(
                                Duration::from_secs(2),
                                kani_clipboard::write_to_clipboard(text),
                            )
                            .await
                            {
                                Ok(Ok(())) => {}
                                Ok(Err(e)) => {
                                    tracing::warn!(error = %e, "Failed to write clipboard locally")
                                }
                                Err(_) => {
                                    tracing::warn!("Clipboard write timed out (2s)")
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Clipboard receive error");
                    }
                }
            }

            _ = async {
                match engine.transition_deadline() {
                    Some(dl) => tokio::time::sleep_until(dl).await,
                    None => std::future::pending().await,
                }
            }, if engine.transition_deadline().is_some() => {
                let actions = engine.handle_transition_timeout();
                execute_actions(&transport, &engine, actions).await;
            }
        }
    }

    // Cleanup
    engine.stop();
    Ok(())
}

/// Execute KvmActions returned by the engine.
async fn execute_actions(
    transport: &UdpTransport,
    engine: &crate::kvm_engine::KvmEngine,
    actions: Vec<crate::kvm_engine::KvmAction>,
) {
    use crate::kvm_engine::KvmAction;
    for action in actions {
        match action {
            KvmAction::SendEvent { to, event } => {
                if let Some(addr) = engine.peer_addr(&to) {
                    let _ = transport.send_event(&event, addr).await;
                }
            }
            KvmAction::SendToAddr { addr, event } => {
                let _ = transport.send_event(&event, addr).await;
            }
            KvmAction::PeerDisconnected(host_id) => {
                tracing::info!(peer = %host_id, "Peer disconnected (heartbeat timeout)");
            }
            KvmAction::CursorReturnedToHost(host_id) => {
                tracing::info!(peer = %host_id, "Cursor emergency reset (peer disconnected)");
            }
            KvmAction::StatusMessage(msg) => {
                tracing::info!("{}", msg);
            }
        }
    }
}

/// Build HostTopology list from config.
pub(crate) fn build_topologies(config: &KaniConfig) -> Vec<HostTopology> {
    config
        .hosts
        .iter()
        .map(|host_config| {
            // Warn if platform is unknown (old config without platform field)
            if host_config.platform == "unknown" {
                tracing::warn!(
                    host = %host_config.name,
                    "Host platform is 'unknown' — using physical pixels. Set platform in config."
                );
            }
            let displays: Vec<Display> = host_config
                .displays
                .iter()
                .map(|d| Display {
                    id: d.id,
                    origin: (d.origin[0], d.origin[1]),
                    resolution: (d.resolution[0], d.resolution[1]),
                    scale_factor: d.scale_factor,
                    logical_size: {
                        let (rw, rh) = (d.resolution[0] as f64, d.resolution[1] as f64);
                        if host_config.platform == "macos" {
                            (rw / d.scale_factor, rh / d.scale_factor)
                        } else {
                            // Windows, unknown: physical pixels as-is.
                            // Note: Windows high-DPI (scale>1.0) still uses physical pixels
                            // with Per-Monitor V2 DPI awareness for SetCursorPos.
                            (rw, rh)
                        }
                    },
                    orientation: d.orientation,
                })
                .collect();
            HostTopology::new(host_config.host_id, displays)
        })
        .collect()
}

/// Build BorderLink list from config, computing edge_coord from display topology when not specified.
pub(crate) fn build_border_links(
    config: &KaniConfig,
    topologies: &[HostTopology],
) -> Vec<BorderLink> {
    config
        .border_links
        .iter()
        .map(|bl| {
            let from_edge_coord = bl.from_edge_coord.unwrap_or_else(|| {
                compute_edge_coord(topologies, bl.from_host, bl.from_display, bl.from_edge)
            });
            let to_edge_coord = bl.to_edge_coord.unwrap_or_else(|| {
                compute_edge_coord(topologies, bl.to_host, bl.to_display, bl.to_edge)
            });
            let from_scale = display_scale_for_logical(config, bl.from_host, bl.from_display);
            let to_scale = display_scale_for_logical(config, bl.to_host, bl.to_display);

            BorderLink {
                from: EdgeSegment {
                    host_id: bl.from_host,
                    display_id: bl.from_display,
                    edge: bl.from_edge,
                    range_start: bl.from_range[0] / from_scale,
                    range_end: bl.from_range[1] / from_scale,
                    edge_coord: from_edge_coord,
                },
                to: EdgeSegment {
                    host_id: bl.to_host,
                    display_id: bl.to_display,
                    edge: bl.to_edge,
                    range_start: bl.to_range[0] / to_scale,
                    range_end: bl.to_range[1] / to_scale,
                    edge_coord: to_edge_coord,
                },
                mapping: bl.mapping,
            }
        })
        .collect()
}

/// Scale factor for converting GUI physical-pixel ranges to logical coordinates.
/// Returns 1.0 for Windows (physical = logical under Per-Monitor V2).
/// Returns the display's scale_factor for macOS.
pub(crate) fn display_scale_for_logical(
    config: &KaniConfig,
    host_id: HostId,
    display_id: DisplayId,
) -> f64 {
    let host = match config.hosts.iter().find(|h| h.host_id == host_id) {
        Some(h) => h,
        None => return 1.0,
    };
    if host.platform != "macos" {
        return 1.0;
    }
    host.displays
        .iter()
        .find(|d| d.id == display_id)
        .map(|d| d.scale_factor)
        .unwrap_or(1.0)
}

/// Compute the edge_coord for a display edge from its topology.
pub(crate) fn compute_edge_coord(
    topologies: &[HostTopology],
    host_id: HostId,
    display_id: DisplayId,
    edge: Edge,
) -> f64 {
    for topo in topologies {
        if topo.host_id == host_id {
            for d in &topo.displays {
                if d.id == display_id {
                    let r = d.logical_rect();
                    return match edge {
                        Edge::Left => r.x,
                        Edge::Right => r.x + r.w,
                        Edge::Top => r.y,
                        Edge::Bottom => r.y + r.h,
                    };
                }
            }
        }
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_topologies_respects_scale_factor() {
        let server_id = uuid::Uuid::new_v4();
        // platform: "macos" → logical_size = resolution / scale_factor
        let config = KaniConfig {
            server: kani_proto::config::ServerConfig {
                host_id: server_id,
                bind_port: 0,
                clipboard_port: None,
            },
            gui: kani_proto::config::GuiConfig::default(),
            hosts: vec![kani_proto::config::HostConfig {
                host_id: server_id,
                name: "test".into(),
                address: "127.0.0.1".into(),
                platform: "macos".into(),
                world_offset: None,
                displays: vec![kani_proto::config::DisplayConfig {
                    id: 0,
                    origin: [0, 0],
                    resolution: [3024, 1964],
                    scale_factor: 2.0,
                    orientation: kani_proto::topology::Orientation::Normal,
                }],
            }],
            border_links: vec![],
            trusted_peers: std::collections::HashMap::new(),
        };

        let topos = build_topologies(&config);
        assert_eq!(topos.len(), 1);
        let topo = &topos[0];

        // macOS host: logical_size = resolution / scale = (1512, 982)
        // This now works on ALL compile targets (not just macOS)
        assert!(
            (topo.bounding_box.w - 1512.0).abs() < 0.01,
            "w={}",
            topo.bounding_box.w
        );
        assert!(
            (topo.bounding_box.h - 982.0).abs() < 0.01,
            "h={}",
            topo.bounding_box.h
        );
    }

    #[test]
    fn test_build_topologies_scale_1_unchanged() {
        let server_id = uuid::Uuid::new_v4();
        let config = KaniConfig {
            server: kani_proto::config::ServerConfig {
                host_id: server_id,
                bind_port: 0,
                clipboard_port: None,
            },
            gui: kani_proto::config::GuiConfig::default(),
            hosts: vec![kani_proto::config::HostConfig {
                host_id: server_id,
                name: "test".into(),
                address: "127.0.0.1".into(),
                platform: "windows".into(),
                world_offset: None,
                displays: vec![kani_proto::config::DisplayConfig {
                    id: 0,
                    origin: [0, 0],
                    resolution: [1920, 1080],
                    scale_factor: 1.0,
                    orientation: kani_proto::topology::Orientation::Normal,
                }],
            }],
            border_links: vec![],
            trusted_peers: std::collections::HashMap::new(),
        };

        let topos = build_topologies(&config);
        let topo = &topos[0];
        assert!((topo.bounding_box.w - 1920.0).abs() < 0.01);
        assert!((topo.bounding_box.h - 1080.0).abs() < 0.01);
    }

    #[test]
    fn test_build_topologies_respects_host_platform() {
        use kani_proto::config::*;
        use kani_proto::topology::Orientation;

        let config = KaniConfig {
            server: ServerConfig {
                host_id: uuid::Uuid::from_u128(1),
                bind_port: 24900,
                clipboard_port: None,
            },
            gui: GuiConfig {
                role: "Host".into(),
                ..GuiConfig::default()
            },
            hosts: vec![
                HostConfig {
                    host_id: uuid::Uuid::from_u128(1),
                    name: "windows".into(),
                    address: "127.0.0.1".into(),
                    platform: "windows".into(),
                    world_offset: None,
                    displays: vec![DisplayConfig {
                        id: 0,
                        origin: [0, 0],
                        resolution: [3440, 1440],
                        scale_factor: 1.0,
                        orientation: Orientation::Normal,
                    }],
                },
                HostConfig {
                    host_id: uuid::Uuid::from_u128(2),
                    name: "macmini".into(),
                    address: "192.168.1.20".into(),
                    platform: "macos".into(),
                    world_offset: None,
                    displays: vec![DisplayConfig {
                        id: 0,
                        origin: [0, 0],
                        resolution: [3840, 2160],
                        scale_factor: 2.0,
                        orientation: Orientation::Normal,
                    }],
                },
            ],
            border_links: vec![],
            trusted_peers: Default::default(),
        };

        let topos = build_topologies(&config);

        let win = topos
            .iter()
            .find(|t| t.host_id == uuid::Uuid::from_u128(1))
            .unwrap();
        let wr = win.displays[0].logical_rect();
        assert!((wr.w - 3440.0).abs() < 0.01, "Win w={}", wr.w);
        assert!((wr.h - 1440.0).abs() < 0.01, "Win h={}", wr.h);

        let mac = topos
            .iter()
            .find(|t| t.host_id == uuid::Uuid::from_u128(2))
            .unwrap();
        let mr = mac.displays[0].logical_rect();
        assert!((mr.w - 1920.0).abs() < 0.01, "Mac w={}", mr.w);
        assert!((mr.h - 1080.0).abs() < 0.01, "Mac h={}", mr.h);
    }

    #[tokio::test]
    async fn test_run_with_stop_cancels_in_dry_run() {
        let server_id = uuid::Uuid::new_v4();
        let config = KaniConfig {
            server: kani_proto::config::ServerConfig {
                host_id: server_id,
                bind_port: 0,
                clipboard_port: None,
            },
            gui: kani_proto::config::GuiConfig::default(),
            hosts: vec![kani_proto::config::HostConfig {
                host_id: server_id,
                name: "test".into(),
                address: "127.0.0.1".into(),
                platform: "windows".into(),
                world_offset: None,
                displays: vec![kani_proto::config::DisplayConfig {
                    id: 0,
                    origin: [0, 0],
                    resolution: [1920, 1080],
                    scale_factor: 1.0,
                    orientation: kani_proto::topology::Orientation::Normal,
                }],
            }],
            border_links: vec![],
            trusted_peers: std::collections::HashMap::new(),
        };

        let token = CancellationToken::new();
        let token_clone = token.clone();

        let handle = tokio::spawn(async move { run_with_stop(config, true, token_clone).await });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        token.cancel();

        let result = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;

        assert!(result.is_ok(), "run_with_stop should stop within 2s");
        let inner = result.unwrap().unwrap();
        assert!(inner.is_ok());
    }

    #[test]
    fn test_build_border_links_converts_range_to_logical() {
        use kani_proto::config::*;
        use kani_proto::topology::{CoordinateMapping, Edge, Orientation};

        let config = KaniConfig {
            server: ServerConfig {
                host_id: uuid::Uuid::from_u128(1),
                bind_port: 24900,
                clipboard_port: None,
            },
            gui: GuiConfig {
                role: "Host".into(),
                ..GuiConfig::default()
            },
            hosts: vec![
                HostConfig {
                    host_id: uuid::Uuid::from_u128(1),
                    name: "windows".into(),
                    address: "127.0.0.1".into(),
                    platform: "windows".into(),
                    world_offset: None,
                    displays: vec![DisplayConfig {
                        id: 0,
                        origin: [0, 0],
                        resolution: [3440, 1440],
                        scale_factor: 1.0,
                        orientation: Orientation::Normal,
                    }],
                },
                HostConfig {
                    host_id: uuid::Uuid::from_u128(2),
                    name: "macmini".into(),
                    address: "192.168.1.20".into(),
                    platform: "macos".into(),
                    world_offset: None,
                    displays: vec![DisplayConfig {
                        id: 0,
                        origin: [0, 0],
                        resolution: [3840, 2160],
                        scale_factor: 2.0,
                        orientation: Orientation::Normal,
                    }],
                },
            ],
            border_links: vec![BorderLinkConfig {
                from_host: uuid::Uuid::from_u128(1),
                from_display: 0,
                from_edge: Edge::Left,
                from_range: [0.0, 1440.0],
                from_edge_coord: None,
                to_host: uuid::Uuid::from_u128(2),
                to_display: 0,
                to_edge: Edge::Right,
                to_range: [0.0, 1440.0],
                to_edge_coord: None,
                mapping: CoordinateMapping::Linear,
            }],
            trusted_peers: Default::default(),
        };

        let topos = build_topologies(&config);
        let links = build_border_links(&config, &topos);
        assert_eq!(links.len(), 1);
        let link = &links[0];

        // from (Windows scale=1.0): range unchanged
        assert!((link.from.range_end - 1440.0).abs() < 0.01);
        // to (Mac scale=2.0): 1440/2 = 720
        assert!(
            (link.to.range_end - 720.0).abs() < 0.01,
            "got {}",
            link.to.range_end
        );
        // to edge_coord: Mac logical Right = 0 + 1920
        assert!(
            (link.to.edge_coord - 1920.0).abs() < 0.01,
            "got {}",
            link.to.edge_coord
        );
    }

    #[test]
    fn test_display_scale_for_logical_windows_returns_1() {
        use kani_proto::config::*;
        use kani_proto::topology::Orientation;
        let config = KaniConfig {
            server: ServerConfig {
                host_id: uuid::Uuid::from_u128(1),
                bind_port: 24900,
                clipboard_port: None,
            },
            gui: GuiConfig {
                role: "Host".into(),
                ..GuiConfig::default()
            },
            hosts: vec![HostConfig {
                host_id: uuid::Uuid::from_u128(1),
                name: "win".into(),
                address: "127.0.0.1".into(),
                platform: "windows".into(),
                world_offset: None,
                displays: vec![DisplayConfig {
                    id: 0,
                    origin: [0, 0],
                    resolution: [2560, 1440],
                    scale_factor: 1.5,
                    orientation: Orientation::Normal,
                }],
            }],
            border_links: vec![],
            trusted_peers: Default::default(),
        };
        // Windows with scale 1.5 still returns 1.0 (Per-Monitor V2 uses physical pixels)
        assert!(
            (display_scale_for_logical(&config, uuid::Uuid::from_u128(1), 0) - 1.0).abs() < 0.01
        );
    }

    #[test]
    fn test_display_scale_for_logical_mac_returns_scale() {
        use kani_proto::config::*;
        use kani_proto::topology::Orientation;
        let config = KaniConfig {
            server: ServerConfig {
                host_id: uuid::Uuid::from_u128(1),
                bind_port: 24900,
                clipboard_port: None,
            },
            gui: GuiConfig {
                role: "Host".into(),
                ..GuiConfig::default()
            },
            hosts: vec![HostConfig {
                host_id: uuid::Uuid::from_u128(2),
                name: "mac".into(),
                address: "192.168.1.20".into(),
                platform: "macos".into(),
                world_offset: None,
                displays: vec![DisplayConfig {
                    id: 0,
                    origin: [0, 0],
                    resolution: [3840, 2160],
                    scale_factor: 2.0,
                    orientation: Orientation::Normal,
                }],
            }],
            border_links: vec![],
            trusted_peers: Default::default(),
        };
        assert!(
            (display_scale_for_logical(&config, uuid::Uuid::from_u128(2), 0) - 2.0).abs() < 0.01
        );
    }
}
