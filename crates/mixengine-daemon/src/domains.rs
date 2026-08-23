//! `domain.*` — roadmap task **T46**.
//!
//! Two verbs and a diagnostic. The verbs are **thin over [`Sites`] on purpose**: each computes a new
//! domain list and hands it to the same update the `site.*` half uses, so the TLD check, the hosts
//! queueing and the front-end re-render have exactly one implementation between them (T46 design,
//! D1). What is here that is not there is the *reason* a caller cannot compose them itself — a read,
//! an append and a write back is a race with whatever another client did in between.

pub(crate) mod lookup;

use std::sync::Arc;

use mixengine_proto::{
    DomainAdd, DomainRemove, DomainStatus, DomainStatusQuery, DomainStatusReport, Error,
    SiteDetail, SiteRef,
};

use crate::error::ToWire as _;
use crate::sites::Sites;

/// The `domain.*` half of the API.
#[derive(Debug)]
pub(crate) struct Domains {
    /// What actually writes a site.
    sites: Arc<Sites>,

    /// The rows, for the diagnostic's "which site declares this" and nothing else.
    store: mixengine_core::Store,

    /// The server, for its address and for which TLDs this machine routes — T44 and T45.
    dns: Arc<crate::dns::Dns>,

    /// This machine, for the one fact that is on disk rather than in the database.
    host: Arc<dyn mixengine_platform::Host>,
}

impl Domains {
    /// How long any one instrument is waited on.
    ///
    /// Short, because both are loopback or a stub resolver: a machine that has not answered in two
    /// seconds is answering "no" in every way a person cares about, and a diagnostic that takes a
    /// minute to say so is one nobody runs a second time.
    const PATIENCE: std::time::Duration = std::time::Duration::from_secs(2);

    /// The one of these the API holds.
    pub(crate) fn new(
        sites: Arc<Sites>,
        store: &mixengine_core::Store,
        dns: Arc<crate::dns::Dns>,
        host: Arc<dyn mixengine_platform::Host>,
    ) -> Arc<Self> {
        Arc::new(Self {
            sites,
            store: store.clone(),
            dns,
            host,
        })
    }

    /// Give a site one more name.
    ///
    /// **The new name goes on the end**, so the primary stays the primary: `site.update` reorders,
    /// and a verb that says "add a domain" does not get to decide what a site is (T46 design, D3).
    ///
    /// # Errors
    ///
    /// Whatever [`Sites::replace_domains`] refuses — an unknown site, a public TLD, `.local` without
    /// the acknowledgement, a domain another site already holds.
    pub(crate) async fn add(&self, add: &DomainAdd) -> Result<SiteDetail, Error> {
        let (site, _project) = self.sites.expect(&add.site).await?;

        let domains = mixengine_core::domains::after_adding(&site.domains, &add.domain);

        self.sites
            .replace_domains(&add.site, domains, add.accept_risky_tld)
            .await
    }

    /// Take one name away.
    ///
    /// # Errors
    ///
    /// `not_found` for a name nothing declares — answered by [`Sites::expect`], which already says
    /// where to look — and `conflict` for a site's last domain or its primary.
    pub(crate) async fn remove(&self, remove: &DomainRemove) -> Result<SiteDetail, Error> {
        // The domain names its own site: `site_domains_domain` is `UNIQUE`, which is why the request
        // carries no site to disagree with (T46 design, D2).
        let site = SiteRef::Domain(remove.domain.to_ascii_lowercase());
        let (record, _project) = self.sites.expect(&site).await?;

        let domains = mixengine_core::domains::after_removing(&record.domains, &remove.domain)
            .map_err(|error| error.to_wire())?;

        // Acknowledged rather than asked for again: a `.local` still on this list was accepted when
        // it was added, and refusing to *remove* a domain because of the TLD of a domain being kept
        // would be a refusal nobody could act on.
        self.sites.replace_domains(&site, domains, true).await
    }

    /// What actually happens to a name — roadmap task **T46**.
    ///
    /// Four facts, three of them read and one asked. Nothing here decides whether the name *works*:
    /// they fail independently and a client renders the one that is wrong (T46 design, D4).
    ///
    /// # Errors
    ///
    /// `core::domains`' refusals for a request carrying something that is not a domain, and a
    /// database that cannot be read.
    pub(crate) async fn status(
        &self,
        query: &DomainStatusQuery,
    ) -> Result<DomainStatusReport, Error> {
        let records = mixengine_core::sites::records(&self.store, None)
            .await
            .map_err(|error| error.to_wire())?;

        // Every declared name to the primary of the site declaring it, so a row can name its site
        // without a query each.
        let declared: std::collections::BTreeMap<String, String> = records
            .iter()
            .flat_map(|record| {
                let primary = record.domains.first().cloned().unwrap_or_default();
                record
                    .domains
                    .iter()
                    .map(move |domain| (domain.clone(), primary.clone()))
            })
            .collect();

        let asked: Vec<String> = match &query.domain {
            // Through the one module that owns the policy, so a request carrying something that is
            // not a domain at all is refused as one. `.local` is accepted here because asking about
            // a name is not declaring one — D5.
            Some(domain) => {
                vec![
                    mixengine_core::domains::normalised(domain, true)
                        .map_err(|error| error.to_wire())?,
                ]
            }
            None => declared.keys().cloned().collect(),
        };

        let wired = self.dns.wired();
        let server = self.dns.address();

        // A hosts file that cannot be read is a home with no entries in it as far as this report is
        // concerned, which is what `Elevation::require_hosts` already assumes one line over.
        let block = self.host.hosts_file().managed().unwrap_or_default();

        let mut domains = Vec::with_capacity(asked.len());

        for domain in asked {
            let tld = domain.rsplit('.').next().unwrap_or_default();
            let wildcard = wired.iter().any(|one| one == tld);
            let hosts_entry = block.iter().any(|entry| entry.domain == domain);

            let resolves_to = lookup::resolves(&domain, Self::PATIENCE).await;

            let server_answers = match server {
                Some(address) => lookup::server_answers(address, &domain, Self::PATIENCE).await,
                None => None,
            };

            let site = declared.get(&domain).cloned();

            domains.push(DomainStatus {
                because: why_not(site.as_deref(), hosts_entry, wildcard, &resolves_to),
                domain,
                site,
                hosts_entry,
                wildcard,
                server_answers,
                resolves_to,
            });
        }

        Ok(DomainStatusReport { domains })
    }
}

/// One sentence for a name that will not work, or [`None`].
///
/// **The first thing that is wrong, in the order a person would fix them**, rather than every fact
/// restated as prose: a name nothing declares is not *also* missing a hosts entry, it is undeclared,
/// and saying both would bury the one that matters.
///
/// It never says what to do. Repair is T47's, and advice written here would drift from the thing
/// that performs it (T46 design, D4).
fn why_not(
    site: Option<&str>,
    hosts_entry: bool,
    wildcard: bool,
    resolves_to: &[std::net::IpAddr],
) -> Option<String> {
    if site.is_none() {
        return Some("no site in this home declares this name".to_owned());
    }

    if !wildcard && !hosts_entry {
        return Some(
            "nothing routes this name: its TLD is not wired to the DNS server, and it has no line              in the hosts file"
                .to_owned(),
        );
    }

    if resolves_to.is_empty() {
        return Some("this machine's resolver does not answer for this name".to_owned());
    }

    None
}
