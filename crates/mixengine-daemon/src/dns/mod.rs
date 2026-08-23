//! The built-in DNS server, and which of this home's two name mechanisms is running — roadmap task
//! **T44**, which also closes **T46a**.
//!
//! `site.create` must prompt for nothing. Writing a hosts entry per domain costs an elevation
//! prompt per site, which is the repeated cost [ADR
//! 0005](../../../../.claude/decisions/0005-on-demand-elevation.md) says this product may not pay;
//! a server that answers `*.test` by pattern pays it once, at first-run setup, and never again
//! however many sites there are.
//!
//! Three modules: [`answer`] is the policy as a pure function, [`server`] is the two sockets and
//! the task on them, and this one is the mode — which mechanism this home is actually running on,
//! and why.
//!
//! # The mode has two terms, and both of them are now real
//!
//! The T44 design, D4. "Running on DNS" is not "the server is listening". It is "the server is
//! listening **and** something is routing a managed TLD to it" — and T44 could only ever produce
//! the first, so it hard-coded the second to "not wired" and every home stayed on its hosts file.
//!
//! **T45 is the producer of the second term**, through
//! [`ResolverConfig`](mixengine_platform::ResolverConfig): the machine is asked, on every start,
//! which of the managed TLDs it already sends here. So the branch T44 wrote and could only reach
//! from a test is the branch this build first runs for real.
//!
//! # And it is per TLD, not per home
//!
//! The T45 design, D6. Every mechanism measured scopes to **one TLD**, and `.local` is never wired
//! at all — so [`Dns::wired`] is a list rather than a flag, and a home can answer `*.blog.test` by
//! pattern while still needing a hosts line for `shop.local`. [`DnsMode`] survives as the one-word
//! summary a person reads; it is no longer what the hosts block is computed from.

mod answer;
mod server;

use std::net::SocketAddr;

use std::sync::Mutex;

use mixengine_core::config;
use mixengine_platform::{Host, PortHolder};
use mixengine_proto::domains::WIRED_TLDS;
use mixengine_proto::{DnsMode, DnsStatus};
use tokio_util::sync::CancellationToken;

/// The port this server listens on when `config.toml` names none.
///
/// **53 on Windows and 53535 everywhere else** — the T44 design, D2. The split is not about
/// privilege but about what can be *wired*: Windows' NRPT rule names a namespace and a nameserver
/// address with no way to state a port, so on Windows it has to be 53 — which Windows lets an
/// unprivileged process bind, having no privileged-port concept at all. `/etc/resolver`,
/// `resolvectl` and dnsmasq can all state a port, so on macOS and Linux the number is free.
///
/// **And it is deliberately not 5353**, which is mDNS's: `mDNSResponder` on macOS and
/// `avahi-daemon` on Linux hold it on every ordinary desktop, so choosing it would mean the
/// hosts-only branch is the only branch that ever runs.
///
/// `cfg!` rather than `#[cfg]` so both arms compile on all three systems.
const DEFAULT_PORT: u16 = if cfg!(windows) { 53 } else { 53_535 };

/// What the server is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    /// Bound and answering, on this address.
    Listening(SocketAddr),

    /// `[dns] enabled = false`. Nothing was bound, and nothing went wrong.
    Disabled,

    /// The bind failed, with the best sentence this machine could give for why.
    Unavailable {
        /// Written for a person, not for a client to translate — see [`DnsStatus::because`].
        because: String,
    },
}

/// The DNS server this daemon runs, and the mode it puts this home in.
#[derive(Debug)]
pub(crate) struct Dns {
    /// What the server is doing.
    state: State,

    /// Whether a resolver could be wired to this server at all.
    ///
    /// **`[dns] port = 0` asks the operating system for a port, and a port that changes on every
    /// start is a port no resolver can be pointed at** — `config::Dns::port` says so in as many
    /// words, and every test home in this workspace uses it so that no suite takes 53 off the
    /// machine running it. A daemon on one must therefore ask for **nothing**: an elevated
    /// operation that wires a resolver to a number this process will not have again is a prompt
    /// spent to break name resolution until the next restart.
    wirable: bool,

    /// Which managed TLDs this machine routes here, as of the last probe.
    ///
    /// **Behind a lock because it changes while the daemon runs** — the T45 design, D8. A grant
    /// wires the machine, and a daemon that only learned that at its next start would go on writing
    /// hosts entries while the user watched their permission do nothing. [`Dns::reprobe`] is what
    /// replaces it, and the grant is what calls that.
    wired: Mutex<Vec<String>>,
}

impl Dns {
    /// Bind and start answering, or work out what to say instead.
    ///
    /// **This never fails.** A bind that did not work is a mode and a sentence, not a refusal to
    /// start — the T44 design, D6, which is T40b's D10 one capability along: refusing to start
    /// leaves a user with no daemon at all, which is strictly worse than a daemon that says which
    /// of its two name mechanisms it is running on.
    pub(crate) async fn start(
        config: &config::Dns,
        host: &dyn Host,
        shutdown: CancellationToken,
    ) -> Self {
        if !config.enabled {
            return Self {
                state: State::Disabled,
                wirable: false,
                wired: Mutex::new(Vec::new()),
            };
        }

        let port = config.port.unwrap_or(DEFAULT_PORT);

        let state = match server::start(port, shutdown).await {
            Ok(address) => {
                tracing::info!(%address, "the DNS server is answering for the managed TLDs");
                State::Listening(address)
            }
            Err(error) => {
                let because = unavailable(host, port, &error);
                tracing::warn!(%because, "this home is running on the hosts file");
                State::Unavailable { because }
            }
        };

        let dns = Self {
            state,
            // `Some(0)` is the ephemeral request; `None` is this system's real default, which is
            // fixed and therefore wirable.
            wirable: config.port != Some(0),
            wired: Mutex::new(Vec::new()),
        };
        dns.reprobe(host);

        dns
    }

    /// Ask this machine again which managed TLDs it sends here.
    ///
    /// **Called at start and after every grant** — the T45 design, D8. A probe that fails leaves the
    /// answer empty and says so in the log: reading the wiring is documented as something every
    /// caller may treat as "no answer", and a daemon that refused to serve because it could not read
    /// one file would be strictly worse than one that stays on the hosts file and reports why.
    pub(crate) fn reprobe(&self, host: &dyn Host) {
        let Some(port) = self.wirable_port() else {
            return;
        };

        let wired = match host.resolver().probe(&WIRED_TLDS, port) {
            Ok(state) => state.wired,
            Err(error) => {
                tracing::warn!(%error, "this machine's resolver cannot be read; assuming it routes nothing here");
                Vec::new()
            }
        };

        *self
            .wired
            .lock()
            .expect("the wiring is not held across an await") = wired;
    }

    /// The port the server is answering on, or [`None`] when it is not answering.
    fn port(&self) -> Option<u16> {
        match &self.state {
            State::Listening(address) => Some(address.port()),
            State::Disabled | State::Unavailable { .. } => None,
        }
    }

    /// The port a resolver could be pointed at, or [`None`] when there is not one.
    ///
    /// Two different questions from [`port`](Self::port), and the difference is the whole of
    /// `wirable`: a server on an operating-system-chosen port *is* answering, and is still not
    /// something anything may be wired to.
    pub(crate) fn wirable_port(&self) -> Option<u16> {
        self.wirable.then(|| self.port()).flatten()
    }

    /// Where the server is listening, for something that wants to ask it a question.
    ///
    /// **A different question from [`Dns::wirable_port`]**, which answers "may a resolver be pointed
    /// here" and says no on an operating-system-chosen port. A diagnostic may ask a server on any
    /// port at all; what it must not do is ask one that is not there.
    pub(crate) fn address(&self) -> Option<SocketAddr> {
        match self.state {
            State::Listening(address) => Some(address),
            State::Disabled | State::Unavailable { .. } => None,
        }
    }

    /// The managed TLDs this machine routes here.
    pub(crate) fn wired(&self) -> Vec<String> {
        self.wired
            .lock()
            .expect("the wiring is not held across an await")
            .clone()
    }

    /// Which of the two name mechanisms this home is running on.
    ///
    /// **Both terms, and neither alone.** A server that is listening while nothing routes a name to
    /// it resolves exactly as many names as no server at all, and a resolver wired to a port
    /// nothing answers on resolves fewer.
    pub(crate) fn mode(&self) -> DnsMode {
        match &self.state {
            State::Listening(_) if !self.wired().is_empty() => DnsMode::Dns,
            State::Listening(_) | State::Disabled | State::Unavailable { .. } => DnsMode::HostsOnly,
        }
    }

    /// What `daemon.status` reports about names on this machine.
    pub(crate) fn status(&self) -> DnsStatus {
        let mode = self.mode();

        DnsStatus {
            mode,
            listening: match &self.state {
                State::Listening(address) => Some(address.to_string()),
                State::Disabled | State::Unavailable { .. } => None,
            },
            // Wildcards are the thing DNS has and a hosts file does not: one line per name and no
            // patterns, so `blog.test` works and `api.blog.test` does not. The API says so rather
            // than leaving a client to infer it — the T44 design, D9 — and says it per TLD, because
            // from T45 on that is the shape of the answer (D6).
            wildcards: self.wired(),
            because: self.because(),
        }
    }

    /// Why this home is not running on DNS, or [`None`] when it is.
    fn because(&self) -> Option<String> {
        match &self.state {
            State::Disabled => Some("[dns] enabled = false in config.toml".to_owned()),
            State::Unavailable { because } => Some(because.clone()),
            State::Listening(_) if !self.wired().is_empty() => None,
            State::Listening(address) => Some(format!(
                "nothing on this machine routes a managed TLD to {address} yet"
            )),
        }
    }
}

/// The two modes, without a socket, for the tests of everything that reads one.
///
/// `require_hosts` branches on [`Dns::mode`] and both of its branches have to be exercised — the
/// T44 design, D4 — and neither of them is about binding a port.
#[cfg(test)]
impl Dns {
    /// A home resolving through its hosts file: nothing bound, nothing wired.
    pub(crate) fn hosts_only_for_tests() -> Self {
        Self {
            state: State::Disabled,
            wirable: true,
            wired: Mutex::new(Vec::new()),
        }
    }

    /// A home resolving through DNS: listening, and routing every TLD that may be routed.
    pub(crate) fn wired_for_tests() -> Self {
        Self::listening_for_tests(&WIRED_TLDS)
    }

    /// A listening server that routes exactly `wired` — the shape D6 made necessary, since a home
    /// can now route some of its TLDs and not others.
    pub(crate) fn listening_for_tests(wired: &[&str]) -> Self {
        Self {
            state: State::Listening("127.0.0.1:53535".parse().expect("an address")),
            wirable: true,
            wired: Mutex::new(wired.iter().map(|tld| (*tld).to_owned()).collect()),
        }
    }

    /// A server the operating system chose the port for, which nothing may be wired to.
    pub(crate) fn on_an_ephemeral_port_for_tests() -> Self {
        Self {
            state: State::Listening("127.0.0.1:51234".parse().expect("an address")),
            wirable: false,
            wired: Mutex::new(Vec::new()),
        }
    }
}

/// The best sentence this machine can give for a bind that did not work.
///
/// **Asked only after the bind failed**, never before it — the T44 design, D6. A probe first would
/// be a race, and [`mixengine_platform::PortOwner`] is documented as something every caller must be
/// able to treat as "no diagnosis": a failure to explain must not become the failure being
/// explained.
fn unavailable(host: &dyn Host, port: u16, error: &std::io::Error) -> String {
    match host.port_owner().listening_on(port).ok().flatten() {
        Some(PortHolder {
            name: Some(name), ..
        }) => format!("port {port} is held by {name}"),
        Some(PortHolder { pid: Some(pid), .. }) => format!("port {port} is held by process {pid}"),
        Some(PortHolder { .. }) => {
            format!("port {port} is held by another program on this machine")
        }
        // Nobody is listening on TCP and the bind failed anyway. That is a UDP-only holder — which
        // `PortOwner` cannot see (D6) — or a Windows excluded port range, which T47 owns detecting.
        // The operating system's own words are the best thing left to say.
        None => format!("port {port} could not be bound: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use mixengine_platform::mock;

    use super::*;

    /// The home every mock here is given. Nothing in this module touches it.
    const HOME: &str = "/mixengine";

    fn listening(wired: &[&str]) -> Dns {
        Dns::listening_for_tests(wired)
    }

    /// The row the whole of T44 lives on: the server can be up and this home is still on the hosts
    /// file, because nothing sends it a name. Deleting this test is how somebody would break D4.
    #[test]
    fn a_listening_server_nothing_is_wired_to_is_still_hosts_only() {
        let dns = listening(&[]);
        let status = dns.status();

        assert_eq!(status.mode, DnsMode::HostsOnly);
        assert_eq!(status.listening.as_deref(), Some("127.0.0.1:53535"));
        assert!(status.wildcards.is_empty());
        assert!(
            status
                .because
                .as_deref()
                .is_some_and(|because| because.contains("routes a managed TLD")),
            "{status:?}"
        );
    }

    /// The other side of the same row, and T45 is what first reaches it outside a test: with both
    /// terms true this home is on DNS, and it names the TLDs whose subdomains resolve.
    #[test]
    fn a_wired_server_reports_which_tlds_have_wildcards() {
        let status = listening(&["test", "localhost"]).status();

        assert_eq!(status.mode, DnsMode::Dns);
        assert_eq!(
            status.wildcards,
            vec!["test".to_owned(), "localhost".to_owned()]
        );
        assert_eq!(status.because, None);
    }

    /// The T45 design, D6. A home can route some of its TLDs and not others — `.local` never being
    /// routable at all — so the mode is `dns` while `shop.local` still needs a hosts line, and the
    /// field says exactly which names got wildcards.
    #[tokio::test]
    async fn a_partly_wired_home_is_on_dns_and_says_which_names_it_covers() {
        let status = listening(&["test"]).status();

        assert_eq!(status.mode, DnsMode::Dns);
        assert_eq!(status.wildcards, vec!["test".to_owned()]);
        assert!(!status.wildcards.iter().any(|tld| tld == "local"));
    }

    /// D8. A grant that wires the machine has to change this daemon's answer, or the user grants
    /// permission and watches nothing happen until somebody restarts it.
    #[test]
    fn a_reprobe_after_a_grant_changes_the_mode() {
        let dns = listening(&[]);
        assert_eq!(dns.mode(), DnsMode::HostsOnly);

        let host =
            mock::Host::with_resolver(HOME, mixengine_platform::ResolverMethod::Nrpt, &["test"]);

        dns.reprobe(&host);

        assert_eq!(dns.mode(), DnsMode::Dns);
        assert_eq!(dns.status().wildcards, vec!["test".to_owned()]);
    }

    /// And the other direction, which is what `mix doctor` will meet: a machine somebody unwired by
    /// hand goes back to the hosts file on the next probe rather than claiming names it has lost.
    #[test]
    fn a_reprobe_after_the_wiring_went_away_goes_back_to_hosts_only() {
        let dns = listening(&["test"]);
        assert_eq!(dns.mode(), DnsMode::Dns);

        dns.reprobe(&mock::Host::with_home(HOME));

        assert_eq!(dns.mode(), DnsMode::HostsOnly);
        assert!(dns.status().wildcards.is_empty());
    }

    /// **`[dns] port = 0` is a server nothing may be wired to**, and this is the test that says so.
    ///
    /// The daemon still binds, still answers and still reports where — the setting exists so that no
    /// suite in this workspace takes port 53 off the machine running it. What it must not do is let
    /// a resolver be pointed at a number the operating system will hand to somebody else after the
    /// next restart: that is one elevation prompt spent to break name resolution until then.
    #[test]
    fn a_server_on_an_ephemeral_port_is_answering_and_still_not_wirable() {
        let dns = Dns::on_an_ephemeral_port_for_tests();

        assert_eq!(dns.wirable_port(), None);

        // And a probe cannot turn the mode on behind its back, however wired the machine looks.
        dns.reprobe(&mock::Host::with_resolver(
            HOME,
            mixengine_platform::ResolverMethod::Nrpt,
            &["test"],
        ));

        assert_eq!(dns.mode(), DnsMode::HostsOnly);
        assert!(dns.status().wildcards.is_empty());
    }

    /// A server that is not answering has no port to route anything to, so a probe cannot make one
    /// up — and a machine wired to somebody else's port must not turn this home's mode on.
    #[test]
    fn a_server_that_is_not_listening_has_no_port_and_stays_hosts_only() {
        let dns = Dns::hosts_only_for_tests();

        assert_eq!(dns.port(), None);

        dns.reprobe(&mock::Host::with_resolver(
            HOME,
            mixengine_platform::ResolverMethod::Nrpt,
            &["test"],
        ));

        assert_eq!(dns.mode(), DnsMode::HostsOnly);
    }

    /// Turning it off in `config.toml` is a mode, and the reason says so in the words of the file
    /// somebody would edit to change it.
    #[tokio::test]
    async fn a_server_that_was_turned_off_says_which_key_turned_it_off() {
        let host = mock::Host::with_home(HOME);
        let config = config::Dns {
            enabled: false,
            port: None,
        };

        let dns = Dns::start(&config, &host, CancellationToken::new()).await;
        let status = dns.status();

        assert_eq!(status.mode, DnsMode::HostsOnly);
        assert_eq!(status.listening, None);
        assert!(status.wildcards.is_empty());
        assert!(
            status
                .because
                .as_deref()
                .is_some_and(|because| because.contains("[dns] enabled")),
            "{status:?}"
        );
    }

    /// A port somebody else is on: the mode is the same, and the sentence names them.
    #[test]
    fn a_port_somebody_else_holds_is_reported_with_their_name() {
        let host = mock::Host::with_a_port_held(
            HOME,
            53,
            PortHolder {
                pid: Some(4242),
                name: Some("Docker Desktop Backend.exe".to_owned()),
            },
        );

        let because = unavailable(&host, 53, &std::io::Error::other("address in use"));

        assert_eq!(because, "port 53 is held by Docker Desktop Backend.exe");
    }

    /// A holder this account may not name is still a holder, and still a better sentence than the
    /// error code.
    #[test]
    fn a_holder_who_cannot_be_named_is_still_named_as_a_holder() {
        let host = mock::Host::with_a_port_held(
            HOME,
            53,
            PortHolder {
                pid: None,
                name: None,
            },
        );

        assert_eq!(
            unavailable(&host, 53, &std::io::Error::other("address in use")),
            "port 53 is held by another program on this machine"
        );
    }

    /// Nobody is listening and the bind failed anyway — a UDP-only holder, or a Windows excluded
    /// port range. The operating system's words are what is left.
    #[test]
    fn a_bind_that_failed_with_nobody_listening_reports_what_the_os_said() {
        let host = mock::Host::with_home(HOME);

        let because = unavailable(&host, 53, &std::io::Error::other("permission denied"));

        assert!(because.contains("permission denied"), "{because}");
    }

    /// A machine that cannot be asked leaves the operating system's own error in place, exactly as
    /// `PortOwner`'s contract says every caller must.
    #[test]
    fn a_machine_that_cannot_name_ports_still_gives_a_sentence() {
        let host = mock::Host::unable_to_name_ports(HOME, "no listening table here");

        let because = unavailable(&host, 53, &std::io::Error::other("address in use"));

        assert!(because.contains("address in use"), "{because}");
    }

    /// The default is the one place the two numbers are written, and the reason they differ is not
    /// privilege but what each system's resolver mechanism can express.
    #[test]
    fn the_default_port_is_53_only_where_a_resolver_rule_cannot_carry_one() {
        if cfg!(windows) {
            assert_eq!(DEFAULT_PORT, 53);
        } else {
            assert_ne!(DEFAULT_PORT, 53);
            assert_ne!(DEFAULT_PORT, 5353, "5353 is mDNS's, and it is always taken");
        }
    }
}
