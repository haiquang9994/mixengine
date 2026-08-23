//! What macOS and Linux do identically.
//!
//! Not a fourth platform: `linux/` and `macos/` stay the two places their behaviour is decided,
//! and each names what it takes from here. This exists so a capability that is genuinely POSIX —
//! file modes, signals, process groups — is written once rather than copied and left to drift.
//! Anything one of them does differently belongs in that one, not behind a `cfg` in here.

#[cfg(feature = "host")]
pub(crate) mod access;
#[cfg(feature = "elevated")]
pub(crate) mod elevated;
// The hosts file: one path and one replace for both systems — `linux/` and `macos/` name it.
#[cfg(any(feature = "host", feature = "elevated"))]
pub(crate) mod hosts;
#[cfg(feature = "ipc")]
pub(crate) mod ipc;
pub(crate) mod lock;
// A marked block in a shell profile is POSIX in everything but *which* profiles: the mechanism is
// written once here, and each system passes its own list in. That is the pattern `access` uses in
// the other direction — shared code that one OS wraps — rather than a `cfg` inside this directory.
#[cfg(feature = "host")]
pub(crate) mod path;
#[cfg(feature = "process")]
pub(crate) mod process;
#[cfg(feature = "signal")]
pub(crate) mod signal;
