//! What `site.*` asks and answers: what is served out of a project's directory, and at what name.
//!
//! [`crate::project_api`]'s shape one table down, with one difference the schema forces:
//! a site has no name column, so it is addressed by a domain — which `site_domains_domain` already
//! makes globally unique — or by a directory (spec D5).
//!
//! # Unrepresentable rather than merely undocumented
//!
//! Three shapes here exist to delete states nothing should be able to spell. [`SiteKind`] is tagged,
//! so a static site with an upstream cannot be written down. Domains travel as an ordered list whose
//! head is the primary, so "no primary" and "two primaries" are not values. And
//! [`SitePool::declared`] is an [`Option`] because `sites.php_service_id` is `ON DELETE SET NULL`:
//! a type that could not say [`None`] would be a type lying about a row the database can produce.

use crate::{ProjectRef, ServiceId, ServiceState};

/// What a site serves, and what that kind needs to know.
///
/// **Internally tagged**, which is what lets one definition read a JSON-RPC request and a flat
/// `[site]` table in TOML with no conversion in between (spec D7).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SiteKind {
    /// PHP through a php-fpm pool.
    PhpFpm {
        /// The pool, which the daemon fills from `core::resolve` when a create does not name one.
        ///
        /// [`None`] means *this site names no pool*, in both directions: on the way in, decide it;
        /// on the way out, the pool it named has been deleted (spec D3).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pool: Option<ServiceId>,
    },

    /// Files, and nothing running.
    Static,

    /// Everything forwarded to an address the user already has listening.
    ReverseProxy {
        /// An absolute `http` or `https` URL with a host. A path is allowed; a query is not.
        upstream: String,
    },

    /// A node process the user runs, on this port.
    ///
    /// **A declaration and no more.** Nothing in this build starts `npm run dev`; what distinguishes
    /// this from [`SiteKind::ReverseProxy`] is the scope of the address rather than a mechanism.
    NodeApp {
        /// The loopback port it listens on.
        port: u16,
    },
}

/// Which site a call is about.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteRef {
    /// Any of its domains — the primary or an alias, which the unique index makes unambiguous.
    Domain(String),

    /// Any directory at or inside its project's root. The **nearest** registered root wins, and a
    /// project holding several sites is refused rather than guessed at.
    Path(String),
}

/// Whether the web server should have a server block for this site.
///
/// Two words, because a site is not a process: `starting`, `running` and `failed` belong to the
/// services it uses, which have seven states of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteState {
    /// Rendered and served.
    Enabled,

    /// Declared and deliberately not rendered.
    Disabled,
}

impl SiteState {
    /// The word the database and the wire both use.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }
}

/// Create a site under a project.
///
/// Every field but the project falls through — the argument, then `[site]` in the project's
/// manifest, then a default — which is what makes `site.create { project }` the import (spec D7).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SiteCreate {
    /// Which project it belongs to.
    pub project: ProjectRef,

    /// Ordered; the head is the primary. Falls through to `[site] domain` + `aliases`, then to
    /// `<slug>.test`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domains: Option<Vec<String>>,

    /// Absolute, or relative to the root. Falls through to `[site] doc_root`, then to the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_root: Option<String>,

    /// Falls through to `[site]`, then to php-fpm with the resolved pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SiteKind>,

    /// The services it declares. Falls through to `[[services]]`, then to none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub services: Option<Vec<ServiceId>>,

    /// Whether HTTPS is wanted. A declaration Phase 5 reads; nothing today acts on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub https: Option<bool>,

    /// `.local`, acknowledged. `--i-know` on the CLI.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub accept_risky_tld: bool,
}

/// Change what a site is.
///
/// `domains` and `services` **replace** rather than merge, on [`crate::ProjectUpdate`]'s rule and
/// for its reason: with a merge there is no way to remove one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SiteUpdate {
    /// Which site.
    pub site: SiteRef,

    /// The domains, replacing the list the site had. The head becomes the primary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domains: Option<Vec<String>>,

    /// A new doc root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_root: Option<String>,

    /// A new kind, payload and all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SiteKind>,

    /// The services, replacing the links the site had.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub services: Option<Vec<ServiceId>>,

    /// Whether HTTPS is wanted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub https: Option<bool>,

    /// Whether the web server should serve it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<SiteState>,

    /// `.local`, acknowledged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub accept_risky_tld: bool,
}

/// Which site `site.show` and `site.delete` are about.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SiteQuery {
    /// Which site.
    pub site: SiteRef,
}

/// Which site to put on the local network, and on which interface — roadmap task **T74**.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SiteShare {
    /// Which site.
    pub site: SiteRef,

    /// The interface to share on, by the name this machine gives it.
    ///
    /// [`None`] where the machine has exactly one candidate, which is the ordinary case. Where it
    /// has more than one the daemon refuses and names them all rather than choosing — a machine
    /// that picked would put a site on a network the user did not mean to be on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,

    /// How long this share should last, in seconds, or [`None`] for one with no end — roadmap task
    /// **T76**.
    ///
    /// **Measured from when the share began and not from this request** — the T76 design, D6. T74
    /// preserves `shared_since` across a repeated share precisely so that typing the command again
    /// extends nothing, and a deadline that restarted would undo that. A value that lands in the
    /// past is refused rather than honoured: a URL that is dead by the time it is printed is worse
    /// than a sentence saying so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_seconds: Option<u64>,
}

/// Which sites a listing should answer with.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct SiteListQuery {
    /// One project's sites, or every site in this home.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectRef>,
}

/// What `site.list` answers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SiteList {
    /// Every site asked for, in primary-domain order.
    pub sites: Vec<SiteSummary>,
}

/// One site, as a listing shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SiteSummary {
    /// Its primary domain, which is also how it is addressed.
    pub domain: String,

    /// The project it belongs to, by the name that is the project's wire handle.
    pub project: String,

    /// What it serves.
    pub kind: SiteKind,

    /// Relative to the project's root, as stored. `""` is the root itself.
    pub doc_root: String,

    /// Whether HTTPS is declared.
    pub https: bool,

    /// Whether the web server should serve it.
    pub state: SiteState,

    /// Where the local network can reach it, when it can — roadmap task **T74**.
    ///
    /// **On the summary and not only on the detail**, because "what is exposed right now" is a
    /// question about every site at once: a list that could not answer it would make a client ask
    /// per site, and the one thing a person wants at a glance is whether *anything* is shared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sharing: Option<SiteSharing>,
}

/// A site the local network can reach, and how — roadmap task **T74**.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SiteSharing {
    /// The interface it is shared on, by the name the OS gives it.
    pub interface: String,

    /// The IPv4 address bound, certified and printed. IPv4 only — the T74 design, D4.
    pub address: String,

    /// What to open, ready to hand to a browser or draw as a QR code.
    ///
    /// **HTTP, and the address rather than the name** — the T75 design, D11. The certificate covers
    /// this address and this site's [`name`](Self::name) both, but a phone does not trust this
    /// home's authority until it has installed it from [`ca_url`](Self::ca_url), and Android's
    /// resolver does not answer `.local` for a browser. A URL a person is told to open must be one
    /// that opens, on the device they happen to be holding.
    pub url: String,

    /// The mDNS name this site answers to, `<slug>-mixengine.local` — roadmap task **T75**.
    ///
    /// **One label before `.local`**, which is measured rather than chosen: a multi-label name
    /// under `.local` does not resolve. Present whenever the site is shared and its primary domain
    /// yields a label, whatever the responder is doing — the name is in the configuration and in
    /// the certificate either way, and [`advertised`](Self::advertised) is the separate question of
    /// whether anything is answering for it.
    pub name: Option<String>,

    /// Whether this daemon is currently answering mDNS queries for [`name`](Self::name).
    ///
    /// `false` on a home where UDP 5353 could not be bound, or where a firewall blocks it. The
    /// share still works by address, which is what T74 shipped; a client says so rather than
    /// printing a name as though it resolved.
    pub advertised: bool,

    /// Where a phone downloads this home's certificate authority — roadmap task **T75**.
    ///
    /// Served by the front end from this site's own block, and only while the site is shared. The
    /// public certificate and nothing else: the signing key is not in any directory a front end is
    /// pointed at.
    pub ca_url: String,

    /// When sharing began.
    pub since: crate::Timestamp,

    /// When this share ends by itself, or [`None`] for one that does not — roadmap task **T76**.
    ///
    /// Set by `--for`, and measured from [`since`](Self::since) rather than from the request that
    /// set it. A share also ends when this machine leaves the network it was shared on, which is
    /// not a deadline and so is not here — it arrives as
    /// [`SiteSharingChanged`](crate::DaemonEvent::SiteSharingChanged) when it happens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<crate::Timestamp>,
}

/// Why a site's sharing changed — roadmap task **T76**.
///
/// **Internally tagged under `kind`, not `type`.** It travels inside
/// [`DaemonEvent::SiteSharingChanged`](crate::DaemonEvent), which is itself tagged `type`, and two
/// discriminators spelled the same word collide the moment the outer variant flattens — the lesson
/// [`JobFinish`](crate::JobFinish) paid for with `ending`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SharingChange {
    /// Somebody asked — `site.share` or `site.unshare`.
    ///
    /// An empty struct variant rather than a unit one, on
    /// [`Outcome`](crate::doctor_api::Outcome)'s rule: `deny_unknown_fields` never fires on a unit
    /// variant of an internally tagged enum.
    Requested {},

    /// The length it was shared for ran out.
    Expired {},

    /// This machine is not on the network the site was shared on.
    ///
    /// **Both addresses, because the pair is the explanation.** "The network changed" is a sentence
    /// nobody can verify or act on; `192.168.1.10` became `10.0.0.4` is one somebody can read off
    /// their own router.
    NetworkChanged {
        /// The address that was bound and written into the certificate.
        was: String,

        /// What the interface holds now, or [`None`] where the interface itself is gone.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        now: Option<String>,
    },
}

/// One site, and everything only a lookup can answer.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SiteDetail {
    /// The site itself.
    pub site: SiteSummary,

    /// Its project's root.
    pub root: String,

    /// Root plus doc root, as the filesystem spells it.
    pub doc_root_full: String,

    /// Whether that directory is there.
    ///
    /// **Reported, never refused** (spec D2): a colleague's `public/` is built by `npm run build`,
    /// and a create that refused it would refuse the case the import path exists for.
    pub doc_root_exists: bool,

    /// Every domain, ordered, the head being the primary.
    pub domains: Vec<String>,

    /// The php-fpm pool, both answers, for a php-fpm site.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<SitePool>,

    /// The services it declares, and what each is doing.
    pub services: Vec<SiteServiceLink>,
}

/// What the row names, and what the resolver would name today.
///
/// **Two fields because the row cannot remember who chose it.** A pool is frozen at create while the
/// project's shell keeps following the default, so 8.3.35 arriving tomorrow moves the shell and not
/// the site — and these two side by side are how a person sees that rather than guesses at it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SitePool {
    /// What the row holds. [`None`] after a `service.delete --force` took the pool away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared: Option<ServiceId>,

    /// What `core::resolve` answers at this root today, when it answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<ServiceId>,
}

/// One service a site declares.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SiteServiceLink {
    /// Which service.
    pub service: ServiceId,

    /// What it is doing, so a listing does not need a second call per link.
    pub state: ServiceState,
}

/// What `site.create` answers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SiteCreation {
    /// The site that now exists, as `site.show` would answer it.
    pub site: SiteDetail,
}

/// What `site.delete` answers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SiteRemoval {
    /// The site as it stood before its row went.
    pub removed: SiteSummary,

    /// Freed for another site, and said out loud.
    pub domains_released: Vec<String>,

    /// The files were never ours — [`crate::ProjectRemoval::root_kept`]'s rule.
    pub doc_root_kept: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// **D3.** The tag is the word a person types, and the payload rides beside it — which is what
    /// makes one type serve both the wire and a flat `[site]` table in TOML.
    #[test]
    fn a_kind_carries_its_own_payload_and_nothing_it_has_no_use_for() {
        let proxy: SiteKind = serde_json::from_value(
            json!({"kind": "reverse-proxy", "upstream": "http://127.0.0.1:8080"}),
        )
        .expect("a proxy");
        assert_eq!(
            proxy,
            SiteKind::ReverseProxy {
                upstream: "http://127.0.0.1:8080".to_owned()
            }
        );

        // A proxy with no address is refused by the definition rather than by a check somebody has
        // to remember to write.
        assert!(serde_json::from_value::<SiteKind>(json!({"kind": "reverse-proxy"})).is_err());

        // And a php-fpm site that names no pool is spellable, because `ON DELETE SET NULL` can
        // produce one.
        let php: SiteKind = serde_json::from_value(json!({"kind": "php-fpm"})).expect("a php site");
        assert_eq!(php, SiteKind::PhpFpm { pool: None });
    }

    /// **D5.** A site is reached by any of its domains or by any directory inside its project.
    #[test]
    fn a_site_is_named_by_a_domain_or_by_a_directory() {
        let by_domain: SiteRef =
            serde_json::from_value(json!({"domain": "blog.test"})).expect("a domain");
        let by_path: SiteRef =
            serde_json::from_value(json!({"path": "/srv/blog/public"})).expect("a path");

        assert_eq!(by_domain, SiteRef::Domain("blog.test".to_owned()));
        assert_eq!(by_path, SiteRef::Path("/srv/blog/public".to_owned()));
    }

    /// **D7.** `site.create { project }` with nothing else typed is a whole request: everything
    /// falls through to the manifest and then to a default.
    #[test]
    fn a_create_that_names_only_a_project_is_a_whole_request() {
        let create: SiteCreate =
            serde_json::from_value(json!({"project": {"name": "blog"}})).expect("a create");

        assert_eq!(create.domains, None);
        assert_eq!(create.doc_root, None);
        assert_eq!(create.kind, None);
        assert!(!create.accept_risky_tld);
    }
}
