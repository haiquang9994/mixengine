//! The file protocol between `mixengined` and `mixengine-elevate`.
//!
//! The daemon writes a [`PrivilegedRequest`] into a fresh single-use directory, raises the OS
//! elevation prompt on the helper with that file's path as its one argument, and reads the
//! [`PrivilegedResponse`] the helper leaves beside it. See
//! `.claude/decisions/0005-on-demand-elevation.md` and the T40 design for why it is files and not a
//! socket: the helper has no listener, no idle state, and exists for seconds.
//!
//! **The response file is the protocol.** When it is there, it is the answer and the exit code says
//! nothing; the exit code matters only when there is no file. That is not a preference — the macOS
//! launcher raises an AppleScript error instead of handing back a status, so an outcome encoded as a
//! number is an outcome one of the three systems has to reconstruct from an error string.
//!
//! **What is denied and what is tolerated runs in opposite directions on purpose.** The request and
//! every operation in it use `deny_unknown_fields`: a helper that silently ignored a field inside an
//! operation it thought it understood would apply a weaker version of that operation and tell nobody.
//! The response does not: the helper is excluded from auto-update, so a helper newer than the daemon
//! reading it is routine, and a field it added must not make its answer unreadable.

use std::net::IpAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ProtocolVersion;

/// The name the response takes, beside the request it answers.
///
/// Not passed as an argument: one fewer argument is one fewer thing the elevated process has to
/// validate, and the daemon already knows where it will be. Its **existence** is also the whole of
/// the anti-replay check — a request with an answer beside it has been processed and is refused.
pub const RESPONSE_FILE_NAME: &str = "response.json";

/// One batch of privileged operations, covered by one prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct PrivilegedRequest {
    /// The protocol this daemon speaks. A helper that does not know it refuses the whole request.
    pub version: ProtocolVersion,

    /// `MIXENGINE_HOME`. Every path in every operation is canonicalised and must resolve inside it,
    /// and the helper checks that this directory belongs to whoever owns the request file — without
    /// that, `--home C:\Windows\System32` is an escalation for every operation that takes a path.
    pub home: PathBuf,

    /// Echoed into the response, so a daemon cannot read the answer to an earlier request as the
    /// answer to this one. It is **not** the anti-replay check; [`RESPONSE_FILE_NAME`] is.
    pub nonce: String,

    /// The operations, left undecoded.
    ///
    /// A `Vec<PrivilegedOp>` would fail as a whole on one variant this build has never heard of,
    /// which — the helper being excluded from auto-update — is a routine event and not a corruption.
    /// Decoded one element at a time, an unknown operation becomes
    /// [`OpOutcome::Unsupported`] at its own index and its neighbours are applied. The daemon builds
    /// a `Vec<PrivilegedOp>` and serialises it into this field; the asymmetry is confined here.
    pub ops: Vec<serde_json::Value>,
}

/// One line of the managed block: a name, and the address it resolves to.
///
/// The address is an [`IpAddr`] and not a string because the helper refuses anything that is not
/// loopback, and a refusal that had to parse the field first would be a refusal with a second way
/// to be wrong. `serde` renders it the way a hosts file spells one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct HostEntry {
    /// Where the name points. Only `127.0.0.1` and `::1` are ever accepted — see the T41 design, D5.
    pub address: IpAddr,

    /// The name, lowercased and already checked by whoever built this.
    pub domain: String,
}

/// One port a site is reached on, and the ordinary port a program binds to answer it.
///
/// On macOS these differ — a packet-filter rule sends 80 to 8080 — and on the other two they do not.
/// The pair travels together because the layer that generates a front end's configuration needs both
/// numbers and may not ask which operating system it is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct PortRedirect {
    /// What a browser asks for: 80 or 443.
    pub answer: u16,

    /// What a program actually binds, which an ordinary account may.
    pub bind: u16,
}

/// What granting port access means on the machine being asked — the T42 design, D2 and D4.
///
/// **Two variants rather than one struct holding both a binary and a redirect list**, because a
/// field the helper does not use is a field the helper cannot validate, and validating is that
/// binary's entire job. Every OS refuses the variant that is not its mechanism; no branch quietly
/// does nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PortAccessPlan {
    /// Linux: `cap_net_bind_service` on the front end's binary, which then binds 80 itself.
    Capability {
        /// The program the capability goes on. The helper checks that the caller owns it and that
        /// nobody else can write it — the T42 design, D5.
        binary: PathBuf,

        /// Which reserved ports it is being allowed. Only 80 and 443 are ever accepted.
        ports: Vec<u16>,
    },

    /// macOS: a packet-filter anchor, its declaration in `/etc/pf.conf`, and the boot job that
    /// enables pf — see ADR 0012. The program binds an ordinary port instead.
    Redirect {
        /// Every port that moves, and where to.
        redirects: Vec<PortRedirect>,
    },
}

/// What taking port access away means.
///
/// Mirrors [`PortAccessPlan`] with only the fields a removal reads: a capability is cleared whole,
/// so there is no port to name, and the three files a redirect leaves behind are constants in the
/// helper rather than anything a request may choose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PortAccessTarget {
    /// Clear `security.capability` from this binary.
    Capability {
        /// The program to clear it from, checked exactly as a grant's is.
        binary: PathBuf,
    },

    /// Remove the anchor, its block in `/etc/pf.conf` and the boot job.
    ///
    /// `Redirect {}` and not `Redirect`, for the reason [`PrivilegedOp::Probe`] is written that way:
    /// serde reads a unit variant of an internally tagged enum through `deserialize_any`, where
    /// `deny_unknown_fields` never gets a chance to fire.
    Redirect {},
}

/// How this machine is asked to route managed TLDs to MixEngine's own DNS server — the T45
/// design, D2 and D3.
///
/// What ports this machine should have open for MixEngine — roadmap task **T74**.
///
/// **Whole state, like [`ResolverPlan`] and [`PortAccessPlan`]**: the plan names every port that
/// should end up open, so a second request supersedes the first, "already done" is a comparison
/// rather than a judgement, and revoking is this same operation carrying an empty list. There is no
/// `FirewallRevoke` beside it for that reason.
///
/// **And unlike [`ResolverPlan`], it carries no OS mechanism.** That type has one variant per
/// mechanism because the daemon reads which one this machine has — through `ResolverConfig`, in
/// `mixengine-platform`, which this crate sits below and cannot link to — before it plans. Nothing
/// reads the firewall: the daemon has no trait for it and never asks what rules exist, so the helper
/// picks
/// the mechanism itself — `netsh` on Windows, `ufw` or `firewalld` on Linux where one is active,
/// and nothing at all on macOS, whose application firewall needs no rule for a listening socket.
/// A field the helper does not need is a field it cannot validate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirewallPlan {
    /// Every port that should be reachable from the local network, sorted and deduplicated.
    ///
    /// Empty is the revoke, and is valid. Each one is checked by the helper itself against rules it
    /// can apply without a database — see `mixengine-elevate`'s own `firewall` module.
    pub ports: Vec<u16>,

    /// What the rules are named, so they can be found and removed again.
    ///
    /// The helper refuses anything that does not begin [`FIREWALL_LABEL`], which is what stops a
    /// compromised daemon from writing a rule MixEngine would never recognise as its own — and
    /// therefore never clean up.
    pub label: String,
}

/// What every firewall rule MixEngine writes is called.
///
/// A rule outside this prefix is not ours: uninstall enumerates by it, `mix doctor` reports by it,
/// and the helper refuses a plan whose label does not start with it.
pub const FIREWALL_LABEL: &str = "MixEngine — ";

/// **It carries no nameserver address, no link name and no registry key**, and that is the security
/// decision of the task rather than an economy. `mixengine-elevate` exists because a compromised
/// daemon *is* the attacker, so an operation that accepted an address from the request would be one
/// that let whoever owns the daemon redirect this machine's name resolution anywhere — with a valid
/// signature, through the audited binary, under the user's own Allow click. Every one of those
/// values is compiled into the helper. What travels is the two things the helper cannot know: which
/// of the managed TLDs to wire, and which port the server is listening on.
///
/// One variant per OS mechanism, as [`PortAccessPlan`] has and for its reason: a field the helper
/// does not use is a field the helper cannot validate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ResolverPlan {
    /// macOS: one `/etc/resolver/<tld>` per TLD, each naming a port.
    ResolverDirectory {
        /// Which TLDs to route. Every one is checked against
        /// [`WIRED_TLDS`](crate::domains::WIRED_TLDS) by the helper itself.
        tlds: Vec<String>,

        /// Where the server is listening.
        ///
        /// A real field because `[dns] port` is a real setting and a test daemon binds an ephemeral
        /// one; bounded rather than trusted, being refused at zero.
        port: u16,
    },

    /// Linux: a dummy link of MixEngine's own, declared to `systemd-networkd`.
    ///
    /// **A link rather than a file of resolver settings** — the T45 design, D10, and every
    /// alternative was measured out: a `resolved.conf.d` drop-in redirects the whole machine, the
    /// loopback link is refused by systemd-resolved by name, a real link would have its own servers
    /// replaced rather than added to, and a link with no address is configured and inert.
    SystemdLink {
        /// Which TLDs become routing domains on that link.
        tlds: Vec<String>,

        /// Where the server is listening.
        port: u16,
    },

    /// Windows: **one** NRPT rule naming every TLD.
    ///
    /// Its `Name` value is a `REG_MULTI_SZ`, so all of them live under the one GUID the helper
    /// compiles in rather than one rule each — which is what makes "already done" a read of a single
    /// key. **No port**: NRPT has no field for one, which is why T44 puts the server on 53 here.
    Nrpt {
        /// Which namespaces the rule names.
        tlds: Vec<String>,
    },
}

/// What unwiring means, per mechanism.
///
/// Mirrors [`ResolverPlan`] with only what a removal reads. Every artifact each variant deletes is
/// a constant in the helper, so no variant carries a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ResolverTarget {
    /// Remove every `/etc/resolver/<tld>` file MixEngine marked.
    ResolverDirectory {},

    /// Remove both `systemd-networkd` files and the link they declare.
    SystemdLink {},

    /// Remove the one registry key.
    Nrpt {},
}

/// How this machine is asked to trust MixEngine's own certificate authority — roadmap task **T49a**.
///
/// **The certificate's DER travels; a path to it does not.** [`ResolverPlan`] above carries the
/// argument in full: what the helper can know is compiled into the helper, and a path is somebody
/// else choosing which file root reads after root has decided to trust the request. The destination
/// store, the file name on Linux and the update command are all constants in `mixengine-elevate`.
///
/// One variant per OS mechanism, as [`PortAccessPlan`] and [`ResolverPlan`] have and for their
/// reason: a plan naming a mechanism is a plan the helper re-validates against the machine it is
/// actually running on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TrustPlan {
    /// Windows: the `Root` store under `LocalMachine`.
    SystemRoot {
        /// The certificate, DER. Checked against the T49a design's D4 table by the helper itself,
        /// before a store is opened.
        der: Vec<u8>,
    },

    /// macOS: `/Library/Keychains/System.keychain`, as a trusted root.
    SystemKeychain {
        /// As [`SystemRoot`](Self::SystemRoot).
        der: Vec<u8>,
    },

    /// Linux, Debian family: `/usr/local/share/ca-certificates`, then `update-ca-certificates`.
    CaCertificates {
        /// As [`SystemRoot`](Self::SystemRoot).
        der: Vec<u8>,
    },

    /// Linux, Red Hat family: `/etc/pki/ca-trust/source/anchors`, then `update-ca-trust`.
    CaTrustAnchors {
        /// As [`SystemRoot`](Self::SystemRoot).
        der: Vec<u8>,
    },
}

/// Which authority to take back out, and **not which certificate** — the T49a design, D5.
///
/// **There is no fingerprint field, and that is the whole of this type's security decision.** The
/// install direction is close to harmless: a daemon compromised badly enough to forge one already
/// holds the private key of the authority this machine trusts, and can sign any certificate for any
/// name without installing a second root. A *removal* that named a certificate by its hash is not —
/// it could take out the root that validates Windows Update, or an organisation's own root, through
/// the audited binary and under the user's own Allow click.
///
/// So what travels is T48's key-id: eight lowercase hex characters, refused by the helper before a
/// store is opened, and unable to describe a corporate root at all. The helper then finds
/// certificates whose subject is exactly `MixEngine Local CA <key_id>`, checks each against D4's
/// table, and removes only those that pass. This is [`ResolverPlan`]'s own argument one capability
/// along: the value an attacker would abuse is not validated, it is absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TrustTarget {
    /// Windows.
    SystemRoot {
        /// Eight lowercase hex characters, and nothing else is accepted.
        key_id: String,
    },

    /// macOS.
    SystemKeychain {
        /// As [`SystemRoot`](Self::SystemRoot).
        key_id: String,
    },

    /// Linux, Debian family.
    CaCertificates {
        /// As [`SystemRoot`](Self::SystemRoot).
        key_id: String,
    },

    /// Linux, Red Hat family.
    CaTrustAnchors {
        /// As [`SystemRoot`](Self::SystemRoot).
        key_id: String,
    },
}

/// The closed list of things that cross into the elevated process.
///
/// See `.claude/architecture/platform-abstraction.md`: the list is closed against operations **with
/// effects**, and adding one of those requires an ADR. [`PrivilegedOp::Probe`] has none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PrivilegedOp {
    /// Report this build: nothing is read, nothing is written, nothing is changed.
    ///
    /// What it reports arrives in the [`PrivilegedResponse`] header rather than in its own outcome,
    /// because every answer carries it — see that type.
    ///
    /// It is `Probe {}` and not `Probe` because serde deserialises a *unit* variant of an
    /// internally tagged enum through `deserialize_any`, which reads the map and drops every key
    /// but the tag — `deny_unknown_fields` never gets a chance to fire. An empty struct variant is
    /// deserialised as a struct, where it does. The rule above is only worth having if it holds for
    /// the operation that carries no fields as well as the ones that do.
    Probe {},

    /// Set MixEngine's block in the hosts file to exactly `entries`.
    ///
    /// **The whole state, not a delta** — the T41 design, D1. A block that has drifted cannot be
    /// pulled back by "add this line", so a whole-state operation is idempotent, is its own repair,
    /// and makes "already done" a byte comparison rather than a judgement. An empty list removes
    /// the block.
    HostsApply {
        /// Sorted and deduplicated by [`PrivilegedOp::hosts_apply`], which is the only way one
        /// should be built.
        entries: Vec<HostEntry>,
    },

    /// Let this machine's front end answer on the ports the OS reserves — roadmap task **T42**.
    ///
    /// **Whole state, like [`HostsApply`](Self::HostsApply)**: the plan says what the machine should
    /// end up allowing, so a second request supersedes the first rather than queueing behind it, and
    /// "already done" is a comparison rather than a judgement.
    PortAccessGrant {
        /// What to grant, and how — one variant per OS mechanism.
        plan: PortAccessPlan,
    },

    /// Take it away again.
    ///
    /// **Nothing in T42 enqueues one** — the T42 design, D12. The producer asks in one direction
    /// only, deliberately: on Linux the question needs the front end's binary, which is exactly what
    /// a home with no front end cannot supply. Uninstall (T87) is the producer that can. It ships
    /// built, validated and tested, which is the shape T20, T21 and T22 landed in.
    PortAccessRevoke {
        /// What to take away.
        target: PortAccessTarget,
    },
    /// Route this home's managed TLDs to MixEngine's own DNS server — roadmap task **T45**.
    ///
    /// **Whole state**, like [`HostsApply`](Self::HostsApply) and
    /// [`PortAccessGrant`](Self::PortAccessGrant): the plan says what this machine should end up
    /// routing, so a second request supersedes the first, "already done" is a comparison rather
    /// than a judgement, and a wiring that drifted is repaired by the operation that created it.
    ResolverApply {
        /// What to route, and how — one variant per OS mechanism.
        plan: ResolverPlan,
    },

    /// Take it away again.
    ///
    /// **Nothing in T45 enqueues one** — the T45 design, D13, on T42's precedent. Uninstall (T87)
    /// is the producer. It ships built, validated and tested because reversing a wiring written
    /// five phases earlier is a worse task than writing both halves while the mechanism is in view.
    ResolverRevoke {
        /// Which mechanism's artifacts to remove.
        target: ResolverTarget,
    },

    /// Make this machine trust MixEngine's own certificate authority — roadmap task **T49a**.
    ///
    /// **The helper refuses a certificate that is not shaped like one T48 generates, and the reason
    /// is not that this stops a compromised daemon.** One holding the CA key can already sign
    /// anything. It is that `mix cert ca-uninstall` (T54) and uninstall (T87) must be able to
    /// enumerate everything an install could ever have put into a store, and an unconstrained one
    /// could leave behind a root called anything at all, which nothing would ever find again.
    TrustCaInstall {
        /// Which store, and the certificate.
        plan: TrustPlan,
    },

    /// Take it back out again.
    ///
    /// **Nothing in T49a enqueues one** — the T49a design, D5, on T42's D12 and T45's D13. T54 and
    /// T87 are the producers. It ships built, validated and tested because reversing a mechanism
    /// phases after it was written is a worse task than writing both halves while it is in view.
    TrustCaRemove {
        /// Which authority, by key-id. Never a fingerprint — see [`TrustTarget`].
        target: TrustTarget,
    },

    /// Let the local network reach the ports a shared site answers on — roadmap task **T74**.
    ///
    /// **Whole state**, like every operation above it, so unsharing the last shared site is this
    /// same operation carrying no ports rather than a revoke of its own.
    FirewallApply {
        /// What should end up open, and under what name.
        plan: FirewallPlan,
    },
}

impl PrivilegedOp {
    /// Every operation this build knows, by wire name.
    ///
    /// Reported in [`PrivilegedResponse::supported_ops`] so a daemon can find out what the installed
    /// helper can do without spending a prompt to discover it by failure.
    pub const ALL: &'static [&'static str] = &[
        "probe",
        "hosts-apply",
        "port-access-grant",
        "port-access-revoke",
        "resolver-apply",
        "resolver-revoke",
        "trust-ca-install",
        "trust-ca-remove",
        "firewall-apply",
    ];

    /// A hosts change from whatever order its caller happened to have.
    ///
    /// Sorted and deduplicated, so two orderings of one change are one operation: the queue
    /// deduplicates on identity (see [`dedupe_key`](Self::dedupe_key)) and the *equality* below it
    /// is what decides whether anything is announced.
    #[must_use]
    pub fn hosts_apply(entries: impl IntoIterator<Item = HostEntry>) -> Self {
        let mut entries: Vec<HostEntry> = entries.into_iter().collect();

        // By name first: this order is what the block is rendered in and what `describe` reads out,
        // and a person scanning a dialog is scanning names.
        entries.sort_by(|left, right| {
            (left.domain.as_str(), left.address).cmp(&(right.domain.as_str(), right.address))
        });
        entries.dedup();

        Self::HostsApply { entries }
    }

    /// The identity a queue deduplicates on — the T41 design, D2.
    ///
    /// For an operation that carries no state this is its serialisation, so two identical requests
    /// are one row. For a **whole-state** operation it is the bare kind: two `hosts-apply` rows
    /// disagreeing about what the file should hold would both be valid and both be rendered on the
    /// one screen whose job is to say what is about to happen, so the newer state supersedes the
    /// older one instead.
    #[must_use]
    pub fn dedupe_key(&self) -> String {
        match self {
            // Falling back to the tag cannot happen — this type holds nothing serde refuses — and
            // is written rather than unwrapped because nothing in this crate panics.
            Self::Probe {} => {
                serde_json::to_string(self).unwrap_or_else(|_| self.name().to_owned())
            }
            Self::HostsApply { .. } => self.name().to_owned(),
            // D12: two values of one question — what port access should this machine have? — so a
            // revoke enqueued behind a pending grant replaces it rather than queueing after it.
            Self::PortAccessGrant { .. } | Self::PortAccessRevoke { .. } => {
                "port-access".to_owned()
            }
            // D4, on the line above's shape: two values of one question — what should this
            // machine route? — so a revoke enqueued behind a pending apply replaces it rather
            // than queueing after it.
            Self::ResolverApply { .. } | Self::ResolverRevoke { .. } => "resolver".to_owned(),
            // The line above's shape once more: two values of one question — what should this
            // machine trust? — so a removal enqueued behind a pending install replaces it rather
            // than queueing after it.
            Self::TrustCaInstall { .. } | Self::TrustCaRemove { .. } => "trust-store".to_owned(),
            // One value of one question — what should this machine have open? — so a second plan
            // enqueued behind a pending one replaces it. Unsharing while a share is still waiting
            // for the prompt therefore leaves nothing to allow, which is the correct outcome.
            Self::FirewallApply { .. } => "firewall".to_owned(),
        }
    }

    /// Does this operation need an administrative token to mean anything?
    ///
    /// **A property of the operation, not a gate on the process.** The obvious frame refuses to do
    /// anything at all when it is not elevated, and `Probe` is what shows that to be wrong: the
    /// operation whose job includes reporting whether the token is elevated could then never report
    /// `false`. The helper applies this at one place, which is what keeps it auditable.
    #[must_use]
    pub fn requires_elevation(&self) -> bool {
        match self {
            Self::Probe {} => false,
            Self::HostsApply { .. } => true,
            Self::PortAccessGrant { .. } | Self::PortAccessRevoke { .. } => true,
            Self::ResolverApply { .. } | Self::ResolverRevoke { .. } => true,
            Self::TrustCaInstall { .. } | Self::TrustCaRemove { .. } => true,
            // True even on macOS, where the helper will answer `Unmanaged` without touching
            // anything: what the machine turns out not to need is decided by the helper, and a
            // planner that decided it here would be deciding it from the wrong side of the trust
            // boundary.
            Self::FirewallApply { .. } => true,
        }
    }

    /// The wire tag, which is also what the audit log records.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Probe {} => "probe",
            Self::HostsApply { .. } => "hosts-apply",
            Self::PortAccessGrant { .. } => "port-access-grant",
            Self::PortAccessRevoke { .. } => "port-access-revoke",
            Self::ResolverApply { .. } => "resolver-apply",
            Self::ResolverRevoke { .. } => "resolver-revoke",
            Self::TrustCaInstall { .. } => "trust-ca-install",
            Self::TrustCaRemove { .. } => "trust-ca-remove",
            Self::FirewallApply { .. } => "firewall-apply",
        }
    }

    /// What this operation will literally change, for a person about to allow it.
    ///
    /// **Derived from the operation rather than stored beside it** — the T40b design, D7. The
    /// alternative is a `summary` written by whoever enqueued the operation and kept in its row,
    /// which is a description that can disagree with what will be applied and would preserve that
    /// disagreement across a restart, on the one screen whose whole job is to tell the truth before
    /// somebody clicks Allow.
    ///
    /// [`String`] and not `&'static str`: the operations that matter carry data — `HostsApply`'s
    /// description is its domains (T41) — and a constant would be a shape the next operation has to
    /// break immediately.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Probe {} => "report the installed helper's version, whether it holds an \
                               administrative token, and where it writes its audit log"
                .to_owned(),
            Self::HostsApply { entries } => describe_hosts(entries),
            Self::PortAccessGrant { plan } => describe_grant(plan),
            Self::PortAccessRevoke { target } => describe_revoke(target),
            Self::ResolverApply { plan } => describe_resolver(plan),
            Self::ResolverRevoke { target } => describe_unwire(target),
            Self::TrustCaInstall { plan } => describe_trust(plan),
            Self::TrustCaRemove { target } => describe_untrust(target),
            Self::FirewallApply { plan } => describe_firewall(plan),
        }
    }
}

/// What a hosts change will literally do, for a person about to allow it.
///
/// The addresses are named when they differ, because the helper permits `::1` as well as
/// `127.0.0.1` (D5) and a description that hid the difference would be describing something else.
fn describe_hosts(entries: &[HostEntry]) -> String {
    let Some(first) = entries.first() else {
        return "remove MixEngine's block from the hosts file".to_owned();
    };

    let uniform = entries.iter().all(|entry| entry.address == first.address);
    let plural = if entries.len() == 1 { "" } else { "s" };

    let names: Vec<String> = entries
        .iter()
        .map(|entry| {
            if uniform {
                entry.domain.clone()
            } else {
                format!("{} ({})", entry.domain, entry.address)
            }
        })
        .collect();

    let at = if uniform {
        first.address.to_string()
    } else {
        "loopback".to_owned()
    };

    format!(
        "point {} name{plural} at {at} in the hosts file: {}",
        entries.len(),
        names.join(", ")
    )
}

/// What a port-access grant will literally do, for a person about to allow it.
///
/// The binary's whole path, never its file name: T42's D11 leaves exactly one control against a
/// compromised daemon pointing the grant at a program of its own choosing, and this is it.
fn describe_grant(plan: &PortAccessPlan) -> String {
    match plan {
        PortAccessPlan::Capability { binary, ports } => format!(
            "let {} bind port{} {} without an administrator, by giving that file the \
             cap_net_bind_service capability",
            binary.display(),
            if ports.len() == 1 { "" } else { "s" },
            list(ports)
        ),
        PortAccessPlan::Redirect { redirects } => format!(
            "send {} on 127.0.0.1 to a port an ordinary program may bind, through a packet-filter \
             anchor, a block in /etc/pf.conf and a boot-time job that enables the packet filter: {}",
            if redirects.len() == 1 {
                "one port"
            } else {
                "two ports"
            },
            redirects
                .iter()
                .map(|redirect| format!("{} to {}", redirect.answer, redirect.bind))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// What taking it away will literally do.
fn describe_revoke(target: &PortAccessTarget) -> String {
    match target {
        PortAccessTarget::Capability { binary } => format!(
            "take the cap_net_bind_service capability back off {}",
            binary.display()
        ),
        PortAccessTarget::Redirect {} => "remove MixEngine's packet-filter anchor, its block in \
                                          /etc/pf.conf and its boot-time job"
            .to_owned(),
    }
}

/// What wiring a resolver will literally do, for a person about to allow it.
///
/// Names the files or the rule, because "wire the resolver" is not a sentence anybody can consent
/// to. The port is named where the mechanism can carry one and omitted where it cannot.
fn describe_resolver(plan: &ResolverPlan) -> String {
    match plan {
        ResolverPlan::ResolverDirectory { tlds, port } => format!(
            "send {} to MixEngine's DNS server on 127.0.0.1:{port}, by writing {}",
            patterns(tlds),
            tlds.iter()
                .map(|tld| format!("/etc/resolver/{tld}"))
                .collect::<Vec<_>>()
                .join(" and ")
        ),
        ResolverPlan::SystemdLink { tlds, port } => format!(
            "send {} to MixEngine's DNS server on 127.0.0.1:{port}, by adding a network link \
             called mixengine0 in /etc/systemd/network and reloading systemd-networkd",
            patterns(tlds)
        ),
        ResolverPlan::Nrpt { tlds } => format!(
            "send {} to MixEngine's DNS server on 127.0.0.1, by adding one Name Resolution Policy \
             rule to this machine",
            patterns(tlds)
        ),
    }
}

/// What unwiring will literally do.
fn describe_unwire(target: &ResolverTarget) -> String {
    match target {
        ResolverTarget::ResolverDirectory {} => {
            "remove the resolver files MixEngine wrote in /etc/resolver".to_owned()
        }
        ResolverTarget::SystemdLink {} => {
            "remove MixEngine's network link and its two files in /etc/systemd/network".to_owned()
        }
        ResolverTarget::Nrpt {} => {
            "remove MixEngine's Name Resolution Policy rule from this machine".to_owned()
        }
    }
}

/// What trusting MixEngine's authority will literally do, for a person about to allow it.
///
/// **The store is named**, because "trust a certificate" and "add a root to this machine's own
/// store, for every account on it" are different sentences and only the second one is true. Nothing
/// is said about the certificate's contents: this screen is read before the helper has checked
/// them, so any claim about them here would be the daemon's word rather than a fact.
fn describe_trust(plan: &TrustPlan) -> String {
    let store = match plan {
        TrustPlan::SystemRoot { .. } => "this machine's Trusted Root Certification Authorities",
        TrustPlan::SystemKeychain { .. } => "this machine's System keychain",
        TrustPlan::CaCertificates { .. } => "/usr/local/share/ca-certificates",
        TrustPlan::CaTrustAnchors { .. } => "/etc/pki/ca-trust/source/anchors",
    };

    format!(
        "add MixEngine's own certificate authority to {store}, so this machine trusts the \
         certificates MixEngine issues for local sites"
    )
}

/// And what removing it will do. The authority is named, because a machine may hold more than one.
fn describe_untrust(target: &TrustTarget) -> String {
    let (store, key_id) = match target {
        TrustTarget::SystemRoot { key_id } => (
            "this machine's Trusted Root Certification Authorities",
            key_id,
        ),
        TrustTarget::SystemKeychain { key_id } => ("this machine's System keychain", key_id),
        TrustTarget::CaCertificates { key_id } => ("/usr/local/share/ca-certificates", key_id),
        TrustTarget::CaTrustAnchors { key_id } => ("/etc/pki/ca-trust/source/anchors", key_id),
    };

    format!("remove MixEngine's certificate authority {key_id} from {store}")
}

/// What a firewall change will literally do, for a person about to allow it.
///
/// The ports, in the order they will be written, and the name the rules will carry — which is what
/// a person needs to go and find them afterwards. An empty plan says the rules are going away,
/// because that is what an empty whole state means.
fn describe_firewall(plan: &FirewallPlan) -> String {
    if plan.ports.is_empty() {
        return format!("remove the firewall rules named \"{}\"", plan.label);
    }

    let ports = plan
        .ports
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "let this machine's local network reach TCP {ports}, as a firewall rule named \"{}\"",
        plan.label
    )
}

/// `*.test, *.internal`, which is what a wildcard route reads as in a sentence.
fn patterns(tlds: &[String]) -> String {
    tlds.iter()
        .map(|tld| format!("*.{tld}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `80 and 443`, the way a sentence names a short list.
fn list(ports: &[u16]) -> String {
    let rendered: Vec<String> = ports.iter().map(u16::to_string).collect();

    match rendered.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// What the helper did, one entry per operation, plus what it is.
///
/// **The report is a property of the response and not the outcome of `Probe`.** Nesting it in
/// [`OpOutcome::Applied`]'s `detail` would put a JSON document inside a JSON string, and would mean
/// the daemon learns what the installed helper can do only on the round trips where it thought to
/// ask. Here it costs a few strings, arrives on every answer, and is read the same way whatever the
/// request contained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PrivilegedResponse {
    /// The protocol the helper speaks.
    pub version: ProtocolVersion,

    /// The helper binary's own version, which is not the protocol's: it is installed once and
    /// excluded from auto-update, so it drifts behind the daemon by design.
    pub elevate_version: String,

    /// Echoed from the request.
    pub nonce: String,

    /// Was this process actually running with an administrative token?
    pub elevated: bool,

    /// [`PrivilegedOp::ALL`] for the build that answered.
    pub supported_ops: Vec<String>,

    /// Where this helper records what it applied — reported whether or not anything was written to
    /// it, so `mix doctor` can find it on a machine where nothing has been applied yet.
    pub audit_log: PathBuf,

    /// One outcome per element of [`PrivilegedRequest::ops`], at the same index.
    pub results: Vec<OpOutcome>,
}

/// What became of one operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum OpOutcome {
    /// Done, and the machine changed.
    Applied {
        /// What changed, for a log line and for `mix doctor`.
        detail: String,
    },

    /// The machine was already in the state this asked for. Not a failure and not a change.
    AlreadyDone,

    /// Validation said no — the caller's fault, and the same request will be refused again.
    Refused {
        /// Which rule it broke.
        reason: String,
    },

    /// This build does not know this operation, or does not understand it as it was written.
    Unsupported {
        /// What could not be decoded.
        reason: String,
    },

    /// The operating system refused. Trying again may work; nothing about the request is wrong.
    Failed {
        /// The OS's own complaint.
        message: String,
    },

    /// This machine has no mechanism for what was asked, and saying so is the honest answer —
    /// roadmap task **T74**.
    ///
    /// **Neither [`Applied`](Self::Applied) nor [`Failed`](Self::Failed), and the distinction is the
    /// point.** macOS's application firewall needs no rule for a listening socket, and a Linux with
    /// neither `ufw` nor `firewalld` running has nothing to add a rule to. Reporting that as success
    /// would tell a user their phone can reach the site when the machine has done nothing to make
    /// it so; reporting it as failure would stop a share that is, on those machines, already
    /// working. So it is its own outcome, carrying the command a person would run if their machine
    /// does turn out to block the port.
    Unmanaged {
        /// Why nothing was done, phrased for a user.
        reason: String,

        /// What to run by hand where the port does turn out to be blocked. Empty where there is
        /// nothing sensible to suggest.
        manual: String,
    },
}

/// What became of one *attempt to elevate* — the outcome of raising the prompt, not of the batch.
///
/// Defined here and used by T40a, where the three launchers live. A declined prompt cannot be an exit
/// code of the helper's, because when the user clicks Cancel the helper never ran: `ERROR_CANCELLED`
/// (1223), osascript's `-128` and `pkexec`'s 126 all map onto [`ElevationOutcome::Declined`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum ElevationOutcome {
    /// The helper ran. Whether the batch succeeded is in the response file.
    Completed,

    /// The user said no. A normal outcome, and the daemon goes into degraded mode (T40b).
    Declined,

    /// There is no way to raise a prompt on this machine — no polkit agent, no session.
    Unavailable {
        /// What is missing, phrased for a user, with the manual command where one exists.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::PROTOCOL_VERSION;

    /// The daemon writes this; the helper reads it. A change to either side that the other did not
    /// make shows up here first.
    #[test]
    fn a_request_round_trips() {
        let request = PrivilegedRequest {
            version: PROTOCOL_VERSION,
            home: PathBuf::from("/home/someone/.mixengine"),
            nonce: "b8f0…".to_owned(),
            ops: vec![serde_json::json!({ "op": "probe" })],
        };

        let encoded = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<PrivilegedRequest>(&encoded).unwrap(),
            request
        );
    }

    /// D3, the reading half: an operation this build has never heard of survives as an undecoded
    /// value and does not take its neighbours down with it.
    #[test]
    fn an_unknown_operation_does_not_fail_the_envelope() {
        let text = r#"{
            "version": 1,
            "home": "/home/someone/.mixengine",
            "nonce": "n",
            "ops": [{ "op": "probe" }, { "op": "trust-ca-install", "der": [1, 2, 3] }]
        }"#;

        let request: PrivilegedRequest = serde_json::from_str(text).unwrap();

        assert_eq!(request.ops.len(), 2);
        assert!(serde_json::from_value::<PrivilegedOp>(request.ops[0].clone()).is_ok());
        assert!(serde_json::from_value::<PrivilegedOp>(request.ops[1].clone()).is_err());
    }

    /// D3, the intolerant half: a field inside an operation this build *does* know is fatal for that
    /// operation. Silently ignoring it is how a weaker version of an operation gets applied.
    #[test]
    fn an_unknown_field_inside_a_known_operation_is_fatal() {
        let value = serde_json::json!({ "op": "probe", "and-also": "something new" });

        assert!(serde_json::from_value::<PrivilegedOp>(value).is_err());
    }

    /// A field the envelope does not know is a daemon speaking a protocol this build does not have.
    #[test]
    fn an_unknown_field_in_the_envelope_is_fatal() {
        let text = r#"{
            "version": 1, "home": "/h", "nonce": "n", "ops": [], "deadline": 5
        }"#;

        assert!(serde_json::from_str::<PrivilegedRequest>(text).is_err());
    }

    /// D5: the one operation that changes nothing is the one that does not need a token.
    #[test]
    fn probe_is_the_operation_that_needs_no_privilege() {
        assert!(!PrivilegedOp::Probe {}.requires_elevation());
    }

    /// `name()` is what goes in the audit log and in `supported_ops`; the tag is what goes on the
    /// wire. Two spellings of one operation would make the log unreadable against the protocol.
    #[test]
    fn the_name_of_an_operation_is_its_wire_tag() {
        let encoded = serde_json::to_value(PrivilegedOp::Probe {}).unwrap();

        assert_eq!(encoded["op"], PrivilegedOp::Probe {}.name());
        assert!(PrivilegedOp::ALL.contains(&PrivilegedOp::Probe {}.name()));
        assert_eq!(PrivilegedOp::ALL.len(), 9, "ALL and the enum have drifted");
    }

    /// The response is read by a daemon that may be older than the helper that wrote it, so an
    /// added field must not make it unreadable. The opposite rule to the request, deliberately.
    #[test]
    fn a_response_tolerates_a_field_the_reader_does_not_know() {
        let text = r#"{
            "version": 1, "elevate-version": "0.1.0", "nonce": "n", "elevated": true,
            "supported-ops": ["probe"], "audit-log": "/var/log/mixengine/elevate.log",
            "results": [{ "outcome": "applied", "detail": "…" }],
            "duration-ms": 4
        }"#;

        let response: PrivilegedResponse = serde_json::from_str(text).unwrap();

        assert_eq!(response.results.len(), 1);
        assert!(response.elevated);
    }

    #[test]
    fn every_outcome_round_trips() {
        let outcomes = vec![
            OpOutcome::Applied {
                detail: "d".to_owned(),
            },
            OpOutcome::AlreadyDone,
            OpOutcome::Refused {
                reason: "r".to_owned(),
            },
            OpOutcome::Unsupported {
                reason: "u".to_owned(),
            },
            OpOutcome::Failed {
                message: "m".to_owned(),
            },
        ];

        let encoded = serde_json::to_string(&outcomes).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<OpOutcome>>(&encoded).unwrap(),
            outcomes
        );
    }

    /// T40a's vocabulary, defined here so that task has a word to use — D11.
    #[test]
    fn a_declined_prompt_is_a_word_rather_than_a_number() {
        let encoded = serde_json::to_string(&ElevationOutcome::Declined).unwrap();

        assert_eq!(encoded, r#"{"outcome":"declined"}"#);
    }

    /// D7: a description is derived from the operation every time it is rendered, so it cannot
    /// disagree with what will actually be applied. What is asserted here is that it says something
    /// a person could act on — the wire tag repeated back is not that.
    #[test]
    fn every_operation_says_what_it_will_change() {
        for op in [
            PrivilegedOp::Probe {},
            PrivilegedOp::hosts_apply([entry("127.0.0.1", "blog.test")]),
            PrivilegedOp::hosts_apply([]),
        ] {
            let described = op.describe();

            assert!(!described.is_empty());
            assert_ne!(described, op.name(), "a tag is not a description");
            assert!(
                described.chars().next().is_some_and(char::is_lowercase),
                "descriptions are rendered in a list and start mid-sentence: {described}"
            );
        }
    }

    /// D1: the operation carries the whole managed block, so two orderings of one change are one
    /// operation and not two rows on the screen that asks a person to allow them.
    #[test]
    fn a_hosts_change_is_a_set_and_not_a_sequence() {
        let one = PrivilegedOp::hosts_apply([
            entry("127.0.0.1", "api.blog.test"),
            entry("127.0.0.1", "blog.test"),
        ]);
        let other = PrivilegedOp::hosts_apply([
            entry("127.0.0.1", "blog.test"),
            entry("127.0.0.1", "api.blog.test"),
            entry("127.0.0.1", "blog.test"),
        ]);

        assert_eq!(
            one, other,
            "order and repetition are not part of the request"
        );
    }

    /// D2: a whole-state operation deduplicates on its *kind*, so a newer state supersedes an older
    /// one rather than queueing beside it. `Probe`'s key is unchanged, which is what makes the
    /// column need no migration.
    #[test]
    fn a_whole_state_operation_deduplicates_on_its_kind() {
        let one = PrivilegedOp::hosts_apply([entry("127.0.0.1", "blog.test")]);
        let other = PrivilegedOp::hosts_apply([entry("127.0.0.1", "shop.test")]);

        assert_eq!(one.dedupe_key(), other.dedupe_key());
        assert_eq!(one.dedupe_key(), "hosts-apply");
        assert_ne!(
            one, other,
            "the same key, and deliberately not the same operation"
        );

        assert_eq!(
            PrivilegedOp::Probe {}.dedupe_key(),
            serde_json::to_string(&PrivilegedOp::Probe {}).unwrap(),
            "Probe's key is still its serialisation, so no row in an existing home moves"
        );
    }

    /// It needs a token, unlike `Probe`, and the helper's one gate is what reads this.
    #[test]
    fn writing_the_hosts_file_needs_an_administrative_token() {
        assert!(PrivilegedOp::hosts_apply([entry("127.0.0.1", "blog.test")]).requires_elevation());
        assert_eq!(
            PrivilegedOp::hosts_apply([]).name(),
            "hosts-apply",
            "the tag is the audit log's word for it"
        );
    }

    /// The screen T64 renders exists to be read before somebody clicks Allow, so the description is
    /// the domains themselves and not a count.
    #[test]
    fn a_hosts_change_describes_itself_by_naming_every_domain() {
        let described = PrivilegedOp::hosts_apply([
            entry("127.0.0.1", "blog.test"),
            entry("127.0.0.1", "api.blog.test"),
        ])
        .describe();

        assert!(described.contains("blog.test"), "{described}");
        assert!(described.contains("api.blog.test"), "{described}");
        assert!(described.contains("127.0.0.1"), "{described}");

        assert_eq!(
            PrivilegedOp::hosts_apply([]).describe(),
            "remove MixEngine's block from the hosts file"
        );
    }

    /// The request is intolerant, and an operation that carries data is where that matters.
    #[test]
    fn a_hosts_entry_with_a_field_this_build_does_not_know_is_fatal() {
        let value = serde_json::json!({
            "op": "hosts-apply",
            "entries": [{ "address": "127.0.0.1", "domain": "blog.test", "comment": "hi" }]
        });

        assert!(serde_json::from_value::<PrivilegedOp>(value).is_err());
    }

    #[test]
    fn a_hosts_change_round_trips() {
        let op = PrivilegedOp::hosts_apply([entry("::1", "blog.test")]);

        let encoded = serde_json::to_string(&op).unwrap();
        assert_eq!(serde_json::from_str::<PrivilegedOp>(&encoded).unwrap(), op);
        assert!(encoded.contains(r#""op":"hosts-apply""#), "{encoded}");
    }

    /// A `HostEntry` for a test, from the two strings a reader recognises.
    fn entry(address: &str, domain: &str) -> HostEntry {
        HostEntry {
            address: address.parse().expect("a literal address"),
            domain: domain.to_owned(),
        }
    }

    /// D12: they are two values of one question — *what port access should this machine have?* — so
    /// the guarded upsert supersedes rather than queues, and execution order never has to be
    /// reasoned about.
    #[test]
    fn granting_and_revoking_port_access_are_one_row() {
        let grant = PrivilegedOp::PortAccessGrant {
            plan: PortAccessPlan::Capability {
                binary: PathBuf::from("/home/someone/.mixengine/packages/caddy/caddy"),
                ports: vec![80, 443],
            },
        };
        let revoke = PrivilegedOp::PortAccessRevoke {
            target: PortAccessTarget::Redirect {},
        };

        assert_eq!(grant.dedupe_key(), "port-access");
        assert_eq!(revoke.dedupe_key(), "port-access");
        assert_ne!(
            grant, revoke,
            "the same key, and deliberately not the same operation"
        );
    }

    /// Both write outside the home, so both need the token — and the helper's one gate reads this.
    #[test]
    fn port_access_needs_an_administrative_token_in_both_directions() {
        assert!(
            PrivilegedOp::PortAccessGrant {
                plan: PortAccessPlan::Redirect {
                    redirects: vec![PortRedirect {
                        answer: 80,
                        bind: 8080
                    }],
                },
            }
            .requires_elevation()
        );
        assert!(
            PrivilegedOp::PortAccessRevoke {
                target: PortAccessTarget::Capability {
                    binary: PathBuf::from("/x/caddy")
                },
            }
            .requires_elevation()
        );
    }

    /// The screen T64 renders is read before somebody clicks Allow. D11 leaves exactly one control
    /// against a compromised daemon pointing a grant at a binary of its own choosing, and it is that
    /// the whole path is printed here.
    #[test]
    fn a_port_access_change_describes_what_it_will_do_to_the_machine() {
        let capability = PrivilegedOp::PortAccessGrant {
            plan: PortAccessPlan::Capability {
                binary: PathBuf::from("/home/someone/.mixengine/packages/caddy/caddy"),
                ports: vec![80, 443],
            },
        }
        .describe();

        assert!(capability.contains("/home/someone"), "{capability}");
        assert!(capability.contains("80"), "{capability}");
        assert!(capability.contains("443"), "{capability}");

        let redirect = PrivilegedOp::PortAccessGrant {
            plan: PortAccessPlan::Redirect {
                redirects: vec![PortRedirect {
                    answer: 80,
                    bind: 8080,
                }],
            },
        }
        .describe();

        assert!(redirect.contains("8080"), "{redirect}");

        let taken = PrivilegedOp::PortAccessRevoke {
            target: PortAccessTarget::Capability {
                binary: PathBuf::from("/x/caddy"),
            },
        }
        .describe();

        assert!(taken.contains("/x/caddy"), "{taken}");
    }

    #[test]
    fn both_port_access_operations_round_trip() {
        for op in [
            PrivilegedOp::PortAccessGrant {
                plan: PortAccessPlan::Capability {
                    binary: PathBuf::from("/x/caddy"),
                    ports: vec![80],
                },
            },
            PrivilegedOp::PortAccessRevoke {
                target: PortAccessTarget::Redirect {},
            },
        ] {
            let encoded = serde_json::to_string(&op).unwrap();

            assert_eq!(serde_json::from_str::<PrivilegedOp>(&encoded).unwrap(), op);
            assert!(PrivilegedOp::ALL.contains(&op.name()), "{}", op.name());
        }
    }

    /// D3's intolerant half, on the operation that carries the most data: a field this build does
    /// not know, inside one it thinks it understands, is fatal — or a weaker grant gets applied and
    /// nobody finds out.
    #[test]
    fn a_port_access_plan_with_a_field_this_build_does_not_know_is_fatal() {
        let value = serde_json::json!({
            "op": "port-access-grant",
            "plan": { "method": "capability", "binary": "/x/caddy", "ports": [80], "force": true }
        });

        assert!(serde_json::from_value::<PrivilegedOp>(value).is_err());
    }

    /// The T45 design, D3, written as a test so that putting the field back is a failure rather
    /// than a review comment. The plan names TLDs and a port. It cannot name an address, because a
    /// request that could name one is a request that could point this machine's name resolution
    /// anywhere — with a valid signature and the user's own Allow click.
    #[test]
    fn a_resolver_plan_cannot_name_a_nameserver() {
        let plan = ResolverPlan::ResolverDirectory {
            tlds: vec!["test".to_owned()],
            port: 53_535,
        };

        let json = serde_json::to_string(&plan).expect("it serialises");

        assert!(!json.contains("127.0.0.1"), "{json}");
        assert!(!json.contains("nameserver"), "{json}");
        assert!(!json.contains("address"), "{json}");
    }

    /// D4: two answers to one question — what should this machine route? — so a revoke enqueued
    /// behind a pending apply replaces it rather than queueing after it.
    #[test]
    fn both_directions_of_resolver_share_one_dedupe_key() {
        let apply = PrivilegedOp::ResolverApply {
            plan: ResolverPlan::Nrpt {
                tlds: vec!["test".to_owned()],
            },
        };
        let revoke = PrivilegedOp::ResolverRevoke {
            target: ResolverTarget::Nrpt {},
        };

        assert_eq!(apply.dedupe_key(), revoke.dedupe_key());
        assert_eq!(apply.dedupe_key(), "resolver");
        assert_ne!(
            apply.dedupe_key(),
            PrivilegedOp::hosts_apply([]).dedupe_key()
        );
    }

    /// Both need a token, and both are named in the audit log by their wire tag.
    #[test]
    fn resolver_operations_need_a_token_and_have_names() {
        let apply = PrivilegedOp::ResolverApply {
            plan: ResolverPlan::SystemdLink {
                tlds: vec!["test".to_owned()],
                port: 53_535,
            },
        };
        let revoke = PrivilegedOp::ResolverRevoke {
            target: ResolverTarget::SystemdLink {},
        };

        assert!(apply.requires_elevation());
        assert!(revoke.requires_elevation());
        assert_eq!(apply.name(), "resolver-apply");
        assert_eq!(revoke.name(), "resolver-revoke");
        assert!(PrivilegedOp::ALL.contains(&"resolver-apply"));
        assert!(PrivilegedOp::ALL.contains(&"resolver-revoke"));
    }

    /// T64's screen prints this before anything is raised, so it says the names and the files
    /// rather than the mechanism's jargon.
    #[test]
    fn a_resolver_apply_describes_the_names_and_the_port() {
        let described = PrivilegedOp::ResolverApply {
            plan: ResolverPlan::ResolverDirectory {
                tlds: vec!["test".to_owned(), "internal".to_owned()],
                port: 53_535,
            },
        }
        .describe();

        assert!(described.contains("test"), "{described}");
        assert!(described.contains("internal"), "{described}");
        assert!(described.contains("53535"), "{described}");
        assert!(described.contains("/etc/resolver"), "{described}");
    }

    /// Windows has nowhere to put a port, so its description does not invent one.
    #[test]
    fn an_nrpt_plan_describes_no_port() {
        let described = PrivilegedOp::ResolverApply {
            plan: ResolverPlan::Nrpt {
                tlds: vec!["test".to_owned()],
            },
        }
        .describe();

        assert!(described.contains("test"), "{described}");
        assert!(!described.contains("port"), "{described}");
    }

    /// The intolerant half of the wire contract, as every other operation has it: an older helper
    /// must refuse a field it does not understand rather than apply a weaker operation quietly.
    #[test]
    fn a_resolver_plan_with_an_unknown_field_is_refused() {
        let value = serde_json::json!({
            "op": "resolver-apply",
            "plan": { "method": "nrpt", "tlds": ["test"], "only-if": "something new" }
        });

        assert!(serde_json::from_value::<PrivilegedOp>(value).is_err());
    }

    /// D3: the plan carries the certificate itself and never a path to one.
    #[test]
    fn a_trust_plan_carries_the_certificate_and_not_a_path_to_it() {
        let encoded =
            serde_json::to_value(TrustPlan::SystemRoot { der: vec![1, 2, 3] }).expect("it encodes");

        assert_eq!(encoded["method"], "system-root");
        assert_eq!(encoded["der"], serde_json::json!([1, 2, 3]));
        assert!(
            encoded.get("path").is_none(),
            "a path is somebody else choosing which file root reads: {encoded}"
        );
    }

    /// D3's intolerant half: a helper that ignored a field inside a plan it thought it understood
    /// would apply a weaker version of it and tell nobody.
    #[test]
    fn a_trust_plan_with_a_field_this_build_does_not_know_is_refused() {
        let value = serde_json::json!({ "method": "system-root", "der": [1], "and": "this" });

        assert!(serde_json::from_value::<TrustPlan>(value).is_err());
    }

    /// D5: there is no field a fingerprint could travel in, and that is this type's whole security
    /// decision. A removal that named a certificate could name the root that validates Windows
    /// Update; eight hex characters cannot describe one.
    #[test]
    fn a_trust_removal_names_an_authority_and_never_a_certificate() {
        let target = TrustTarget::SystemKeychain {
            key_id: "deadbeef".to_owned(),
        };

        let encoded = serde_json::to_string(&target).expect("it encodes");

        assert!(encoded.contains("deadbeef"), "{encoded}");
        assert!(
            !encoded.contains("fingerprint"),
            "a fingerprint field is what would let a compromised daemon name a corporate root:              {encoded}"
        );
    }

    /// D3: the operation the fixtures in `mixengine-elevate` and `mixengine-core` already spell.
    #[test]
    fn a_trust_install_is_tagged_the_way_the_existing_fixtures_spell_it() {
        let op = PrivilegedOp::TrustCaInstall {
            plan: TrustPlan::SystemRoot { der: vec![1, 2, 3] },
        };

        let encoded = serde_json::to_value(&op).expect("it encodes");

        assert_eq!(encoded["op"], "trust-ca-install");
        assert_eq!(encoded["plan"]["method"], "system-root");
        assert_eq!(encoded["plan"]["der"], serde_json::json!([1, 2, 3]));
    }

    /// Two values of one question, as the resolver pair is — D3, on T45's D4.
    #[test]
    fn installing_and_removing_supersede_each_other_in_the_queue() {
        let install = PrivilegedOp::TrustCaInstall {
            plan: TrustPlan::SystemRoot { der: vec![1] },
        };
        let remove = PrivilegedOp::TrustCaRemove {
            target: TrustTarget::SystemRoot {
                key_id: "deadbeef".to_owned(),
            },
        };

        assert_eq!(install.dedupe_key(), remove.dedupe_key());
        assert_eq!(install.dedupe_key(), "trust-store");
    }

    /// Or a daemon has to spend a prompt to discover what the installed helper can do.
    #[test]
    fn both_new_operations_are_in_the_reported_list() {
        assert!(PrivilegedOp::ALL.contains(&"trust-ca-install"));
        assert!(PrivilegedOp::ALL.contains(&"trust-ca-remove"));
    }

    /// Both need a token, because both change a store no ordinary account may write.
    #[test]
    fn trusting_and_untrusting_both_need_an_administrative_token() {
        assert!(
            PrivilegedOp::TrustCaInstall {
                plan: TrustPlan::SystemRoot { der: vec![1] },
            }
            .requires_elevation()
        );
        assert!(
            PrivilegedOp::TrustCaRemove {
                target: TrustTarget::SystemRoot {
                    key_id: "deadbeef".to_owned(),
                },
            }
            .requires_elevation()
        );
    }

    /// The screen whose whole job is to say what is about to happen names the store, because
    /// "trust a certificate" and "add a root for every account on this machine" are different
    /// sentences and only the second is true.
    #[test]
    fn what_a_person_is_asked_to_allow_names_the_store() {
        let described = PrivilegedOp::TrustCaInstall {
            plan: TrustPlan::SystemKeychain { der: vec![1] },
        }
        .describe();

        assert!(described.contains("keychain"), "{described}");

        let described = PrivilegedOp::TrustCaRemove {
            target: TrustTarget::CaCertificates {
                key_id: "deadbeef".to_owned(),
            },
        }
        .describe();

        assert!(described.contains("deadbeef"), "{described}");
        assert!(described.contains("ca-certificates"), "{described}");
    }
}
