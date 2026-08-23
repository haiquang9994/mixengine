//! macOS' resolver files, as text.
//!
//! Pure, and compiled on all three systems exactly as [`crate::port_access::pf`] is: the file this
//! generates is the whole of what macOS needs, so generating it here means a developer on any of
//! the three can test it in full and only the write itself is per-OS.

use std::path::{Path, PathBuf};

/// Where macOS looks for per-domain resolver configuration — `man 5 resolver`.
pub(crate) const DIRECTORY: &str = "/etc/resolver";

/// The line that says whose file this is.
///
/// **A marker rather than a spliced block** — the T45 design, D5. The whole file is ours, so there
/// is nothing to splice into; what is needed is the ability to tell our file from somebody else's
/// configuration for the same TLD, which must never be replaced silently.
const MARKER: &str = "# Managed by MixEngine. Remove this file to stop routing this TLD.";

/// The file that routes one TLD to the server on `port`.
///
/// The TLD itself does not appear in the contents: macOS takes the domain from the *file name*, and
/// a name repeated inside would be a second place for it to disagree with the first.
pub(crate) fn file_for(port: u16) -> String {
    format!("{MARKER}\nnameserver 127.0.0.1\nport {port}\n")
}

/// Did MixEngine write this?
pub(crate) fn is_ours(contents: &str) -> bool {
    contents.lines().any(|line| line.trim() == MARKER)
}

/// The file one TLD's configuration goes in.
///
/// `tld` is a single label that has already been checked against
/// [`WIRED_TLDS`](mixengine_proto::domains::WIRED_TLDS), so it can hold no separator and this
/// cannot become a traversal.
pub(crate) fn path_for(tld: &str) -> PathBuf {
    Path::new(DIRECTORY).join(tld)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two lines measured to work, and the marker that says whose file it is.
    #[test]
    fn a_resolver_file_names_loopback_and_the_port() {
        let file = file_for(53_535);

        assert!(file.contains("nameserver 127.0.0.1"), "{file}");
        assert!(file.contains("port 53535"), "{file}");
        assert!(is_ours(&file));
    }

    /// D5. A file without our marker is somebody else's configuration for that TLD, and replacing
    /// it silently is the failure T41's marker block exists to prevent.
    #[test]
    fn a_file_without_the_marker_is_not_ours() {
        assert!(!is_ours("nameserver 192.168.1.1\n"));
        assert!(!is_ours(""));
    }

    /// Two homes on one machine differ by port, so the comparison that decides "already wired" has
    /// to be able to tell them apart.
    #[test]
    fn two_ports_produce_two_different_files() {
        assert_ne!(file_for(53_535), file_for(60_000));
    }

    /// The path is the TLD under the resolver directory and nothing else — asserted so that a
    /// future caller cannot quietly make it a join of something longer.
    #[test]
    fn the_path_is_the_tld_under_the_resolver_directory() {
        assert_eq!(path_for("test"), Path::new(DIRECTORY).join("test"));
        assert_eq!(path_for("internal"), Path::new(DIRECTORY).join("internal"));
    }
}
