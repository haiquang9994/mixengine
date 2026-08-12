//! Linux implementations of the platform traits.

mod home;
pub(crate) mod process;

// File modes are POSIX, not Linux: `macos/` builds on the same implementation, wrapping it with the
// ACL handling that only its ACLs need.
use crate::unix::access;

// The local endpoint is POSIX end to end — a Unix socket, `SO_PEERCRED` behind tokio's `peer_cred`
// — so unlike `access` there is nothing here for this OS to wrap. The same holds for `flock` and
// the two signals, which are BSD's or POSIX's and identical on both systems. Re-exported rather
// than imported because `crate::ipc` reaches them as `sys::ipc` and so on. Starting a process is
// the one that is *not* purely POSIX — `PR_SET_PDEATHSIG` is this system's alone — so `process`
// above is a module here that adds to `unix/` rather than a re-export of it.
pub(crate) use crate::unix::{ipc, lock, signal};

/// The Linux host.
#[derive(Debug, Default)]
pub(crate) struct Host {
    home: home::Home,
    access: access::Access,
    // Not a `linux/` module, and not a `unix/` one either: the secret service is reached through the
    // same crate the other two systems' stores are. See `crate::secrets`.
    secrets: crate::secrets::Secrets,
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

    fn keyring(&self) -> &dyn crate::Keyring {
        &self.secrets
    }
}
