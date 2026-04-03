//! Windows display enumeration via EnumDisplayMonitors + GetMonitorInfoW.
//! Uses Per-Monitor DPI Awareness V2 for correct coordinates without gaps.

use crate::types::DisplayInfo;
use std::mem;
use std::sync::Once;
use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

static DPI_AWARENESS_INIT: Once = Once::new();

/// Set process DPI awareness to Per-Monitor V2.
/// Must be called before any display enumeration or window creation.
/// Safe to call multiple times — only the first call takes effect.
pub(crate) fn ensure_dpi_awareness() {
    DPI_AWARENESS_INIT.call_once(|| unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    });
}

/// Enumerate all active displays and return their layout information.
/// With Per-Monitor DPI Awareness V2, coordinates are in physical pixels
/// and tile correctly without gaps.
pub fn enumerate_displays() -> Vec<DisplayInfo> {
    ensure_dpi_awareness();

    let mut displays: Vec<DisplayInfo> = Vec::new();
    let displays_ptr = &mut displays as *mut Vec<DisplayInfo>;

    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(monitor_enum_callback),
            LPARAM(displays_ptr as isize),
        );
    }

    displays
}

unsafe extern "system" fn monitor_enum_callback(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let displays = unsafe { &mut *(lparam.0 as *mut Vec<DisplayInfo>) };

    let mut info: MONITORINFOEXW = unsafe { mem::zeroed() };
    info.monitorInfo.cbSize = mem::size_of::<MONITORINFOEXW>() as u32;

    let result = unsafe { GetMonitorInfoW(hmonitor, &mut info.monitorInfo as *mut _ as *mut _) };
    if !result.as_bool() {
        return TRUE;
    }

    let rc = info.monitorInfo.rcMonitor;
    // With Per-Monitor V2, these are in physical pixels
    let physical_w = (rc.right - rc.left) as f64;
    let physical_h = (rc.bottom - rc.top) as f64;

    let (dpi_x, _dpi_y) = get_dpi_for_monitor(hmonitor);
    let scale_factor = dpi_x as f64 / 96.0;

    let is_primary = (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0;

    let display = DisplayInfo {
        id: displays.len() as u32,
        origin_x: rc.left as f64,
        origin_y: rc.top as f64,
        width_logical: physical_w / scale_factor,
        height_logical: physical_h / scale_factor,
        width_pixels: physical_w as u32,
        height_pixels: physical_h as u32,
        scale_factor,
        is_primary,
    };

    displays.push(display);
    TRUE
}

/// Get the effective DPI for a monitor using GetDpiForMonitor.
fn get_dpi_for_monitor(hmonitor: HMONITOR) -> (u32, u32) {
    let mut dpi_x: u32 = 96;
    let mut dpi_y: u32 = 96;
    unsafe {
        let _ = GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
    }
    (dpi_x, dpi_y)
}
