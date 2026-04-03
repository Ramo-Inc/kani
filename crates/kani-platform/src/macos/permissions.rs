//! macOS permission checks for input monitoring and accessibility.

use tracing::{info, warn};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
    fn CGPreflightPostEventAccess() -> bool;
    fn CGRequestPostEventAccess() -> bool;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

/// Three-level permission status for macOS input access.
#[derive(Debug, Clone, Copy)]
pub struct PermissionStatus {
    pub listen: bool,
    pub post: bool,
    pub accessibility: bool,
}

impl PermissionStatus {
    /// Returns true if all permissions are granted.
    pub fn all_granted(&self) -> bool {
        self.listen && self.post && self.accessibility
    }
}

/// Check current permission status without triggering any system dialogs.
pub fn check_permissions() -> PermissionStatus {
    let listen = unsafe { CGPreflightListenEventAccess() };
    let post = unsafe { CGPreflightPostEventAccess() };
    let accessibility = unsafe { AXIsProcessTrusted() };

    let status = PermissionStatus {
        listen,
        post,
        accessibility,
    };

    info!(
        listen = status.listen,
        post = status.post,
        accessibility = status.accessibility,
        "macOS permission check"
    );

    // Check if Secure Input is active (can block synthetic keyboard events).
    check_secure_input();

    status
}

/// Check if macOS Secure Input is currently active (blocks synthetic keyboard events).
pub fn check_secure_input() {
    // CGSIsSecureEventInputSet is private API, use ioreg as alternative
    let output = std::process::Command::new("ioreg")
        .args(["-l", "-w", "0"])
        .output();
    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        if text.contains("kCGSSessionSecureInputPID") {
            warn!(
                "Secure Input is ACTIVE — synthetic keyboard events may be blocked. \
                 Check if Terminal.app or a password manager has enabled it."
            );
        } else {
            info!("Secure Input is not active");
        }
    }
}

/// Request any missing permissions. This may trigger system dialogs.
pub fn request_permissions() {
    let status = check_permissions();

    if !status.listen {
        warn!("Requesting ListenEvent access...");
        let granted = unsafe { CGRequestListenEventAccess() };
        if !granted {
            warn!(
                "ListenEvent access denied. Enable in System Settings > \
                 Privacy & Security > Input Monitoring."
            );
        }
    }

    if !status.post {
        warn!("Requesting PostEvent access...");
        let granted = unsafe { CGRequestPostEventAccess() };
        if !granted {
            warn!(
                "PostEvent access denied. Enable in System Settings > \
                 Privacy & Security > Accessibility."
            );
        }
    }

    if !status.accessibility {
        warn!(
            "Accessibility permission not granted. Required for input suppression (grab). \
             Enable in System Settings > Privacy & Security > Accessibility."
        );
    }
}
