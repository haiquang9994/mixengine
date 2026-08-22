//! Where macOS and Linux keep the hosts file, and how a file is replaced on either of them.
//!
//! One path and one mechanism for both systems, which is what `unix/` is for — `linux/mod.rs` and
//! `macos/mod.rs` each name it rather than repeating it.

use std::path::PathBuf;

/// `/etc/hosts` on both systems, and it has been since 4.2BSD.
pub(crate) fn path() -> PathBuf {
    PathBuf::from("/etc/hosts")
}
