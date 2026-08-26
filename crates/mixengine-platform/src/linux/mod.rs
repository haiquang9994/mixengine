//! Linux implementations of the platform traits.

#[cfg(feature = "elevated")]
pub(crate) mod elevated;
#[cfg(feature = "host")]
mod home;
#[cfg(feature = "host")]
mod limits;
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
// The read half is `host` and the write half is `elevated`, as `port_access` is.
#[cfg(feature = "host")]
pub(crate) mod browsers;
#[cfg(any(feature = "host", feature = "elevated"))]
#[cfg(feature = "host")]
mod reserved;
pub(crate) mod resolver;
pub(crate) mod trust;
// Reading one `keyring` failure, which `crate::secrets` cannot do for all three systems at once —
// see the module itself for why the one capability with a single implementation still needs this
// per OS.
#[cfg(feature = "host")]
pub(crate) mod secrets;

// File modes are POSIX, not Linux: `macos/` builds on the same implementation, wrapping it with the
// ACL handling that only its ACLs need.
#[cfg(feature = "host")]
use crate::unix::{access, path};

/// The profiles a Linux login reads, in the order somebody looking for them would.
///
/// `~/.profile` first and deliberately: a graphical session on Linux is started by a display
/// manager that sources it, so what it sets is inherited by every terminal window afterwards —
/// including the ones running non-login shells, which read neither of the other two. `.bash_profile`
/// is here because bash reads it *instead* of `~/.profile` when it exists, which would otherwise
/// make a home that has one the one home where this quietly does nothing.
///
/// **`~/.bashrc` and `~/.zshrc` are not on the list.** They are read by every interactive shell
/// rather than once per session, and a `PATH` set there is one that grows down a pipeline of nested
/// shells — the guard in the block makes that harmless, but the file it belongs in is still the
/// profile.
#[cfg(feature = "host")]
const PROFILES: &[&str] = &[".profile", ".bash_profile", ".zprofile"];

/// The one to create when a home has none, which is what a fresh container looks like.
#[cfg(feature = "host")]
const FALLBACK: &str = ".profile";

// The local endpoint is POSIX end to end — a Unix socket, `SO_PEERCRED` behind tokio's `peer_cred`
// — so unlike `access` there is nothing here for this OS to wrap. The same holds for `flock` and
// the two signals, which are BSD's or POSIX's and identical on both systems. Re-exported rather
// than imported because `crate::ipc` reaches them as `sys::ipc` and so on. Starting a process is
// the one that is *not* purely POSIX — `PR_SET_PDEATHSIG` is this system's alone — so `process`
// above is a module here that adds to `unix/` rather than a re-export of it.
#[cfg(any(feature = "host", feature = "elevated"))]
pub(crate) use crate::unix::hosts;
#[cfg(feature = "ipc")]
pub(crate) use crate::unix::ipc;
// A file mode is POSIX, so this is `unix/`'s and not this system's — the split `access` makes in
// the other direction, where one OS wraps shared code.
pub(crate) use crate::unix::lock;
#[cfg(feature = "host")]
pub(crate) use crate::unix::private_file;
#[cfg(feature = "elevated")]
pub(crate) use crate::unix::replace;
#[cfg(feature = "signal")]
pub(crate) use crate::unix::signal;

/// The Linux host.
#[cfg(feature = "host")]
#[derive(Debug)]
pub(crate) struct Host {
    home: home::Home,
    access: access::Access,
    // Not a `linux/` module, and not a `unix/` one either: the secret service is reached through the
    // same crate the other two systems' stores are. See `crate::secrets`.
    secrets: crate::secrets::Secrets,
    profiles: path::Profiles,
    ports: ports::Ports,
    port_access: port_access::Ports,
    reserved: reserved::Reserved,
    limits: limits::Limits,
    resolver: resolver::Resolver,
    trust: trust::Trust,
    browsers: browsers::Browsers,
    prompts: prompt::Prompt,
    hosts: crate::hosts::Managed,
}

#[cfg(feature = "host")]
impl Host {
    pub(crate) fn new() -> Self {
        Self {
            home: home::Home,
            access: access::Access,
            secrets: crate::secrets::Secrets,
            profiles: path::Profiles::of_this_user(PROFILES, FALLBACK),
            ports: ports::Ports,
            port_access: port_access::Ports,
            reserved: reserved::Reserved,
            limits: limits::Limits,
            resolver: resolver::Resolver,
            trust: trust::Trust,
            browsers: browsers::Browsers::of_this_user(),
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
        &self.profiles
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

    fn trust_store(&self) -> &dyn crate::TrustStore {
        &self.trust
    }

    fn browsers(&self) -> &dyn crate::BrowserTrust {
        &self.browsers
    }

    fn reserved_ports(&self) -> &dyn crate::ReservedPorts {
        &self.reserved
    }

    fn resource_control(&self) -> &dyn crate::ResourceControl {
        &self.limits
    }

    fn hosts_file(&self) -> &dyn crate::HostsFile {
        &self.hosts
    }

    fn elevation(&self) -> &dyn crate::Elevation {
        &self.prompts
    }
}
