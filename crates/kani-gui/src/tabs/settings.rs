use crate::model::*;
use eframe::egui;
use kani_proto::event::DisplaySnapshot;

pub fn draw_settings_tab(ui: &mut egui::Ui, state: &mut GuiState) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        let kvm_active = matches!(
            state.kvm_status,
            KvmStatus::Running | KvmStatus::Starting | KvmStatus::Stopping
        );
        if kvm_active {
            ui.colored_label(egui::Color32::YELLOW, "Stop KVM to change settings.");
            ui.add_space(8.0);
        }

        ui.heading("Host Settings");
        ui.separator();

        ui.add_enabled_ui(!kvm_active, |ui| {
            ui.horizontal(|ui| {
                ui.label("Host Name:");
                if let Some(local) = state.hosts.iter_mut().find(|h| h.is_local) {
                    if ui.text_edit_singleline(&mut local.name).changed() {
                        state.dirty_since = Some(std::time::Instant::now());
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Port (default: 24900):");
                let mut port_str = state.bind_port.to_string();
                if ui.text_edit_singleline(&mut port_str).changed() {
                    if let Ok(p) = port_str.parse::<u16>() {
                        state.bind_port = p;
                        state.dirty_since = Some(std::time::Instant::now());
                    }
                }
            });

            ui.add_space(16.0);
            ui.heading("Role");
            ui.separator();

            let prev_role = state.role;
            ui.horizontal(|ui| {
                ui.radio_value(&mut state.role, Role::Host, "Host");
                ui.radio_value(&mut state.role, Role::Client, "Client");
            });

            // Role switch reset
            if state.role != prev_role {
                match (prev_role, state.role) {
                    (Role::Host, Role::Client) => {
                        // Drop the unified server (cancel + join)
                        state.unified_server.take();
                        state.kvm_status = KvmStatus::Stopped;
                        state.hosts.retain(|h| h.is_local);
                        state.border_links.clear();
                        state.selected_host = None;
                        state.dragging = None;
                        state.dirty_since = Some(std::time::Instant::now());
                    }
                    (Role::Client, Role::Host) => {
                        // Drop the client agent (cancel + join)
                        state.client_agent.take();
                        state.connection = ConnectionState::Disconnected;
                        state.client_kvm_status = ClientKvmStatus::Idle;
                        state.kvm_status = KvmStatus::Stopped;
                        state.hosts.retain(|h| h.is_local);
                        state.border_links.clear();
                        state.selected_host = None;
                        state.dragging = None;
                        state.dirty_since = Some(std::time::Instant::now());
                    }
                    _ => {}
                }
            }

            ui.add_space(4.0);
            match state.role {
                Role::Host => {
                    ui.label(
                        "Host: Main machine with keyboard & mouse. Controls the display layout.",
                    );
                }
                Role::Client => {
                    ui.label("Client: Controlled by Host. Display layout is received (read-only).");
                }
            }

            ui.add_space(8.0);
            match state.role {
                Role::Host => draw_host_mode(ui, state),
                Role::Client => draw_client_mode(ui, state),
            }
        });

        ui.add_space(16.0);
        ui.heading("Remote Hosts");
        ui.separator();

        let mut to_remove = None;
        for (i, host) in state.hosts.iter().enumerate() {
            if host.is_local {
                continue;
            }
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} — {} ({} displays)",
                    host.name,
                    host.address,
                    host.displays.len()
                ));
                if state.role == Role::Host && ui.button("Remove").clicked() {
                    to_remove = Some(i);
                }
            });
        }
        if let Some(idx) = to_remove {
            state.hosts.remove(idx);
            state.border_links = crate::border_gen::generate_border_links(&state.hosts);
            state.selected_host = None;
            state.dragging = None;
            state.needs_layout_broadcast = true;
            state.dirty_since = Some(std::time::Instant::now());
        }
    });
}

fn draw_host_mode(ui: &mut egui::Ui, state: &mut GuiState) {
    let is_listening = state.unified_server.is_some();

    if is_listening {
        ui.horizontal(|ui| {
            ui.colored_label(
                egui::Color32::from_rgb(0, 200, 0),
                format!("Listening on port {}", state.bind_port),
            );
            if ui.button("Stop Listening").clicked() {
                state.unified_server.take();
            }
        });
    } else if ui.button("Start Listening").clicked() {
        let local_host = state.hosts.iter().find(|h| h.is_local);
        let snapshots: Vec<DisplaySnapshot> = local_host
            .map(|h| {
                h.displays
                    .iter()
                    .map(|d| DisplaySnapshot {
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

        match crate::unified_server::start_unified_server(
            state.bind_port,
            state.server_host_id,
            host_name,
            snapshots,
            vec![],
        ) {
            Ok(handle) => {
                state.unified_server = Some(handle);
                state.status_message = None;
            }
            Err(e) => {
                state.status_message = Some(format!("Failed to start server: {e}"));
            }
        }
    }
}

fn draw_client_mode(ui: &mut egui::Ui, state: &mut GuiState) {
    ui.horizontal(|ui| {
        ui.label("Host IP Address:");
        if ui
            .text_edit_singleline(&mut state.connect_address)
            .changed()
        {
            state.dirty_since = Some(std::time::Instant::now());
        }
    });

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label("Mouse Sensitivity:");
        if ui
            .add(egui::Slider::new(&mut state.mouse_sensitivity, 0.5..=2.0).step_by(0.1))
            .changed()
        {
            state.dirty_since = Some(std::time::Instant::now());
            if let Some(ref agent) = state.client_agent {
                agent.set_mouse_sensitivity(state.mouse_sensitivity);
            }
        }
    });

    let is_connected = matches!(state.connection, ConnectionState::Connected { .. })
        || matches!(state.connection, ConnectionState::Connecting);

    if is_connected {
        ui.horizontal(|ui| {
            match &state.connection {
                ConnectionState::Connecting => {
                    ui.spinner();
                    ui.label("Connecting...");
                }
                ConnectionState::Connected { host_addr } => {
                    ui.colored_label(
                        egui::Color32::from_rgb(0, 200, 0),
                        format!("Connected to {host_addr}"),
                    );
                }
                _ => {}
            }
            if ui.button("Disconnect").clicked() {
                // Drop cancels the agent
                state.client_agent.take();
                state.connection = ConnectionState::Disconnected;
                state.client_kvm_status = ClientKvmStatus::Idle;
                state.hosts.retain(|h| h.is_local);
                state.border_links.clear();
            }
        });
    } else {
        let can_connect = !state.connect_address.trim().is_empty();

        ui.add_enabled_ui(can_connect, |ui| {
            if ui.button("Connect").clicked() {
                let raw_addr = state.connect_address.trim();
                let addr = if raw_addr.contains(':') {
                    raw_addr.to_string()
                } else {
                    format!("{raw_addr}:{}", state.bind_port)
                };
                if let Ok(target) = addr.parse::<std::net::SocketAddr>() {
                    let local_host = state.hosts.iter().find(|h| h.is_local);
                    let snapshots: Vec<DisplaySnapshot> = local_host
                        .map(|h| {
                            h.displays
                                .iter()
                                .map(|d| DisplaySnapshot {
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
                    match crate::client_agent::start_client_agent(
                        target,
                        state.server_host_id,
                        client_name,
                        snapshots,
                        state.mouse_sensitivity,
                    ) {
                        Ok(handle) => {
                            state.client_agent = Some(handle);
                            state.connection = ConnectionState::Connecting;
                        }
                        Err(e) => {
                            state.connection =
                                ConnectionState::Error(format!("Connection failed: {e}"));
                        }
                    }
                } else {
                    state.connection = ConnectionState::Error("Invalid address format".into());
                }
            }
        });

        if let ConnectionState::Error(ref err) = state.connection {
            ui.colored_label(egui::Color32::RED, err);
        }
    }
}
