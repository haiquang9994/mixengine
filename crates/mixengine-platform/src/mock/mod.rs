//! An in-memory host. Always compiled — tests and `--dry-run` both run against it.
//!
//! Tests never touch the real machine (`.claude/standards/testing.md`), so every capability added
//! here answers from memory and, once mutations exist, records what it was asked to do so
//! assertions can be made on the recorded sequence rather than on side effects.

mod access;
mod browsers;
mod connections;
mod elevation;
mod home;
mod hosts;
mod keyring;
mod limits;
mod metrics;
mod path;
mod port_access;
mod ports;
mod reserved;
mod resolver;
mod trust;

use std::path::PathBuf;
use std::time::Duration;

use crate::PortHolder;

pub use elevation::Prompt;
pub use keyring::SecretOp;
pub use path::PathOp;

/// A host that exists only in memory.
///
/// ```
/// use mixengine_platform::{Host as _, mock};
///
/// let host = mock::Host::with_home("/tmp/mixengine-test");
/// assert_eq!(
///     host.home_dirs().default_home().unwrap(),
///     std::path::Path::new("/tmp/mixengine-test")
/// );
/// ```
#[derive(Debug)]
pub struct Host {
    home: home::Home,
    access: access::Access,
    secrets: keyring::Secrets,
    env: path::Env,
    ports: ports::Ports,
    connected: connections::Connections,
    port_access: port_access::Access,

    /// What this machine routes to our DNS server.
    reserved: reserved::Reserved,
    resolver: resolver::Resolver,
    trust: trust::Trust,
    browsers: browsers::Browsers,
    prompts: elevation::Prompts,
    hosts: hosts::Hosts,

    /// What this mock says it will enforce of a service's limits.
    limits: limits::Limits,

    /// What this mock says each supervised group is spending.
    metrics: metrics::Readings,
}

impl Host {
    /// A host whose default root is `home`.
    #[must_use]
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        Self::answering(Some(home.into()))
    }

    /// A host that cannot say where the user's data belongs — the service-account case.
    #[must_use]
    pub fn without_home() -> Self {
        Self::answering(None)
    }

    /// A host whose OS refuses to restrict a directory, with `reason`.
    ///
    /// For the caller's side of [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform):
    /// startup has to fail loudly rather than carry on with a world-readable home.
    #[must_use]
    pub fn refusing_to_restrict(home: impl Into<PathBuf>, reason: &'static str) -> Self {
        Self {
            access: access::Access::refusing(reason),
            ..Self::with_home(home)
        }
    }

    /// A host with no credential store, with `reason`.
    ///
    /// The headless-Linux case: a session with no secret service running. What the caller does about
    /// it is the interesting part — a spec naming a credential cannot be started, and saying so is
    /// better than starting a service with an empty password.
    #[must_use]
    pub fn without_keyring(home: impl Into<PathBuf>, reason: &'static str) -> Self {
        Self {
            secrets: keyring::Secrets::refusing(reason),
            ..Self::with_home(home)
        }
    }

    /// A host whose credential store takes `how_long` to answer a read.
    ///
    /// The locked-keyring case, which is not the missing-keyring one above: a store that is prompting
    /// a user who is not at the machine answers late or never, where a store that is absent answers
    /// at once. Every deadline a caller puts around a keyring read is written against this, and
    /// nothing could reach it before.
    #[must_use]
    pub fn stalling_on_the_keyring(home: impl Into<PathBuf>, how_long: Duration) -> Self {
        Self {
            secrets: keyring::Secrets::stalling(how_long),
            ..Self::with_home(home)
        }
    }

    /// A host where `port` is already being listened on by `holder`.
    ///
    /// The XAMPP case, which is what roadmap task **T38** exists for: a program MixEngine does not
    /// manage, on the port a service was about to bind, with no `services` row to look it up in.
    #[must_use]
    pub fn with_a_port_held(home: impl Into<PathBuf>, port: u16, holder: PortHolder) -> Self {
        Self {
            ports: ports::Ports::holding(port, holder),
            ..Self::with_home(home)
        }
    }

    /// A host that cannot say who is listening on anything, with `reason`.
    ///
    /// **The case every caller of that capability is written around**: the diagnosis is asked for on
    /// an error path, so a machine that cannot answer must leave the failure being diagnosed exactly
    /// as it was rather than turn it into a failure to diagnose.
    #[must_use]
    pub fn unable_to_name_ports(home: impl Into<PathBuf>, reason: &'static str) -> Self {
        Self {
            ports: ports::Ports::refusing(reason),
            ..Self::with_home(home)
        }
    }

    /// A host with connections open, by port.
    ///
    /// What a service that is *being used* looks like, which is the state the idle sweeper has to
    /// not stop. Its opposite is the default: a host nobody named a port on has nothing connected.
    #[must_use]
    pub fn with_connections(
        home: impl Into<PathBuf>,
        open: std::collections::BTreeMap<u16, usize>,
    ) -> Self {
        Self {
            connected: connections::Connections::holding(open),
            ..Self::with_home(home)
        }
    }

    /// A host that cannot count connections at all, with `reason`.
    ///
    /// **Not a host with nothing connected**, and the whole of the sweeper's safety is in that
    /// distinction: a machine with no `lsof` must leave every service running rather than read its
    /// own ignorance as an empty table.
    #[must_use]
    pub fn unable_to_count_connections(home: impl Into<PathBuf>, reason: &'static str) -> Self {
        Self {
            connected: connections::Connections::refusing(reason),
            ..Self::with_home(home)
        }
    }

    /// Change what the next reading of `port` will say.
    ///
    /// A service becomes idle by something *stopping*, so a test of the sweeper needs the answer to
    /// change between two readings of one host rather than between two hosts.
    pub fn set_connections(&self, port: u16, count: usize) {
        self.connected.set(port, count);
    }

    /// Make `pid` measurable, as a process that began at `started`.
    ///
    /// [`set_connections`](Self::set_connections)' reason applies here twice over: what a group is
    /// spending is the reading a test changes between two ticks, and *whether* it can be measured at
    /// all is what a service starting and stopping looks like from the sampler.
    pub fn set_group_reading(
        &self,
        pid: u32,
        started: crate::process::StartTime,
        reading: crate::GroupReading,
    ) {
        self.metrics.set(pid, started, reading);
    }

    /// Stop `pid` being measurable — the process ended.
    ///
    /// **Not a reading of zero.** A subject that cannot be measured has no sample at all, which is
    /// what makes a missing minute mean *nobody measured* rather than *nothing was used*.
    pub fn clear_group_reading(&self, pid: u32) {
        self.metrics.clear(pid);
    }

    /// A host where the person at the machine says no to the prompt.
    ///
    /// **Not the same as [`unable_to_elevate`](Self::unable_to_elevate)**, and the distinction is the
    /// one T40b's degraded mode turns on: a machine that *could* prompt and was refused will accept
    /// the same operation later, and a machine that cannot prompt at all never will.
    #[must_use]
    pub fn declining_elevation(home: impl Into<PathBuf>) -> Self {
        Self {
            prompts: elevation::Prompts::declining(),
            ..Self::with_home(home)
        }
    }

    /// A host with no way to raise a prompt, with `reason`.
    ///
    /// The headless-Linux case for this capability: polkit installed and no authentication agent to
    /// show anything. What matters is that the caller degrades rather than waits.
    #[must_use]
    pub fn unable_to_elevate(home: impl Into<PathBuf>, reason: &str) -> Self {
        Self {
            prompts: elevation::Prompts::refusing(reason),
            ..Self::with_home(home)
        }
    }

    /// A host whose OS will not put anything on the PATH, with `reason`.
    ///
    /// The headless case for this capability: an account with no home directory to write a shell
    /// profile into. What matters is that the caller says so rather than reporting a PATH it did
    /// not change.
    #[must_use]
    pub fn refusing_to_change_the_path(home: impl Into<PathBuf>, reason: &'static str) -> Self {
        Self {
            env: path::Env::refusing(reason),
            ..Self::with_home(home)
        }
    }

    /// Make this mock answer `support` when asked what it enforces.
    ///
    /// **A setter rather than a constructor**, unlike its neighbours here, because the degraded
    /// answers this exists for are per field: a test wants one machine with no `cpu` delegation and
    /// another with a `memory` field this system will never support, and four constructors would be
    /// four names for one value.
    pub fn set_limit_support(&mut self, support: crate::LimitSupport) {
        self.limits.support = support;
    }

    /// A host whose hosts file already holds `lines`.
    ///
    /// The producer's whole question — the T41 design, D11 — is whether the machine already says
    /// what the database says it should, and this is the half a test can set.
    #[must_use]
    pub fn with_hosts<'a>(
        home: impl Into<PathBuf>,
        lines: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        Self {
            hosts: hosts::Hosts::holding(lines),
            ..Self::with_home(home)
        }
    }

    /// A host whose hosts file cannot be read, with `reason`.
    ///
    /// **Not a reason to refuse a site.** The helper is the authority on what is in that file, so a
    /// read that fails is logged and the operation is enqueued anyway — this is the fixture for the
    /// test that says so.
    #[must_use]
    pub fn unable_to_read_the_hosts_file(home: impl Into<PathBuf>, reason: &str) -> Self {
        Self {
            hosts: hosts::Hosts::refusing(reason),
            ..Self::with_home(home)
        }
    }

    /// A host whose machine uses `method` and already has whatever it grants.
    ///
    /// The default is [`PortAccessMethod::Direct`](crate::PortAccessMethod::Direct) and granted,
    /// which is Windows — so a suite that says nothing about ports asks for no prompt, exactly as
    /// every suite written before T42 does.
    #[must_use]
    pub fn with_port_access(home: impl Into<PathBuf>, method: crate::PortAccessMethod) -> Self {
        Self {
            port_access: port_access::Access::granting(method),
            ..Self::with_home(home)
        }
    }

    /// A host whose machine uses `method` and has not been granted it, with `missing` saying why.
    ///
    /// The producer's whole question — the T42 design, D7 — is whether the grant is still there, and
    /// this is the half a test can set. It is also what an update looks like: a capability is
    /// cleared by any write to the binary.
    #[must_use]
    pub fn without_port_access(
        home: impl Into<PathBuf>,
        method: crate::PortAccessMethod,
        missing: &str,
    ) -> Self {
        Self {
            port_access: port_access::Access::withholding(method, missing),
            ..Self::with_home(home)
        }
    }

    /// A host that cannot say whether the grant is there, with `reason`.
    ///
    /// **Not a reason to fail a start.** The probe is asked before the first client, and a daemon
    /// that refused to run because it could not read one attribute would be a worse machine than one
    /// whose front end cannot bind 80 — this is the fixture for the test that says so.
    #[must_use]
    pub fn unable_to_probe_port_access(home: impl Into<PathBuf>, reason: &str) -> Self {
        Self {
            port_access: port_access::Access::refusing(reason),
            ..Self::with_home(home)
        }
    }

    /// A machine that has reserved these ranges.
    ///
    /// The failing branch of `mix doctor`'s reserved-ports check cannot be arranged on a real
    /// machine — what it reads is that machine's own reservations, and no test may change them — so
    /// this is how that branch gets exercised on all three systems.
    #[must_use]
    pub fn with_reserved_ports(home: impl Into<PathBuf>, ranges: &[(u16, u16)]) -> Self {
        Self {
            reserved: reserved::Reserved {
                ranges: ranges
                    .iter()
                    .map(|&(start, end)| crate::PortRange { start, end })
                    .collect(),
            },
            ..Self::with_home(home)
        }
    }

    /// A host whose machine uses `method` and already routes `wired` to our DNS server.
    ///
    /// The default is [`ResolverMethod::None`](crate::ResolverMethod::None) routing nothing —
    /// so a suite that says nothing about names stays in `hosts_only`, exactly as every suite
    /// written before T45 was.
    #[must_use]
    pub fn with_resolver(
        home: impl Into<PathBuf>,
        method: crate::ResolverMethod,
        wired: &[&str],
    ) -> Self {
        Self {
            resolver: resolver::Resolver::routing(method, wired),
            ..Self::with_home(home)
        }
    }

    /// A host that cannot say what it routes, with `reason`.
    ///
    /// **Not a reason to fail a start**, for the reason
    /// [`unable_to_probe_port_access`](Self::unable_to_probe_port_access) is not: the probe runs
    /// before the first client, and a daemon that refused to run because it could not read one
    /// file would be worse than one that stays on the hosts file and says so.
    #[must_use]
    pub fn unable_to_read_resolver(home: impl Into<PathBuf>, reason: &str) -> Self {
        Self {
            resolver: resolver::Resolver::refusing(reason),
            ..Self::with_home(home)
        }
    }

    /// A host whose machine has `method` and does or does not already hold the authority.
    #[must_use]
    pub fn with_trust_store(
        home: impl Into<PathBuf>,
        method: crate::TrustStoreMethod,
        installed: bool,
    ) -> Self {
        Self {
            trust: trust::Trust::holding(method, installed),
            ..Self::with_home(home)
        }
    }

    /// A host whose browsers answer exactly `survey` — roadmap task **T49b**.
    ///
    /// **A whole survey rather than a `bool`**, unlike [`with_trust_store`](Self::with_trust_store)
    /// above, because the states a caller has to tell apart are the point: no tool, not this
    /// system, and N databases of which some hold it are four different screens and three different
    /// `mix doctor` outcomes.
    #[must_use]
    pub fn with_browsers(home: impl Into<PathBuf>, survey: crate::BrowserSurvey) -> Self {
        Self {
            browsers: browsers::Browsers::answering(survey),
            ..Self::with_home(home)
        }
    }

    /// Every certificate this host's browsers were asked to hold — roadmap task **T49b**.
    ///
    /// The producer runs at every daemon start and writes without a prompt, so what a suite can
    /// assert on is what was asked rather than a file it would have to go and read.
    #[must_use]
    pub fn browsers_installed(&self) -> Vec<Vec<u8>> {
        self.browsers.installed()
    }

    /// Every authority this host's browsers were asked to let go of.
    ///
    /// **Nothing in T49b calls the removal** — T54 and T87 are its producers — so this is here for
    /// them rather than for a caller that exists today.
    #[must_use]
    pub fn browsers_removed(&self) -> Vec<String> {
        self.browsers.removed()
    }

    /// A host that cannot say what it trusts, with `reason`.
    ///
    /// **Not a reason to fail a start**, for the reason
    /// [`unable_to_read_resolver`](Self::unable_to_read_resolver) is not: the probe runs on every
    /// start, and one that could not read a store has said nothing about what to ask for.
    #[must_use]
    pub fn unable_to_read_trust_store(home: impl Into<PathBuf>, reason: &str) -> Self {
        Self {
            trust: trust::Trust::refusing(reason),
            ..Self::with_home(home)
        }
    }

    /// The one place every constructor above starts from, so a capability added here is added to
    /// all of them rather than to whichever four somebody remembered.
    fn answering(home: Option<PathBuf>) -> Self {
        Self {
            home: home::Home::answering(home),
            access: access::Access::recording(),
            secrets: keyring::Secrets::remembering(),
            env: path::Env::recording(),
            ports: ports::Ports::default(),
            connected: connections::Connections::default(),
            port_access: port_access::Access::default(),
            reserved: reserved::Reserved::default(),
            resolver: resolver::Resolver::default(),
            trust: trust::Trust::default(),
            browsers: browsers::Browsers::default(),
            prompts: elevation::Prompts::accepting(),
            hosts: hosts::Hosts::default(),
            limits: limits::Limits::default(),
            metrics: metrics::Readings::default(),
        }
    }

    /// Every path [`DirectoryAccess::restrict_to_owner`](crate::DirectoryAccess::restrict_to_owner)
    /// was called with, in order.
    #[must_use]
    pub fn restricted(&self) -> Vec<PathBuf> {
        self.access.restricted()
    }

    /// Every credential this host was asked to store or forget, in order.
    ///
    /// Reads are absent on purpose, and so are the values: see [`SecretOp`].
    #[must_use]
    pub fn secret_operations(&self) -> Vec<SecretOp> {
        self.secrets.operations()
    }

    /// Every directory this host was asked to put on the PATH or take off it, in order.
    ///
    /// Reads are absent for [`SecretOp`]'s reason: what a test has to be able to see is the
    /// mutations, and a `state` that changed nothing is not one.
    #[must_use]
    pub fn path_operations(&self) -> Vec<PathOp> {
        self.env.operations()
    }

    /// Every prompt this host was asked to raise, in order.
    ///
    /// Both paths of each, unlike [`SecretOp`]: there is no secret in a path, and the pair is what an
    /// assertion about a batched prompt is made of.
    #[must_use]
    pub fn prompts_raised(&self) -> Vec<Prompt> {
        self.prompts.raised()
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

    fn process_metrics(&self) -> &dyn crate::ProcessMetrics {
        &self.metrics
    }

    fn port_owner(&self) -> &dyn crate::PortOwner {
        &self.ports
    }

    fn connections(&self) -> &dyn crate::ConnectionCount {
        &self.connected
    }

    fn elevation(&self) -> &dyn crate::Elevation {
        &self.prompts
    }

    fn hosts_file(&self) -> &dyn crate::HostsFile {
        &self.hosts
    }
}
