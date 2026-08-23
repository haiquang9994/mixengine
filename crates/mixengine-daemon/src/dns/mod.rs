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
//! # The mode has two terms, and this task can only produce one of them
//!
//! The T44 design, D4. "Running on DNS" is not "the server is listening". It is "the server is
//! listening **and** something is routing a managed TLD to it", and nothing in T44 can make the
//! second one true — wiring a resolver is **T45**'s elevated, per-OS work, and [`Dns::start`] hard-
//! codes [`ResolverRouting::NotWired`] until it lands.
//!
//! So on every machine, for the whole of this task, the mode is [`DnsMode::HostsOnly`] and
//! `site.create` goes on queueing hosts entries exactly as it does today. That is the task not
//! breaking the product rather than the task doing nothing: a mode that read only "is it listening"
//! would stop [`crate::elevation::Elevation::require_hosts`] queueing anything the moment this
//! merged, while no resolver pointed anywhere and no name resolved at all.

mod answer;
mod server;

use std::net::SocketAddr;

use mixengine_core::config;
use mixengine_platform::{Host, PortHolder};
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

/// Whether anything on this machine sends a managed TLD to this server.
///
/// **Nothing in this build produces [`ResolverRouting::Wired`].** Wiring a resolver is an elevated,
/// per-OS operation — `/etc/resolver/<tld>`, an NRPT rule, `resolvectl domain … ~test` — and T44
/// performs none of them: [`Dns::start`] hard-codes [`ResolverRouting::NotWired`], and **T45**
/// replaces that constant with a probe.
///
/// The variant exists ahead of its producer for one reason: the branch that depends on it decides
/// whether this home writes hosts entries at all, and a branch no test can reach is a branch that
/// breaks quietly on the day T45 first reaches it. Typed rather than a `bool` so that the two
/// halves of the mode read as the two different questions they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolverRouting {
    /// No resolver on this machine routes a managed TLD here.
    NotWired,

    /// Something does — an `/etc/resolver` file, an NRPT rule, a `systemd-resolved` domain.
    ///
    /// Constructed by tests only, until T45.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "T45 is its producer; the branch it selects is tested here so that the day                       T45 first reaches it is not the day it is first run"
        )
    )]
    Wired,
}

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

    /// Whether anything routes a managed TLD here.
    routing: ResolverRouting,
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
        let routing = ResolverRouting::NotWired;

        if !config.enabled {
            return Self {
                state: State::Disabled,
                routing,
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

        Self { state, routing }
    }

    /// Which of the two name mechanisms this home is running on.
    ///
    /// **Both terms, and neither alone.** A server that is listening while nothing routes a name to
    /// it resolves exactly as many names as no server at all, and a resolver wired to a port
    /// nothing answers on resolves fewer.
    pub(crate) fn mode(&self) -> DnsMode {
        match (&self.state, self.routing) {
            (State::Listening(_), ResolverRouting::Wired) => DnsMode::Dns,
            (State::Listening(_), ResolverRouting::NotWired)
            | (State::Disabled | State::Unavailable { .. }, _) => DnsMode::HostsOnly,
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
            // than leaving a client to infer it — the T44 design, D9.
            wildcards: mode == DnsMode::Dns,
            because: self.because(),
        }
    }

    /// Why this home is not running on DNS, or [`None`] when it is.
    fn because(&self) -> Option<String> {
        match (&self.state, self.routing) {
            (State::Listening(_), ResolverRouting::Wired) => None,
            (State::Disabled, _) => Some("[dns] enabled = false in config.toml".to_owned()),
            (State::Unavailable { because }, _) => Some(because.clone()),
            (State::Listening(address), ResolverRouting::NotWired) => Some(format!(
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
            routing: ResolverRouting::NotWired,
        }
    }

    /// A home resolving through DNS: listening, and something routes managed names here.
    pub(crate) fn wired_for_tests() -> Self {
        Self {
            state: State::Listening("127.0.0.1:53535".parse().expect("an address")),
            routing: ResolverRouting::Wired,
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

    fn listening(routing: ResolverRouting) -> Dns {
        Dns {
            state: State::Listening("127.0.0.1:53535".parse().expect("an address")),
            routing,
        }
    }

    /// The row the whole of T44 lives on: the server can be up and this home is still on the hosts
    /// file, because nothing sends it a name. Deleting this test is how somebody would break D4.
    #[test]
    fn a_listening_server_nothing_is_wired_to_is_still_hosts_only() {
        let dns = listening(ResolverRouting::NotWired);
        let status = dns.status();

        assert_eq!(status.mode, DnsMode::HostsOnly);
        assert_eq!(status.listening.as_deref(), Some("127.0.0.1:53535"));
        assert!(!status.wildcards);
        assert!(
            status
                .because
                .as_deref()
                .is_some_and(|because| because.contains("routes a managed TLD")),
            "{status:?}"
        );
    }

    /// The other side of the same row, which nothing but a test can build until T45: with both
    /// terms true this home is on DNS, wildcards and all, and has nothing to explain.
    #[test]
    fn a_listening_server_a_resolver_points_at_is_the_dns_mode() {
        let status = listening(ResolverRouting::Wired).status();

        assert_eq!(status.mode, DnsMode::Dns);
        assert!(status.wildcards);
        assert_eq!(status.because, None);
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
        assert!(!status.wildcards);
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
