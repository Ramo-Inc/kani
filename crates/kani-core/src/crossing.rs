use kani_proto::event::{DisplayId, HostId};
use kani_proto::topology::{BorderLink, CoordinateMapping, Edge, EdgeSegment};

pub struct BorderLinkResolver {
    links: Vec<BorderLink>,
}

#[derive(Debug, PartialEq)]
pub struct CrossingTarget {
    pub target_host: HostId,
    pub target_display: DisplayId,
    pub landing_x: f64,
    pub landing_y: f64,
}

impl BorderLinkResolver {
    pub fn new(links: Vec<BorderLink>) -> Self {
        Self { links }
    }

    pub fn resolve(
        &self,
        from_host: HostId,
        from_display: DisplayId,
        edge: Edge,
        along_edge: f64,
    ) -> Option<CrossingTarget> {
        for link in &self.links {
            if link.from.host_id == from_host
                && link.from.display_id == from_display
                && link.from.edge == edge
                && along_edge >= link.from.range_start
                && along_edge < link.from.range_end
            {
                let landing_along = map_coordinate(
                    along_edge,
                    link.from.range_start,
                    link.from.range_end,
                    link.to.range_start,
                    link.to.range_end,
                    link.mapping,
                );
                let (lx, ly) = edge_to_coordinates(&link.to, landing_along);
                return Some(CrossingTarget {
                    target_host: link.to.host_id,
                    target_display: link.to.display_id,
                    landing_x: lx,
                    landing_y: ly,
                });
            }
        }
        None
    }
}

pub fn map_coordinate(
    value: f64,
    from_start: f64,
    from_end: f64,
    to_start: f64,
    to_end: f64,
    mapping: CoordinateMapping,
) -> f64 {
    match mapping {
        CoordinateMapping::Linear => {
            let ratio = (value - from_start) / (from_end - from_start);
            to_start + ratio * (to_end - to_start)
        }
        CoordinateMapping::OneToOneClamp => {
            let offset = value - from_start;
            (to_start + offset).clamp(to_start, to_end - 0.01)
        }
    }
}

/// Convert along-edge position to (x, y) using edge_coord for the fixed axis.
/// Offsets landing coordinates 1px inward from the edge to prevent immediate
/// bounce-back (Rect::contains uses exclusive upper bound).
fn edge_to_coordinates(segment: &EdgeSegment, along: f64) -> (f64, f64) {
    let coord = match segment.edge {
        Edge::Left | Edge::Top => segment.edge_coord + 1.0,
        Edge::Right | Edge::Bottom => segment.edge_coord - 1.0,
    };
    match segment.edge {
        Edge::Left | Edge::Right => (coord, along),
        Edge::Top | Edge::Bottom => (along, coord),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cursor::{MoveResult, VirtualCursor};
    use kani_proto::topology::{Display, HostTopology, Orientation};
    use uuid::Uuid;

    fn host_a() -> HostId {
        Uuid::from_bytes([0xAA; 16])
    }

    fn host_b() -> HostId {
        Uuid::from_bytes([0xBB; 16])
    }

    #[allow(clippy::too_many_arguments)]
    fn make_link(
        from_host: HostId,
        from_display: DisplayId,
        from_edge: Edge,
        from_range: (f64, f64),
        from_edge_coord: f64,
        to_host: HostId,
        to_display: DisplayId,
        to_edge: Edge,
        to_range: (f64, f64),
        to_edge_coord: f64,
        mapping: CoordinateMapping,
    ) -> BorderLink {
        BorderLink {
            from: EdgeSegment {
                host_id: from_host,
                display_id: from_display,
                edge: from_edge,
                range_start: from_range.0,
                range_end: from_range.1,
                edge_coord: from_edge_coord,
            },
            to: EdgeSegment {
                host_id: to_host,
                display_id: to_display,
                edge: to_edge,
                range_start: to_range.0,
                range_end: to_range.1,
                edge_coord: to_edge_coord,
            },
            mapping,
        }
    }

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

    #[test]
    fn test_resolve_right_to_left() {
        let link = make_link(
            host_a(),
            1,
            Edge::Right,
            (0.0, 1080.0),
            1920.0,
            host_b(),
            1,
            Edge::Left,
            (0.0, 1080.0),
            0.0,
            CoordinateMapping::Linear,
        );
        let resolver = BorderLinkResolver::new(vec![link]);
        let result = resolver.resolve(host_a(), 1, Edge::Right, 540.0);
        assert!(result.is_some());
        let target = result.unwrap();
        assert_eq!(target.target_host, host_b());
        assert_eq!(target.target_display, 1);
        assert!((target.landing_x - 1.0).abs() < 0.01); // was 0.0, now 1px inward
        assert!((target.landing_y - 540.0).abs() < 0.01);
    }

    #[test]
    fn test_linear_mapping_different_heights() {
        // Source is 1080 tall, target is 1440 tall
        let link = make_link(
            host_a(),
            1,
            Edge::Right,
            (0.0, 1080.0),
            1920.0,
            host_b(),
            1,
            Edge::Left,
            (0.0, 1440.0),
            0.0,
            CoordinateMapping::Linear,
        );
        let resolver = BorderLinkResolver::new(vec![link]);
        // Middle of source (540) should map to middle of target (720)
        let result = resolver.resolve(host_a(), 1, Edge::Right, 540.0).unwrap();
        assert!((result.landing_y - 720.0).abs() < 0.01);
    }

    #[test]
    fn test_one_to_one_clamp() {
        // Source 1440 tall, target 1080 tall. OneToOneClamp.
        let link = make_link(
            host_a(),
            1,
            Edge::Right,
            (0.0, 1440.0),
            2560.0,
            host_b(),
            1,
            Edge::Left,
            (0.0, 1080.0),
            0.0,
            CoordinateMapping::OneToOneClamp,
        );
        let resolver = BorderLinkResolver::new(vec![link]);
        // Position at 1200 in source (beyond target range) should clamp
        let result = resolver.resolve(host_a(), 1, Edge::Right, 1200.0).unwrap();
        assert!(result.landing_y <= 1080.0 - 0.01);
        assert!(result.landing_y >= 0.0);
        assert!(
            (result.landing_x - 1.0).abs() < 0.01,
            "landing_x={}, expected 1.0",
            result.landing_x
        );
    }

    #[test]
    fn test_landing_with_non_zero_origin() {
        // Target display at (1920, 0), left edge coord = 1920
        let link = make_link(
            host_a(),
            1,
            Edge::Right,
            (0.0, 1080.0),
            1920.0,
            host_b(),
            1,
            Edge::Left,
            (0.0, 1080.0),
            1920.0,
            CoordinateMapping::Linear,
        );
        let resolver = BorderLinkResolver::new(vec![link]);
        let result = resolver.resolve(host_a(), 1, Edge::Right, 540.0).unwrap();
        assert!((result.landing_x - 1921.0).abs() < 0.01); // was 1920.0, now 1px inward
        assert!((result.landing_y - 540.0).abs() < 0.01);
    }

    #[test]
    fn test_no_match_outside_range() {
        let link = make_link(
            host_a(),
            1,
            Edge::Right,
            (100.0, 500.0),
            1920.0,
            host_b(),
            1,
            Edge::Left,
            (0.0, 400.0),
            0.0,
            CoordinateMapping::Linear,
        );
        let resolver = BorderLinkResolver::new(vec![link]);
        // along_edge = 50 is outside [100, 500)
        assert!(resolver.resolve(host_a(), 1, Edge::Right, 50.0).is_none());
        // along_edge = 500 is at exclusive upper bound
        assert!(resolver.resolve(host_a(), 1, Edge::Right, 500.0).is_none());
    }

    #[test]
    fn test_no_match_wrong_edge() {
        let link = make_link(
            host_a(),
            1,
            Edge::Right,
            (0.0, 1080.0),
            1920.0,
            host_b(),
            1,
            Edge::Left,
            (0.0, 1080.0),
            0.0,
            CoordinateMapping::Linear,
        );
        let resolver = BorderLinkResolver::new(vec![link]);
        // Try left edge instead of right
        assert!(resolver.resolve(host_a(), 1, Edge::Left, 540.0).is_none());
    }

    #[test]
    fn test_landing_top_bottom_edge_offset_inward() {
        // Top edge: edge_coord = 0 → landing_y should be 1.0
        let link_top = make_link(
            host_a(),
            1,
            Edge::Top,
            (0.0, 1920.0),
            0.0,
            host_b(),
            2,
            Edge::Top,
            (0.0, 1920.0),
            0.0,
            CoordinateMapping::Linear,
        );
        let resolver = BorderLinkResolver::new(vec![link_top]);
        let result = resolver.resolve(host_a(), 1, Edge::Top, 960.0).unwrap();
        assert!(
            (result.landing_y - 1.0).abs() < 0.01,
            "landing_y={}, expected 1.0 (1px inside Top edge)",
            result.landing_y
        );

        // Bottom edge: edge_coord = 1080 → landing_y should be 1079.0
        let link_bottom = make_link(
            host_a(),
            1,
            Edge::Bottom,
            (0.0, 1920.0),
            1080.0,
            host_b(),
            2,
            Edge::Bottom,
            (0.0, 1920.0),
            1080.0,
            CoordinateMapping::Linear,
        );
        let resolver = BorderLinkResolver::new(vec![link_bottom]);
        let result = resolver.resolve(host_a(), 1, Edge::Bottom, 960.0).unwrap();
        assert!(
            (result.landing_y - 1079.0).abs() < 0.01,
            "landing_y={}, expected 1079.0 (1px inside Bottom edge)",
            result.landing_y
        );
    }

    #[test]
    fn test_landing_left_edge_offset_inward() {
        // Target display at origin 0. Left edge_coord = 0.
        // Landing on Left edge should be at 1.0 (1px inside), not 0.0.
        let link = make_link(
            host_a(),
            1,
            Edge::Left,
            (0.0, 1080.0),
            0.0,
            host_b(),
            2,
            Edge::Left,
            (0.0, 1920.0),
            0.0,
            CoordinateMapping::Linear,
        );
        let resolver = BorderLinkResolver::new(vec![link]);
        let result = resolver.resolve(host_a(), 1, Edge::Left, 540.0).unwrap();
        assert!(
            (result.landing_x - 1.0).abs() < 0.01,
            "landing_x={}, expected 1.0 (1px inside Left edge)",
            result.landing_x
        );
    }

    #[test]
    fn test_landing_right_edge_offset_inward() {
        // Target display: 1080 wide at origin 0. Right edge_coord = 1080.
        // Landing on Right edge should be at 1079 (1px inside), not 1080.
        let link = make_link(
            host_a(),
            1,
            Edge::Right,
            (0.0, 1080.0),
            1920.0,
            host_b(),
            2,
            Edge::Right,
            (0.0, 1920.0),
            1080.0,
            CoordinateMapping::Linear,
        );
        let resolver = BorderLinkResolver::new(vec![link]);
        let result = resolver.resolve(host_a(), 1, Edge::Right, 540.0).unwrap();
        // landing_x should be 1079.0 (1px inside Right edge), not 1080.0
        assert!(
            (result.landing_x - 1079.0).abs() < 0.01,
            "landing_x={}, expected 1079.0 (1px inside Right edge)",
            result.landing_x
        );
    }

    #[test]
    fn test_edge_to_coordinates_all_edges() {
        let make_seg = |edge: Edge, coord: f64| EdgeSegment {
            host_id: Uuid::nil(),
            display_id: 0,
            edge,
            range_start: 0.0,
            range_end: 1000.0,
            edge_coord: coord,
        };

        // Left: edge_coord=0 → offset to 1.0
        let (x, y) = edge_to_coordinates(&make_seg(Edge::Left, 0.0), 500.0);
        assert!((x - 1.0).abs() < 0.01);
        assert!((y - 500.0).abs() < 0.01);

        // Right: edge_coord=1920 → offset to 1919.0
        let (x, y) = edge_to_coordinates(&make_seg(Edge::Right, 1920.0), 300.0);
        assert!((x - 1919.0).abs() < 0.01);
        assert!((y - 300.0).abs() < 0.01);

        // Top: edge_coord=0 → offset to 1.0
        let (x, y) = edge_to_coordinates(&make_seg(Edge::Top, 0.0), 800.0);
        assert!((x - 800.0).abs() < 0.01);
        assert!((y - 1.0).abs() < 0.01);

        // Bottom: edge_coord=1080 → offset to 1079.0
        let (x, y) = edge_to_coordinates(&make_seg(Edge::Bottom, 1080.0), 400.0);
        assert!((x - 400.0).abs() < 0.01);
        assert!((y - 1079.0).abs() < 0.01);
    }

    #[test]
    fn test_no_bounce_back_after_landing_on_right_edge() {
        // The actual bug scenario: landing on Right edge (edge_coord = display width).
        // Without fix: landing at x=1920 → Rect::contains returns false → immediate exit.
        // With fix: landing at x=1919 → Rect::contains returns true → InBounds on next move.
        let target_topo = HostTopology::new(
            Uuid::new_v4(),
            vec![make_display(2, (0, 0), (1920, 1080), 1.0)],
        );
        let link = make_link(
            host_a(),
            1,
            Edge::Left,
            (0.0, 1080.0),
            0.0,
            host_b(),
            2,
            Edge::Right,
            (0.0, 1080.0),
            1920.0,
            CoordinateMapping::Linear,
        );
        let resolver = BorderLinkResolver::new(vec![link]);
        let result = resolver.resolve(host_a(), 1, Edge::Left, 540.0).unwrap();

        // Verify landing is inside the target display rect
        let target_rect = target_topo.displays[0].logical_rect();
        assert!(
            target_rect.contains(result.landing_x, result.landing_y),
            "Landing ({}, {}) must be inside target rect {:?}",
            result.landing_x,
            result.landing_y,
            target_rect
        );

        // Verify small inward movement stays InBounds (no bounce-back)
        let mut cursor =
            VirtualCursor::new(result.landing_x, result.landing_y, result.target_display);
        let move_result = cursor.apply_delta(-5.0, 0.0, &target_topo);
        assert_eq!(
            move_result,
            MoveResult::InBounds,
            "Moving inward from landing should be InBounds, got {:?}",
            move_result
        );
    }
}
