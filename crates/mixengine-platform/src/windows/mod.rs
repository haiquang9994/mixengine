//! Windows implementations of the platform traits.

#[cfg(feature = "host")]
mod access;
#[cfg(feature = "ipc")]
pub(crate) mod activation;
// What this machine will refuse to load, for want of a signature — T94.
#[cfg(feature = "host")]
mod app_control;
// The daemon's autostart entry, as a Task Scheduler logon task — T85b.
#[cfg(feature = "host")]
mod autostart;
// Running a Windows tool as an argument vector, which both `access` (behind `host`) and `elevated`
// need — so it sits here rather than inside either of them.
#[cfg(any(feature = "host", feature = "elevated"))]
pub(crate) mod command;
// Letting go of a console Task Scheduler handed this process — T85b. Behind `process`, whose
// public function it answers.
#[cfg(feature = "process")]
pub(crate) mod console;
// Under both features since T85: the daemon reads who owns a file before it runs one as root, and
// the writing half — the elevation bit, the audit directory, the root-owned `mkdir` — is still the
// helper's alone. Every item inside carries its own gate.
#[cfg(any(feature = "host", feature = "elevated"))]
pub(crate) mod elevated;
pub(crate) mod fullname;
#[cfg(feature = "host")]
mod home;
#[cfg(any(feature = "host", feature = "elevated"))]
pub(crate) mod hosts;
// Where the privileged helper is installed — T85. Under both features because both sides ask it.
#[cfg(any(feature = "host", feature = "elevated"))]
pub(crate) mod install;
#[cfg(feature = "ipc")]
pub(crate) mod ipc;
// A system directory read from the shell rather than from an environment variable this process
// cannot show it chose. Under both features because `install` and `elevated` each have a caller.
#[cfg(any(feature = "host", feature = "elevated"))]
pub(crate) mod known_folder;
#[cfg(feature = "host")]
mod limits;
pub(crate) mod lock;
#[cfg(feature = "host")]
mod path;
// The read half is `host` and the write half is `elevated`, so the module is declared for
// both and every item inside it carries its own gate.
#[cfg(any(feature = "host", feature = "elevated"))]
pub(crate) mod port_access;
#[cfg(feature = "host")]
mod ports;
// Writing a file only this account may read. Its own rather than `unix/`'s, and not a reuse of
// `access` either: the inherit flags that method grants are directory-only.
#[cfg(feature = "host")]
pub(crate) mod private_file;
#[cfg(feature = "process")]
pub(crate) mod process;
#[cfg(feature = "host")]
mod prompt;
#[cfg(feature = "elevated")]
pub(crate) mod replace;
// The read half is `host` and the write half is `elevated`, as `port_access` is.
#[cfg(feature = "elevated")]
pub(crate) mod firewall;
#[cfg(feature = "host")]
mod firewall_rules;
#[cfg(any(feature = "host", feature = "elevated"))]
#[cfg(feature = "host")]
mod reserved;
pub(crate) mod resolver;
// Every unsafe call T49a makes, in one file — see its header.
#[cfg(feature = "host")]
mod browsers;
// Finding an installed desktop application in the registry — T83.
#[cfg(feature = "host")]
mod desktop;
#[cfg(any(feature = "host", feature = "elevated"))]
mod store;
pub(crate) mod trust;
// Reading one `keyring` failure, which `crate::secrets` cannot do for all three systems at once —
// see the module itself for why the one capability with a single implementation still needs this
// per OS.
#[cfg(feature = "process")]
mod restricted;
#[cfg(feature = "host")]
pub(crate) mod secrets;
// SIDs are read by the pipe's peer check (`ipc`), by the restricted token (`process`) and by the
// owner of a file (`elevated`) — so the module belongs to none of them and is gated by all three.
#[cfg(any(
    feature = "ipc",
    feature = "process",
    feature = "host",
    feature = "elevated"
))]
pub(crate) mod sid;
#[cfg(feature = "signal")]
pub(crate) mod signal;

/// The Windows host.
#[cfg(feature = "host")]
#[derive(Debug)]
pub(crate) struct Host {
    home: home::Home,
    access: access::Access,
    autostart: autostart::Logon,
    // Not a `windows/` module: the Credential Manager is reached through the same crate the other
    // two systems' stores are. See `crate::secrets`.
    secrets: crate::secrets::Secrets,
    env: path::Env,
    ports: ports::Ports,
    port_access: port_access::Ports,
    reserved: reserved::Reserved,
    app_control: app_control::Policy,
    network: crate::network::Network,
    firewall_rules: firewall_rules::Rules,
    limits: limits::Limits,
    metrics: crate::metrics::Sampler,
    resolver: resolver::Resolver,
    trust: trust::Trust,
    browsers: browsers::Browsers,
    prompts: prompt::Prompt,
    hosts: crate::hosts::Managed,
    desktop: desktop::Apps,
}

#[cfg(feature = "host")]
impl Host {
    pub(crate) fn new() -> Self {
        Self {
            home: home::Home,
            access: access::Access::default(),
            autostart: autostart::Logon::of_this_user(),
            secrets: crate::secrets::Secrets,
            env: path::Env::of_this_user(),
            ports: ports::Ports,
            port_access: port_access::Ports,
            reserved: reserved::Reserved,
            app_control: app_control::Policy,
            network: crate::network::Network,
            firewall_rules: firewall_rules::Rules,
            limits: limits::Limits,
            metrics: crate::metrics::Sampler::default(),
            resolver: resolver::Resolver,
            trust: trust::Trust,
            browsers: browsers::Browsers,
            prompts: prompt::Prompt,
            hosts: crate::hosts::Managed,
            desktop: desktop::Apps::of_this_machine(),
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

    fn service_installer(&self) -> &dyn crate::ServiceInstaller {
        &self.autostart
    }

    fn port_owner(&self) -> &dyn crate::PortOwner {
        &self.ports
    }

    fn connections(&self) -> &dyn crate::ConnectionCount {
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

    fn app_control(&self) -> &dyn crate::AppControl {
        &self.app_control
    }

    fn network(&self) -> &dyn crate::NetworkInfo {
        &self.network
    }

    fn firewall_rules(&self) -> &dyn crate::FirewallRules {
        &self.firewall_rules
    }

    fn process_metrics(&self) -> &dyn crate::ProcessMetrics {
        &self.metrics
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

    fn desktop_apps(&self) -> &dyn crate::DesktopApps {
        &self.desktop
    }
}
