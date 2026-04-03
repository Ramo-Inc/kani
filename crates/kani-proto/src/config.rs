use crate::event::{DisplayId, HostId};
use crate::topology::{CoordinateMapping, Edge, Orientation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Returns the current platform identifier at compile time.
pub fn current_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct KaniConfig {
    pub server: ServerConfig,
    #[serde(default)]
    pub gui: GuiConfig,
    #[serde(default)]
    pub hosts: Vec<HostConfig>,
    #[serde(default)]
    pub border_links: Vec<BorderLinkConfig>,
    #[serde(default)]
    pub trusted_peers: HashMap<HostId, String>,
}

/// GUI-specific settings (persisted but not used by kani-app).
#[derive(Debug, Serialize, Deserialize)]
pub struct GuiConfig {
    #[serde(default = "default_role")]
    pub role: String,
    #[serde(default)]
    pub connect_address: String,
    #[serde(default = "default_mouse_sensitivity")]
    pub mouse_sensitivity: f64,
}

fn default_role() -> String {
    "Client".into()
}

fn default_mouse_sensitivity() -> f64 {
    1.0
}

impl Default for GuiConfig {
    fn default() -> Self {
        Self {
            role: default_role(),
            connect_address: String::new(),
            mouse_sensitivity: default_mouse_sensitivity(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host_id: HostId,
    pub bind_port: u16,
    #[serde(default)]
    pub clipboard_port: Option<u16>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HostConfig {
    pub host_id: HostId,
    pub name: String,
    pub address: String,
    #[serde(default = "default_platform")]
    pub platform: String,
    #[serde(default)]
    pub world_offset: Option<[f64; 2]>,
    #[serde(default)]
    pub displays: Vec<DisplayConfig>,
}

fn default_platform() -> String {
    "unknown".into()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DisplayConfig {
    pub id: DisplayId,
    pub origin: [i32; 2],
    pub resolution: [u32; 2],
    pub scale_factor: f64,
    pub orientation: Orientation,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BorderLinkConfig {
    pub from_host: HostId,
    pub from_display: DisplayId,
    pub from_edge: Edge,
    pub from_range: [f64; 2],
    pub from_edge_coord: Option<f64>,
    pub to_host: HostId,
    pub to_display: DisplayId,
    pub to_edge: Edge,
    pub to_range: [f64; 2],
    pub to_edge_coord: Option<f64>,
    pub mapping: CoordinateMapping,
}

impl KaniConfig {
    pub fn load(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&content)?)
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
        let content = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, &content)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Validate config: server host_id exists in hosts, border link hosts exist.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        let host_ids: std::collections::HashSet<_> = self.hosts.iter().map(|h| h.host_id).collect();

        if !host_ids.contains(&self.server.host_id) {
            errors.push(format!(
                "server.host_id {} not found in hosts",
                self.server.host_id
            ));
        }

        for (i, bl) in self.border_links.iter().enumerate() {
            if !host_ids.contains(&bl.from_host) {
                errors.push(format!(
                    "border_links[{}].from_host {} not found in hosts",
                    i, bl.from_host
                ));
            }
            if !host_ids.contains(&bl.to_host) {
                errors.push(format!(
                    "border_links[{}].to_host {} not found in hosts",
                    i, bl.to_host
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE_CONFIG: &str = r#"
[server]
host_id = "11111111-1111-1111-1111-111111111111"
bind_port = 24900

[[hosts]]
host_id = "11111111-1111-1111-1111-111111111111"
name = "macbook"
address = "192.168.1.10"

[[hosts.displays]]
id = 0
origin = [0, 0]
resolution = [3840, 2160]
scale_factor = 2.0
orientation = "Normal"

[[hosts]]
host_id = "22222222-2222-2222-2222-222222222222"
name = "windows-pc"
address = "192.168.1.20"

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

    #[test]
    fn test_parse_example_config() {
        let config: KaniConfig = toml::from_str(EXAMPLE_CONFIG).unwrap();
        assert_eq!(config.hosts.len(), 2);
        assert_eq!(config.border_links.len(), 1);
        assert_eq!(config.server.bind_port, 24900);
    }

    #[test]
    fn test_parse_with_trusted_peers() {
        let toml_str = r#"
[server]
host_id = "11111111-1111-1111-1111-111111111111"
bind_port = 24900

[[hosts]]
host_id = "11111111-1111-1111-1111-111111111111"
name = "test"
address = "127.0.0.1"

[trusted_peers]
"22222222-2222-2222-2222-222222222222" = "sha256:AB:CD:EF"
"#;
        let config: KaniConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.trusted_peers.len(), 1);
    }

    #[test]
    fn test_config_save_and_reload() {
        let config: KaniConfig = toml::from_str(EXAMPLE_CONFIG).unwrap();
        let tmp = std::env::temp_dir().join("kani-test-config.toml");
        config.save(&tmp).unwrap();
        let reloaded = KaniConfig::load(&tmp).unwrap();
        assert_eq!(reloaded.hosts.len(), config.hosts.len());
        assert_eq!(reloaded.border_links.len(), config.border_links.len());
        std::fs::remove_file(tmp).ok();
    }

    #[test]
    fn test_validate_valid_config() {
        let config: KaniConfig = toml::from_str(EXAMPLE_CONFIG).unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_missing_server_host() {
        let toml_str = r#"
[server]
host_id = "99999999-9999-9999-9999-999999999999"
bind_port = 24900

[[hosts]]
host_id = "11111111-1111-1111-1111-111111111111"
name = "test"
address = "127.0.0.1"
"#;
        let config: KaniConfig = toml::from_str(toml_str).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err()[0].contains("server.host_id"));
    }

    #[test]
    fn test_validate_missing_border_link_host() {
        let toml_str = r#"
[server]
host_id = "11111111-1111-1111-1111-111111111111"
bind_port = 24900

[[hosts]]
host_id = "11111111-1111-1111-1111-111111111111"
name = "test"
address = "127.0.0.1"

[[border_links]]
from_host = "11111111-1111-1111-1111-111111111111"
from_display = 0
from_edge = "Right"
from_range = [0.0, 1080.0]
to_host = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
to_display = 0
to_edge = "Left"
to_range = [0.0, 1440.0]
mapping = "Linear"
"#;
        let config: KaniConfig = toml::from_str(toml_str).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("to_host")));
    }

    #[test]
    fn test_parse_border_link_with_edge_coord() {
        let toml_str = r#"
[server]
host_id = "11111111-1111-1111-1111-111111111111"
bind_port = 24900

[[border_links]]
from_host = "11111111-1111-1111-1111-111111111111"
from_display = 0
from_edge = "Right"
from_range = [0.0, 1080.0]
from_edge_coord = 1920.0
to_host = "22222222-2222-2222-2222-222222222222"
to_display = 0
to_edge = "Left"
to_range = [0.0, 1440.0]
to_edge_coord = 0.0
mapping = "Linear"
"#;
        let config: KaniConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.border_links[0].from_edge_coord, Some(1920.0));
        assert_eq!(config.border_links[0].to_edge_coord, Some(0.0));
    }

    #[test]
    fn test_parse_border_link_without_edge_coord() {
        let toml_str = r#"
[server]
host_id = "11111111-1111-1111-1111-111111111111"
bind_port = 24900

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
        let config: KaniConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.border_links[0].from_edge_coord, None);
        assert_eq!(config.border_links[0].to_edge_coord, None);
    }

    #[test]
    fn test_clipboard_port_default_none() {
        let toml_str = r#"
[server]
host_id = "11111111-1111-1111-1111-111111111111"
bind_port = 24900

[[hosts]]
host_id = "11111111-1111-1111-1111-111111111111"
name = "test"
address = "127.0.0.1"
"#;
        let config: KaniConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.server.clipboard_port, None);
        // Resolved default: bind_port + 1
        assert_eq!(
            config
                .server
                .clipboard_port
                .unwrap_or(config.server.bind_port + 1),
            24901
        );
    }

    #[test]
    fn test_clipboard_port_explicit() {
        let toml_str = r#"
[server]
host_id = "11111111-1111-1111-1111-111111111111"
bind_port = 24900
clipboard_port = 25000

[[hosts]]
host_id = "11111111-1111-1111-1111-111111111111"
name = "test"
address = "127.0.0.1"
"#;
        let config: KaniConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.server.clipboard_port, Some(25000));
    }

    #[test]
    fn test_host_config_platform_default() {
        let toml_str = r#"
            host_id = "11111111-1111-1111-1111-111111111111"
            name = "test"
            address = "127.0.0.1"
            [[displays]]
            id = 0
            origin = [0, 0]
            resolution = [1920, 1080]
            scale_factor = 1.0
            orientation = "Normal"
        "#;
        let config: HostConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.platform, "unknown");
    }

    #[test]
    fn test_host_config_platform_explicit() {
        let toml_str = r#"
            host_id = "11111111-1111-1111-1111-111111111111"
            name = "mac"
            address = "192.168.1.20"
            platform = "macos"
            [[displays]]
            id = 0
            origin = [0, 0]
            resolution = [3840, 2160]
            scale_factor = 2.0
            orientation = "Normal"
        "#;
        let config: HostConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.platform, "macos");
    }

    #[test]
    fn test_save_does_not_leave_tmp_file() {
        let config: KaniConfig = toml::from_str(EXAMPLE_CONFIG).unwrap();
        let tmp_dir = std::env::temp_dir().join("kani-atomic-save-test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let path = tmp_dir.join("kani.toml");
        config.save(&path).unwrap();

        // Config file should exist
        assert!(path.exists());
        // Temp file should NOT remain
        let tmp_path = path.with_extension("toml.tmp");
        assert!(
            !tmp_path.exists(),
            ".toml.tmp should be cleaned up after rename"
        );

        // Content should be valid TOML that round-trips
        let reloaded = KaniConfig::load(&path).unwrap();
        assert_eq!(reloaded.hosts.len(), config.hosts.len());

        std::fs::remove_dir_all(tmp_dir).ok();
    }

    #[test]
    fn test_host_config_world_offset_explicit() {
        let toml_str = r#"
            host_id = "11111111-1111-1111-1111-111111111111"
            name = "remote"
            address = "192.168.1.20"
            platform = "macos"
            world_offset = [1920.0, 500.0]
            [[displays]]
            id = 0
            origin = [0, 0]
            resolution = [2560, 1440]
            scale_factor = 2.0
            orientation = "Normal"
        "#;
        let config: HostConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.world_offset, Some([1920.0, 500.0]));
    }

    #[test]
    fn test_host_config_world_offset_default_none() {
        let toml_str = r#"
            host_id = "11111111-1111-1111-1111-111111111111"
            name = "test"
            address = "127.0.0.1"
            [[displays]]
            id = 0
            origin = [0, 0]
            resolution = [1920, 1080]
            scale_factor = 1.0
            orientation = "Normal"
        "#;
        let config: HostConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.world_offset, None);
    }
}
