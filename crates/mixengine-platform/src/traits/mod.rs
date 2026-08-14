//! One file per capability. `Host` bundles them so callers take a single injected dependency.

mod access;
mod home;
mod keyring;
mod path;

pub use access::DirectoryAccess;
pub use home::HomeDirs;
pub use keyring::Keyring;
pub use path::{PathIntegration, PathLocation, PathState};

/// Every OS capability MixEngine needs, in one injectable object.
///
/// The daemon is constructed with an `Arc<dyn Host>` rather than calling free functions, which is
/// what makes the whole system testable: `mock::Host` answers the same questions from memory and
/// records the mutations it was asked for.
///
/// Capabilities arrive one accessor at a time as the roadmap reaches them —
/// `HostsFile`, `TrustStore`, `ResolverConfig`, `Elevation` and the rest are still to come.
pub trait Host: std::fmt::Debug + Send + Sync {
    /// Where this OS wants application data to live.
    fn home_dirs(&self) -> &dyn HomeDirs;

    /// Keeping other local users out of the directories MixEngine owns.
    fn directory_access(&self) -> &dyn DirectoryAccess;

    /// Where a password lives, since nothing MixEngine writes may hold one.
    fn keyring(&self) -> &dyn Keyring;

    /// Where this user's PATH is kept, so `<root>/bin` can go on it and come off again.
    fn path_integration(&self) -> &dyn PathIntegration;
}
