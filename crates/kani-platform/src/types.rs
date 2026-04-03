use kani_proto::event::DisplayId;

#[derive(Debug, Clone)]
pub struct DisplayInfo {
    pub id: DisplayId,
    pub origin_x: f64,
    pub origin_y: f64,
    pub width_logical: f64,
    pub height_logical: f64,
    pub width_pixels: u32,
    pub height_pixels: u32,
    pub scale_factor: f64,
    pub is_primary: bool,
}
