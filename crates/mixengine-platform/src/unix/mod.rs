//! What macOS and Linux do identically.
//!
//! Not a fourth platform: `linux/` and `macos/` stay the two places their behaviour is decided,
//! and each names what it takes from here. This exists so a capability that is genuinely POSIX —
//! file modes, signals, process groups — is written once rather than copied and left to drift.
//! Anything one of them does differently belongs in that one, not behind a `cfg` in here.

pub(crate) mod access;
