use crate::model::*;
use eframe::egui;

/// Draw the global footer status bar at the bottom of the window.
/// Must be called before other panels in `update()` so egui allocates space from the bottom first.
pub fn draw_footer_status_bar(ctx: &egui::Context, state: &GuiState) {
    let (dot_color, status_text, guidance) = compute_status(state);

    egui::TopBottomPanel::bottom("global_footer")
        .exact_height(28.0)
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                // Status dot
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, dot_color);

                ui.add_space(4.0);

                // Status text
                ui.label(
                    egui::RichText::new(&status_text)
                        .size(12.0)
                        .color(status_text_color(dot_color)),
                );

                if let Some(guide) = guidance {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("|")
                            .size(12.0)
                            .color(egui::Color32::from_rgb(85, 85, 85)),
                    );
                    ui.add_space(8.0);
                    // Guidance with arrow
                    ui.label(
                        egui::RichText::new(format!("\u{279c} {guide}"))
                            .size(12.0)
                            .color(egui::Color32::from_rgb(255, 213, 79)), // yellow
                    );
                }
            });
        });
}

const GREEN: egui::Color32 = egui::Color32::from_rgb(76, 175, 80);
const ORANGE: egui::Color32 = egui::Color32::from_rgb(255, 152, 0);
const RED: egui::Color32 = egui::Color32::from_rgb(244, 67, 54);

fn status_text_color(dot: egui::Color32) -> egui::Color32 {
    if dot == GREEN {
        GREEN
    } else {
        egui::Color32::from_rgb(204, 204, 204) // light gray
    }
}

fn compute_status(state: &GuiState) -> (egui::Color32, String, Option<String>) {
    match state.role {
        Role::Host => compute_host_status(state),
        Role::Client => compute_client_status(state),
    }
}

fn compute_host_status(state: &GuiState) -> (egui::Color32, String, Option<String>) {
    // KVM status takes priority when active
    match &state.kvm_status {
        KvmStatus::Starting => {
            return (ORANGE, "Starting KVM...".into(), None);
        }
        KvmStatus::Running => {
            return (
                GREEN,
                "KVM Active".into(),
                Some("Move mouse to screen edge to switch \u{2014} Ctrl+Alt+Esc to return".into()),
            );
        }
        KvmStatus::Stopping => {
            return (ORANGE, "Stopping KVM...".into(), None);
        }
        KvmStatus::Error(msg) => {
            return (
                RED,
                format!("KVM Error: {msg}"),
                Some("Check settings and click Start KVM to retry".into()),
            );
        }
        KvmStatus::Stopped => {} // fall through to server/client checks
    }

    // Server not running
    if state.unified_server.is_none() {
        return (
            RED,
            "Not listening".into(),
            Some("Go to Settings and click \"Start Listening\"".into()),
        );
    }

    // Server running — check connected client count
    let client_count = state
        .hosts
        .iter()
        .filter(|h| !h.is_local && h.connected)
        .count();

    if client_count == 0 {
        (
            ORANGE,
            format!("Listening on port {}", state.bind_port),
            Some("Waiting for client to connect...".into()),
        )
    } else {
        let plural = if client_count == 1 { "" } else { "s" };
        (
            GREEN,
            format!("Ready \u{2014} {client_count} client{plural} connected"),
            Some("Press Start KVM to begin sharing".into()),
        )
    }
}

fn compute_client_status(state: &GuiState) -> (egui::Color32, String, Option<String>) {
    match &state.connection {
        ConnectionState::Disconnected => {
            if state.client_kvm_status == ClientKvmStatus::Disconnected {
                (
                    RED,
                    "Lost connection to host".into(),
                    Some("Go to Settings and click \"Connect\" to reconnect".into()),
                )
            } else {
                (
                    RED,
                    "Disconnected".into(),
                    Some("Go to Settings and click \"Connect\"".into()),
                )
            }
        }
        ConnectionState::Connecting => (ORANGE, "Connecting...".into(), None),
        ConnectionState::Connected { host_addr } => match state.client_kvm_status {
            ClientKvmStatus::Active => (
                GREEN,
                "KVM Active".into(),
                Some("Host is controlling this machine".into()),
            ),
            _ => (
                GREEN,
                format!("Connected to {host_addr}"),
                Some("Waiting for Host to start KVM...".into()),
            ),
        },
        ConnectionState::Error(msg) => (
            RED,
            msg.clone(),
            Some("Check host address and retry".into()),
        ),
    }
}
