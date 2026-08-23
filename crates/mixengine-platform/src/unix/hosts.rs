//! Where macOS and Linux keep the hosts file.
//!
//! One path for both systems, which is what `unix/` is for — `linux/mod.rs` and `macos/mod.rs` each
//! name it rather than repeating it. The replace itself left for `unix/replace.rs` when T42 brought
//! a second file that needed it.

use std::path::PathBuf;

/// `/etc/hosts` on both systems, and it has been since 4.2BSD.
pub(crate) fn path() -> PathBuf {
    PathBuf::from("/etc/hosts")
}
