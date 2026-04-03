#![windows_subsystem = "windows"]

mod app;
mod border_gen;
mod canvas;
mod client_agent;
mod config_io;
mod footer;
mod icon;
mod model;
mod tabs;
mod tray;
mod unified_server;

use clap::Parser;
use eframe::egui;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[derive(Parser)]
struct Cli {
    #[arg(short, long)]
    config: Option<PathBuf>,
}

/// Resolve config path: explicit flag > cwd/kani.toml > ~/.config/kani/kani.toml
fn resolve_config_path(cli_config: Option<PathBuf>) -> PathBuf {
    if let Some(p) = cli_config {
        return p;
    }
    // If kani.toml exists in cwd, use it (backwards compat for dev)
    let cwd_config = PathBuf::from("kani.toml");
    if cwd_config.exists() {
        return cwd_config;
    }
    // Default: ~/.config/kani/kani.toml
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kani");
    std::fs::create_dir_all(&config_dir).ok();
    config_dir.join("kani.toml")
}

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    let config_path = resolve_config_path(cli.config);
    tracing::info!("Using config: {}", config_path.display());

    // Detect local displays
    let local_displays = {
        let platform = kani_platform::create_platform();

        // macOS: check permissions at startup and warn user
        #[cfg(target_os = "macos")]
        if !platform.check_permissions() {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Warning)
                .set_title("Kani KVM — Permissions Required")
                .set_description(
                    "Kani needs the following macOS permissions to function:\n\n\
                     • Input Monitoring\n\
                     • Accessibility\n\n\
                     Please open System Settings > Privacy & Security and enable both for Kani.\n\n\
                     After granting permissions, restart Kani.",
                )
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        }

        platform.enumerate_displays()
    };

    // Load GUI state from config
    let mut state = config_io::load_gui_state(&config_path, &local_displays).unwrap_or_else(|e| {
        tracing::warn!("Failed to load config: {e}, using defaults");
        model::GuiState {
            hosts: Vec::new(),
            border_links: Vec::new(),
            selected_host: None,
            dragging: None,
            config_path: config_path.clone(),
            server_host_id: uuid::Uuid::new_v4(),
            bind_port: 24900,
            active_tab: model::Tab::DisplayConfig,
            status_message: None,
            trusted_peers: std::collections::HashMap::new(),
            role: model::Role::Client,
            connect_address: String::new(),
            connection: model::ConnectionState::Disconnected,
            needs_layout_broadcast: false,
            kvm_status: model::KvmStatus::Stopped,
            unified_server: None,
            client_agent: None,
            client_kvm_status: model::ClientKvmStatus::Idle,
            dirty_since: None,
            status_set_at: None,
            mouse_sensitivity: 1.0,
            remote_host_id: None,
        }
    });

    // Generate initial border links
    state.border_links = border_gen::generate_border_links(&state.hosts);

    // Auto-start based on saved role
    match state.role {
        model::Role::Host => {
            let local_host = state.hosts.iter().find(|h| h.is_local);
            let snapshots: Vec<kani_proto::event::DisplaySnapshot> = local_host
                .map(|h| {
                    h.displays
                        .iter()
                        .map(|d| kani_proto::event::DisplaySnapshot {
                            id: d.id,
                            origin_x: d.local_origin_x,
                            origin_y: d.local_origin_y,
                            width: d.width,
                            height: d.height,
                            width_pixels: d.resolution.0,
                            height_pixels: d.resolution.1,
                            scale_factor: d.scale_factor,
                            is_primary: d.id == 0,
                        })
                        .collect()
                })
                .unwrap_or_default();
            let host_name = local_host
                .map(|h| h.name.clone())
                .unwrap_or_else(|| "unknown".into());
            let initial_layout = vec![]; // empty initial layout
            match unified_server::start_unified_server(
                state.bind_port,
                state.server_host_id,
                host_name,
                snapshots,
                initial_layout,
            ) {
                Ok(handle) => {
                    state.unified_server = Some(handle);
                    tracing::info!("Auto-started unified server on port {}", state.bind_port);
                }
                Err(e) => {
                    tracing::warn!("Failed to start unified server: {e}");
                }
            }
        }
        model::Role::Client => {
            if !state.connect_address.is_empty() {
                let raw_addr = state.connect_address.trim();
                let addr = if raw_addr.contains(':') {
                    raw_addr.to_string()
                } else {
                    format!("{raw_addr}:{}", state.bind_port)
                };
                if let Ok(target) = addr.parse::<std::net::SocketAddr>() {
                    let local_host = state.hosts.iter().find(|h| h.is_local);
                    let snapshots: Vec<kani_proto::event::DisplaySnapshot> = local_host
                        .map(|h| {
                            h.displays
                                .iter()
                                .map(|d| kani_proto::event::DisplaySnapshot {
                                    id: d.id,
                                    origin_x: d.local_origin_x,
                                    origin_y: d.local_origin_y,
                                    width: d.width,
                                    height: d.height,
                                    width_pixels: d.resolution.0,
                                    height_pixels: d.resolution.1,
                                    scale_factor: d.scale_factor,
                                    is_primary: d.id == 0,
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let client_name = local_host
                        .map(|h| h.name.clone())
                        .unwrap_or_else(|| "unknown".into());
                    match client_agent::start_client_agent(
                        target,
                        state.server_host_id,
                        client_name,
                        snapshots,
                        state.mouse_sensitivity,
                    ) {
                        Ok(handle) => {
                            state.client_agent = Some(handle);
                            state.connection = model::ConnectionState::Connecting;
                            tracing::info!(
                                address = %state.connect_address,
                                "Auto-started client agent"
                            );
                        }
                        Err(e) => {
                            tracing::warn!("Failed to connect: {e}");
                        }
                    }
                }
            }
        }
    }

    // Create tray icon (before eframe takes the event loop)
    let tray_state = tray::create_tray_icon().ok();
    if tray_state.is_none() {
        tracing::warn!("Failed to create tray icon, continuing without tray");
    }

    // Create channel for tray events (callback → update())
    let (tray_tx, tray_rx) = std::sync::mpsc::channel::<tray::TrayCommand>();
    let should_quit = Arc::new(AtomicBool::new(false));
    let should_quit_for_callback = should_quit.clone();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 500.0])
            .with_title("Kani Display Configuration")
            .with_minimize_button(false)
            .with_maximize_button(false)
            .with_icon(icon::load_window_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "kani-gui",
        options,
        Box::new(move |cc| {
            // Extract Win32 HWND from eframe's CreationContext
            #[cfg(target_os = "windows")]
            let hwnd_isize: isize = {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                match cc.window_handle().unwrap().as_raw() {
                    RawWindowHandle::Win32(h) => h.hwnd.get(),
                    _ => panic!("Expected Win32 window handle on Windows"),
                }
            };
            #[cfg(not(target_os = "windows"))]
            let hwnd_isize: isize = {
                let _ = &cc; // suppress unused warning
                0
            };

            // Set up tray event callback (must be done after HWND is available)
            if let Some(ref tray) = tray_state {
                tray::setup_tray_event_handler(
                    hwnd_isize,
                    tray_tx,
                    should_quit_for_callback,
                    tray.settings_id.clone(),
                    tray.display_id.clone(),
                    tray.toggle_id.clone(),
                    tray.quit_id.clone(),
                );
            }

            Ok(Box::new(app::KaniGuiApp {
                state,
                tray: tray_state,
                should_quit,
                tray_rx: Some(tray_rx),
                #[cfg(target_os = "windows")]
                hwnd: hwnd_isize,
            }))
        }),
    )
}
