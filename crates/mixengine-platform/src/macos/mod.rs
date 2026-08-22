//! macOS implementations of the platform traits.

#[cfg(feature = "host")]
mod access;
#[cfg(feature = "elevated")]
pub(crate) mod elevated;
#[cfg(feature = "host")]
mod home;
#[cfg(feature = "host")]
mod ports;
#[cfg(feature = "process")]
pub(crate) mod process;
#[cfg(feature = "host")]
mod prompt;

// The local endpoint is POSIX end to end — a Unix socket, `LOCAL_PEERCRED` behind tokio's
// `peer_cred` — so unlike `access` there is nothing here for this OS to wrap. The same holds for
// `flock` and the two signals, which are BSD's or POSIX's and identical on both systems.
// Re-exported rather than imported because `crate::ipc` reaches them as `sys::ipc` and so on.
// Starting a process is the one that is *not* purely POSIX, and this system's `process` module is
// mostly there to record what it consequently cannot promise.
#[cfg(feature = "ipc")]
pub(crate) use crate::unix::ipc;
pub(crate) use crate::unix::lock;
#[cfg(feature = "host")]
pub(crate) use crate::unix::path;
#[cfg(feature = "signal")]
pub(crate) use crate::unix::signal;

/// The profiles a macOS login reads, in the order somebody looking for them would.
///
/// `~/.zprofile` first, because zsh has been the default shell since Catalina and because
/// Terminal.app and iTerm both start a **login** shell for every window — so unlike Linux, the
/// login profile is read on each new tab rather than once per session, and it is the only file that
/// needs writing for a terminal opened five minutes from now to find `php`.
///
/// **`/etc/paths.d` is not used**, although it is the mechanism macOS documents for exactly this. It
/// is owned by root, so a drop-in there is the one PATH change on any of the three systems that
/// would need `mixengine-elevate` — for a directory belonging to one user, in a product whose rule
/// is that elevation is one-shot and asked for. A per-user profile does the same job with nobody
/// typing a password.
#[cfg(feature = "host")]
const PROFILES: &[&str] = &[".zprofile", ".bash_profile", ".profile"];

/// The one to create when a home has none of them.
#[cfg(feature = "host")]
const FALLBACK: &str = ".zprofile";

/// The macOS host.
#[cfg(feature = "host")]
#[derive(Debug)]
pub(crate) struct Host {
    home: home::Home,
    access: access::Access,
    // Not a `macos/` module: the login Keychain is reached through the same crate the other two
    // systems' stores are. See `crate::secrets`.
    secrets: crate::secrets::Secrets,
    profiles: path::Profiles,
    ports: ports::Ports,
    prompts: prompt::Prompt,
}

#[cfg(feature = "host")]
impl Host {
    pub(crate) fn new() -> Self {
        Self {
            home: home::Home,
            access: access::Access::default(),
            secrets: crate::secrets::Secrets,
            profiles: path::Profiles::of_this_user(PROFILES, FALLBACK),
            ports: ports::Ports,
            prompts: prompt::Prompt,
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

    fn elevation(&self) -> &dyn crate::Elevation {
        &self.prompts
    }
}
