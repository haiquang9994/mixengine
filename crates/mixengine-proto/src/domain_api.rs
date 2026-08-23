//! What `domain.*` asks and answers — roadmap task **T46**.
//!
//! Two verbs and a diagnostic. The verbs add nothing [`crate::SiteUpdate`] cannot already do, and
//! exist because of what a client would otherwise have to do to use it: read the site, append one
//! name, send the whole list back. That is business logic in a client, which `CLAUDE.md` forbids,
//! and a read-modify-write that drops a domain another client added in between (T46 design, D1).

use std::net::{IpAddr, Ipv4Addr};

use crate::SiteRef;

/// Give a site one more name.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DomainAdd {
    /// Which site gets it.
    pub site: SiteRef,

    /// The name to add.
    ///
    /// **Never becomes the primary.** [`crate::SiteUpdate`] reorders and the head of that list is
    /// the primary; a verb that says "add a domain" does not get to decide what a site *is* (T46
    /// design, D3).
    pub domain: String,

    /// `.local` is mDNS territory and is allowed only when the caller says they know.
    #[serde(default)]
    pub accept_risky_tld: bool,
}

/// Take one name away.
///
/// **No site, and none to give.** `site_domains_domain` is `UNIQUE` — the index its own migration
/// calls "the one that decides ownership" — so a domain names its site. Asking a caller for it as
/// well would be asking for a fact the database holds, and would let them name a site the domain is
/// not on (T46 design, D2).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DomainRemove {
    /// The name to take away.
    pub domain: String,
}

/// Which names a diagnostic should answer about.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct DomainStatusQuery {
    /// One name, or every name this home declares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

/// What `domain.dns_status` answers.
///
/// A report around the list rather than a bare `Vec`, on [`crate::SiteList`]'s precedent: the
/// one-domain and every-domain questions then have one answer shape, and a later field has somewhere
/// to go that is not inside every row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DomainStatusReport {
    /// One row per name asked about, in domain order.
    pub domains: Vec<DomainStatus>,
}

/// What happens to one name, as four facts that fail independently.
///
/// **Four facts and no verdict** (T46 design, D4). A hosts line with no server, a server with no
/// resolver, and a resolver wired to a TLD this name is not on are three different faults with three
/// different fixes; one boolean would leave every client working out which of them it had — which is
/// the derivation [`crate::DnsStatus::wildcards`] had to stop making in T45.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DomainStatus {
    /// The name asked about, lowercased.
    pub domain: String,

    /// The site that declares it, by its primary domain, or [`None`] for a name nothing declares.
    ///
    /// **Reported rather than refused** (T46 design, D5): somebody asking why `foo.test` does not
    /// work when they never declared it is owed exactly that answer, and the other three facts still
    /// hold one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,

    /// Is there a line for this name in the managed hosts block, on disk, now.
    pub hosts_entry: bool,

    /// Is this name's TLD wired, so every name under it resolves without being written down.
    pub wildcard: bool,

    /// What this daemon's own DNS server answers, asked over its socket.
    ///
    /// [`None`] is a server that is not listening or did not reply — a different fault from a server
    /// that answers while nothing on the machine sends it a name, and the reason this is asked over
    /// the socket rather than of the zone (T46 design, D7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_answers: Option<Ipv4Addr>,

    /// What the operating system actually resolves it to, through `getaddrinfo`.
    ///
    /// **Never `nslookup`**, which T45 measured to bypass the Name Resolution Policy Table on
    /// Windows: it answers NXDOMAIN for a name `getaddrinfo` resolves at the same moment, so a
    /// diagnostic built with it would report a correctly wired machine as broken. Empty is a name
    /// that does not resolve.
    pub resolves_to: Vec<IpAddr>,

    /// One sentence saying what is wrong, or [`None`] when nothing is.
    ///
    /// A sentence and not a code, on [`crate::DnsStatus::because`]'s precedent next door. It says
    /// what is wrong and never what to do about it: repair is T47's, and a diagnostic that suggests
    /// a fix it cannot perform is one that will drift from the thing that performs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The optional fields are absent rather than null, which is how every other request in this
    /// crate travels — a client that omits them sends the same bytes the daemon writes back.
    #[test]
    fn a_status_row_leaves_out_what_it_has_nothing_to_say_about() {
        let row = DomainStatus {
            domain: "blog.test".to_owned(),
            site: None,
            hosts_entry: false,
            wildcard: false,
            server_answers: None,
            resolves_to: Vec::new(),
            because: None,
        };

        let wire = serde_json::to_string(&row).expect("a row serialises");

        assert_eq!(
            wire,
            r#"{"domain":"blog.test","hosts_entry":false,"wildcard":false,"resolves_to":[]}"#
        );
    }

    /// A request naming no domain is every domain, and it has to be spellable as an empty object —
    /// `mix domain status` with no argument sends exactly that.
    #[test]
    fn a_query_with_no_domain_is_an_empty_object() {
        let query: DomainStatusQuery = serde_json::from_str("{}").expect("an empty query");

        assert_eq!(query.domain, None);
    }

    /// `.local` needs the acknowledgement, and a request that does not mention it did not ask.
    #[test]
    fn an_add_defaults_to_refusing_the_risky_tld() {
        let add: DomainAdd =
            serde_json::from_str(r#"{"site":{"domain":"blog.test"},"domain":"shop.test"}"#)
                .expect("an add");

        assert!(!add.accept_risky_tld);
    }
}
