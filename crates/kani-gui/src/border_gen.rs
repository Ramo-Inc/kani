use kani_proto::event::{DisplayId, HostId};
use kani_proto::topology::{CoordinateMapping, Edge};

use crate::model::{GuiBorderLink, GuiHost};

const ADJACENCY_TOLERANCE: f64 = 300.0;
const OVERLAP_TOLERANCE: f64 = 150.0;

struct WorldEdge {
    host_id: HostId,
    display_id: DisplayId,
    edge: Edge,
    edge_coord: f64,
    range_start: f64,
    range_end: f64,
}

pub fn generate_border_links(hosts: &[GuiHost]) -> Vec<GuiBorderLink> {
    let edges = collect_all_edges(hosts);
    let mut links = Vec::new();
    for (i, a) in edges.iter().enumerate() {
        for b in edges.iter().skip(i + 1) {
            if a.host_id == b.host_id {
                continue;
            }
            if let Some((link_ab, link_ba)) = try_make_bidirectional_link(a, b) {
                links.push(link_ab);
                links.push(link_ba);
            }
        }
    }
    links
}

fn collect_all_edges(hosts: &[GuiHost]) -> Vec<WorldEdge> {
    let mut edges = Vec::new();
    for host in hosts {
        for d in &host.displays {
            let wx = d.world_x(host.gui_offset);
            let wy = d.world_y(host.gui_offset);
            edges.push(WorldEdge {
                host_id: host.host_id,
                display_id: d.id,
                edge: Edge::Left,
                edge_coord: wx,
                range_start: wy,
                range_end: wy + d.height,
            });
            edges.push(WorldEdge {
                host_id: host.host_id,
                display_id: d.id,
                edge: Edge::Right,
                edge_coord: wx + d.width,
                range_start: wy,
                range_end: wy + d.height,
            });
            edges.push(WorldEdge {
                host_id: host.host_id,
                display_id: d.id,
                edge: Edge::Top,
                edge_coord: wy,
                range_start: wx,
                range_end: wx + d.width,
            });
            edges.push(WorldEdge {
                host_id: host.host_id,
                display_id: d.id,
                edge: Edge::Bottom,
                edge_coord: wy + d.height,
                range_start: wx,
                range_end: wx + d.width,
            });
        }
    }
    edges
}

fn try_make_bidirectional_link(
    a: &WorldEdge,
    b: &WorldEdge,
) -> Option<(GuiBorderLink, GuiBorderLink)> {
    let (from, to) = match (a.edge, b.edge) {
        (Edge::Right, Edge::Left) => {
            let gap = b.edge_coord - a.edge_coord;
            if (-OVERLAP_TOLERANCE..ADJACENCY_TOLERANCE).contains(&gap) {
                (a, b)
            } else {
                return None;
            }
        }
        (Edge::Left, Edge::Right) => {
            let gap = a.edge_coord - b.edge_coord;
            if (-OVERLAP_TOLERANCE..ADJACENCY_TOLERANCE).contains(&gap) {
                (b, a)
            } else {
                return None;
            }
        }
        (Edge::Bottom, Edge::Top) => {
            let gap = b.edge_coord - a.edge_coord;
            if (-OVERLAP_TOLERANCE..ADJACENCY_TOLERANCE).contains(&gap) {
                (a, b)
            } else {
                return None;
            }
        }
        (Edge::Top, Edge::Bottom) => {
            let gap = a.edge_coord - b.edge_coord;
            if (-OVERLAP_TOLERANCE..ADJACENCY_TOLERANCE).contains(&gap) {
                (b, a)
            } else {
                return None;
            }
        }
        _ => return None,
    };

    let overlap_start = from.range_start.max(to.range_start);
    let overlap_end = from.range_end.min(to.range_end);
    if overlap_end <= overlap_start {
        return None;
    }

    let link_forward = GuiBorderLink {
        from_host: from.host_id,
        from_display: from.display_id,
        from_edge: from.edge,
        from_range: (overlap_start, overlap_end),
        to_host: to.host_id,
        to_display: to.display_id,
        to_edge: to.edge,
        to_range: (overlap_start, overlap_end),
        mapping: CoordinateMapping::Linear,
    };
    let link_reverse = GuiBorderLink {
        from_host: to.host_id,
        from_display: to.display_id,
        from_edge: to.edge,
        from_range: (overlap_start, overlap_end),
        to_host: from.host_id,
        to_display: from.display_id,
        to_edge: from.edge,
        to_range: (overlap_start, overlap_end),
        mapping: CoordinateMapping::Linear,
    };
    Some((link_forward, link_reverse))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::GuiDisplay;
    use kani_proto::topology::Orientation;
    use uuid::Uuid;

    fn host_a() -> HostId {
        Uuid::from_u128(1)
    }
    fn host_b() -> HostId {
        Uuid::from_u128(2)
    }

    fn make_host(
        id: HostId,
        name: &str,
        is_local: bool,
        offset: (f64, f64),
        displays: Vec<GuiDisplay>,
    ) -> GuiHost {
        GuiHost {
            host_id: id,
            name: name.into(),
            address: "127.0.0.1".into(),
            is_local,
            platform: "windows".into(),
            gui_offset: offset,
            displays,
            connected: true,
        }
    }

    fn make_display(id: u32, lx: f64, ly: f64, w: f64, h: f64) -> GuiDisplay {
        GuiDisplay {
            id,
            local_origin_x: lx,
            local_origin_y: ly,
            width: w,
            height: h,
            resolution: (w as u32, h as u32),
            scale_factor: 1.0,
            orientation: Orientation::Normal,
        }
    }

    #[test]
    fn test_adjacent_right_left_generates_bidirectional_links() {
        let hosts = vec![
            make_host(
                host_a(),
                "A",
                true,
                (0.0, 0.0),
                vec![make_display(0, 0.0, 0.0, 1920.0, 1080.0)],
            ),
            make_host(
                host_b(),
                "B",
                false,
                (1920.0, 0.0),
                vec![make_display(0, 0.0, 0.0, 1080.0, 1920.0)],
            ),
        ];
        let links = generate_border_links(&hosts);
        assert_eq!(links.len(), 2);
        let ab = links.iter().find(|l| l.from_edge == Edge::Right).unwrap();
        assert_eq!(ab.from_host, host_a());
        assert_eq!(ab.to_host, host_b());
        assert_eq!(ab.to_edge, Edge::Left);
        assert!((ab.from_range.0 - 0.0).abs() < 1.0);
        assert!((ab.from_range.1 - 1080.0).abs() < 1.0);
        let ba = links.iter().find(|l| l.from_edge == Edge::Left).unwrap();
        assert_eq!(ba.from_host, host_b());
        assert_eq!(ba.to_host, host_a());
    }

    #[test]
    fn test_no_link_when_not_adjacent() {
        let hosts = vec![
            make_host(
                host_a(),
                "A",
                true,
                (0.0, 0.0),
                vec![make_display(0, 0.0, 0.0, 1920.0, 1080.0)],
            ),
            make_host(
                host_b(),
                "B",
                false,
                (5000.0, 0.0),
                vec![make_display(0, 0.0, 0.0, 1080.0, 1920.0)],
            ),
        ];
        let links = generate_border_links(&hosts);
        assert!(links.is_empty());
    }

    #[test]
    fn test_no_link_within_same_host() {
        let hosts = vec![make_host(
            host_a(),
            "A",
            true,
            (0.0, 0.0),
            vec![
                make_display(0, 0.0, 0.0, 1920.0, 1080.0),
                make_display(1, 1920.0, 0.0, 1920.0, 1080.0),
            ],
        )];
        let links = generate_border_links(&hosts);
        assert!(links.is_empty());
    }

    #[test]
    fn test_bottom_top_adjacency() {
        let hosts = vec![
            make_host(
                host_a(),
                "A",
                true,
                (0.0, 0.0),
                vec![make_display(0, 0.0, 0.0, 1920.0, 1080.0)],
            ),
            make_host(
                host_b(),
                "B",
                false,
                (0.0, 1080.0),
                vec![make_display(0, 0.0, 0.0, 1920.0, 1080.0)],
            ),
        ];
        let links = generate_border_links(&hosts);
        assert_eq!(links.len(), 2);
        assert!(links
            .iter()
            .any(|l| l.from_edge == Edge::Bottom && l.to_edge == Edge::Top));
        assert!(links
            .iter()
            .any(|l| l.from_edge == Edge::Top && l.to_edge == Edge::Bottom));
    }

    #[test]
    fn test_small_overlap_creates_links() {
        // 20px overlap (gap = -20) is within OVERLAP_TOLERANCE (150)
        let hosts = vec![
            make_host(
                host_a(),
                "A",
                true,
                (0.0, 0.0),
                vec![make_display(0, 0.0, 0.0, 1920.0, 1080.0)],
            ),
            make_host(
                host_b(),
                "B",
                false,
                (1900.0, 0.0),
                vec![make_display(0, 0.0, 0.0, 1080.0, 1920.0)],
            ),
        ];
        let links = generate_border_links(&hosts);
        assert_eq!(links.len(), 2, "20px overlap should create links");
    }

    #[test]
    fn test_large_overlap_no_links() {
        // 200px overlap (gap = -200) exceeds OVERLAP_TOLERANCE (150)
        let hosts = vec![
            make_host(
                host_a(),
                "A",
                true,
                (0.0, 0.0),
                vec![make_display(0, 0.0, 0.0, 1920.0, 1080.0)],
            ),
            make_host(
                host_b(),
                "B",
                false,
                (1720.0, 0.0),
                vec![make_display(0, 0.0, 0.0, 1080.0, 1920.0)],
            ),
        ];
        let links = generate_border_links(&hosts);
        assert!(links.is_empty(), "200px overlap should NOT create links");
    }

    #[test]
    fn test_150px_gap_creates_links() {
        let hosts = vec![
            make_host(
                host_a(),
                "A",
                true,
                (0.0, 0.0),
                vec![make_display(0, 0.0, 0.0, 1920.0, 1080.0)],
            ),
            make_host(
                host_b(),
                "B",
                false,
                (2070.0, 0.0), // 1920 + 150 = 2070
                vec![make_display(0, 0.0, 0.0, 1920.0, 1080.0)],
            ),
        ];
        let links = generate_border_links(&hosts);
        assert_eq!(
            links.len(),
            2,
            "150px gap should create bidirectional links"
        );
    }

    #[test]
    fn test_250px_gap_creates_links() {
        // 250px gap is within ADJACENCY_TOLERANCE (300)
        let hosts = vec![
            make_host(
                host_a(),
                "A",
                true,
                (0.0, 0.0),
                vec![make_display(0, 0.0, 0.0, 1920.0, 1080.0)],
            ),
            make_host(
                host_b(),
                "B",
                false,
                (2170.0, 0.0), // 1920 + 250 = 2170
                vec![make_display(0, 0.0, 0.0, 1920.0, 1080.0)],
            ),
        ];
        let links = generate_border_links(&hosts);
        assert_eq!(links.len(), 2, "250px gap should create links");
    }

    #[test]
    fn test_350px_gap_no_links() {
        // 350px gap exceeds ADJACENCY_TOLERANCE (300)
        let hosts = vec![
            make_host(
                host_a(),
                "A",
                true,
                (0.0, 0.0),
                vec![make_display(0, 0.0, 0.0, 1920.0, 1080.0)],
            ),
            make_host(
                host_b(),
                "B",
                false,
                (2270.0, 0.0), // 1920 + 350 = 2270
                vec![make_display(0, 0.0, 0.0, 1920.0, 1080.0)],
            ),
        ];
        let links = generate_border_links(&hosts);
        assert!(links.is_empty(), "350px gap should NOT create links");
    }

    #[test]
    fn test_tolerance_allows_near_adjacency() {
        let hosts = vec![
            make_host(
                host_a(),
                "A",
                true,
                (0.0, 0.0),
                vec![make_display(0, 0.0, 0.0, 1920.0, 1080.0)],
            ),
            make_host(
                host_b(),
                "B",
                false,
                (1930.0, 0.0),
                vec![make_display(0, 0.0, 0.0, 1080.0, 1920.0)],
            ),
        ];
        let links = generate_border_links(&hosts);
        assert_eq!(links.len(), 2);
    }
}
