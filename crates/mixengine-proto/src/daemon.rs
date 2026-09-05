//! What `daemon.*` answers.
//!
//! Every field here is something this build genuinely knows. Services, sites and runtimes are
//! absent rather than present-and-empty: a client that renders "0 services" before the concept
//! exists is showing a fact nobody established, and adding a field in Phase 1 costs a client
//! nothing while removing one costs it a release.

use crate::{Error, ProtocolVersion, ServiceWalk, Timestamp, Uptime};

/// Everything the daemon knows about itself, for `daemon.status`.
///
/// Paths are strings and not `PathBuf`s. serde will refuse a `PathBuf` that is not valid UTF-8, and
/// a home directory with an unusual name is a reason to see it spelled oddly in `mix status`, not a
/// reason for `mix status` to fail. They are for reading; nothing joins or opens them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DaemonStatus {
    /// The daemon's build version — `CARGO_PKG_VERSION`, the same string `mixengined --version`
    /// prints.
    pub version: String,

    /// The API version this daemon speaks. A client compares it with its own
    /// [`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION) before it trusts anything else in this struct.
    pub protocol: ProtocolVersion,

    /// The daemon's process id, so a user can find it in a task manager and a client can tell one
    /// restart from another.
    pub pid: u32,

    /// `MIXENGINE_HOME` as it was resolved. The single most useful line when somebody is talking to
    /// a daemon they did not expect to be talking to.
    pub home: String,

    /// Where it listens: a socket path, or a named pipe.
    pub endpoint: String,

    /// The SQLite file it opened. Not derivable from `home` — `[paths]` can move it.
    pub database: String,

    /// When this daemon started.
    pub started_at: Timestamp,

    /// How long ago that was, computed by the daemon rather than by the client.
    ///
    /// Redundant with [`DaemonStatus::started_at`] only if the two clocks agree, which is exactly
    /// the assumption worth avoiding: a monotonic reading here means "up 3 days" stays right across
    /// a system clock that was corrected while the daemon ran.
    pub uptime: Uptime,

    /// What is waiting for permission, and whether this machine could ask for it, or [`None`] from a
    /// daemon built before this member existed.
    ///
    /// **In the call every client already makes**, so `mix status` can say "3 operations are waiting
    /// for permission" without a second round trip and without a client deciding what *degraded*
    /// means — the T40b design, D6. The list itself is `elevation.status`, because that is a screen
    /// and this is a status line.
    ///
    /// **Optional because it arrived after protocol 1 was frozen** —
    /// [ADR 0019](../../../.claude/decisions/0019-an-added-response-member-is-optional.md), roadmap
    /// task **T88c**. It was required until then, which meant a `mix` from a new build could not
    /// *decode* the answer of an older daemon that had not been restarted yet.
    ///
    /// **[`None`] means the daemon predates the member and nothing else.** It is never "the queue
    /// could not be read": this call is fallible for exactly that case, so an unreadable queue is an
    /// [`Error`] rather than a zero nobody established — which is the stale-clear failure D6 exists
    /// to prevent, and the reason there is no `Default` for
    /// [`ElevationSummary`](crate::ElevationSummary) here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevation: Option<crate::ElevationSummary>,

    /// Which of the two name mechanisms this home is running on, and why — roadmap task **T44** — or
    /// [`None`] from a daemon built before this member existed.
    ///
    /// **In the call every client already makes**, on [`DaemonStatus::elevation`]'s reasoning: "is
    /// `blog.test` going to resolve, and will a wildcard under it" is a status line, not a screen,
    /// and a client that had to ask a second method for it would render the first line of `mix
    /// status` a round trip late.
    ///
    /// **Optional on [`DaemonStatus::elevation`]'s reasoning too**, and for the same skew — **T88c**
    /// changed both at once, because fixing one while the other was required bought nothing. [`None`]
    /// means the daemon predates the member; a daemon that has it always answers [`Some`], since this
    /// is read from state it owns and cannot fail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns: Option<DnsStatus>,

    /// A release this daemon has been offered, or [`None`] — roadmap task **T88**.
    ///
    /// **In the call every client already makes**, on [`DaemonStatus::elevation`]'s reasoning: *"is
    /// there a newer MixEngine"* is a status line and not a screen, and the screen is
    /// `update.status`.
    ///
    /// **Optional because of what it means, and skew-tolerant as a consequence rather than as a
    /// workaround.** [`None`] is the honest value for a daemon that has not checked yet, for one
    /// whose check found nothing, and for one built before this member existed — three states a
    /// client renders identically, which is *nothing at all*. This is the only member here whose
    /// [`None`] carries more than the wire fact, and **T88c** — which made
    /// [`DaemonStatus::elevation`] and [`DaemonStatus::dns`] optional as well, under
    /// [ADR 0019](../../../.claude/decisions/0019-an-added-response-member-is-optional.md) — left it
    /// exactly as it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<crate::UpdateOffer>,
}

/// What this daemon's own DNS server is doing, and what it costs when it is not — roadmap task
/// **T44**, which also closes T46a.
///
/// A home resolves its names one of two ways: through the built-in DNS server, which answers a
/// whole managed TLD by pattern, or through the hosts file, which has one line per name. The
/// difference is not a detail of implementation — it is whether `api.blog.test` works — so it is
/// reported rather than left for a client to work out.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DnsStatus {
    /// Which mechanism is in use.
    pub mode: DnsMode,

    /// Where the DNS server is answering, as `address:port`, or [`None`] when it is not.
    ///
    /// **Not the same question as [`DnsStatus::mode`]**, and that is the field's whole reason for
    /// existing: a server can be listening perfectly while nothing on the machine sends it a name,
    /// which is every machine until the resolver wiring of T45. A string for
    /// [`DaemonStatus`]' reason — it is for reading, and nothing parses it back.
    pub listening: Option<String>,

    /// The TLDs whose subdomains resolve — every name under one of these, at any depth.
    ///
    /// Empty in hosts-only mode, where it is the specific thing a user loses: a hosts file holds one
    /// line per name and no patterns, so a subdomain nobody wrote down does not resolve. Stated here
    /// rather than inferred from `mode`, because what a client renders is the loss and not the
    /// mechanism.
    ///
    /// **A list rather than a `bool`, from T45 on.** While nothing could be wired, "does this home
    /// have wildcards?" had one answer for the whole home. It no longer does: every mechanism there
    /// is scopes to one TLD, and `.local` is deliberately never wired — so a home can perfectly well
    /// answer `*.blog.test` by pattern and still need a hosts line for `shop.local`. A boolean would
    /// have to say `true` and leave a client to work out which half of its sites it applies to,
    /// which is the derivation this field exists to prevent.
    pub wildcards: Vec<String>,

    /// Why this home is not on DNS, phrased for a person, or [`None`] when it is.
    ///
    /// A sentence and not a code, on [`Error`]'s neighbouring precedent: the reasons are a port
    /// somebody else is holding — named where this machine will name them — a key in `config.toml`,
    /// and a resolver that has not been wired yet, and a vocabulary for them would be three
    /// variants that every client would have to spell back out in English.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
}

/// Which mechanism a home's names resolve through.
///
/// Closed rather than a string: a client renders a warning for one of these and not the other, and
/// a free-form value would make that a comparison against a spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum DnsMode {
    /// The built-in server answers, and a resolver on this machine sends managed names to it.
    Dns,

    /// Names resolve through the hosts file, one line at a time, with no wildcards.
    HostsOnly,
}

/// The cheap half of [`DaemonStatus`], for `daemon.version`.
///
/// Its own method because a client asks this before it can safely ask anything else — a daemon from
/// another release may answer `daemon.status` with fields this client cannot decode, and finding
/// that out by failing to parse the answer is worse than asking first.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DaemonVersion {
    /// The daemon's build version.
    pub version: String,

    /// The API version it speaks.
    pub protocol: ProtocolVersion,
}

/// What a `daemon.shutdown` did on its way out.
///
/// **The answer arrives before the daemon does go, and after everything it stopped has stopped.**
/// That ordering is the whole reason this type has anything in it: a client told only "accepted"
/// would have to re-derive from the event stream whether the database it cares about was flushed or
/// killed, which is the business-logic-in-a-client bug `CLAUDE.md` forbids. The connection is closed
/// by the daemon exiting a moment later, which a client reads as the shutdown having happened rather
/// than as a request that failed.
///
/// A struct rather than a bare [`ServiceWalk`] so that what a shutdown reports can grow — which it
/// since has: [`DaemonShutdown::unordered`] arrived beside the walk rather than as a shape every
/// client had to decode a second way.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DaemonShutdown {
    /// Stopping every supervised service, in reverse dependency order.
    ///
    /// [`ServiceWalk::failed`] here is what T18 made possible and nothing else: a process that
    /// outlived a previous daemon, was adopted by this one, and would not die. **The daemon stops
    /// anyway** — refusing to shut down because something will not is a machine with no way out of
    /// the situation — and the next daemon meets that survivor as the crash recovery it already
    /// performs on every boot. Whoever asked is told, which is the point.
    pub services: ServiceWalk,

    /// Why there was no order to stop them in, on the shutdown where there was none.
    ///
    /// **The reporting half of T9a's "it is reported, the ordered walk is skipped".** A daemon that
    /// cannot say what it declares — an `extension.toml` somebody is in the middle of editing —
    /// still stops, because refusing over a half-typed file would leave them a daemon they can only
    /// kill. What stops the services then is the root token, all at once, the way a signal would
    /// have: each still stops the way its own spec asks, and what is lost is the *order* — a site is
    /// no longer guaranteed to go before the database it is serving requests against.
    ///
    /// Without this field that walk goes out empty and [`complete`](ServiceWalk::complete), which on
    /// the wire is **indistinguishable from a home that declares no services at all** — so a client
    /// could only say `mixengined is stopping`, and could not say that the ordering the whole method
    /// exists for did not happen. A client cannot work that out for itself from an answer the daemon
    /// never sent, which is the business-logic-in-a-client bug `CLAUDE.md` forbids, arriving from
    /// the side a client cannot fix.
    ///
    /// The wire [`Error`] and not a vocabulary of its own, because this is the same failure the same
    /// declarations hand `service.list` — the same code, the same message, the same hint about where
    /// services are written down — and a second spelling of it would be a second thing to keep in
    /// step with whatever T30's generator can fail with.
    ///
    /// [`None`] is every ordinary shutdown, and it is absent from the wire rather than `null`: a
    /// daemon built before this field answers byte for byte what one that walked the plan in order
    /// answers now, so neither side of a version skew has to special-case the other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unordered: Option<Error>,
}

/// The body of `GET /health`.
///
/// Unauthenticated and deliberately trivial: its one job is to tell a client whether to autostart a
/// daemon (`.claude/architecture/daemon-and-ipc.md`), and it must stay answerable while everything
/// else is still coming up. The version rides along because it is free and saves the caller a second
/// round trip.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Health {
    /// Always `true` — a daemon that could not answer this does not answer at all. A field rather
    /// than an empty object so the body is self-describing in a log or a `curl`.
    pub ok: bool,

    /// The daemon's build version.
    pub version: String,

    /// The API version it speaks.
    pub protocol: ProtocolVersion,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ErrorCode, PROTOCOL_VERSION, ServiceId};

    /// A shutdown that stopped `services`, in the order they are given.
    fn shutdown(services: &[&str]) -> DaemonShutdown {
        let walked: Vec<ServiceId> = services
            .iter()
            .map(|id| ServiceId::parse(*id).expect("a valid service id"))
            .collect();

        DaemonShutdown {
            services: ServiceWalk {
                planned: walked.clone(),
                complete: true,
                reached: walked,
                failed: None,
                blocked: Vec::new(),
            },
            unordered: None,
        }
    }

    /// A daemon reporting on itself, as every test below varies it.
    fn status() -> DaemonStatus {
        DaemonStatus {
            version: "0.1.0".to_owned(),
            protocol: PROTOCOL_VERSION,
            pid: 4123,
            home: "/home/dev/.local/share/mixengine".to_owned(),
            endpoint: "/home/dev/.local/share/mixengine/run/mixengined.sock".to_owned(),
            database: "/home/dev/.local/share/mixengine/data/mixengine.db".to_owned(),
            started_at: Timestamp(1_723_000_000_500),
            uptime: Uptime(812),
            elevation: Some(crate::ElevationSummary {
                elevated: false,
                can_prompt: true,
                pending: 3,
            }),
            dns: Some(DnsStatus {
                mode: DnsMode::HostsOnly,
                listening: Some("127.0.0.1:53535".to_owned()),
                wildcards: Vec::new(),
                because: Some("nothing routes a managed TLD here yet".to_owned()),
            }),
            update: None,
        }
    }

    #[test]
    fn a_status_is_flat_json_with_no_nested_envelope() {
        let status = status();

        let encoded = serde_json::to_value(&status).unwrap();
        assert_eq!(encoded["protocol"], 1);
        assert_eq!(encoded["uptime"], 812);
        assert_eq!(encoded["started_at"], 1_723_000_000_500_i64);
        // Three operations waiting *is* degraded — there is no flag, and a client that renders one
        // reads this number. See the T40b design, D6.
        assert_eq!(encoded["elevation"]["pending"], 3);
        assert_eq!(encoded["elevation"]["elevated"], false);
        // The mechanism is a closed vocabulary on the wire, and the loss it costs is a field of its
        // own rather than something a client re-derives from the mode.
        assert_eq!(encoded["dns"]["mode"], "hosts_only");
        assert_eq!(encoded["dns"]["wildcards"], serde_json::json!([]));

        assert_eq!(
            serde_json::from_value::<DaemonStatus>(encoded).unwrap(),
            status
        );
    }

    /// A status with nothing to say about updates says nothing at all — roadmap task **T88**.
    ///
    /// Absent rather than `null`, so a daemon that has not checked answers byte for byte what one
    /// built before this field existed answers, and a client renders neither.
    #[test]
    fn a_status_with_no_offer_does_not_write_the_field_at_all() {
        let encoded = serde_json::to_value(status()).unwrap();

        assert!(encoded.get("update").is_none(), "{encoded}");
    }

    /// The other half of the same property: a daemon built before [`DaemonStatus::update`] existed
    /// is still readable by a `mix` that knows about it.
    ///
    /// **Every member added since protocol 1 was frozen now behaves this way** — **T88c** brought
    /// `elevation` and `dns` into line, and
    /// [`a_status_from_before_elevation_and_dns_existed_still_reads`] is the floor that keeps the
    /// next one there.
    #[test]
    fn a_status_from_a_daemon_that_predates_the_field_still_reads() {
        let mut encoded = serde_json::to_value(status()).unwrap();
        encoded.as_object_mut().expect("an object").remove("update");

        let decoded: DaemonStatus = serde_json::from_value(encoded).expect("a status");
        assert_eq!(decoded.update, None);
    }

    /// **The floor of protocol 1**, as the JSON a daemon from before T40b actually sent — roadmap
    /// task **T88c**.
    ///
    /// Every member below is one protocol 1 was frozen with. Everything added since —
    /// [`DaemonStatus::elevation`] (T40b), [`DaemonStatus::dns`] (T44), [`DaemonStatus::update`]
    /// (T88) — is absent, which is what such a daemon puts on the wire, and this build reads it as
    /// [`None`] rather than refusing the answer.
    ///
    /// **This is the guard on ADR 0019 and it is the whole reason the rule holds.** A member added
    /// as required turns this red here, in the crate the rule lives in, rather than in a CLI suite
    /// that would blame the command.
    #[test]
    fn a_status_from_before_elevation_and_dns_existed_still_reads() {
        let floor = r#"{
            "version": "0.0.1",
            "protocol": 1,
            "pid": 4123,
            "home": "/home/dev/.local/share/mixengine",
            "endpoint": "/home/dev/.local/share/mixengine/run/mixengined.sock",
            "database": "/home/dev/.local/share/mixengine/data/mixengine.db",
            "started_at": 1723000000500,
            "uptime": 812
        }"#;

        let decoded: DaemonStatus = serde_json::from_str(floor).expect("a status");

        assert_eq!(decoded.version, "0.0.1");
        assert_eq!(decoded.protocol, PROTOCOL_VERSION);
        assert_eq!(decoded.elevation, None);
        assert_eq!(decoded.dns, None);
        assert_eq!(decoded.update, None);
    }

    #[test]
    fn a_status_carrying_an_offer_names_the_version_and_nothing_more() {
        let offered = DaemonStatus {
            update: Some(crate::UpdateOffer {
                version: "0.2.0".to_owned(),
                published_at: "2026-09-05T09:12:00Z".to_owned(),
            }),
            ..status()
        };

        let encoded = serde_json::to_value(&offered).unwrap();
        assert_eq!(encoded["update"]["version"], "0.2.0");

        assert_eq!(
            serde_json::from_value::<DaemonStatus>(encoded).unwrap(),
            offered
        );
    }

    #[test]
    fn a_shutdown_that_kept_the_order_says_nothing_at_all_about_one() {
        let ordinary = shutdown(&["web", "db"]);
        let encoded = serde_json::to_value(&ordinary).unwrap();

        // Absent rather than `null`, which is what makes the field free for a client that renders a
        // note whenever it is there: an ordinary shutdown never puts one on the wire to be ignored.
        assert!(encoded.get("unordered").is_none(), "{encoded}");
        assert_eq!(
            serde_json::from_value::<DaemonShutdown>(encoded).unwrap(),
            ordinary
        );
    }

    #[test]
    fn a_shutdown_that_could_not_order_the_stop_carries_the_failure_that_stopped_it() {
        // The empty walk on its own says "this home declares nothing"; the two together say what
        // actually happened, which is that the daemon went and every runner was released at once.
        let skipped = DaemonShutdown {
            unordered: Some(
                Error::new(
                    ErrorCode::Internal,
                    "cannot read the declarations in /home/dev/extensions/mailpit/extension.toml",
                )
                .with_hint("`logs/daemon.log` has the detail a report needs"),
            ),
            ..shutdown(&[])
        };

        let encoded = serde_json::to_value(&skipped).unwrap();
        assert_eq!(encoded["services"]["complete"], true);
        assert_eq!(encoded["unordered"]["code"], "internal");
        assert!(encoded["unordered"]["hint"].is_string(), "{encoded}");
        assert_eq!(
            serde_json::from_value::<DaemonShutdown>(encoded).unwrap(),
            skipped
        );
    }

    /// Both directions of the skew this field is additive for, as JSON rather than as a claim about
    /// what `serde` does — which is the sort of sentence `.claude/standards/rust.md` asks for a test
    /// instead of.
    #[test]
    fn a_shutdown_decodes_whichever_side_of_this_field_the_daemon_is_from() {
        // A daemon that predates the field: the member is simply not there, and `default` is what
        // makes that "nothing was skipped" rather than a payload this build refuses.
        let older = r#"{"services":{"planned":[],"complete":true,"reached":[],"blocked":[]}}"#;
        assert_eq!(
            serde_json::from_str::<DaemonShutdown>(older).unwrap(),
            shutdown(&[])
        );

        // And a daemon from after the next field, read by a client from before it. Nothing here is
        // `deny_unknown_fields`, so what such a client loses is the member it cannot render — not
        // the answer, at the moment something has already gone wrong.
        let newer = r#"{"services":{"planned":[],"complete":true,"reached":[],"blocked":[]},
                        "checkpointed":true}"#;
        assert_eq!(
            serde_json::from_str::<DaemonShutdown>(newer).unwrap(),
            shutdown(&[])
        );
    }

    #[test]
    fn health_says_which_protocol_it_speaks_so_one_request_is_enough() {
        let health = Health {
            ok: true,
            version: "0.1.0".to_owned(),
            protocol: PROTOCOL_VERSION,
        };

        assert_eq!(
            serde_json::to_string(&health).unwrap(),
            r#"{"ok":true,"version":"0.1.0","protocol":1}"#
        );
    }
}
