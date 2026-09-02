//! What the hosts file should hold, according to this home's database — roadmap task **T41**.
//!
//! **Rendered from the rows every time and never parsed back**, which is the standing rule about
//! generated configuration one file smaller: the block is disposable, and the operation that writes
//! it carries the whole state rather than a delta (T41 design, D1).
//!
//! Nothing here decides whether that state is worth an elevation prompt. That is the daemon's
//! question, because it needs the machine's current answer as well as this one — see
//! `Elevation::require_hosts` and D11.

use std::net::{IpAddr, Ipv4Addr};

use mixengine_proto::privileged::HostEntry;

use crate::{Result, Store};

/// The one address a managed name resolves to.
///
/// **`127.0.0.1` alone, and no `::1`** — the T41 design, D5. Nothing decides that the web server
/// binds `::1` until T43, and a name that resolves to an address nothing is listening on is a
/// browser timing out before it retries. The helper permits `::1` so T43 can start emitting it
/// without touching the audited binary.
const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// One entry per domain of every site in this home **whose TLD nothing routes**, sorted by name.
///
/// **Every site, whatever `sites.state` says** — the T41 design, D10. A disabled site is one that is
/// not served; the hosts block is about name resolution, and a disabled name that resolves to
/// loopback and is refused by the web server is a better failure than a name that does not resolve
/// at all, because the first one is diagnosable.
///
/// **`wired` is per TLD, and that is a correction to T44** — the T45 design, D6. T44 gave this home
/// one `DnsMode` and computed the whole block from it: hosts-only meant a line per domain, DNS meant
/// an empty block. That was right while nothing could be wired, because both terms were false for
/// every TLD at once. It stops being right the moment a resolver is pointed anywhere, because every
/// mechanism there is scopes to **one TLD** — a file per TLD on macOS, a namespace per rule on
/// Windows, an entry in `Domains=` on Linux — and `.local` is deliberately never wired at all. A
/// home holding both `blog.test` and `shop.local` needs a block with exactly one line in it, and it
/// is the `.local` one.
///
/// A wired TLD is answered by pattern, so a line per name under it adds nothing and asking for one
/// would spend an elevation prompt on a block that could only ever be redundant. An unwired TLD has
/// no other mechanism, wired home or not.
///
/// # Errors
///
/// [`Error::Database`](crate::Error::Database) when the tables cannot be read, and
/// [`Error::UnreadableSiteRow`](crate::Error::UnreadableSiteRow) for a row this build cannot decode.
pub async fn desired(store: &Store, wired: &[String]) -> Result<Vec<HostEntry>> {
    let records = crate::sites::records(store, None).await?;

    let mut entries: Vec<HostEntry> = records
        .into_iter()
        .flat_map(|record| record.domains)
        .filter(|domain| !is_wired(domain, wired))
        .map(|domain| HostEntry {
            address: LOOPBACK,
            domain,
        })
        .collect();

    // The canonical order is
    // [`PrivilegedOp::hosts_apply`](mixengine_proto::privileged::PrivilegedOp::hosts_apply)'s, which
    // is applied again by whoever builds the operation. Sorted here as well so this function's own
    // answer is deterministic to assert on.
    entries.sort_by(|left, right| left.domain.cmp(&right.domain));
    entries.dedup();

    Ok(entries)
}

/// Does something on this machine already route `domain`'s TLD to MixEngine's DNS server?
///
/// The TLD is the last label, which is how every other reader of this table finds it — see
/// `mixengine_elevate::hosts` and `mixengine_core::domains`.
fn is_wired(domain: &str, wired: &[String]) -> bool {
    let tld = domain.rsplit('.').next().unwrap_or_default();

    wired.iter().any(|one| one == tld)
}

#[cfg(test)]
mod tests {
    use super::*;

    use mixengine_proto::{SiteKind, SiteState, Timestamp};

    /// A database with one project in it, exactly as `sites`' own tests build one.
    async fn home() -> (tempfile::TempDir, Store, i64) {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&temp.path().join("mixengine.db"))
            .await
            .expect("a database");
        let project = crate::projects::create(
            &store,
            &crate::projects::Registration {
                name: "blog".to_owned(),
                root: temp.path().join("blog"),
                pins: std::collections::BTreeMap::new(),
            },
            Timestamp::from_system_time(std::time::SystemTime::UNIX_EPOCH),
        )
        .await
        .expect("a project");

        (temp, store, project.id)
    }

    /// A static site under `project`, holding `domains`.
    fn a_site(project: i64, domains: &[&str]) -> crate::sites::NewSite {
        crate::sites::NewSite {
            owner: crate::sites::SiteOwner::Project(project),
            doc_root: String::new(),
            kind: SiteKind::Static,
            https_enabled: true,
            domains: domains.iter().map(|domain| (*domain).to_owned()).collect(),
            services: Vec::new(),
        }
    }

    /// D10: a disabled site is one that is not *served*; the block is about name resolution.
    /// Excluding it would make `site.disable` cost a password dialog for a state change that
    /// touches nothing on disk.
    #[tokio::test]
    async fn every_domain_of_every_site_gets_a_line_whatever_its_state_says() {
        let (_temp, store, project) = home().await;

        let served =
            crate::sites::create(&store, &a_site(project, &["blog.test", "api.blog.test"]))
                .await
                .expect("a site");

        crate::sites::create(&store, &a_site(project, &["shop.test"]))
            .await
            .expect("a second site");

        crate::sites::update(
            &store,
            served.id,
            &crate::sites::Change {
                state: Some(SiteState::Disabled),
                ..Default::default()
            },
        )
        .await
        .expect("the site is disabled");

        let desired = desired(&store, &[]).await.unwrap();

        assert_eq!(
            desired
                .iter()
                .map(|entry| entry.domain.as_str())
                .collect::<Vec<_>>(),
            vec!["api.blog.test", "blog.test", "shop.test"],
            "sorted, and the disabled site's names are still there"
        );
        assert!(
            desired.iter().all(|entry| entry.address == LOOPBACK),
            "D5: one address, and it is the one something is listening on"
        );
    }

    /// A home with nothing in it asks for an empty block, which is what removes ours.
    #[tokio::test]
    async fn a_home_with_no_sites_wants_no_block() {
        let (_temp, store, _project) = home().await;

        assert_eq!(desired(&store, &[]).await.unwrap(), Vec::new());
    }

    /// The T45 design, D6. The wiring is scoped to one TLD on every system measured, so the block
    /// is too: a home with a wired `.test` and an unwired `.local` needs exactly one line, and it
    /// is the `.local` one.
    #[tokio::test]
    async fn only_the_domains_of_unwired_tlds_need_a_hosts_entry() {
        let (_temp, store, project) = home().await;

        crate::sites::create(&store, &a_site(project, &["blog.test", "api.blog.test"]))
            .await
            .expect("a site");
        crate::sites::create(&store, &a_site(project, &["shop.local"]))
            .await
            .expect("a second site");

        let entries = desired(&store, &["test".to_owned()])
            .await
            .expect("entries");

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.domain.as_str())
                .collect::<Vec<_>>(),
            vec!["shop.local"],
            "a wired TLD is answered by pattern; an unwired one has nothing else"
        );
    }

    /// A home with nothing wired is every home before the first grant, and it keeps the whole block
    /// T41 built.
    #[tokio::test]
    async fn a_home_with_nothing_wired_needs_every_domain() {
        let (_temp, store, project) = home().await;

        crate::sites::create(&store, &a_site(project, &["blog.test", "shop.local"]))
            .await
            .expect("a site");

        assert_eq!(desired(&store, &[]).await.expect("entries").len(), 2);
    }

    /// And a home whose every TLD is wired needs an empty block — which is what clears one a
    /// previous mode left behind, rather than leaving stale names on loopback for ever.
    #[tokio::test]
    async fn a_home_whose_tlds_are_all_wired_needs_no_block() {
        let (_temp, store, project) = home().await;

        crate::sites::create(&store, &a_site(project, &["blog.test", "api.blog.test"]))
            .await
            .expect("a site");

        assert!(
            desired(&store, &["test".to_owned()])
                .await
                .expect("entries")
                .is_empty()
        );
    }

    /// A subdomain is wired by its TLD and not by its own name, which is the whole difference
    /// between a pattern and a line per name.
    #[test]
    fn a_domain_is_wired_by_its_last_label() {
        let wired = ["test".to_owned()];

        assert!(is_wired("blog.test", &wired));
        assert!(is_wired("api.deep.blog.test", &wired));
        assert!(!is_wired("shop.local", &wired));
        assert!(!is_wired("blog.test", &[]));
    }
}
