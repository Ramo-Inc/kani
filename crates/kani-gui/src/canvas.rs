use crate::model::*;
use eframe::egui;
use kani_proto::topology::Edge;

pub struct CanvasTransform {
    pub world_min_x: f64,
    pub world_min_y: f64,
    pub scale: f64,
    pub canvas_offset: egui::Vec2,
}

impl CanvasTransform {
    pub fn compute(hosts: &[GuiHost], canvas_rect: egui::Rect) -> Self {
        let (min_x, min_y, max_x, max_y) = Self::world_extent(hosts);
        let world_w = (max_x - min_x).max(1.0);
        let world_h = (max_y - min_y).max(1.0);
        let scale_x = canvas_rect.width() as f64 / world_w;
        let scale_y = canvas_rect.height() as f64 / world_h;
        let scale = scale_x.min(scale_y) * 0.85;
        let rendered_w = world_w * scale;
        let rendered_h = world_h * scale;
        let offset_x = canvas_rect.left() + (canvas_rect.width() - rendered_w as f32) / 2.0;
        let offset_y = canvas_rect.top() + (canvas_rect.height() - rendered_h as f32) / 2.0;
        Self {
            world_min_x: min_x,
            world_min_y: min_y,
            scale,
            canvas_offset: egui::vec2(offset_x, offset_y),
        }
    }

    fn world_extent(hosts: &[GuiHost]) -> (f64, f64, f64, f64) {
        let mut min_x = f64::MAX;
        let mut min_y = f64::MAX;
        let mut max_x = f64::MIN;
        let mut max_y = f64::MIN;
        for host in hosts {
            for d in &host.displays {
                let wx = d.world_x(host.gui_offset);
                let wy = d.world_y(host.gui_offset);
                min_x = min_x.min(wx);
                min_y = min_y.min(wy);
                max_x = max_x.max(wx + d.width);
                max_y = max_y.max(wy + d.height);
            }
        }
        if min_x > max_x {
            (0.0, 0.0, 100.0, 100.0)
        } else {
            (min_x, min_y, max_x, max_y)
        }
    }

    pub fn world_to_screen(&self, wx: f64, wy: f64) -> egui::Pos2 {
        egui::pos2(
            ((wx - self.world_min_x) * self.scale) as f32 + self.canvas_offset.x,
            ((wy - self.world_min_y) * self.scale) as f32 + self.canvas_offset.y,
        )
    }

    pub fn screen_to_world(&self, sp: egui::Pos2) -> (f64, f64) {
        (
            (sp.x - self.canvas_offset.x) as f64 / self.scale + self.world_min_x,
            (sp.y - self.canvas_offset.y) as f64 / self.scale + self.world_min_y,
        )
    }

    pub fn world_size_to_screen(&self, w: f64, h: f64) -> egui::Vec2 {
        egui::vec2((w * self.scale) as f32, (h * self.scale) as f32)
    }
}

pub fn draw_canvas(ui: &mut egui::Ui, state: &mut GuiState) {
    let (response, painter) =
        ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
    let transform = CanvasTransform::compute(&state.hosts, response.rect);

    // Draw displays
    for (hi, host) in state.hosts.iter().enumerate() {
        let is_selected = state.selected_host == Some(hi);
        for d in &host.displays {
            let top_left =
                transform.world_to_screen(d.world_x(host.gui_offset), d.world_y(host.gui_offset));
            let size = transform.world_size_to_screen(d.width, d.height);
            let rect = egui::Rect::from_min_size(top_left, size);
            let fill = if host.is_local {
                egui::Color32::from_rgb(50, 120, 200)
            } else if host.connected {
                egui::Color32::from_rgb(80, 80, 80)
            } else {
                egui::Color32::from_rgba_unmultiplied(60, 60, 60, 40)
            };
            painter.rect_filled(rect, 4.0, fill);
            if !host.connected && !host.is_local {
                // Dashed border
                let stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(100, 100, 100));
                let corners = [
                    rect.left_top(),
                    rect.right_top(),
                    rect.right_bottom(),
                    rect.left_bottom(),
                ];
                for i in 0..4 {
                    let shapes = egui::Shape::dashed_line(
                        &[corners[i], corners[(i + 1) % 4]],
                        stroke,
                        4.0,
                        2.0,
                    );
                    painter.extend(shapes);
                }
                // "Offline" label
                painter.text(
                    rect.center() + egui::vec2(0.0, -16.0),
                    egui::Align2::CENTER_CENTER,
                    "Offline",
                    egui::FontId::proportional(10.0),
                    egui::Color32::from_rgb(180, 180, 180),
                );
                // × button (only when KVM stopped and display is large enough)
                if matches!(state.kvm_status, KvmStatus::Stopped)
                    && size.x >= 48.0
                    && size.y >= 48.0
                {
                    let btn_size = 16.0;
                    let btn_rect = egui::Rect::from_min_size(
                        rect.right_top() + egui::vec2(-btn_size - 2.0, 2.0),
                        egui::vec2(btn_size, btn_size),
                    );
                    let hover = response
                        .hover_pos()
                        .map(|p| btn_rect.contains(p))
                        .unwrap_or(false);
                    let bg = if hover {
                        egui::Color32::from_rgba_unmultiplied(200, 60, 60, 180)
                    } else {
                        egui::Color32::from_rgba_unmultiplied(120, 60, 60, 120)
                    };
                    painter.rect_filled(btn_rect, 2.0, bg);
                    painter.text(
                        btn_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "\u{00d7}",
                        egui::FontId::proportional(12.0),
                        egui::Color32::WHITE,
                    );
                }
            }
            if is_selected {
                painter.rect_stroke(
                    rect,
                    4.0,
                    egui::Stroke::new(2.0, egui::Color32::GOLD),
                    egui::StrokeKind::Inside,
                );
            }
            let label = format!("{}", d.id);
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &label,
                egui::FontId::proportional(14.0),
                egui::Color32::WHITE,
            );
            let res_label = format!("{}x{}", d.resolution.0, d.resolution.1);
            painter.text(
                rect.center() + egui::vec2(0.0, 16.0),
                egui::Align2::CENTER_CENTER,
                &res_label,
                egui::FontId::proportional(10.0),
                egui::Color32::LIGHT_GRAY,
            );
        }
        let (bx, by, _, _) = host.world_bounding_box();
        let name_pos = transform.world_to_screen(bx, by) + egui::vec2(0.0, -16.0);
        painter.text(
            name_pos,
            egui::Align2::LEFT_BOTTOM,
            &host.name,
            egui::FontId::proportional(12.0),
            egui::Color32::GRAY,
        );
    }

    // Draw border links (only between connected hosts)
    for link in &state.border_links {
        let from_connected = state
            .hosts
            .iter()
            .find(|h| h.host_id == link.from_host)
            .map(|h| h.connected)
            .unwrap_or(false);
        let to_connected = state
            .hosts
            .iter()
            .find(|h| h.host_id == link.to_host)
            .map(|h| h.connected)
            .unwrap_or(false);
        if from_connected && to_connected {
            draw_border_link(&painter, &transform, link, state);
        }
    }

    // Handle interaction
    handle_interaction(&response, &transform, state);
}

fn draw_border_link(
    painter: &egui::Painter,
    transform: &CanvasTransform,
    link: &GuiBorderLink,
    state: &GuiState,
) {
    let from_host = match state.hosts.iter().find(|h| h.host_id == link.from_host) {
        Some(h) => h,
        None => return,
    };
    let fd = match from_host
        .displays
        .iter()
        .find(|d| d.id == link.from_display)
    {
        Some(d) => d,
        None => return,
    };

    let (p1, p2) = match link.from_edge {
        Edge::Right => {
            let x = fd.world_x(from_host.gui_offset) + fd.width;
            (
                transform.world_to_screen(x, link.from_range.0),
                transform.world_to_screen(x, link.from_range.1),
            )
        }
        Edge::Left => {
            let x = fd.world_x(from_host.gui_offset);
            (
                transform.world_to_screen(x, link.from_range.0),
                transform.world_to_screen(x, link.from_range.1),
            )
        }
        Edge::Bottom => {
            let y = fd.world_y(from_host.gui_offset) + fd.height;
            (
                transform.world_to_screen(link.from_range.0, y),
                transform.world_to_screen(link.from_range.1, y),
            )
        }
        Edge::Top => {
            let y = fd.world_y(from_host.gui_offset);
            (
                transform.world_to_screen(link.from_range.0, y),
                transform.world_to_screen(link.from_range.1, y),
            )
        }
    };
    painter.line_segment(
        [p1, p2],
        egui::Stroke::new(3.0, egui::Color32::from_rgba_premultiplied(0, 200, 0, 180)),
    );
}

fn handle_interaction(
    response: &egui::Response,
    transform: &CanvasTransform,
    state: &mut GuiState,
) {
    // × button click detection — must be checked BEFORE host selection
    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            for hi in 0..state.hosts.len() {
                let host = &state.hosts[hi];
                if host.is_local || host.connected {
                    continue;
                }
                if !matches!(state.kvm_status, KvmStatus::Stopped) {
                    continue;
                }
                for di in (0..host.displays.len()).rev() {
                    let d = &host.displays[di];
                    let top_left = transform
                        .world_to_screen(d.world_x(host.gui_offset), d.world_y(host.gui_offset));
                    let size = transform.world_size_to_screen(d.width, d.height);
                    if size.x < 48.0 || size.y < 48.0 {
                        continue;
                    }
                    let btn_size = 16.0;
                    let btn_rect = egui::Rect::from_min_size(
                        top_left + egui::vec2(size.x - btn_size - 2.0, 2.0),
                        egui::vec2(btn_size, btn_size),
                    );
                    if btn_rect.contains(pos) {
                        // Guard: prevent stale click on freshly reconnected host
                        if state.hosts[hi].connected {
                            return;
                        }
                        state.hosts[hi].displays.remove(di);
                        if state.hosts[hi].displays.is_empty() {
                            state.hosts.remove(hi);
                            // Fix selected_host index after removal
                            match state.selected_host {
                                Some(s) if s == hi => state.selected_host = None,
                                Some(s) if s > hi => state.selected_host = Some(s - 1),
                                _ => {}
                            }
                        }
                        state.border_links = crate::border_gen::generate_border_links(&state.hosts);
                        state.dirty_since = Some(std::time::Instant::now());
                        state.needs_layout_broadcast = true;
                        return; // early return — do not fall through to host selection
                    }
                }
            }
        }
    }

    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let (wx, wy) = transform.screen_to_world(pos);
            state.selected_host = None;
            for (i, host) in state.hosts.iter().enumerate() {
                let (bx, by, bw, bh) = host.world_bounding_box();
                if wx >= bx && wx < bx + bw && wy >= by && wy < by + bh {
                    state.selected_host = Some(i);
                    break;
                }
            }
        }
    }

    if response.drag_started() {
        // Client mode: read-only, no dragging
        if state.role == Role::Client {
            return;
        }
        // KVM running: no drag
        if matches!(
            state.kvm_status,
            KvmStatus::Running | KvmStatus::Starting | KvmStatus::Stopping
        ) {
            return;
        }
        if let Some(pos) = response.interact_pointer_pos() {
            let (wx, wy) = transform.screen_to_world(pos);
            for (i, host) in state.hosts.iter().enumerate() {
                if host.is_local {
                    continue;
                }
                let (bx, by, bw, bh) = host.world_bounding_box();
                if wx >= bx && wx < bx + bw && wy >= by && wy < by + bh {
                    state.dragging = Some(DragState { host_index: i });
                    state.selected_host = Some(i);
                    if let Some(ref server) = state.unified_server {
                        server
                            .send_command(crate::unified_server::ServerCommand::SetDragging(true));
                    }
                    break;
                }
            }
        }
    }

    if response.dragged() {
        if let Some(ref drag) = state.dragging {
            let delta = response.drag_delta();
            let world_dx = delta.x as f64 / transform.scale;
            let world_dy = delta.y as f64 / transform.scale;
            state.hosts[drag.host_index].gui_offset.0 += world_dx;
            state.hosts[drag.host_index].gui_offset.1 += world_dy;
        }
    }

    if response.drag_stopped() {
        if let Some(ref drag) = state.dragging {
            snap_to_nearest_edge(&mut state.hosts, drag.host_index);
            state.border_links = crate::border_gen::generate_border_links(&state.hosts);
            state.dragging = None;
            state.needs_layout_broadcast = true;
            state.dirty_since = Some(std::time::Instant::now());
            if let Some(ref server) = state.unified_server {
                server.send_command(crate::unified_server::ServerCommand::SetDragging(false));
            }
        }
    }
}

fn snap_to_nearest_edge(hosts: &mut [GuiHost], dragged_index: usize) {
    let snap_threshold = 200.0;
    let dragged = &hosts[dragged_index];
    let (dx, dy, dw, dh) = dragged.world_bounding_box();
    // local_offset = bounding_box_origin - gui_offset (accounts for non-zero local_origin in displays)
    let local_offset_x = dx - dragged.gui_offset.0;
    let local_offset_y = dy - dragged.gui_offset.1;

    let mut best_snap_x: Option<f64> = None;
    let mut best_snap_y: Option<f64> = None;
    let mut best_dist_x = snap_threshold;
    let mut best_dist_y = snap_threshold;

    for (i, other) in hosts.iter().enumerate() {
        if i == dragged_index {
            continue;
        }
        let (ox, oy, ow, oh) = other.world_bounding_box();

        // Snap dragged right edge to other left edge
        let dist = ((dx + dw) - ox).abs();
        if dist < best_dist_x {
            best_dist_x = dist;
            best_snap_x = Some(ox - dw - local_offset_x);
        }
        // Snap dragged left edge to other right edge
        let dist = (dx - (ox + ow)).abs();
        if dist < best_dist_x {
            best_dist_x = dist;
            best_snap_x = Some(ox + ow - local_offset_x);
        }

        // Snap dragged bottom edge to other top edge
        let dist = ((dy + dh) - oy).abs();
        if dist < best_dist_y {
            best_dist_y = dist;
            best_snap_y = Some(oy - dh - local_offset_y);
        }
        // Snap dragged top edge to other bottom edge
        let dist = (dy - (oy + oh)).abs();
        if dist < best_dist_y {
            best_dist_y = dist;
            best_snap_y = Some(oy + oh - local_offset_y);
        }
    }

    if let Some(sx) = best_snap_x {
        hosts[dragged_index].gui_offset.0 = sx;
    }
    if let Some(sy) = best_snap_y {
        hosts[dragged_index].gui_offset.1 = sy;
    }
}
