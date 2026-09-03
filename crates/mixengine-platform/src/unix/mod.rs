//! What macOS and Linux do identically.
//!
//! Not a fourth platform: `linux/` and `macos/` stay the two places their behaviour is decided,
//! and each names what it takes from here. This exists so a capability that is genuinely POSIX —
//! file modes, signals, process groups — is written once rather than copied and left to drift.
//! Anything one of them does differently belongs in that one, not behind a `cfg` in here.

#[cfg(feature = "host")]
pub(crate) mod access;
// Under both features since T85: the daemon reads a `st_uid` and a mode before it runs a file as
// root, and the writing half — `geteuid`, the root-owned `mkdir` — is still the helper's alone.
#[cfg(any(feature = "host", feature = "elevated"))]
pub(crate) mod elevated;
// The hosts file: one path and one replace for both systems — `linux/` and `macos/` name it.
#[cfg(feature = "ipc")]
pub(crate) mod activation;
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
// Writing a file only this account may read: `open(2)` carries the mode, which is POSIX and
// identical on both systems. `windows/` has its own, and it is a different shape rather than a
// different constant — see either module.
#[cfg(feature = "host")]
pub(crate) mod private_file;
#[cfg(feature = "process")]
pub(crate) mod process;
// Replacing a system file without ever leaving a torn one: the same mechanism for the hosts file
// and for the three files `port_access` writes on macOS.
#[cfg(feature = "elevated")]
pub(crate) mod replace;
#[cfg(feature = "signal")]
pub(crate) mod signal;
