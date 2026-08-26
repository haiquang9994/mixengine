//! One file per capability. `Host` bundles them so callers take a single injected dependency.

mod access;
mod browsers;
mod elevation;
mod home;
mod hosts;
mod keyring;
mod limits;
mod orphans;
mod path;
mod port_access;
mod ports;
mod reserved;
mod resolver;
mod trust;

pub use access::DirectoryAccess;
pub use browsers::{BrowserChange, BrowserSurvey, BrowserTrust, DatabaseState};
pub use elevation::{Elevation, ElevationSupport};
pub use home::HomeDirs;
pub use hosts::HostsFile;
pub use keyring::{KEYRING_SERVICE, Keyring};
pub use limits::{
    Enforcement, LimitMechanism, LimitSupport, MemoryMeasure, ResourceControl, WhenExceeded,
};
pub use orphans::{OrphanGuarantee, orphan_guarantee};
pub use path::{PathIntegration, PathLocation, PathState};
pub use port_access::{PortAccess, PortAccessMethod, PortAccessState, PortBinding};
pub use ports::{PortHolder, PortOwner};
pub use reserved::{PortRange, ReservedPorts};
pub use resolver::{ResolverConfig, ResolverMethod, ResolverState};
pub use trust::{TrustState, TrustStore, TrustStoreMethod};

/// Every OS capability MixEngine needs, in one injectable object.
///
/// The daemon is constructed with an `Arc<dyn Host>` rather than calling free functions, which is
/// what makes the whole system testable: `mock::Host` answers the same questions from memory and
/// records the mutations it was asked for.
///
/// Capabilities arrive one accessor at a time as the roadmap reaches them, and the firewall is
/// still to come.
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

    /// Whether this machine will let an unprivileged front end answer on 80 and 443.
    ///
    /// Reading only: the grant needs a token this process does not have — see [`PortAccess`].
    fn port_access(&self) -> &dyn PortAccess;

    /// Raising the OS elevation prompt on the one-shot helper.
    fn elevation(&self) -> &dyn Elevation;

    /// What the machine's hosts file currently says MixEngine put in it.
    ///
    /// Reading only: the write needs a token this process does not have — see [`HostsFile`].
    fn hosts_file(&self) -> &dyn HostsFile;

    /// Whether a managed TLD arrives at this daemon's own DNS server.
    ///
    /// Reading only: the wiring needs a token this process does not have — see
    /// [`ResolverConfig`].
    fn resolver(&self) -> &dyn ResolverConfig;

    /// Whether this machine trusts MixEngine's own certificate authority.
    ///
    /// **Reads only**, as [`resolver`](Self::resolver) does and for its reason: the write needs a
    /// token the daemon does not have, and belongs to `mixengine-elevate` — roadmap task **T49a**.
    fn trust_store(&self) -> &dyn TrustStore;

    /// Whether Firefox and Chrome trust that same authority — roadmap task **T49b**.
    ///
    /// **Beside [`trust_store`](Self::trust_store) rather than inside it**: browsers on Linux read
    /// NSS databases and not the system store at all, there are N of them, and one `bool` cannot
    /// answer for both. Unlike its neighbour this one **writes as well as reads**, because these
    /// databases belong to the user and no token is needed for them.
    fn browsers(&self) -> &dyn BrowserTrust;

    /// What this machine will enforce of a service's declared limits — roadmap task **T68**.
    ///
    /// **Reads only**, as [`port_access`](Self::port_access) and [`resolver`](Self::resolver) do:
    /// applying a limit is done to a child through [`process`](crate::process), not asked of the
    /// machine. See [`ResourceControl`].
    fn resource_control(&self) -> &dyn ResourceControl;

    /// What this system has taken out of circulation — roadmap task **T47a**.
    ///
    /// The third of three port capabilities, and the one about the operating system rather than
    /// about another program or about privilege — see [`ReservedPorts`].
    fn reserved_ports(&self) -> &dyn ReservedPorts;
}
