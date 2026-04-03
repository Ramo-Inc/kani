//! Display wake via IOKit power management.
//!
//! Uses `IOPMAssertionDeclareUserActivity` to signal user activity,
//! which wakes the display from sleep. This is the same mechanism
//! as `caffeinate -u` and is used by Input Leap for the same purpose.

use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use std::sync::atomic::{AtomicU32, Ordering};

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMAssertionDeclareUserActivity(
        assertion_name: core_foundation::string::CFStringRef,
        user_type: u32,
        assertion_id: *mut u32,
    ) -> i32;
}

/// `kIOPMUserActiveLocal` — user is physically present at this machine.
const IOPM_USER_ACTIVE_LOCAL: u32 = 0;

/// `kIOReturnSuccess`
const IO_RETURN_SUCCESS: i32 = 0;

/// Assertion ID reused across calls (as per IOKit convention).
static ASSERTION_ID: AtomicU32 = AtomicU32::new(0);

/// Declare user activity to wake the display from sleep.
///
/// Safe to call repeatedly — IOKit reuses the assertion ID.
/// Errors are logged at warn level but do not propagate (non-fatal).
pub fn wake_display() {
    let name = CFString::new("Kani KVM cursor enter");
    let mut assertion_id = ASSERTION_ID.load(Ordering::Relaxed);

    let result = unsafe {
        IOPMAssertionDeclareUserActivity(
            name.as_concrete_TypeRef(),
            IOPM_USER_ACTIVE_LOCAL,
            &mut assertion_id,
        )
    };

    if result == IO_RETURN_SUCCESS {
        ASSERTION_ID.store(assertion_id, Ordering::Relaxed);
        tracing::debug!(assertion_id, "Declared user activity to wake display");
    } else {
        tracing::warn!(
            io_return = result,
            "IOPMAssertionDeclareUserActivity failed"
        );
    }
}
