use std::path::Path;

use kani_proto::config::{BorderLinkConfig, DisplayConfig, HostConfig, KaniConfig, ServerConfig};
use kani_proto::topology::{Edge, Orientation};

use crate::model::{
    ConnectionState, GuiBorderLink, GuiDisplay, GuiHost, GuiState, KvmStatus, Role, Tab,
};

pub fn load_gui_state(
    config_path: &Path,
    local_displays: &[kani_platform::types::DisplayInfo],
) -> Result<GuiState, Box<dyn std::error::Error>> {
    let config = if config_path.exists() {
        KaniConfig::load(config_path)?
    } else {
        default_config()
    };

    let mut hosts = Vec::new();
    let local_host_id = config.server.host_id;
    let local_config_host = config.hosts.iter().find(|h| h.host_id == local_host_id);
    let local_name = local_config_host
        .map(|h| h.name.clone())
        .unwrap_or_else(|| "local".into());
    let local_addr = local_config_host
        .map(|h| h.address.clone())
        .unwrap_or_else(|| "127.0.0.1".into());

    let local_gui_displays: Vec<GuiDisplay> = local_displays
        .iter()
        .map(|d| GuiDisplay {
            id: d.id,
            local_origin_x: d.origin_x,
            local_origin_y: d.origin_y,
            width: d.width_pixels as f64,
            height: d.height_pixels as f64,
            resolution: (d.width_pixels, d.height_pixels),
            scale_factor: d.scale_factor,
            orientation: Orientation::Normal,
        })
        .collect();

    hosts.push(GuiHost {
        host_id: local_host_id,
        name: local_name,
        address: local_addr,
        is_local: true,
        platform: kani_proto::config::current_platform().into(),
        gui_offset: (0.0, 0.0),
        displays: local_gui_displays,
        connected: true,
    });

    // Load saved remote hosts from config (restores previous session layout)
    for h in &config.hosts {
        if h.host_id == local_host_id {
            continue; // Local host already added with fresh display info
        }
        hosts.push(GuiHost {
            host_id: h.host_id,
            name: h.name.clone(),
            address: h.address.clone(),
            is_local: false,
            platform: h.platform.clone(),
            gui_offset: h
                .world_offset
                .map(|o| (o[0], o[1]))
                .unwrap_or_else(|| compute_gui_offset_from_displays(&h.displays)),
            displays: h
                .displays
                .iter()
                .map(|d| GuiDisplay {
                    id: d.id,
                    local_origin_x: d.origin[0] as f64,
                    local_origin_y: d.origin[1] as f64,
                    width: d.resolution[0] as f64,
                    height: d.resolution[1] as f64,
                    resolution: (d.resolution[0], d.resolution[1]),
                    scale_factor: d.scale_factor,
                    orientation: d.orientation,
                })
                .collect(),
            connected: false,
        });
    }

    // Load saved border links from config
    let border_links = config
        .border_links
        .iter()
        .filter_map(|bl| config_border_to_gui(bl, &hosts))
        .collect();

    Ok(GuiState {
        hosts,
        border_links,
        selected_host: None,
        dragging: None,
        config_path: config_path.to_path_buf(),
        server_host_id: config.server.host_id,
        bind_port: config.server.bind_port,
        active_tab: Tab::DisplayConfig,
        status_message: None,
        trusted_peers: config.trusted_peers,
        role: match config.gui.role.as_str() {
            "Host" => Role::Host,
            _ => Role::Client,
        },
        connect_address: config.gui.connect_address,
        connection: ConnectionState::Disconnected,
        needs_layout_broadcast: false,
        kvm_status: KvmStatus::Stopped,
        unified_server: None,
        client_agent: None,
        client_kvm_status: crate::model::ClientKvmStatus::Idle,
        dirty_since: None,
        status_set_at: None,
        mouse_sensitivity: config.gui.mouse_sensitivity,
        remote_host_id: None,
    })
}

pub fn save_gui_state(state: &GuiState) -> Result<(), Box<dyn std::error::Error>> {
    let hosts: Vec<HostConfig> = state
        .hosts
        .iter()
        .map(|h| {
            // Strip port from address if present (e.g. "192.168.0.111:64524" → "192.168.0.111")
            let address = h
                .address
                .split(':')
                .next()
                .unwrap_or(&h.address)
                .to_string();
            HostConfig {
                host_id: h.host_id,
                name: h.name.clone(),
                address,
                platform: h.platform.clone(),
                world_offset: Some([h.gui_offset.0, h.gui_offset.1]),
                displays: h
                    .displays
                    .iter()
                    .map(|d| DisplayConfig {
                        id: d.id,
                        origin: [d.local_origin_x as i32, d.local_origin_y as i32],
                        resolution: [d.resolution.0, d.resolution.1],
                        scale_factor: d.scale_factor,
                        orientation: d.orientation,
                    })
                    .collect(),
            }
        })
        .collect();

    let border_links: Vec<BorderLinkConfig> = state
        .border_links
        .iter()
        .filter(|bl| {
            state.hosts.iter().any(|h| h.host_id == bl.from_host)
                && state.hosts.iter().any(|h| h.host_id == bl.to_host)
        })
        .map(|bl| gui_border_to_config(bl, &state.hosts))
        .collect();

    let config = KaniConfig {
        server: ServerConfig {
            host_id: state.server_host_id,
            bind_port: state.bind_port,
            clipboard_port: None,
        },
        gui: kani_proto::config::GuiConfig {
            role: match state.role {
                Role::Host => "Host".into(),
                Role::Client => "Client".into(),
            },
            connect_address: state.connect_address.clone(),
            mouse_sensitivity: state.mouse_sensitivity,
        },
        hosts,
        border_links,
        trusted_peers: state.trusted_peers.clone(),
    };
    config.save(&state.config_path)?;
    Ok(())
}

/// Build KaniConfig from GUI state (for KVM start, no file write).
/// Only connected hosts and their border links are included.
pub fn build_config(state: &GuiState) -> KaniConfig {
    let connect_ip = state
        .connect_address
        .split(':')
        .next()
        .unwrap_or(&state.connect_address)
        .to_string();
    let connected_ids: std::collections::HashSet<kani_proto::event::HostId> = state
        .hosts
        .iter()
        .filter(|h| h.connected)
        .map(|h| h.host_id)
        .collect();
    let hosts: Vec<HostConfig> = state
        .hosts
        .iter()
        .filter(|h| h.connected)
        .map(|h| {
            let address = if h.is_local {
                h.address.clone()
            } else {
                // Extract IP only (strip port if present, e.g. "192.168.0.111:64524" → "192.168.0.111")
                let raw = &h.address;
                let ip_only = raw.split(':').next().unwrap_or(raw).to_string();
                if ip_only.is_empty() || ip_only == "127.0.0.1" {
                    connect_ip.clone()
                } else {
                    ip_only
                }
            };
            HostConfig {
                host_id: h.host_id,
                name: h.name.clone(),
                address,
                platform: h.platform.clone(),
                world_offset: Some([h.gui_offset.0, h.gui_offset.1]),
                displays: h
                    .displays
                    .iter()
                    .map(|d| DisplayConfig {
                        id: d.id,
                        origin: [d.local_origin_x as i32, d.local_origin_y as i32],
                        resolution: [d.resolution.0, d.resolution.1],
                        scale_factor: d.scale_factor,
                        orientation: d.orientation,
                    })
                    .collect(),
            }
        })
        .collect();

    let border_links: Vec<BorderLinkConfig> = state
        .border_links
        .iter()
        .filter(|bl| connected_ids.contains(&bl.from_host) && connected_ids.contains(&bl.to_host))
        .map(|bl| gui_border_to_config(bl, &state.hosts))
        .collect();

    KaniConfig {
        server: ServerConfig {
            host_id: state.server_host_id,
            bind_port: state.bind_port,
            clipboard_port: None,
        },
        gui: kani_proto::config::GuiConfig {
            role: match state.role {
                Role::Host => "Host".into(),
                Role::Client => "Client".into(),
            },
            connect_address: state.connect_address.clone(),
            mouse_sensitivity: state.mouse_sensitivity,
        },
        hosts,
        border_links,
        trusted_peers: state.trusted_peers.clone(),
    }
}

pub fn gui_border_to_config(link: &GuiBorderLink, hosts: &[GuiHost]) -> BorderLinkConfig {
    let from_host = hosts.iter().find(|h| h.host_id == link.from_host).unwrap();
    let to_host = hosts.iter().find(|h| h.host_id == link.to_host).unwrap();

    let from_range =
        subtract_offset_from_range(link.from_edge, link.from_range, from_host.gui_offset);
    let to_range = subtract_offset_from_range(link.to_edge, link.to_range, to_host.gui_offset);

    BorderLinkConfig {
        from_host: link.from_host,
        from_display: link.from_display,
        from_edge: link.from_edge,
        from_range,
        from_edge_coord: None,
        to_host: link.to_host,
        to_display: link.to_display,
        to_edge: link.to_edge,
        to_range,
        to_edge_coord: None,
        mapping: link.mapping,
    }
}

fn subtract_offset_from_range(edge: Edge, range: (f64, f64), offset: (f64, f64)) -> [f64; 2] {
    match edge {
        Edge::Left | Edge::Right => [range.0 - offset.1, range.1 - offset.1],
        Edge::Top | Edge::Bottom => [range.0 - offset.0, range.1 - offset.0],
    }
}

/// Compute a GUI offset from saved display origins.
/// Uses the minimum origin as the host's world-space offset.
fn compute_gui_offset_from_displays(displays: &[DisplayConfig]) -> (f64, f64) {
    if displays.is_empty() {
        return (0.0, 0.0);
    }
    let min_x = displays.iter().map(|d| d.origin[0]).min().unwrap_or(0) as f64;
    let min_y = displays.iter().map(|d| d.origin[1]).min().unwrap_or(0) as f64;
    (min_x, min_y)
}

/// Convert a saved BorderLinkConfig back to a GuiBorderLink for the GUI canvas.
fn config_border_to_gui(bl: &BorderLinkConfig, hosts: &[GuiHost]) -> Option<GuiBorderLink> {
    let from_host = hosts.iter().find(|h| h.host_id == bl.from_host)?;
    let to_host = hosts.iter().find(|h| h.host_id == bl.to_host)?;

    let from_range = add_offset_to_range(bl.from_edge, bl.from_range, from_host.gui_offset);
    let to_range = add_offset_to_range(bl.to_edge, bl.to_range, to_host.gui_offset);

    Some(GuiBorderLink {
        from_host: bl.from_host,
        from_display: bl.from_display,
        from_edge: bl.from_edge,
        from_range,
        to_host: bl.to_host,
        to_display: bl.to_display,
        to_edge: bl.to_edge,
        to_range,
        mapping: bl.mapping,
    })
}

fn add_offset_to_range(edge: Edge, range: [f64; 2], offset: (f64, f64)) -> (f64, f64) {
    match edge {
        Edge::Left | Edge::Right => (range[0] + offset.1, range[1] + offset.1),
        Edge::Top | Edge::Bottom => (range[0] + offset.0, range[1] + offset.0),
    }
}

fn default_config() -> KaniConfig {
    KaniConfig {
        server: ServerConfig {
            host_id: uuid::Uuid::new_v4(),
            bind_port: 24900,
            clipboard_port: None,
        },
        gui: kani_proto::config::GuiConfig::default(),
        hosts: Vec::new(),
        border_links: Vec::new(),
        trusted_peers: std::collections::HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ClientKvmStatus;
    use kani_proto::topology::CoordinateMapping;
    use uuid::Uuid;

    fn host_a_id() -> uuid::Uuid {
        Uuid::from_u128(1)
    }
    fn host_b_id() -> uuid::Uuid {
        Uuid::from_u128(2)
    }

    fn make_host(id: uuid::Uuid, offset: (f64, f64)) -> GuiHost {
        GuiHost {
            host_id: id,
            name: "test".into(),
            address: "127.0.0.1".into(),
            is_local: id == host_a_id(),
            platform: "windows".into(),
            gui_offset: offset,
            displays: vec![GuiDisplay {
                id: 0,
                local_origin_x: 0.0,
                local_origin_y: 0.0,
                width: 1920.0,
                height: 1080.0,
                resolution: (1920, 1080),
                scale_factor: 1.0,
                orientation: Orientation::Normal,
            }],
            connected: true,
        }
    }

    /// Host A at offset (0,0), Host B at offset (1920,0).
    /// Border link in world coords from_range Y=[0,1080] to_range Y=[0,1080].
    /// Both should stay [0,1080] since offset_y=0 for both.
    #[test]
    fn test_gui_border_to_config_subtracts_offset() {
        let host_a = make_host(host_a_id(), (0.0, 0.0));
        let host_b = make_host(host_b_id(), (1920.0, 0.0));
        let hosts = vec![host_a, host_b];

        let link = GuiBorderLink {
            from_host: host_a_id(),
            from_display: 0,
            from_edge: Edge::Right,
            from_range: (0.0, 1080.0),
            to_host: host_b_id(),
            to_display: 0,
            to_edge: Edge::Left,
            to_range: (0.0, 1080.0),
            mapping: CoordinateMapping::Linear,
        };

        let config = gui_border_to_config(&link, &hosts);

        assert!((config.from_range[0] - 0.0).abs() < 0.01);
        assert!((config.from_range[1] - 1080.0).abs() < 0.01);
        assert!((config.to_range[0] - 0.0).abs() < 0.01);
        assert!((config.to_range[1] - 1080.0).abs() < 0.01);
    }

    /// Top/Bottom edge: range runs along X axis, subtract offset.0.
    /// Host A at (0,0), Host B at (0, 1080). Bottom→Top link with X range [100, 1820].
    /// From (A, offset_x=0): stays [100, 1820]. To (B, offset_x=0): stays [100, 1820].
    #[test]
    fn test_gui_border_to_config_top_bottom_edge() {
        let host_a = make_host(host_a_id(), (0.0, 0.0));
        let host_b = make_host(host_b_id(), (0.0, 1080.0));
        let hosts = vec![host_a, host_b];

        let link = GuiBorderLink {
            from_host: host_a_id(),
            from_display: 0,
            from_edge: Edge::Bottom,
            from_range: (100.0, 1820.0),
            to_host: host_b_id(),
            to_display: 0,
            to_edge: Edge::Top,
            to_range: (100.0, 1820.0),
            mapping: CoordinateMapping::Linear,
        };

        let config = gui_border_to_config(&link, &hosts);

        // Bottom/Top edges: range is along X, subtract offset.0
        // host A offset_x=0: [100-0, 1820-0] = [100, 1820]
        assert!((config.from_range[0] - 100.0).abs() < 0.01);
        assert!((config.from_range[1] - 1820.0).abs() < 0.01);
        // host B offset_x=0: [100-0, 1820-0] = [100, 1820]
        assert!((config.to_range[0] - 100.0).abs() < 0.01);
        assert!((config.to_range[1] - 1820.0).abs() < 0.01);
    }

    /// Top/Bottom edge with X offset: Host B at (500, 1080).
    /// From A (offset_x=0): X range stays. To B (offset_x=500): X range subtracts 500.
    #[test]
    fn test_gui_border_to_config_top_bottom_with_x_offset() {
        let host_a = make_host(host_a_id(), (0.0, 0.0));
        let host_b = make_host(host_b_id(), (500.0, 1080.0));
        let hosts = vec![host_a, host_b];

        let link = GuiBorderLink {
            from_host: host_a_id(),
            from_display: 0,
            from_edge: Edge::Bottom,
            from_range: (500.0, 1920.0),
            to_host: host_b_id(),
            to_display: 0,
            to_edge: Edge::Top,
            to_range: (500.0, 1920.0),
            mapping: CoordinateMapping::Linear,
        };

        let config = gui_border_to_config(&link, &hosts);

        assert!((config.from_range[0] - 500.0).abs() < 0.01);
        assert!((config.from_range[1] - 1920.0).abs() < 0.01);
        // host B offset_x=500: [500-500, 1920-500] = [0, 1420]
        assert!((config.to_range[0] - 0.0).abs() < 0.01);
        assert!((config.to_range[1] - 1420.0).abs() < 0.01);
    }

    /// Host B at offset (1920, 500). World overlap=[500,1080].
    /// From (host A, offset_y=0) stays [500,1080].
    /// To (host B, offset_y=500) becomes [0,580] (subtract 500).
    #[test]
    fn test_gui_border_to_config_with_y_offset() {
        let host_a = make_host(host_a_id(), (0.0, 0.0));
        let host_b = make_host(host_b_id(), (1920.0, 500.0));
        let hosts = vec![host_a, host_b];

        let link = GuiBorderLink {
            from_host: host_a_id(),
            from_display: 0,
            from_edge: Edge::Right,
            from_range: (500.0, 1080.0),
            to_host: host_b_id(),
            to_display: 0,
            to_edge: Edge::Left,
            to_range: (500.0, 1080.0),
            mapping: CoordinateMapping::Linear,
        };

        let config = gui_border_to_config(&link, &hosts);

        // from_host A has offset_y=0, so from_range stays [500,1080]
        assert!((config.from_range[0] - 500.0).abs() < 0.01);
        assert!((config.from_range[1] - 1080.0).abs() < 0.01);
        // to_host B has offset_y=500, so to_range = [500-500, 1080-500] = [0, 580]
        assert!((config.to_range[0] - 0.0).abs() < 0.01);
        assert!((config.to_range[1] - 580.0).abs() < 0.01);
    }

    #[test]
    fn test_world_offset_roundtrip() {
        let hosts = vec![
            make_host(host_a_id(), (0.0, 0.0)),
            make_host(host_b_id(), (1920.0, 500.0)),
        ];

        let link = GuiBorderLink {
            from_host: host_a_id(),
            from_display: 0,
            from_edge: Edge::Right,
            from_range: (500.0, 1080.0),
            to_host: host_b_id(),
            to_display: 0,
            to_edge: Edge::Left,
            to_range: (500.0, 1080.0),
            mapping: CoordinateMapping::Linear,
        };

        // Save
        let config_link = gui_border_to_config(&link, &hosts);

        // Simulate save: host B world_offset = [1920, 500]
        let saved_host_b_config = kani_proto::config::HostConfig {
            host_id: host_b_id(),
            name: "test".into(),
            address: "127.0.0.1".into(),
            platform: "windows".into(),
            world_offset: Some([1920.0, 500.0]),
            displays: vec![kani_proto::config::DisplayConfig {
                id: 0,
                origin: [0, 0],
                resolution: [1920, 1080],
                scale_factor: 1.0,
                orientation: Orientation::Normal,
            }],
        };

        // Simulate load: use world_offset for gui_offset
        let loaded_offset = saved_host_b_config
            .world_offset
            .map(|o| (o[0], o[1]))
            .unwrap_or_else(|| compute_gui_offset_from_displays(&saved_host_b_config.displays));
        assert!((loaded_offset.0 - 1920.0).abs() < 0.01);
        assert!((loaded_offset.1 - 500.0).abs() < 0.01);

        // Reload border link with correct offset
        let loaded_hosts = vec![
            make_host(host_a_id(), (0.0, 0.0)),
            make_host(host_b_id(), loaded_offset),
        ];
        let reloaded = config_border_to_gui(&config_link, &loaded_hosts).unwrap();

        // Ranges should match original
        assert!((reloaded.from_range.0 - link.from_range.0).abs() < 0.01);
        assert!((reloaded.from_range.1 - link.from_range.1).abs() < 0.01);
        assert!((reloaded.to_range.0 - link.to_range.0).abs() < 0.01);
        assert!((reloaded.to_range.1 - link.to_range.1).abs() < 0.01);
    }

    #[test]
    fn test_build_config_excludes_disconnected_hosts() {
        let state = GuiState {
            hosts: vec![
                {
                    let mut h = make_host(host_a_id(), (0.0, 0.0));
                    h.connected = true;
                    h.is_local = true;
                    h
                },
                {
                    let mut h = make_host(host_b_id(), (1920.0, 0.0));
                    h.connected = true;
                    h
                },
                {
                    let mut h = make_host(Uuid::from_u128(3), (3840.0, 0.0));
                    h.connected = false; // disconnected — should be excluded
                    h
                },
            ],
            border_links: vec![
                GuiBorderLink {
                    from_host: host_a_id(),
                    from_display: 0,
                    from_edge: Edge::Right,
                    from_range: (0.0, 1080.0),
                    to_host: host_b_id(),
                    to_display: 0,
                    to_edge: Edge::Left,
                    to_range: (0.0, 1080.0),
                    mapping: CoordinateMapping::Linear,
                },
                GuiBorderLink {
                    from_host: host_b_id(),
                    from_display: 0,
                    from_edge: Edge::Right,
                    from_range: (0.0, 1080.0),
                    to_host: Uuid::from_u128(3),
                    to_display: 0,
                    to_edge: Edge::Left,
                    to_range: (0.0, 1080.0),
                    mapping: CoordinateMapping::Linear,
                },
            ],
            server_host_id: host_a_id(),
            bind_port: 24900,
            role: Role::Host,
            connect_address: String::new(),
            selected_host: None,
            dragging: None,
            config_path: std::path::PathBuf::from("/tmp/test.toml"),
            active_tab: Tab::DisplayConfig,
            status_message: None,
            trusted_peers: std::collections::HashMap::new(),
            connection: ConnectionState::Disconnected,
            needs_layout_broadcast: false,
            kvm_status: KvmStatus::Stopped,
            unified_server: None,
            client_agent: None,
            client_kvm_status: ClientKvmStatus::Idle,
            dirty_since: None,
            status_set_at: None,
            mouse_sensitivity: 1.0,
            remote_host_id: None,
        };

        let config = build_config(&state);
        // Only 2 connected hosts should be included (not the disconnected one)
        assert_eq!(config.hosts.len(), 2);
        assert!(config.hosts.iter().all(|h| h.host_id != Uuid::from_u128(3)));
        // Only the link between connected hosts should be included
        assert_eq!(config.border_links.len(), 1);
        assert_eq!(config.border_links[0].from_host, host_a_id());
        assert_eq!(config.border_links[0].to_host, host_b_id());
    }
}
