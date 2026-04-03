//! Embedded icon loading for window and tray icons.

use eframe::egui;
use image::ImageReader;
use std::io::Cursor;

/// Load the app icon (full illustration) for the window titlebar/taskbar.
pub fn load_window_icon() -> egui::IconData {
    let (rgba, width, height) = decode_png(include_bytes!("../assets/app_32x32.png"));
    egui::IconData {
        rgba,
        width,
        height,
    }
}

/// Load the tray icon (simplified crab silhouette).
pub fn load_tray_icon() -> tray_icon::Icon {
    let (rgba, width, height) = decode_png(include_bytes!("../assets/tray_32x32.png"));
    tray_icon::Icon::from_rgba(rgba, width, height).expect("Failed to create tray icon")
}

fn decode_png(png_bytes: &[u8]) -> (Vec<u8>, u32, u32) {
    let image = ImageReader::new(Cursor::new(png_bytes))
        .with_guessed_format()
        .expect("Failed to guess image format")
        .decode()
        .expect("Failed to decode PNG");
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    (rgba.to_vec(), width, height)
}
