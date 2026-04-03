//! macOS display enumeration via Core Graphics.

use crate::types::DisplayInfo;
use core_graphics::display::{CGDirectDisplayID, CGDisplay, CGRect};
use tracing::{debug, warn};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGGetActiveDisplayList(
        max_displays: u32,
        active_displays: *mut CGDirectDisplayID,
        display_count: *mut u32,
    ) -> i32;
    fn CGDisplayBounds(display: CGDirectDisplayID) -> CGRect;
}

/// Enumerate all active displays and return their layout information.
pub fn enumerate_displays() -> Vec<DisplayInfo> {
    // First call: get the count of active displays.
    let mut display_count: u32 = 0;
    let err = unsafe { CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut display_count) };
    if err != 0 {
        warn!(error = err, "CGGetActiveDisplayList count query failed");
        return Vec::new();
    }

    if display_count == 0 {
        debug!("No active displays found");
        return Vec::new();
    }

    // Second call: get the display IDs.
    let mut display_ids = vec![0u32; display_count as usize];
    let err = unsafe {
        CGGetActiveDisplayList(display_count, display_ids.as_mut_ptr(), &mut display_count)
    };
    if err != 0 {
        warn!(error = err, "CGGetActiveDisplayList list query failed");
        return Vec::new();
    }

    let main_display_id = CGDisplay::main().id;

    display_ids
        .iter()
        .map(|&display_id| {
            let bounds: CGRect = unsafe { CGDisplayBounds(display_id) };
            let display = CGDisplay::new(display_id);

            let pixel_w = display.pixels_wide() as f64;
            let pixel_h = display.pixels_high() as f64;
            let point_w = bounds.size.width;
            let scale = if point_w > 0.0 {
                pixel_w / point_w
            } else {
                1.0
            };

            let info = DisplayInfo {
                id: display_id,
                origin_x: bounds.origin.x,
                origin_y: bounds.origin.y,
                width_logical: bounds.size.width,
                height_logical: bounds.size.height,
                width_pixels: pixel_w as u32,
                height_pixels: pixel_h as u32,
                scale_factor: scale,
                is_primary: display_id == main_display_id,
            };

            debug!(
                id = display_id,
                origin_x = info.origin_x,
                origin_y = info.origin_y,
                width = info.width_logical,
                height = info.height_logical,
                scale = info.scale_factor,
                primary = info.is_primary,
                "Enumerated display"
            );

            info
        })
        .collect()
}
