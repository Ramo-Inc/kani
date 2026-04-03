use muda::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;
use tray_icon::{TrayIcon, TrayIconBuilder};

use crate::model::{ClientKvmStatus, ConnectionState, KvmStatus, Role};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrayCommand {
    ShowSettings,
    ShowDisplayConfig,
    ToggleKvm,
    Quit,
}

pub struct TrayState {
    pub _tray_icon: TrayIcon,
    pub settings_id: MenuId,
    pub display_id: MenuId,
    pub quit_id: MenuId,
    pub status_item: MenuItem,
    pub toggle_item: MenuItem,
    pub toggle_id: MenuId,
}

pub fn create_tray_icon() -> Result<TrayState, Box<dyn std::error::Error>> {
    let menu = Menu::new();

    // Status item (disabled, informational)
    let status_item = MenuItem::new("Status: Stopped", false, None);
    // Toggle item (Start/Stop KVM)
    let toggle_item = MenuItem::new("Start KVM", true, None);
    let toggle_id = toggle_item.id().clone();

    let settings_item = MenuItem::new("Settings...", true, None);
    let display_item = MenuItem::new("Display Layout...", true, None);
    let quit_item = MenuItem::new("Quit", true, None);

    let settings_id = settings_item.id().clone();
    let display_id = display_item.id().clone();
    let quit_id = quit_item.id().clone();

    menu.append(&status_item)?;
    menu.append(&toggle_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&settings_item)?;
    menu.append(&display_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit_item)?;

    let icon = crate::icon::load_tray_icon();

    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Kani KVM")
        .with_icon(icon)
        .build()?;

    Ok(TrayState {
        _tray_icon: tray_icon,
        settings_id,
        display_id,
        quit_id,
        status_item,
        toggle_item,
        toggle_id,
    })
}

/// Set up the global MenuEvent callback handler.
///
/// This MUST be called exactly once, before any menu events can fire.
/// After this call, `MenuEvent::receiver().try_recv()` will no longer receive events.
pub fn setup_tray_event_handler(
    #[allow(unused_variables)] hwnd: isize,
    tx: mpsc::Sender<TrayCommand>,
    should_quit: Arc<AtomicBool>,
    settings_id: MenuId,
    display_id: MenuId,
    toggle_id: MenuId,
    quit_id: MenuId,
) {
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if event.id == settings_id {
            #[cfg(target_os = "windows")]
            show_window(hwnd);
            let _ = tx.send(TrayCommand::ShowSettings);
        } else if event.id == display_id {
            #[cfg(target_os = "windows")]
            show_window(hwnd);
            let _ = tx.send(TrayCommand::ShowDisplayConfig);
        } else if event.id == toggle_id {
            #[cfg(target_os = "windows")]
            show_window(hwnd);
            let _ = tx.send(TrayCommand::ToggleKvm);
        } else if event.id == quit_id {
            should_quit.store(true, std::sync::atomic::Ordering::Relaxed);
            #[cfg(target_os = "windows")]
            show_window(hwnd);
            let _ = tx.send(TrayCommand::Quit);
        }
    }));
}

#[cfg(target_os = "windows")]
fn show_window(hwnd: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOWDEFAULT};
    unsafe {
        let _ = ShowWindow(HWND(hwnd as *mut _), SW_SHOWDEFAULT);
    }
}

/// Update tray menu items to reflect current KVM status.
/// Only calls set_text()/set_enabled() when values actually change.
pub fn update_tray_status(
    tray: &TrayState,
    role: Role,
    kvm_status: &KvmStatus,
    client_kvm_status: ClientKvmStatus,
    connection: &ConnectionState,
) {
    let (status_text, toggle_text, toggle_enabled) = match role {
        Role::Host => match kvm_status {
            KvmStatus::Stopped => ("Status: Stopped", "Start KVM", true),
            KvmStatus::Starting => ("Status: Starting...", "Start KVM", false),
            KvmStatus::Running => ("Status: Running", "Stop KVM", true),
            KvmStatus::Stopping => ("Status: Stopping...", "Stop KVM", false),
            KvmStatus::Error(e) => {
                tray.status_item
                    .set_text(format!("Status: Error \u{2014} {e}"));
                tray.toggle_item.set_text("Start KVM");
                tray.toggle_item.set_enabled(true);
                return;
            }
        },
        Role::Client => {
            let status = match client_kvm_status {
                ClientKvmStatus::Idle
                    if matches!(connection, ConnectionState::Connected { .. }) =>
                {
                    "Status: Connected (Idle)"
                }
                ClientKvmStatus::Active => "Status: Active (Client)",
                ClientKvmStatus::Disconnected => "Status: Host Disconnected",
                _ => "Status: Disconnected",
            };
            (status, "Start KVM", false) // Client: toggle always disabled
        }
    };
    tray.status_item.set_text(status_text);
    tray.toggle_item.set_text(toggle_text);
    tray.toggle_item.set_enabled(toggle_enabled);
}
