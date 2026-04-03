use kani_proto::event::{DisplayId, HostId};
use kani_proto::topology::{CoordinateMapping, Edge, Orientation};
use std::path::PathBuf;

/// A host as seen by the GUI canvas.
#[derive(Debug, Clone)]
pub struct GuiHost {
    pub host_id: HostId,
    pub name: String,
    pub address: String,
    pub is_local: bool,
    pub platform: String,
    pub displays: Vec<GuiDisplay>,
    /// Drag offset in GUI world coordinates.
    /// Local host: always (0.0, 0.0). Remote hosts: updated by dragging.
    pub gui_offset: (f64, f64),
    /// Whether this host is currently connected (runtime-only, not persisted).
    pub connected: bool,
}

impl GuiHost {
    /// Bounding box in GUI world coordinates.
    pub fn world_bounding_box(&self) -> (f64, f64, f64, f64) {
        if self.displays.is_empty() {
            return (self.gui_offset.0, self.gui_offset.1, 0.0, 0.0);
        }
        let mut min_x = f64::MAX;
        let mut min_y = f64::MAX;
        let mut max_x = f64::MIN;
        let mut max_y = f64::MIN;
        for d in &self.displays {
            let wx = d.world_x(self.gui_offset);
            let wy = d.world_y(self.gui_offset);
            min_x = min_x.min(wx);
            min_y = min_y.min(wy);
            max_x = max_x.max(wx + d.width);
            max_y = max_y.max(wy + d.height);
        }
        (min_x, min_y, max_x - min_x, max_y - min_y)
    }
}

/// A single display rectangle.
#[derive(Debug, Clone)]
pub struct GuiDisplay {
    pub id: DisplayId,
    /// Host-local origin from OS (preserved for config save).
    pub local_origin_x: f64,
    pub local_origin_y: f64,
    pub width: f64,
    pub height: f64,
    pub resolution: (u32, u32),
    pub scale_factor: f64,
    pub orientation: Orientation,
}

impl GuiDisplay {
    pub fn world_x(&self, host_offset: (f64, f64)) -> f64 {
        host_offset.0 + self.local_origin_x
    }
    pub fn world_y(&self, host_offset: (f64, f64)) -> f64 {
        host_offset.1 + self.local_origin_y
    }
}

/// A detected border crossing zone in GUI world coordinates.
#[derive(Debug, Clone)]
pub struct GuiBorderLink {
    pub from_host: HostId,
    pub from_display: DisplayId,
    pub from_edge: Edge,
    pub from_range: (f64, f64),
    pub to_host: HostId,
    pub to_display: DisplayId,
    pub to_edge: Edge,
    pub to_range: (f64, f64),
    pub mapping: CoordinateMapping,
}

/// Drag state during interaction.
#[derive(Debug, Clone)]
pub struct DragState {
    pub host_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Settings,
    DisplayConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Host,
    Client,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected { host_addr: String },
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KvmStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKvmStatus {
    Idle,
    Active,
    Disconnected,
}

/// Calculate X offset for placing a new host to the right of all existing hosts.
/// Gap is 0 so that border_gen auto-detects adjacency (ADJACENCY_TOLERANCE = 200px).
pub fn next_host_offset_x(hosts: &[GuiHost]) -> f64 {
    hosts
        .iter()
        .map(|h| {
            let bb = h.world_bounding_box();
            bb.0 + bb.2
        })
        .fold(0.0_f64, f64::max)
}

/// Complete GUI state.
pub struct GuiState {
    pub hosts: Vec<GuiHost>,
    pub border_links: Vec<GuiBorderLink>,
    pub selected_host: Option<usize>,
    pub dragging: Option<DragState>,
    pub config_path: PathBuf,
    pub server_host_id: HostId,
    pub bind_port: u16,
    pub active_tab: Tab,
    pub status_message: Option<String>,
    /// Preserved from loaded config — not editable in GUI, carried through on save.
    pub trusted_peers: std::collections::HashMap<HostId, String>,
    // Host/Client role fields
    pub role: Role,
    pub connect_address: String,
    // Host-Authority sync fields
    pub connection: ConnectionState,
    pub needs_layout_broadcast: bool,
    pub kvm_status: KvmStatus,
    // Unified server (Host) and client agent (Client)
    pub unified_server: Option<crate::unified_server::UnifiedServerHandle>,
    pub client_agent: Option<crate::client_agent::ClientAgentHandle>,
    pub client_kvm_status: ClientKvmStatus,
    pub dirty_since: Option<std::time::Instant>,
    pub status_set_at: Option<std::time::Instant>,
    pub mouse_sensitivity: f64,
    /// The host_id of the actual Host machine (set on Client side from LayoutSync sender).
    pub remote_host_id: Option<HostId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_world_coordinates() {
        let d = GuiDisplay {
            id: 0,
            local_origin_x: 100.0,
            local_origin_y: 200.0,
            width: 1920.0,
            height: 1080.0,
            resolution: (1920, 1080),
            scale_factor: 1.0,
            orientation: Orientation::Normal,
        };
        let offset = (3000.0, 500.0);
        assert!((d.world_x(offset) - 3100.0).abs() < 0.01);
        assert!((d.world_y(offset) - 700.0).abs() < 0.01);
    }

    #[test]
    fn test_host_bounding_box() {
        let host = GuiHost {
            host_id: Uuid::nil(),
            name: "test".into(),
            address: "127.0.0.1".into(),
            is_local: false,
            platform: "windows".into(),
            gui_offset: (1000.0, 0.0),
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
            connected: false,
        };
        let (x, y, w, h) = host.world_bounding_box();
        assert!((x - 1000.0).abs() < 0.01);
        assert!((y - 0.0).abs() < 0.01);
        assert!((w - 1920.0).abs() < 0.01);
        assert!((h - 1080.0).abs() < 0.01);
    }

    #[test]
    fn test_next_host_offset_single_local() {
        let hosts = vec![GuiHost {
            host_id: Uuid::from_u128(1),
            name: "local".into(),
            address: "127.0.0.1".into(),
            is_local: true,
            platform: "windows".into(),
            gui_offset: (0.0, 0.0),
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
        }];
        let x = next_host_offset_x(&hosts);
        // Host has display at (0,0) with width 1920, plus local_origin_x=100
        // world_bounding_box right edge = 0 + 100 + 1920 = 2020
        // No gap added (was +100, now +0 for adjacency)
        assert!((x - 1920.0).abs() < 0.01, "got {x}");
    }

    #[test]
    fn test_next_host_offset_empty() {
        let hosts: Vec<GuiHost> = vec![];
        let x = next_host_offset_x(&hosts);
        assert!((x - 0.0).abs() < 0.01, "got {x}");
    }

    #[test]
    fn test_gui_host_connected_default_values() {
        let local = GuiHost {
            host_id: Uuid::nil(),
            name: "local".into(),
            address: "127.0.0.1".into(),
            is_local: true,
            platform: "windows".into(),
            gui_offset: (0.0, 0.0),
            displays: vec![],
            connected: true,
        };
        assert!(local.connected);

        let remote = GuiHost {
            host_id: Uuid::from_u128(2),
            name: "remote".into(),
            address: "192.168.1.20".into(),
            is_local: false,
            platform: "macos".into(),
            gui_offset: (1920.0, 0.0),
            displays: vec![],
            connected: false,
        };
        assert!(!remote.connected);
    }
}
