//! Linux implementations of the platform traits.

mod home;

// File modes are POSIX, not Linux: `macos/` builds on the same implementation, wrapping it with the
// ACL handling that only its ACLs need.
use crate::unix::access;

// The local endpoint is POSIX end to end — a Unix socket, `SO_PEERCRED` behind tokio's `peer_cred`
// — so unlike `access` there is nothing here for this OS to wrap. The same holds for the other
// three: `flock`, `setsid` and the two signals are BSD's or POSIX's and identical on both systems.
// Re-exported rather than imported because `crate::ipc` reaches them as `sys::ipc` and so on.
pub(crate) use crate::unix::{ipc, lock, process, signal};

/// The Linux host.
#[derive(Debug, Default)]
pub(crate) struct Host {
    home: home::Home,
    access: access::Access,
}

impl Host {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

impl crate::Host for Host {
    fn home_dirs(&self) -> &dyn crate::HomeDirs {
        &self.home
    }

    fn directory_access(&self) -> &dyn crate::DirectoryAccess {
        &self.access
    }
}
