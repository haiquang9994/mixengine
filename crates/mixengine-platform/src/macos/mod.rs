//! macOS implementations of the platform traits.

mod access;
mod home;
pub(crate) mod process;

// The local endpoint is POSIX end to end — a Unix socket, `LOCAL_PEERCRED` behind tokio's
// `peer_cred` — so unlike `access` there is nothing here for this OS to wrap. The same holds for
// `flock` and the two signals, which are BSD's or POSIX's and identical on both systems.
// Re-exported rather than imported because `crate::ipc` reaches them as `sys::ipc` and so on.
// Starting a process is the one that is *not* purely POSIX, and this system's `process` module is
// mostly there to record what it consequently cannot promise.
pub(crate) use crate::unix::{ipc, lock, signal};

/// The macOS host.
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
