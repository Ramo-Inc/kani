use crate::event::{DisplayId, HostId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Orientation {
    Normal,
    Rotated90,
    Rotated180,
    Rotated270,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Display {
    pub id: DisplayId,
    pub origin: (i32, i32),
    pub resolution: (u32, u32),
    pub scale_factor: f64,
    pub logical_size: (f64, f64),
    pub orientation: Orientation,
}

impl Display {
    pub fn logical_rect(&self) -> Rect {
        Rect {
            x: self.origin.0 as f64,
            y: self.origin.1 as f64,
            w: self.logical_size.0,
            h: self.logical_size.1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HostTopology {
    pub host_id: HostId,
    pub displays: Vec<Display>,
    pub bounding_box: Rect,
}

impl HostTopology {
    pub fn new(host_id: HostId, displays: Vec<Display>) -> Self {
        let bb = Self::compute_bounding_box(&displays);
        Self {
            host_id,
            displays,
            bounding_box: bb,
        }
    }

    fn compute_bounding_box(displays: &[Display]) -> Rect {
        if displays.is_empty() {
            return Rect {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            };
        }
        let mut min_x = f64::MAX;
        let mut min_y = f64::MAX;
        let mut max_x = f64::MIN;
        let mut max_y = f64::MIN;
        for d in displays {
            let r = d.logical_rect();
            min_x = min_x.min(r.x);
            min_y = min_y.min(r.y);
            max_x = max_x.max(r.x + r.w);
            max_y = max_y.max(r.y + r.h);
        }
        Rect {
            x: min_x,
            y: min_y,
            w: max_x - min_x,
            h: max_y - min_y,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

/// An edge segment on a specific display.
/// `range_start`/`range_end` = range along the edge in logical coordinates.
/// `edge_coord` = the constant coordinate perpendicular to the edge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EdgeSegment {
    pub host_id: HostId,
    pub display_id: DisplayId,
    pub edge: Edge,
    pub range_start: f64,
    pub range_end: f64,
    pub edge_coord: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CoordinateMapping {
    Linear,
    OneToOneClamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BorderLink {
    pub from: EdgeSegment,
    pub to: EdgeSegment,
    pub mapping: CoordinateMapping,
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn test_bounding_box_single_display() {
        let host = HostTopology::new(
            Uuid::new_v4(),
            vec![make_display(0, (0, 0), (3840, 2160), 2.0)],
        );
        assert_eq!(
            host.bounding_box,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1920.0,
                h: 1080.0
            }
        );
    }

    #[test]
    fn test_bounding_box_dual_display() {
        let host = HostTopology::new(
            Uuid::new_v4(),
            vec![
                make_display(0, (0, 0), (3840, 2160), 2.0),
                make_display(1, (1920, 0), (1920, 1080), 1.0),
            ],
        );
        assert_eq!(
            host.bounding_box,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 3840.0,
                h: 1080.0
            }
        );
    }

    #[test]
    fn test_bounding_box_negative_origin() {
        let host = HostTopology::new(
            Uuid::new_v4(),
            vec![
                make_display(0, (0, 0), (1920, 1080), 1.0),
                make_display(1, (-1920, 0), (1920, 1080), 1.0),
            ],
        );
        assert_eq!(
            host.bounding_box,
            Rect {
                x: -1920.0,
                y: 0.0,
                w: 3840.0,
                h: 1080.0
            }
        );
    }

    #[test]
    fn test_bounding_box_empty() {
        let host = HostTopology::new(Uuid::new_v4(), vec![]);
        assert_eq!(
            host.bounding_box,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0
            }
        );
    }

    #[test]
    fn test_rect_contains() {
        let r = Rect {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        assert!(r.contains(0.0, 0.0));
        assert!(r.contains(960.0, 540.0));
        assert!(!r.contains(1920.0, 540.0)); // exclusive upper bound
        assert!(!r.contains(-1.0, 0.0));
    }

    #[test]
    fn test_display_logical_rect() {
        let d = make_display(0, (100, 200), (3840, 2160), 2.0);
        let r = d.logical_rect();
        assert_eq!(
            r,
            Rect {
                x: 100.0,
                y: 200.0,
                w: 1920.0,
                h: 1080.0
            }
        );
    }

    #[test]
    fn test_edge_segment_serialization() {
        let seg = EdgeSegment {
            host_id: Uuid::nil(),
            display_id: 0,
            edge: Edge::Right,
            range_start: 0.0,
            range_end: 1080.0,
            edge_coord: 1920.0,
        };
        let json = serde_json::to_string(&seg).unwrap();
        let decoded: EdgeSegment = serde_json::from_str(&json).unwrap();
        assert_eq!(seg, decoded);
    }
}
