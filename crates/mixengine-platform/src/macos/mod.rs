//! macOS implementations of the platform traits.

mod access;
mod home;

// The local endpoint is POSIX end to end — a Unix socket, `LOCAL_PEERCRED` behind tokio's
// `peer_cred` — so unlike `access` there is nothing here for this OS to wrap. Re-exported rather
// than imported because `crate::ipc` reaches it as `sys::ipc`.
pub(crate) use crate::unix::ipc;

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
