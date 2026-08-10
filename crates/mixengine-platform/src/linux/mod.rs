//! Linux implementations of the platform traits.

mod home;

// File modes are POSIX, not Linux: `macos/` takes the same implementation.
use crate::unix::access;

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
