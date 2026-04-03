use kani_proto::event::DisplayId;
use kani_proto::topology::{Edge, HostTopology, Rect};

#[derive(Debug, Clone)]
pub struct VirtualCursor {
    pub x: f64,
    pub y: f64,
    pub current_display: DisplayId,
}

#[derive(Debug, PartialEq)]
pub enum MoveResult {
    InBounds,
    ExitedEdge {
        display_id: DisplayId,
        edge: Edge,
        /// Position along the edge in absolute logical coordinates
        along_edge: f64,
        /// How far past the edge the cursor went
        overshoot: f64,
    },
}

impl VirtualCursor {
    pub fn new(x: f64, y: f64, display_id: DisplayId) -> Self {
        Self {
            x,
            y,
            current_display: display_id,
        }
    }

    /// Apply a mouse delta and return whether the cursor stayed in bounds or exited an edge.
    /// Uses per-display edge detection: checks the current display's rect, handles local
    /// display transitions, and reports exits from the current display (not the bounding box).
    pub fn apply_delta(&mut self, dx: f64, dy: f64, topology: &HostTopology) -> MoveResult {
        let prev_x = self.x;
        let prev_y = self.y;
        self.x += dx;
        self.y += dy;

        // Get current display rect (with recovery if invalid)
        let rect = match Self::display_rect(topology, self.current_display) {
            Some(r) => r,
            None => {
                // current_display is invalid — recover
                if let Some((id, _)) = Self::find_display_at(topology, self.x, self.y) {
                    self.current_display = id;
                    return MoveResult::InBounds;
                }
                if let Some(d) = topology.displays.first() {
                    self.current_display = d.id;
                    let r = d.logical_rect();
                    self.x = r.x + r.w / 2.0;
                    self.y = r.y + r.h / 2.0;
                }
                return MoveResult::InBounds;
            }
        };

        // Still in current display?
        if rect.contains(self.x, self.y) {
            return MoveResult::InBounds;
        }

        // Entered another local display?
        for d in &topology.displays {
            if d.id != self.current_display {
                let r = d.logical_rect();
                if r.contains(self.x, self.y) {
                    self.current_display = d.id;
                    return MoveResult::InBounds;
                }
            }
        }

        // Outside all displays — edge exit from current display
        let result = self.detect_display_exit(prev_x, prev_y, dx, dy, &rect);
        self.x = self.x.clamp(rect.x, rect.x + rect.w - 0.01);
        self.y = self.y.clamp(rect.y, rect.y + rect.h - 0.01);
        result
    }

    /// Clamp the cursor position to the current display's rect.
    pub fn clamp_to(&mut self, topology: &HostTopology) {
        if let Some(rect) = Self::display_rect(topology, self.current_display) {
            self.x = self.x.clamp(rect.x, rect.x + rect.w - 0.01);
            self.y = self.y.clamp(rect.y, rect.y + rect.h - 0.01);
        }
    }

    /// Detect which edge of the given display rect the cursor exited through,
    /// using parametric ray intersection from the previous position.
    fn detect_display_exit(
        &self,
        prev_x: f64,
        prev_y: f64,
        dx: f64,
        dy: f64,
        rect: &Rect,
    ) -> MoveResult {
        let exited_right = self.x >= rect.x + rect.w;
        let exited_left = self.x < rect.x;
        let exited_bottom = self.y >= rect.y + rect.h;
        let exited_top = self.y < rect.y;

        if !exited_right && !exited_left && !exited_bottom && !exited_top {
            return MoveResult::InBounds;
        }

        let t_horiz = if exited_right {
            if dx > 0.0 {
                ((rect.x + rect.w) - prev_x) / dx
            } else {
                f64::MAX
            }
        } else if exited_left {
            if dx < 0.0 {
                (rect.x - prev_x) / dx
            } else {
                f64::MAX
            }
        } else {
            f64::MAX
        };

        let t_vert = if exited_bottom {
            if dy > 0.0 {
                ((rect.y + rect.h) - prev_y) / dy
            } else {
                f64::MAX
            }
        } else if exited_top {
            if dy < 0.0 {
                (rect.y - prev_y) / dy
            } else {
                f64::MAX
            }
        } else {
            f64::MAX
        };

        // Horizontal wins on tie (spec: prefer horizontal)
        let (edge, along, overshoot) = if t_horiz <= t_vert {
            if exited_right {
                let crossing_y = prev_y + dy * t_horiz;
                (Edge::Right, crossing_y, self.x - (rect.x + rect.w))
            } else {
                let crossing_y = prev_y + dy * t_horiz;
                (Edge::Left, crossing_y, rect.x - self.x)
            }
        } else if exited_bottom {
            let crossing_x = prev_x + dx * t_vert;
            (Edge::Bottom, crossing_x, self.y - (rect.y + rect.h))
        } else {
            let crossing_x = prev_x + dx * t_vert;
            (Edge::Top, crossing_x, rect.y - self.y)
        };

        MoveResult::ExitedEdge {
            display_id: self.current_display,
            edge,
            along_edge: along,
            overshoot,
        }
    }

    /// Look up a display's logical rect by ID.
    fn display_rect(topology: &HostTopology, display_id: DisplayId) -> Option<Rect> {
        topology
            .displays
            .iter()
            .find(|d| d.id == display_id)
            .map(|d| d.logical_rect())
    }

    /// Find which display contains the given point.
    fn find_display_at(topology: &HostTopology, x: f64, y: f64) -> Option<(DisplayId, Rect)> {
        topology
            .displays
            .iter()
            .find(|d| d.logical_rect().contains(x, y))
            .map(|d| (d.id, d.logical_rect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kani_proto::topology::{Display, HostTopology, Orientation};
    use uuid::Uuid;

    fn make_display(id: DisplayId, origin: (i32, i32), res: (u32, u32), scale: f64) -> Display {
        Display {
            id,
            origin,
            resolution: res,
            scale_factor: scale,
            logical_size: (res.0 as f64 / scale, res.1 as f64 / scale),
            orientation: Orientation::Normal,
        }
    }

    fn single_display_topology() -> HostTopology {
        HostTopology::new(
            Uuid::new_v4(),
            vec![make_display(1, (0, 0), (1920, 1080), 1.0)],
        )
    }

    fn dual_display_topology() -> HostTopology {
        HostTopology::new(
            Uuid::new_v4(),
            vec![
                make_display(1, (0, 0), (1920, 1080), 1.0),
                make_display(2, (1920, 0), (1920, 1080), 1.0),
            ],
        )
    }

    #[test]
    fn test_move_within_bounds() {
        let topo = single_display_topology();
        let mut cursor = VirtualCursor::new(960.0, 540.0, 1);
        let result = cursor.apply_delta(10.0, 5.0, &topo);
        assert_eq!(result, MoveResult::InBounds);
        assert!((cursor.x - 970.0).abs() < 0.01);
        assert!((cursor.y - 545.0).abs() < 0.01);
    }

    #[test]
    fn test_exit_right_edge() {
        let topo = single_display_topology();
        let mut cursor = VirtualCursor::new(1910.0, 540.0, 1);
        let result = cursor.apply_delta(20.0, 0.0, &topo);
        match result {
            MoveResult::ExitedEdge {
                display_id,
                edge,
                along_edge,
                overshoot,
            } => {
                assert_eq!(display_id, 1);
                assert_eq!(edge, Edge::Right);
                assert!((along_edge - 540.0).abs() < 0.01);
                assert!((overshoot - 10.0).abs() < 0.01);
            }
            _ => panic!("Expected ExitedEdge"),
        }
    }

    #[test]
    fn test_exit_left_edge() {
        let topo = single_display_topology();
        let mut cursor = VirtualCursor::new(5.0, 540.0, 1);
        let result = cursor.apply_delta(-10.0, 0.0, &topo);
        match result {
            MoveResult::ExitedEdge {
                display_id,
                edge,
                along_edge,
                overshoot,
            } => {
                assert_eq!(display_id, 1);
                assert_eq!(edge, Edge::Left);
                assert!((along_edge - 540.0).abs() < 0.01);
                assert!((overshoot - 5.0).abs() < 0.01);
            }
            _ => panic!("Expected ExitedEdge"),
        }
    }

    #[test]
    fn test_diagonal_exit_prefers_horizontal() {
        // Cursor near top-right corner, moving diagonally out both right and top simultaneously
        let topo = single_display_topology();
        // Place cursor so it exits both right and top at the same parametric time
        // bb: (0,0)-(1920,1080). Put cursor at (1919, 1), move (2, -2)
        // t_right = (1920 - 1919) / 2 = 0.5
        // t_top = (0 - 1) / -2 = 0.5
        // On tie, horizontal wins
        let mut cursor = VirtualCursor::new(1919.0, 1.0, 1);
        let result = cursor.apply_delta(2.0, -2.0, &topo);
        match result {
            MoveResult::ExitedEdge { edge, .. } => {
                assert_eq!(edge, Edge::Right, "Horizontal should win on tie");
            }
            _ => panic!("Expected ExitedEdge"),
        }
    }

    #[test]
    fn test_clamp() {
        let topo = single_display_topology();
        let mut cursor = VirtualCursor::new(2000.0, 2000.0, 1);
        cursor.clamp_to(&topo);
        assert!(cursor.x <= 1920.0 - 0.01);
        assert!(cursor.y <= 1080.0 - 0.01);
        assert!(cursor.x >= 0.0);
        assert!(cursor.y >= 0.0);
    }

    #[test]
    fn test_exit_with_dual_display() {
        let topo = dual_display_topology();
        // bb is (0,0)-(3840,1080)
        // Place cursor on second display near right edge
        let mut cursor = VirtualCursor::new(3830.0, 540.0, 2);
        let result = cursor.apply_delta(20.0, 0.0, &topo);
        match result {
            MoveResult::ExitedEdge {
                display_id,
                edge,
                along_edge,
                overshoot,
            } => {
                assert_eq!(display_id, 2);
                assert_eq!(edge, Edge::Right);
                assert!((along_edge - 540.0).abs() < 0.01);
                assert!((overshoot - 10.0).abs() < 0.01);
            }
            _ => panic!("Expected ExitedEdge"),
        }
    }

    /// 4-display setup matching the user's actual config.
    /// Border link is on display 0's Left edge (x=0), but bounding box
    /// left is at x=-1556 (display 3). Per-display detection should
    /// return display_id=0 when exiting display 0's left edge.
    #[test]
    fn test_exit_inner_display_edge() {
        let topo = HostTopology::new(
            Uuid::new_v4(),
            vec![
                make_display(0, (0, 0), (3440, 1440), 1.0),
                make_display(1, (2284, 1440), (1920, 1080), 1.0),
                make_display(2, (364, 1440), (1920, 1080), 1.0),
                make_display(3, (-1556, 1440), (1920, 1080), 1.0),
            ],
        );

        // Cursor on display 0, near left edge
        let mut cursor = VirtualCursor::new(50.0, 700.0, 0);
        let result = cursor.apply_delta(-100.0, 0.0, &topo);

        match result {
            MoveResult::ExitedEdge {
                display_id, edge, ..
            } => {
                assert_eq!(display_id, 0, "Should exit from display 0, not display 3");
                assert_eq!(edge, Edge::Left);
            }
            MoveResult::InBounds => panic!("Should have exited display 0's left edge"),
        }

        // Cursor should be clamped to display 0's left edge (x=0), not BB left (x=-1556)
        assert!(
            (cursor.x - 0.0).abs() < 0.01,
            "cursor.x={}, expected 0.0",
            cursor.x
        );
    }

    #[test]
    fn test_local_display_transition() {
        let topo = HostTopology::new(
            Uuid::new_v4(),
            vec![
                make_display(0, (0, 0), (3440, 1440), 1.0),
                make_display(2, (364, 1440), (1920, 1080), 1.0),
            ],
        );

        // Cursor on display 0, move down into display 2
        let mut cursor = VirtualCursor::new(500.0, 1430.0, 0);
        let result = cursor.apply_delta(0.0, 20.0, &topo);

        assert_eq!(result, MoveResult::InBounds);
        assert_eq!(
            cursor.current_display, 2,
            "Should have transitioned to display 2"
        );
        assert!((cursor.y - 1450.0).abs() < 0.01);
    }

    #[test]
    fn test_display_rect_recovery() {
        let topo = HostTopology::new(
            Uuid::new_v4(),
            vec![make_display(0, (0, 0), (1920, 1080), 1.0)],
        );
        // current_display = 99 (non-existent ID) — should be recovered
        let mut cursor = VirtualCursor::new(960.0, 540.0, 99);
        let result = cursor.apply_delta(10.0, 0.0, &topo);
        assert_eq!(result, MoveResult::InBounds);
        assert_eq!(cursor.current_display, 0, "Should recover to display 0");
    }

    #[test]
    fn test_clamp_to_current_display_not_bounding_box() {
        let topo = HostTopology::new(
            Uuid::new_v4(),
            vec![
                make_display(0, (0, 0), (1920, 1080), 1.0),
                make_display(1, (1920, 0), (1920, 1080), 1.0),
            ],
        );
        // Cursor on display 0, but positioned in display 1's area
        let mut cursor = VirtualCursor::new(2500.0, 540.0, 0);
        cursor.clamp_to(&topo);
        // Should clamp to display 0 (x < 1920), NOT bounding box (x < 3840)
        assert!(
            cursor.x < 1920.0,
            "cursor.x={}, should be < 1920 (display 0)",
            cursor.x
        );
    }
}
