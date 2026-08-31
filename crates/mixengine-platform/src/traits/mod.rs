//! One file per capability. `Host` bundles them so callers take a single injected dependency.

mod access;
mod browsers;
mod connections;
mod elevation;
mod firewall_rules;
mod home;
mod hosts;
mod keyring;
mod limits;
mod metrics;
mod network;
mod orphans;
mod path;
mod port_access;
mod ports;
mod reserved;
mod resolver;
mod trust;

pub use access::DirectoryAccess;
pub use browsers::{BrowserChange, BrowserSurvey, BrowserTrust, DatabaseState};
pub use connections::ConnectionCount;
pub use elevation::{Elevation, ElevationSupport};
pub use firewall_rules::FirewallRules;
pub use home::HomeDirs;
pub use hosts::HostsFile;
pub use keyring::{KEYRING_SERVICE, Keyring};
pub use limits::{
    Enforcement, LimitMechanism, LimitSupport, MemoryMeasure, ResourceControl, WhenExceeded,
};
pub use metrics::{GroupReading, GroupRoot, ProcessMetrics};
pub use network::{Interface, NetworkInfo, choose as choose_interface};
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
/// Capabilities arrive one accessor at a time as the roadmap reaches them.
///
/// **The firewall is here to be read and never to be written.** Opening a port needs a token the
/// daemon does not have, so writing stays a
/// [`FirewallApply`](mixengine_proto::privileged::PrivilegedOp::FirewallApply) enqueued the way the
/// resolver's changes are and executed by `mixengine-elevate` out of [`crate::firewall`]; no
/// accessor below leads to it.
///
/// Reading arrived with T76, and the reason is that the interesting rule is not one of ours. Binding
/// UDP 5353 for mDNS makes Windows offer to write an every-port rule for `mixengined.exe`, and a
/// daemon cannot learn about a rule it never made from its own database. Reading needs no privilege
/// and changes nothing — see [`FirewallRules`].
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

    /// How busy a port is, which is how a service is found to have nothing to do.
    fn connections(&self) -> &dyn ConnectionCount;

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

    /// What each supervised process group is spending right now — roadmap task **T71**.
    ///
    /// **Reads only**, as [`resource_control`](Self::resource_control) does and beside it for the
    /// contrast: that one answers what this machine will enforce, once; this one is asked on a timer
    /// for as long as the daemon runs. See [`ProcessMetrics`].
    fn process_metrics(&self) -> &dyn ProcessMetrics;

    /// Which of this machine's networks a site could be shared on — roadmap task **T74**.
    ///
    /// **Reads only**, like [`reserved_ports`](Self::reserved_ports) beside it: sharing binds a
    /// listener this daemon already owns, so nothing here changes the machine.
    fn network(&self) -> &dyn NetworkInfo;

    /// What this system has taken out of circulation — roadmap task **T47a**.
    ///
    /// The third of three port capabilities, and the one about the operating system rather than
    /// about another program or about privilege — see [`ReservedPorts`].
    fn reserved_ports(&self) -> &dyn ReservedPorts;

    /// Inbound firewall rules naming a program — roadmap task **T76**.
    ///
    /// **Reads only**, like [`network`](Self::network) and [`reserved_ports`](Self::reserved_ports)
    /// beside it, and the one direction of the firewall a daemon is allowed. See [`FirewallRules`].
    fn firewall_rules(&self) -> &dyn FirewallRules;
}
