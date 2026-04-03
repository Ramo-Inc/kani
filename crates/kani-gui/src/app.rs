use crate::model::*;
use crate::tabs;
use crate::tray;
use eframe::egui;
use kani_proto::event::*;
use kani_proto::topology::Orientation;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use crate::unified_server::ServerCommand;

pub struct KaniGuiApp {
    pub state: GuiState,
    pub tray: Option<tray::TrayState>,
    pub should_quit: Arc<AtomicBool>,
    pub tray_rx: Option<mpsc::Receiver<tray::TrayCommand>>,
    #[cfg(target_os = "windows")]
    pub hwnd: isize,
}

impl eframe::App for KaniGuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Close intercept: hide to tray instead of quitting
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.should_quit.load(Ordering::Relaxed) || self.tray.is_none() {
                // Quit requested or no tray → allow close
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                #[cfg(target_os = "windows")]
                {
                    use windows::Win32::Foundation::HWND;
                    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
                    unsafe {
                        let _ = ShowWindow(HWND(self.hwnd as *mut _), SW_HIDE);
                    }
                }
                #[cfg(not(target_os = "windows"))]
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }

        // Request repaint when async operations are active
        if self.state.unified_server.is_some()
            && matches!(
                self.state.kvm_status,
                KvmStatus::Running | KvmStatus::Starting | KvmStatus::Stopping
            )
        {
            ctx.request_repaint();
        } else if self.state.unified_server.is_some() || self.state.client_agent.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }

        // Process Host/Client state every frame
        match self.state.role {
            Role::Host => self.process_host_frame(),
            Role::Client => self.process_client_frame(),
        }

        // Process tray commands from callback handler
        if let Some(ref rx) = self.tray_rx {
            while let Ok(cmd) = rx.try_recv() {
                match cmd {
                    tray::TrayCommand::ShowSettings => {
                        self.state.active_tab = Tab::Settings;
                        #[cfg(not(target_os = "windows"))]
                        {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        }
                        #[cfg(target_os = "windows")]
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    tray::TrayCommand::ShowDisplayConfig => {
                        self.state.active_tab = Tab::DisplayConfig;
                        #[cfg(not(target_os = "windows"))]
                        {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        }
                        #[cfg(target_os = "windows")]
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    tray::TrayCommand::ToggleKvm => {
                        match self.state.role {
                            Role::Host => match self.state.kvm_status {
                                KvmStatus::Stopped | KvmStatus::Error(_) => {
                                    self.state.kvm_status = KvmStatus::Stopped;
                                    crate::app::start_kvm(&mut self.state);
                                }
                                KvmStatus::Running => {
                                    crate::app::stop_kvm(&mut self.state);
                                }
                                _ => {} // Starting/Stopping — ignore
                            },
                            Role::Client => {} // disabled in tray, but handle gracefully
                        }
                    }
                    tray::TrayCommand::Quit => {
                        self.should_quit.store(true, Ordering::Relaxed);
                        // Save if dirty before quitting
                        if self.state.dirty_since.is_some() {
                            let _ = crate::config_io::save_gui_state(&self.state);
                        }
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        // On macOS, Close may not reach the window from tray context.
                        // Force exit after a short delay to ensure quit.
                        #[cfg(target_os = "macos")]
                        {
                            std::process::exit(0);
                        }
                    }
                }
            }
        }

        // Update tray status
        if let Some(ref tray) = self.tray {
            tray::update_tray_status(
                tray,
                self.state.role,
                &self.state.kvm_status,
                self.state.client_kvm_status,
                &self.state.connection,
            );
        }

        // Global footer status bar (must be before tab_bar for egui layout ordering)
        crate::footer::draw_footer_status_bar(ctx, &self.state);

        egui::TopBottomPanel::top("tab_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(self.state.active_tab == Tab::Settings, "Settings")
                    .clicked()
                {
                    self.state.active_tab = Tab::Settings;
                }
                if ui
                    .selectable_label(
                        self.state.active_tab == Tab::DisplayConfig,
                        "Display Layout",
                    )
                    .clicked()
                {
                    self.state.active_tab = Tab::DisplayConfig;
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| match self.state.active_tab {
            Tab::Settings => tabs::settings::draw_settings_tab(ui, &mut self.state),
            Tab::DisplayConfig => tabs::display::draw_display_tab(ui, &mut self.state),
        });

        // Auto-save check (runs AFTER all state mutations)
        if let Some(since) = self.state.dirty_since {
            let elapsed = since.elapsed();
            if elapsed >= Duration::from_secs(10) {
                match crate::config_io::save_gui_state(&self.state) {
                    Ok(()) => {
                        self.state.status_message = Some("Auto-saved".into());
                        self.state.status_set_at = Some(std::time::Instant::now());
                        self.state.dirty_since = None;
                    }
                    Err(e) => {
                        self.state.status_message = Some(format!("Save error: {e}"));
                        self.state.status_set_at = Some(std::time::Instant::now());
                        // Do NOT clear dirty_since — retry next cycle
                    }
                }
            } else {
                let remaining = Duration::from_secs(10) - elapsed;
                ctx.request_repaint_after(remaining);
            }
        }

        // Auto-clear status message after 3 seconds
        if let Some(set_at) = self.state.status_set_at {
            let status_elapsed = set_at.elapsed();
            if status_elapsed >= Duration::from_secs(3) {
                self.state.status_message = None;
                self.state.status_set_at = None;
            } else {
                ctx.request_repaint_after(Duration::from_secs(3) - status_elapsed);
            }
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.state.dirty_since.is_some() {
            let _ = crate::config_io::save_gui_state(&self.state);
        }
    }
}

impl KaniGuiApp {
    /// Host mode: check for new client registrations via UnifiedServer + broadcast if needed
    fn process_host_frame(&mut self) {
        if let Some(ref server) = self.state.unified_server {
            while let Some(event) = server.try_recv_event() {
                match event {
                    crate::unified_server::ServerEvent::ClientRegistered(info) => {
                        // Upsert: if host_id exists, update displays; otherwise add new host
                        if let Some(existing) = self
                            .state
                            .hosts
                            .iter_mut()
                            .find(|h| h.host_id == info.host_id)
                        {
                            existing.displays = info
                                .displays
                                .iter()
                                .map(|s| GuiDisplay {
                                    id: s.id,
                                    local_origin_x: s.origin_x,
                                    local_origin_y: s.origin_y,
                                    width: s.width_pixels as f64,
                                    height: s.height_pixels as f64,
                                    resolution: (s.width_pixels, s.height_pixels),
                                    scale_factor: s.scale_factor,
                                    orientation: Orientation::Normal,
                                })
                                .collect();
                            existing.name = info.name;
                            existing.platform = info.platform;
                            existing.connected = true;
                        } else {
                            let next_x = next_host_offset_x(&self.state.hosts);
                            let displays = info
                                .displays
                                .iter()
                                .map(|s| GuiDisplay {
                                    id: s.id,
                                    local_origin_x: s.origin_x,
                                    local_origin_y: s.origin_y,
                                    width: s.width_pixels as f64,
                                    height: s.height_pixels as f64,
                                    resolution: (s.width_pixels, s.height_pixels),
                                    scale_factor: s.scale_factor,
                                    orientation: Orientation::Normal,
                                })
                                .collect();
                            self.state.hosts.push(GuiHost {
                                host_id: info.host_id,
                                name: info.name,
                                address: info.address.to_string(),
                                is_local: false,
                                platform: info.platform.clone(),
                                gui_offset: (next_x, 0.0),
                                displays,
                                connected: true,
                            });
                        }
                        self.state.border_links =
                            crate::border_gen::generate_border_links(&self.state.hosts);
                        self.state.needs_layout_broadcast = true;
                        self.state.dirty_since = Some(std::time::Instant::now());
                    }
                    crate::unified_server::ServerEvent::ClientDisconnected(host_id) => {
                        if let Some(host) =
                            self.state.hosts.iter_mut().find(|h| h.host_id == host_id)
                        {
                            host.connected = false;
                        }
                        tracing::info!(%host_id, "Client disconnected");
                    }
                    crate::unified_server::ServerEvent::KvmStarted => {
                        self.state.kvm_status = KvmStatus::Running;
                    }
                    crate::unified_server::ServerEvent::KvmStopped => {
                        self.state.kvm_status = KvmStatus::Stopped;
                    }
                    crate::unified_server::ServerEvent::KvmError(e) => {
                        self.state.kvm_status = KvmStatus::Error(e);
                    }
                    crate::unified_server::ServerEvent::CursorReturnedToHost(from_host) => {
                        tracing::info!(%from_host, "Cursor returned to host (peer disconnected)");
                        self.state.status_message =
                            Some("Cursor returned — peer disconnected".into());
                        self.state.status_set_at = Some(std::time::Instant::now());
                    }
                }
            }
        }

        // Check if canvas drag-stop set the broadcast flag
        if self.state.needs_layout_broadcast {
            self.broadcast_current_layout();
            self.state.needs_layout_broadcast = false;
        }
    }

    /// Client mode: check for events from ClientAgent
    fn process_client_frame(&mut self) {
        let mut should_drop_agent = false;
        if let Some(ref agent) = self.state.client_agent {
            while let Some(event) = agent.try_recv_event() {
                match event {
                    crate::client_agent::ClientEvent::LayoutReceived(host_id, hosts, links) => {
                        self.state.remote_host_id = Some(host_id);
                        // Check if layout content actually changed before marking dirty
                        let new_host_ids: Vec<_> = hosts.iter().map(|h| h.host_id).collect();
                        let old_host_ids: Vec<_> =
                            self.state.hosts.iter().map(|h| h.host_id).collect();
                        let new_link_count = links.len();
                        let old_link_count = self.state.border_links.len();
                        let was_connected =
                            matches!(self.state.connection, ConnectionState::Connected { .. });
                        let mark_dirty = should_mark_dirty_on_layout(
                            was_connected,
                            &old_host_ids,
                            &new_host_ids,
                            old_link_count,
                            new_link_count,
                        );

                        // Convert HostLayout -> GuiHost, BorderLinkLayout -> GuiBorderLink
                        self.state.hosts = hosts
                            .iter()
                            .map(|h| {
                                let is_local = h.host_id == self.state.server_host_id;
                                let displays = h
                                    .displays
                                    .iter()
                                    .map(|s| GuiDisplay {
                                        id: s.id,
                                        local_origin_x: s.origin_x,
                                        local_origin_y: s.origin_y,
                                        width: s.width_pixels as f64,
                                        height: s.height_pixels as f64,
                                        resolution: (s.width_pixels, s.height_pixels),
                                        scale_factor: s.scale_factor,
                                        orientation: Orientation::Normal,
                                    })
                                    .collect();
                                GuiHost {
                                    host_id: h.host_id,
                                    name: h.name.clone(),
                                    address: h.address.clone(),
                                    is_local,
                                    platform: h.platform.clone(),
                                    gui_offset: (h.world_offset_x, h.world_offset_y),
                                    displays,
                                    connected: true,
                                }
                            })
                            .collect();
                        self.state.border_links = links
                            .iter()
                            .map(|l| GuiBorderLink {
                                from_host: l.from_host,
                                from_display: l.from_display,
                                from_edge: l.from_edge,
                                from_range: (l.from_range[0], l.from_range[1]),
                                to_host: l.to_host,
                                to_display: l.to_display,
                                to_edge: l.to_edge,
                                to_range: (l.to_range[0], l.to_range[1]),
                                mapping: l.mapping,
                            })
                            .collect();
                        self.state.connection = ConnectionState::Connected {
                            host_addr: self.state.connect_address.clone(),
                        };
                        // Only mark dirty on initial connection or actual layout change
                        if mark_dirty {
                            self.state.dirty_since = Some(std::time::Instant::now());
                        }
                    }
                    crate::client_agent::ClientEvent::KvmStarted => {
                        self.state.client_kvm_status = ClientKvmStatus::Active;
                    }
                    crate::client_agent::ClientEvent::KvmStopped => {
                        self.state.client_kvm_status = ClientKvmStatus::Idle;
                    }
                    crate::client_agent::ClientEvent::Disconnected => {
                        self.state.client_kvm_status = ClientKvmStatus::Disconnected;
                        self.state.connection = ConnectionState::Disconnected;
                        self.state.hosts.retain(|h| h.is_local);
                        self.state.border_links.clear();
                        should_drop_agent = true;
                    }
                    crate::client_agent::ClientEvent::Error(e) => {
                        tracing::warn!("Client error: {e}");
                        self.state.status_message = Some(format!("Error: {e}"));
                        self.state.status_set_at = Some(std::time::Instant::now());
                    }
                }
            }
        }
        if should_drop_agent {
            self.state.client_agent.take();
        }
    }

    /// Build LayoutSync from current state and broadcast to all clients
    fn broadcast_current_layout(&self) {
        if let Some(ref server) = self.state.unified_server {
            let connected_ids: std::collections::HashSet<kani_proto::event::HostId> = self
                .state
                .hosts
                .iter()
                .filter(|h| h.connected)
                .map(|h| h.host_id)
                .collect();

            let hosts: Vec<HostLayout> = self
                .state
                .hosts
                .iter()
                .filter(|h| h.connected)
                .map(|h| HostLayout {
                    host_id: h.host_id,
                    name: h.name.clone(),
                    address: h.address.clone(),
                    platform: h.platform.clone(),
                    world_offset_x: h.gui_offset.0,
                    world_offset_y: h.gui_offset.1,
                    displays: h
                        .displays
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
                        .collect(),
                })
                .collect();
            let border_links: Vec<BorderLinkLayout> = self
                .state
                .border_links
                .iter()
                .filter(|bl| {
                    connected_ids.contains(&bl.from_host) && connected_ids.contains(&bl.to_host)
                })
                .map(|l| BorderLinkLayout {
                    from_host: l.from_host,
                    from_display: l.from_display,
                    from_edge: l.from_edge,
                    from_range: [l.from_range.0, l.from_range.1],
                    to_host: l.to_host,
                    to_display: l.to_display,
                    to_edge: l.to_edge,
                    to_range: [l.to_range.0, l.to_range.1],
                    mapping: l.mapping,
                })
                .collect();
            let event = InputEvent::new(
                self.state.server_host_id,
                EventType::LayoutSync {
                    hosts,
                    border_links,
                },
            );
            if let Ok(bytes) = kani_proto::codec::encode(&event) {
                server.send_command(ServerCommand::BroadcastLayout(bytes));
            }
        }
    }
}

pub fn start_kvm(state: &mut GuiState) {
    let connected_count = state.hosts.iter().filter(|h| h.connected).count();
    let connected_ids: std::collections::HashSet<kani_proto::event::HostId> = state
        .hosts
        .iter()
        .filter(|h| h.connected)
        .map(|h| h.host_id)
        .collect();
    let active_links = state
        .border_links
        .iter()
        .filter(|bl| connected_ids.contains(&bl.from_host) && connected_ids.contains(&bl.to_host))
        .count();
    if connected_count < 2 || active_links == 0 {
        state.kvm_status =
            KvmStatus::Error("Need at least 2 connected hosts and 1 border link".into());
        return;
    }

    if let Some(ref server) = state.unified_server {
        let _ = crate::config_io::save_gui_state(state);
        state.dirty_since = None;
        let config = crate::config_io::build_config(state);
        server.send_command(ServerCommand::StartKvm(config));
        state.kvm_status = KvmStatus::Starting;
    }
}

pub fn stop_kvm(state: &mut GuiState) {
    if let Some(ref server) = state.unified_server {
        server.send_command(ServerCommand::StopKvm);
        state.kvm_status = KvmStatus::Stopping;
    }
}

/// Determine whether a LayoutReceived event should mark the state as dirty.
///
/// Returns `true` if:
/// - The client was not previously connected (initial connection), OR
/// - The layout content actually changed (different host IDs or link count).
///
/// This prevents the 2-second periodic LayoutSync re-sends from resetting the
/// auto-save debounce timer, which would cause "Unsaved Changes" to persist forever.
pub fn should_mark_dirty_on_layout(
    was_connected: bool,
    old_host_ids: &[kani_proto::event::HostId],
    new_host_ids: &[kani_proto::event::HostId],
    old_link_count: usize,
    new_link_count: usize,
) -> bool {
    if !was_connected {
        return true;
    }
    new_host_ids != old_host_ids || new_link_count != old_link_count
}

#[cfg(test)]
mod tests {
    use super::*;
    use kani_proto::event::HostId;

    #[test]
    fn test_initial_connection_marks_dirty() {
        let host_a = HostId::new_v4();
        let host_b = HostId::new_v4();
        let new_ids = vec![host_a, host_b];
        let old_ids: Vec<HostId> = vec![];
        // First connection: was_connected = false → should mark dirty
        assert!(should_mark_dirty_on_layout(false, &old_ids, &new_ids, 0, 1));
    }

    #[test]
    fn test_same_layout_does_not_mark_dirty() {
        let host_a = HostId::new_v4();
        let host_b = HostId::new_v4();
        let ids = vec![host_a, host_b];
        // Already connected, same hosts, same link count → should NOT mark dirty
        assert!(!should_mark_dirty_on_layout(true, &ids, &ids, 2, 2));
    }

    #[test]
    fn test_host_added_marks_dirty() {
        let host_a = HostId::new_v4();
        let host_b = HostId::new_v4();
        let host_c = HostId::new_v4();
        let old_ids = vec![host_a, host_b];
        let new_ids = vec![host_a, host_b, host_c];
        // Already connected, but host list changed → should mark dirty
        assert!(should_mark_dirty_on_layout(true, &old_ids, &new_ids, 2, 3));
    }

    #[test]
    fn test_link_count_changed_marks_dirty() {
        let host_a = HostId::new_v4();
        let host_b = HostId::new_v4();
        let ids = vec![host_a, host_b];
        // Same hosts but link count changed → should mark dirty
        assert!(should_mark_dirty_on_layout(true, &ids, &ids, 1, 2));
    }

    #[test]
    fn test_host_removed_marks_dirty() {
        let host_a = HostId::new_v4();
        let host_b = HostId::new_v4();
        let old_ids = vec![host_a, host_b];
        let new_ids = vec![host_a];
        // Host removed → should mark dirty
        assert!(should_mark_dirty_on_layout(true, &old_ids, &new_ids, 2, 1));
    }
}
