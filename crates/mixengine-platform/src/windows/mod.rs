//! Windows implementations of the platform traits.

mod access;
pub(crate) mod fullname;
mod home;
pub(crate) mod ipc;
pub(crate) mod lock;
mod path;
mod ports;
pub(crate) mod process;
mod restricted;
mod sid;
pub(crate) mod signal;

/// The Windows host.
#[derive(Debug)]
pub(crate) struct Host {
    home: home::Home,
    access: access::Access,
    // Not a `windows/` module: the Credential Manager is reached through the same crate the other
    // two systems' stores are. See `crate::secrets`.
    secrets: crate::secrets::Secrets,
    env: path::Env,
    ports: ports::Ports,
}

impl Host {
    pub(crate) fn new() -> Self {
        Self {
            home: home::Home,
            access: access::Access::default(),
            secrets: crate::secrets::Secrets,
            env: path::Env::of_this_user(),
            ports: ports::Ports,
        }
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

    fn path_integration(&self) -> &dyn crate::PathIntegration {
        &self.env
    }

    fn port_owner(&self) -> &dyn crate::PortOwner {
        &self.ports
    }
}
