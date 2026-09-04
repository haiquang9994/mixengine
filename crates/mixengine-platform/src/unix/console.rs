//! Nothing.
//!
//! A Unix process is never handed a controlling terminal it did not ask for, so there is nothing
//! here to let go of. `windows/console.rs` is where the whole of the story is, and this file exists
//! so that `crate::process::release_unattended_console` has one answer per system rather than a
//! `#[cfg]` inside it.

/// [`crate::process::release_unattended_console`] on both Unixes.
pub(crate) fn release_unattended() -> bool {
    false
}
