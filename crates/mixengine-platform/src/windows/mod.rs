//! Windows implementations of the platform traits.

#[cfg(feature = "host")]
mod access;
// Running a Windows tool as an argument vector, which both `access` (behind `host`) and `elevated`
// need — so it sits here rather than inside either of them.
#[cfg(any(feature = "host", feature = "elevated"))]
pub(crate) mod command;
#[cfg(feature = "elevated")]
pub(crate) mod elevated;
pub(crate) mod fullname;
#[cfg(feature = "host")]
mod home;
#[cfg(any(feature = "host", feature = "elevated"))]
pub(crate) mod hosts;
#[cfg(feature = "ipc")]
pub(crate) mod ipc;
pub(crate) mod lock;
#[cfg(feature = "host")]
mod path;
// The read half is `host` and the write half is `elevated`, so the module is declared for
// both and every item inside it carries its own gate.
#[cfg(any(feature = "host", feature = "elevated"))]
pub(crate) mod port_access;
#[cfg(feature = "host")]
mod ports;
#[cfg(feature = "process")]
pub(crate) mod process;
#[cfg(feature = "host")]
mod prompt;
#[cfg(feature = "elevated")]
pub(crate) mod replace;
// The read half is `host` and the write half is `elevated`, as `port_access` is.
#[cfg(any(feature = "host", feature = "elevated"))]
pub(crate) mod resolver;
#[cfg(feature = "process")]
mod restricted;
// SIDs are read by the pipe's peer check (`ipc`), by the restricted token (`process`) and by the
// owner of a file (`elevated`) — so the module belongs to none of them and is gated by all three.
#[cfg(any(feature = "ipc", feature = "process", feature = "elevated"))]
pub(crate) mod sid;
#[cfg(feature = "signal")]
pub(crate) mod signal;

/// The Windows host.
#[cfg(feature = "host")]
#[derive(Debug)]
pub(crate) struct Host {
    home: home::Home,
    access: access::Access,
    // Not a `windows/` module: the Credential Manager is reached through the same crate the other
    // two systems' stores are. See `crate::secrets`.
    secrets: crate::secrets::Secrets,
    env: path::Env,
    ports: ports::Ports,
    port_access: port_access::Ports,
    resolver: resolver::Resolver,
    prompts: prompt::Prompt,
    hosts: crate::hosts::Managed,
}

#[cfg(feature = "host")]
impl Host {
    pub(crate) fn new() -> Self {
        Self {
            home: home::Home,
            access: access::Access::default(),
            secrets: crate::secrets::Secrets,
            env: path::Env::of_this_user(),
            ports: ports::Ports,
            port_access: port_access::Ports,
            resolver: resolver::Resolver,
            prompts: prompt::Prompt,
            hosts: crate::hosts::Managed,
        }
    }
}

#[cfg(feature = "host")]
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

    fn port_access(&self) -> &dyn crate::PortAccess {
        &self.port_access
    }

    fn resolver(&self) -> &dyn crate::ResolverConfig {
        &self.resolver
    }

    fn hosts_file(&self) -> &dyn crate::HostsFile {
        &self.hosts
    }

    fn elevation(&self) -> &dyn crate::Elevation {
        &self.prompts
    }
}
