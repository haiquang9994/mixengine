//! One file per capability. `Host` bundles them so callers take a single injected dependency.

mod access;
mod elevation;
mod home;
mod hosts;
mod keyring;
mod path;
mod ports;

pub use access::DirectoryAccess;
pub use elevation::{Elevation, ElevationSupport};
pub use home::HomeDirs;
pub use hosts::HostsFile;
pub use keyring::{KEYRING_SERVICE, Keyring};
pub use path::{PathIntegration, PathLocation, PathState};
pub use ports::{PortHolder, PortOwner};

/// Every OS capability MixEngine needs, in one injectable object.
///
/// The daemon is constructed with an `Arc<dyn Host>` rather than calling free functions, which is
/// what makes the whole system testable: `mock::Host` answers the same questions from memory and
/// records the mutations it was asked for.
///
/// Capabilities arrive one accessor at a time as the roadmap reaches them —
/// `TrustStore`, `ResolverConfig` and the rest are still to come.
pub trait Host: std::fmt::Debug + Send + Sync {
    /// Where this OS wants application data to live.
    fn home_dirs(&self) -> &dyn HomeDirs;

    /// Keeping other local users out of the directories MixEngine owns.
    fn directory_access(&self) -> &dyn DirectoryAccess;

    /// Where a password lives, since nothing MixEngine writes may hold one.
    fn keyring(&self) -> &dyn Keyring;

    /// Where this user's PATH is kept, so `<root>/bin` can go on it and come off again.
    fn path_integration(&self) -> &dyn PathIntegration;

    /// Who is already listening on a port a service is about to want.
    fn port_owner(&self) -> &dyn PortOwner;

    /// Raising the OS elevation prompt on the one-shot helper.
    fn elevation(&self) -> &dyn Elevation;

    /// What the machine's hosts file currently says MixEngine put in it.
    ///
    /// Reading only: the write needs a token this process does not have — see [`HostsFile`].
    fn hosts_file(&self) -> &dyn HostsFile;
}
