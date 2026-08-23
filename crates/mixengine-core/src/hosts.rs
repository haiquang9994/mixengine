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

/// One entry per domain of every site in this home, sorted by name.
///
/// **Every site, whatever `sites.state` says** — D10. A disabled site is one that is not served; the
/// hosts block is about name resolution, and a disabled name that resolves to loopback and is
/// refused by the web server is a better failure than a name that does not resolve at all, because
/// the first one is diagnosable.
///
/// # Errors
///
/// [`Error::Database`](crate::Error::Database) when the tables cannot be read, and
/// [`Error::UnreadableSiteRow`](crate::Error::UnreadableSiteRow) for a row this build cannot decode.
pub async fn desired(store: &Store) -> Result<Vec<HostEntry>> {
    let records = crate::sites::records(store, None).await?;

    let mut entries: Vec<HostEntry> = records
        .into_iter()
        .flat_map(|record| record.domains)
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
            project_id: project,
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

        let desired = desired(&store).await.unwrap();

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

        assert_eq!(desired(&store).await.unwrap(), Vec::new());
    }
}
