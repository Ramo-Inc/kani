use crate::canvas;
use crate::model::*;
use eframe::egui;

pub fn draw_display_tab(ui: &mut egui::Ui, state: &mut GuiState) {
    egui::SidePanel::left("display_side_panel")
        .resizable(true)
        .default_width(220.0)
        .show_inside(ui, |ui| {
            // Fixed KVM button at bottom of sidebar
            egui::TopBottomPanel::bottom("sidebar_kvm_button").show_inside(ui, |ui| {
                ui.add_space(4.0);
                draw_kvm_control_bar(ui, state);
                ui.add_space(4.0);
            });

            // Scrollable content above
            egui::ScrollArea::vertical().show(ui, |ui| {
                // ── Devices ──
                ui.heading("Devices");
                for (i, host) in state.hosts.iter().enumerate() {
                    let selected = state.selected_host == Some(i);

                    // Status label and color
                    let (status_str, status_color) = if host.is_local {
                        ("local", egui::Color32::GRAY)
                    } else if host.connected {
                        ("online", egui::Color32::from_rgb(76, 175, 80))
                    } else {
                        ("offline", egui::Color32::from_rgb(244, 67, 54))
                    };

                    // Role label and color
                    let is_host_role = match state.role {
                        Role::Host => host.is_local,
                        Role::Client => {
                            // Only the actual Host is [Host], others are [Client]
                            state
                                .remote_host_id
                                .map_or(!host.is_local, |hid| host.host_id == hid)
                        }
                    };
                    let (role_str, role_color) = if is_host_role {
                        ("Host", egui::Color32::from_rgb(255, 152, 0))
                    } else {
                        ("Client", egui::Color32::from_rgb(144, 202, 249))
                    };

                    // Name color
                    let name_color = if host.is_local || selected {
                        egui::Color32::from_rgb(79, 195, 247)
                    } else if host.connected {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_rgb(102, 102, 102)
                    };

                    let response = ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&host.name).color(name_color));
                        ui.label(
                            egui::RichText::new(format!("({status_str})"))
                                .color(status_color)
                                .size(11.0),
                        );
                        ui.label(egui::RichText::new("-").color(egui::Color32::GRAY));
                        ui.label(
                            egui::RichText::new(format!("[{role_str}]"))
                                .color(role_color)
                                .size(11.0)
                                .strong(),
                        );
                    });

                    // Make the whole row clickable for selection
                    let row_rect = response.response.rect;
                    if ui
                        .interact(
                            row_rect,
                            egui::Id::new(("device_select", i)),
                            egui::Sense::click(),
                        )
                        .clicked()
                    {
                        state.selected_host = Some(i);
                    }

                    // Highlight selected row
                    if selected {
                        ui.painter().rect_stroke(
                            row_rect,
                            2.0,
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(79, 195, 247)),
                            egui::StrokeKind::Inside,
                        );
                    }
                }

                // Client read-only label
                if state.role == Role::Client {
                    ui.add_space(8.0);
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "Layout controlled by Host (read-only)",
                    );
                }

                // ── Border Links ──
                ui.separator();
                ui.heading("Border Links");
                if state.border_links.is_empty() {
                    if state.role == Role::Host {
                        ui.label("Drag remote host next to local displays to create links.");
                    } else {
                        ui.label("No border links configured yet.");
                    }
                }
                for link in &state.border_links {
                    let from_name = state
                        .hosts
                        .iter()
                        .find(|h| h.host_id == link.from_host)
                        .map(|h| h.name.as_str())
                        .unwrap_or("?");
                    let to_name = state
                        .hosts
                        .iter()
                        .find(|h| h.host_id == link.to_host)
                        .map(|h| h.name.as_str())
                        .unwrap_or("?");
                    ui.label(format!(
                        "{} ({:?}) \u{2192} {} ({:?})",
                        from_name, link.from_edge, to_name, link.to_edge
                    ));
                }
            });
        });

    // Canvas (must be last - CentralPanel consumes remaining space)
    egui::CentralPanel::default().show_inside(ui, |ui| {
        canvas::draw_canvas(ui, state);
    });
}

fn draw_kvm_control_bar(ui: &mut egui::Ui, state: &mut GuiState) {
    ui.horizontal(|ui| match state.role {
        Role::Host => draw_host_kvm_button(ui, state),
        Role::Client => draw_client_kvm_status(ui, state),
    });
}

fn draw_host_kvm_button(ui: &mut egui::Ui, state: &mut GuiState) {
    match state.kvm_status {
        KvmStatus::Stopped => {
            let can_start = state.hosts.len() > 1 && !state.border_links.is_empty();

            let text = egui::RichText::new("\u{25b6}  Start KVM")
                .size(18.0)
                .strong()
                .color(egui::Color32::WHITE);

            let button = egui::Button::new(text)
                .min_size(egui::vec2(200.0, 40.0))
                .fill(if can_start {
                    egui::Color32::from_rgb(34, 139, 34) // forest green
                } else {
                    egui::Color32::from_rgb(128, 128, 128) // gray when disabled
                });

            let response = ui.add_enabled(can_start, button);

            if response.clicked() {
                crate::app::start_kvm(state);
            }

            if !can_start {
                response.on_hover_text("Connect a remote host and arrange displays first.");
            }
        }
        KvmStatus::Starting => {
            ui.spinner();
            ui.label(egui::RichText::new("Starting KVM...").size(16.0).strong());
        }
        KvmStatus::Running => {
            let text = egui::RichText::new("\u{25a0}  Stop KVM")
                .size(18.0)
                .strong()
                .color(egui::Color32::WHITE);

            let button = egui::Button::new(text)
                .min_size(egui::vec2(200.0, 40.0))
                .fill(egui::Color32::from_rgb(200, 40, 40)); // red

            if ui.add(button).clicked() {
                crate::app::stop_kvm(state);
            }
        }
        KvmStatus::Stopping => {
            ui.spinner();
            ui.label(egui::RichText::new("Stopping KVM...").size(16.0).strong());
        }
        KvmStatus::Error(ref e) => {
            ui.label(
                egui::RichText::new(format!("KVM Error: {e}"))
                    .size(14.0)
                    .color(egui::Color32::RED),
            );
            if ui.button("Retry").clicked() {
                state.kvm_status = KvmStatus::Stopped;
            }
        }
    }
}

fn draw_client_kvm_status(ui: &mut egui::Ui, state: &GuiState) {
    // Client KVM status is shown in the global footer bar — nothing needed here.
    let _ = (ui, state);
}
