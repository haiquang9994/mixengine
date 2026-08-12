//! Windows implementations of the platform traits.

mod access;
mod home;
pub(crate) mod ipc;
pub(crate) mod lock;
pub(crate) mod process;
mod sid;
pub(crate) mod signal;

/// The Windows host.
#[derive(Debug, Default)]
pub(crate) struct Host {
    home: home::Home,
    access: access::Access,
    // Not a `windows/` module: the Credential Manager is reached through the same crate the other
    // two systems' stores are. See `crate::secrets`.
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
