//! `domain.*` — roadmap task **T46**.
//!
//! Two verbs and a diagnostic. The verbs are **thin over [`Sites`] on purpose**: each computes a new
//! domain list and hands it to the same update the `site.*` half uses, so the TLD check, the hosts
//! queueing and the front-end re-render have exactly one implementation between them (T46 design,
//! D1). What is here that is not there is the *reason* a caller cannot compose them itself — a read,
//! an append and a write back is a race with whatever another client did in between.

pub(crate) mod lookup;

use std::sync::Arc;

use mixengine_proto::{DomainAdd, DomainRemove, Error, SiteDetail, SiteRef};

use crate::error::ToWire as _;
use crate::sites::Sites;

/// The `domain.*` half of the API.
#[derive(Debug)]
pub(crate) struct Domains {
    /// What actually writes a site.
    sites: Arc<Sites>,
}

impl Domains {
    /// The one of these the API holds.
    pub(crate) fn new(sites: Arc<Sites>) -> Arc<Self> {
        Arc::new(Self { sites })
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
}
