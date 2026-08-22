//! The three site tables: what is served out of a project's directory, and at what name.
//!
//! Roadmap task **T39a**. [`crate::projects`]' shape one table down, with the difference that makes
//! this module worth having: a site is *three* rows in three tables, and they are only ever right
//! together. `0001_initial.sql` says so at `site_domains_one_primary_per_site` — "at least one is
//! not expressible here … and it stays an invariant the site module upholds inside the transaction
//! that creates a site". This is that module and that transaction.
//!
//! # One place reads these tables
//!
//! `projects.rs` does not gain a `sites_of` and `services.rs` does not gain a `sites_declaring`,
//! though a site belongs to a project and declares services. Both questions are reads of `sites`,
//! and a second door onto a table is a second answer to a question that has one — the rule that put
//! the project walk in one place in the first place. They are [`records`] and [`declaring`] here.
//!
//! # Domains are an ordered list, and the head is the primary
//!
//! `is_primary` never leaves this module. Above it a site has a `Vec<String>` whose first element is
//! the primary, so "no primary" and "two primaries" are not states anything can spell. A replacement
//! deletes every row before it inserts any, which is what lets a domain move from one site to
//! another in two calls rather than being blocked by the unique index halfway through (spec D6).

use std::path::{Component, Path, PathBuf};

use mixengine_platform::paths::in_full;
use mixengine_proto::{ServiceId, SiteKind, SiteState};
use sqlx::Sqlite;

use crate::{Error, Result, Store};

/// One site, whole: the row, its ordered domains and its links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteRecord {
    /// The rowid, which stays inside this crate: the wire handle is a domain (spec D5).
    pub id: i64,

    /// The project it belongs to.
    pub project_id: i64,

    /// Relative to the project's root. `""` is the root itself.
    pub doc_root: String,

    /// What it serves.
    pub kind: SiteKind,

    /// Whether HTTPS is declared.
    pub https_enabled: bool,

    /// Whether the web server should serve it.
    pub state: SiteState,

    /// Every domain, the primary first.
    pub domains: Vec<String>,

    /// The services it declares, in id order.
    pub services: Vec<ServiceId>,
}

/// Everything creating a site has to write down.
///
/// Domains arrive already normalised by [`crate::domains::normalised`] — this module writes what it
/// is given, because a policy applied here as well would be a second place to change it.
#[derive(Debug, Clone)]
pub struct NewSite {
    /// Which project.
    pub project_id: i64,
    /// Relative to the root, already made so by [`relative_doc_root`].
    pub doc_root: String,
    /// What it serves.
    pub kind: SiteKind,
    /// Whether HTTPS is declared.
    pub https_enabled: bool,
    /// Ordered; the head becomes the primary. Must not be empty.
    pub domains: Vec<String>,
    /// The services it declares.
    pub services: Vec<ServiceId>,
}

/// What an update is changing, where [`None`] means "leave it".
#[derive(Debug, Clone, Default)]
pub struct Change {
    /// A new doc root, already relative.
    pub doc_root: Option<String>,
    /// A new kind.
    pub kind: Option<SiteKind>,
    /// Whether HTTPS is declared.
    pub https_enabled: Option<bool>,
    /// Whether the web server should serve it.
    pub state: Option<SiteState>,
    /// The domains, **replacing** the list. The head becomes the primary.
    pub domains: Option<Vec<String>>,
    /// The links, **replacing** them.
    pub services: Option<Vec<ServiceId>>,
}

/// A doc root as the row holds it: relative to the project's root, `""` for the root itself.
///
/// **Both sides are normalised** before they are compared, which is the rule T39/D5 paid for: a
/// project registered at `/tmp/blog` and a doc root typed as `/private/tmp/blog/public` are one
/// directory on macOS, and a test using a temporary directory meets that on the first run.
///
/// # Errors
///
/// [`Error::DocRootOutsideProject`] for a path that resolves outside the root.
pub fn relative_doc_root(root: &Path, doc_root: &str) -> Result<String> {
    let full = in_full(root);
    let candidate = Path::new(doc_root);

    // `in_full` resolves aliases over the prefix that exists; it does not fold `..`, and on Windows
    // it cannot — `GetLongPathNameW` expands short names and leaves the rest as it was handed in. A
    // `..` left in would strip against the root and answer with a path pointing outside it, so the
    // dot segments come off here, after the aliases have gone and against a prefix that is real.
    let joined = without_dot_segments(&match candidate.is_absolute() {
        true => in_full(candidate),
        false => in_full(&full.join(candidate)),
    });

    let relative = joined
        .strip_prefix(&full)
        .map_err(|_| Error::DocRootOutsideProject {
            doc_root: doc_root.to_owned(),
            root: full.display().to_string(),
        })?;

    // Stored with forward slashes whatever this OS writes, because the value is rendered into a
    // web server's configuration and read by a person on a machine that may not be this one.
    Ok(relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

/// `.` and `..` folded away, so a path can be compared with the root it is supposed to be under.
fn without_dot_segments(path: &Path) -> PathBuf {
    let mut folded = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            // `pop` answers false at a root, where `..` is the root itself and there is nothing to
            // fold — keeping it there is what makes the comparison above refuse rather than accept.
            Component::ParentDir if !folded.pop() => folded.push(".."),
            Component::ParentDir => {}
            other => folded.push(other.as_os_str()),
        }
    }

    folded
}

/// Write a site, its domains and its links, or write none of them.
///
/// # Errors
///
/// [`Error::DomainTaken`] naming the site already holding one of the domains, and
/// [`Error::Database`] when the write cannot be made.
pub async fn create(store: &Store, new: &NewSite) -> Result<SiteRecord> {
    // `BEGIN IMMEDIATE` for `crate::services::transition`'s reason: a deferred `BEGIN` would leave
    // the first `INSERT` to upgrade a read snapshot into a write and fail unrecoverably against a
    // concurrent writer.
    let mut tx = store
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|source| store.failure("write", source))?;

    let (kind, pool) = columns(&new.kind);
    let config = payload(&new.kind);

    // `last_insert_rowid` rather than `RETURNING id`, on `crate::projects::create`'s precedent: the
    // column is an `INTEGER PRIMARY KEY`, which `RETURNING` types as nullable and this never is.
    let inserted = sqlx::query!(
        "INSERT INTO sites (project_id, doc_root, kind, php_service_id, https_enabled, config_json,
                            state)
         VALUES (?, ?, ?, ?, ?, ?, 'enabled')",
        new.project_id,
        new.doc_root,
        kind,
        pool,
        new.https_enabled,
        config,
    )
    .execute(&mut *tx)
    .await
    .map_err(|source| store.failure("write", source))?;

    let id = inserted.last_insert_rowid();

    write_domains(store, &mut tx, id, &new.domains).await?;
    write_links(store, &mut tx, id, &new.services).await?;

    tx.commit()
        .await
        .map_err(|source| store.failure("write", source))?;

    tracing::info!(site = id, domain = %new.domains[0], "a site was created");

    Ok(SiteRecord {
        id,
        project_id: new.project_id,
        doc_root: new.doc_root.clone(),
        kind: new.kind.clone(),
        https_enabled: new.https_enabled,
        state: SiteState::Enabled,
        domains: new.domains.clone(),
        services: new.services.clone(),
    })
}

/// Every site, or every site of one project, in primary-domain order.
///
/// # Errors
///
/// [`Error::Database`] when the tables cannot be read, and [`Error::UnreadableSiteRow`] for a row
/// this build cannot make a [`SiteKind`] of.
pub async fn records(store: &Store, project: Option<i64>) -> Result<Vec<SiteRecord>> {
    let rows = sqlx::query!(
        "SELECT id, project_id, doc_root, kind, php_service_id, https_enabled, config_json, state
         FROM sites
         WHERE ?1 IS NULL OR project_id = ?1
         ORDER BY id",
        project
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    let mut sites = Vec::with_capacity(rows.len());

    for row in rows {
        let domains = sqlx::query_scalar!(
            "SELECT domain FROM site_domains WHERE site_id = ? ORDER BY is_primary DESC, domain",
            row.id
        )
        .fetch_all(store.pool())
        .await
        .map_err(|source| store.failure("read", source))?;

        let linked = sqlx::query_scalar!(
            "SELECT service_id FROM site_service_links WHERE site_id = ? ORDER BY service_id",
            row.id
        )
        .fetch_all(store.pool())
        .await
        .map_err(|source| store.failure("read", source))?;

        let mut services = Vec::with_capacity(linked.len());

        for id in linked {
            services.push(ServiceId::parse(&id).map_err(|_| Error::UnreadableSiteRow {
                site: row.id,
                column: "service_id",
                value: id.clone(),
            })?);
        }

        sites.push(SiteRecord {
            id: row.id,
            project_id: row.project_id,
            doc_root: row.doc_root,
            kind: read_kind(row.id, &row.kind, row.php_service_id, &row.config_json)?,
            https_enabled: row.https_enabled != 0,
            state: read_state(row.id, &row.state)?,
            domains,
            services,
        });
    }

    sites.sort_by(|left, right| left.domains.first().cmp(&right.domains.first()));

    Ok(sites)
}

/// The site answering to this domain, primary or alias, or [`None`].
///
/// # Errors
///
/// The errors [`records`] gives.
pub async fn by_domain(store: &Store, domain: &str) -> Result<Option<SiteRecord>> {
    let found = sqlx::query_scalar!("SELECT site_id FROM site_domains WHERE domain = ?", domain)
        .fetch_optional(store.pool())
        .await
        .map_err(|source| store.failure("read", source))?;

    let Some(id) = found else {
        return Ok(None);
    };

    Ok(records(store, None)
        .await?
        .into_iter()
        .find(|site| site.id == id))
}

/// Apply a change, and answer with the site as it now stands.
///
/// # Errors
///
/// [`Error::NotFound`] for a site that is not there, [`Error::DomainTaken`] naming the site holding
/// a domain being claimed, and [`Error::Database`] when the write cannot be made.
pub async fn update(store: &Store, id: i64, change: &Change) -> Result<SiteRecord> {
    let mut tx = store
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|source| store.failure("write", source))?;

    // Asked inside the transaction so an update against a site that has just gone is a `not_found`
    // rather than a set of `UPDATE`s that quietly touch no rows.
    sqlx::query_scalar!("SELECT id FROM sites WHERE id = ?", id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|source| store.failure("read", source))?
        .ok_or_else(|| Error::NotFound {
            kind: "site",
            id: id.to_string(),
        })?;

    if let Some(doc_root) = &change.doc_root {
        sqlx::query!("UPDATE sites SET doc_root = ? WHERE id = ?", doc_root, id)
            .execute(&mut *tx)
            .await
            .map_err(|source| store.failure("write", source))?;
    }

    if let Some(kind) = &change.kind {
        let (word, pool) = columns(kind);
        let config = payload(kind);

        sqlx::query!(
            "UPDATE sites SET kind = ?, php_service_id = ?, config_json = ? WHERE id = ?",
            word,
            pool,
            config,
            id
        )
        .execute(&mut *tx)
        .await
        .map_err(|source| store.failure("write", source))?;
    }

    if let Some(https) = change.https_enabled {
        sqlx::query!("UPDATE sites SET https_enabled = ? WHERE id = ?", https, id)
            .execute(&mut *tx)
            .await
            .map_err(|source| store.failure("write", source))?;
    }

    if let Some(state) = change.state {
        let word = state.as_str();

        sqlx::query!("UPDATE sites SET state = ? WHERE id = ?", word, id)
            .execute(&mut *tx)
            .await
            .map_err(|source| store.failure("write", source))?;
    }

    if let Some(domains) = &change.domains {
        write_domains(store, &mut tx, id, domains).await?;
    }

    if let Some(services) = &change.services {
        write_links(store, &mut tx, id, services).await?;
    }

    tx.commit()
        .await
        .map_err(|source| store.failure("write", source))?;

    // Read back rather than assembled here: the row this answers with is the row on disk, and the
    // three tables have to agree about it.
    records(store, None)
        .await?
        .into_iter()
        .find(|site| site.id == id)
        .ok_or_else(|| Error::NotFound {
            kind: "site",
            id: id.to_string(),
        })
}

/// Delete a site, and let the cascade take its domains and its links.
///
/// # Errors
///
/// [`Error::NotFound`] for a site that is not there, and [`Error::Database`] when the row cannot be
/// removed.
pub async fn delete(store: &Store, id: i64) -> Result<()> {
    let removed = sqlx::query!("DELETE FROM sites WHERE id = ?", id)
        .execute(store.pool())
        .await
        .map_err(|source| store.failure("write", source))?;

    match removed.rows_affected() {
        0 => Err(Error::NotFound {
            kind: "site",
            id: id.to_string(),
        }),
        _ => {
            tracing::info!(site = id, "a site was deleted");
            Ok(())
        }
    }
}

/// The primary domains of every site that names this service — as its pool, or as a link.
///
/// **Both, in one answer**, because the refusal that reads it (`service.delete`, spec D4) does not
/// care which way a site named the service: either way, deleting it changes what the next
/// `site.start` would do.
///
/// # Errors
///
/// [`Error::Database`] when the tables cannot be read, and the errors [`records`] gives.
pub async fn declaring(store: &Store, service: &ServiceId) -> Result<Vec<String>> {
    let id = service.as_str();

    let sites = sqlx::query_scalar!(
        "SELECT DISTINCT s.id FROM sites s
         LEFT JOIN site_service_links l ON l.site_id = s.id
         WHERE s.php_service_id = ? OR l.service_id = ?",
        id,
        id
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    let known = records(store, None).await?;

    Ok(known
        .into_iter()
        .filter(|site| sites.contains(&site.id))
        .filter_map(|site| site.domains.into_iter().next())
        .collect())
}

/// The `kind` word and the `php_service_id` a kind writes into its row.
fn columns(kind: &SiteKind) -> (&'static str, Option<String>) {
    match kind {
        SiteKind::PhpFpm { pool } => (
            "php-fpm",
            pool.as_ref().map(|pool| pool.as_str().to_owned()),
        ),
        SiteKind::Static => ("static", None),
        SiteKind::ReverseProxy { .. } => ("reverse-proxy", None),
        SiteKind::NodeApp { .. } => ("node-app", None),
    }
}

/// The rest of a kind's payload, as `config_json` holds it.
///
/// Built with `serde_json` so escaping an upstream with a path in it is not this module's problem.
fn payload(kind: &SiteKind) -> String {
    let value = match kind {
        SiteKind::PhpFpm { .. } | SiteKind::Static => serde_json::json!({}),
        SiteKind::ReverseProxy { upstream } => serde_json::json!({ "upstream": upstream }),
        SiteKind::NodeApp { port } => serde_json::json!({ "port": port }),
    };

    value.to_string()
}

/// The inverse of [`columns`] and [`payload`].
fn read_kind(site: i64, kind: &str, pool: Option<String>, config: &str) -> Result<SiteKind> {
    let unreadable = |column: &'static str, value: &str| Error::UnreadableSiteRow {
        site,
        column,
        value: value.to_owned(),
    };

    match kind {
        "php-fpm" => {
            let pool = pool
                .map(|id| ServiceId::parse(&id).map_err(|_| unreadable("php_service_id", &id)))
                .transpose()?;

            Ok(SiteKind::PhpFpm { pool })
        }
        "static" => Ok(SiteKind::Static),
        "reverse-proxy" => {
            let payload: serde_json::Value =
                serde_json::from_str(config).map_err(|_| unreadable("config_json", config))?;

            let upstream = payload
                .get("upstream")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| unreadable("config_json", config))?;

            Ok(SiteKind::ReverseProxy {
                upstream: upstream.to_owned(),
            })
        }
        "node-app" => {
            let payload: serde_json::Value =
                serde_json::from_str(config).map_err(|_| unreadable("config_json", config))?;

            let port = payload
                .get("port")
                .and_then(serde_json::Value::as_u64)
                .and_then(|port| u16::try_from(port).ok())
                .ok_or_else(|| unreadable("config_json", config))?;

            Ok(SiteKind::NodeApp { port })
        }
        other => Err(unreadable("kind", other)),
    }
}

/// The state word, or the refusal for a row the CHECK should have made impossible.
fn read_state(site: i64, state: &str) -> Result<SiteState> {
    match state {
        "enabled" => Ok(SiteState::Enabled),
        "disabled" => Ok(SiteState::Disabled),
        other => Err(Error::UnreadableSiteRow {
            site,
            column: "state",
            value: other.to_owned(),
        }),
    }
}

/// Replace a site's domains: every delete before any insert (spec D6).
async fn write_domains(
    store: &Store,
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    site: i64,
    domains: &[String],
) -> Result<()> {
    sqlx::query!("DELETE FROM site_domains WHERE site_id = ?", site)
        .execute(&mut **tx)
        .await
        .map_err(|source| store.failure("write", source))?;

    for (index, domain) in domains.iter().enumerate() {
        let is_primary = i64::from(index == 0);

        let written = sqlx::query!(
            "INSERT INTO site_domains (site_id, domain, is_primary) VALUES (?, ?, ?)",
            site,
            domain,
            is_primary
        )
        .execute(&mut **tx)
        .await;

        if let Err(source) = written {
            // The unique index got there first. Ask who has it, so the answer can name the site
            // rather than say "taken" with nowhere to go and look.
            let holder = holder(store, tx, domain).await?;

            return Err(match holder {
                Some(holder) => Error::DomainTaken {
                    domain: domain.clone(),
                    holder,
                },
                None => store.failure("write", source),
            });
        }
    }

    Ok(())
}

/// Replace a site's service links, same shape as [`write_domains`].
async fn write_links(
    store: &Store,
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    site: i64,
    services: &[ServiceId],
) -> Result<()> {
    sqlx::query!("DELETE FROM site_service_links WHERE site_id = ?", site)
        .execute(&mut **tx)
        .await
        .map_err(|source| store.failure("write", source))?;

    for service in services {
        let id = service.as_str();

        sqlx::query!(
            "INSERT INTO site_service_links (site_id, service_id) VALUES (?, ?)",
            site,
            id
        )
        .execute(&mut **tx)
        .await
        .map_err(|source| store.failure("write", source))?;
    }

    Ok(())
}

/// The primary domain of whichever site owns `domain`.
async fn holder(
    store: &Store,
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    domain: &str,
) -> Result<Option<String>> {
    let owner = sqlx::query_scalar!("SELECT site_id FROM site_domains WHERE domain = ?", domain)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|source| store.failure("read", source))?;

    let Some(site) = owner else {
        return Ok(None);
    };

    sqlx::query_scalar!(
        "SELECT domain FROM site_domains WHERE site_id = ? ORDER BY is_primary DESC, domain LIMIT 1",
        site
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(|source| store.failure("read", source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixengine_proto::Timestamp;

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

    fn php() -> SiteKind {
        SiteKind::PhpFpm { pool: None }
    }

    /// **D2, and the bug T39 paid for once.** A doc root is stored relative to the root, and both
    /// sides of the comparison are normalised — a `/tmp` project and a `/private/tmp` doc root are
    /// one directory on macOS, and a temporary directory is exactly where that shows up.
    #[test]
    fn a_doc_root_is_relativised_through_whatever_the_filesystem_calls_the_root() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let root = temp.path().join("blog");
        std::fs::create_dir_all(root.join("public")).expect("a doc root");

        assert_eq!(relative_doc_root(&root, "public").unwrap(), "public");
        assert_eq!(
            relative_doc_root(&root, &root.join("public").display().to_string()).unwrap(),
            "public"
        );
        assert_eq!(relative_doc_root(&root, "").unwrap(), "");
        assert_eq!(
            relative_doc_root(&root, &root.display().to_string()).unwrap(),
            "",
            "the root itself is the empty string, not \".\""
        );

        // Outside the root is a site whose files belong to somebody else.
        assert!(matches!(
            relative_doc_root(&root, "../elsewhere"),
            Err(Error::DocRootOutsideProject { .. })
        ));
    }

    /// A site, its ordered domains and its links are one write.
    #[tokio::test]
    async fn a_site_is_created_whole_and_read_back_whole() {
        let (_temp, store, project) = home().await;

        let created = create(
            &store,
            &NewSite {
                project_id: project,
                doc_root: "public".to_owned(),
                kind: php(),
                https_enabled: true,
                domains: vec!["blog.test".to_owned(), "www.blog.test".to_owned()],
                services: Vec::new(),
            },
        )
        .await
        .expect("a site");

        assert_eq!(created.domains, ["blog.test", "www.blog.test"]);
        assert_eq!(created.state, SiteState::Enabled);

        let found = by_domain(&store, "www.blog.test")
            .await
            .expect("a read")
            .expect("an alias reaches the site as surely as the primary does");
        assert_eq!(found.id, created.id);
        assert_eq!(found.domains[0], "blog.test", "the head is the primary");
    }

    /// **D6.** Replacing the list is how a domain is removed, and the deletes run before the
    /// inserts so a domain can move from one site to another in two calls.
    #[tokio::test]
    async fn a_domain_moves_between_sites_because_the_deletes_come_first() {
        let (_temp, store, project) = home().await;

        let blog = create(
            &store,
            &NewSite {
                project_id: project,
                doc_root: String::new(),
                kind: php(),
                https_enabled: true,
                domains: vec!["blog.test".to_owned(), "api.blog.test".to_owned()],
                services: Vec::new(),
            },
        )
        .await
        .expect("the first site");

        let shop = create(
            &store,
            &NewSite {
                project_id: project,
                doc_root: String::new(),
                kind: SiteKind::Static,
                https_enabled: false,
                domains: vec!["shop.test".to_owned()],
                services: Vec::new(),
            },
        )
        .await
        .expect("the second site");

        // Taken off the first...
        update(
            &store,
            blog.id,
            &Change {
                domains: Some(vec!["blog.test".to_owned()]),
                ..Change::default()
            },
        )
        .await
        .expect("the alias is dropped");

        // ...and claimed by the second, which the unique index would refuse the other way round.
        let moved = update(
            &store,
            shop.id,
            &Change {
                domains: Some(vec!["shop.test".to_owned(), "api.blog.test".to_owned()]),
                ..Change::default()
            },
        )
        .await
        .expect("the alias moves");

        assert_eq!(moved.domains, ["shop.test", "api.blog.test"]);
        assert_eq!(
            by_domain(&store, "api.blog.test")
                .await
                .unwrap()
                .unwrap()
                .id,
            shop.id
        );
    }

    /// A domain another site owns is refused by name, so the answer can say who has it.
    #[tokio::test]
    async fn a_domain_another_site_owns_names_that_site() {
        let (_temp, store, project) = home().await;

        create(
            &store,
            &NewSite {
                project_id: project,
                doc_root: String::new(),
                kind: php(),
                https_enabled: true,
                domains: vec!["blog.test".to_owned()],
                services: Vec::new(),
            },
        )
        .await
        .expect("the first site");

        let refused = create(
            &store,
            &NewSite {
                project_id: project,
                doc_root: String::new(),
                kind: SiteKind::Static,
                https_enabled: true,
                domains: vec!["blog.test".to_owned()],
                services: Vec::new(),
            },
        )
        .await
        .expect_err("one domain, one site");

        assert!(
            matches!(&refused, Error::DomainTaken { holder, .. } if holder == "blog.test"),
            "{refused:?}"
        );

        // And nothing was left behind by the write that failed halfway.
        assert_eq!(records(&store, Some(project)).await.unwrap().len(), 1);
    }

    /// The kind survives a round trip, payload and all — which is what `config_json` is for.
    #[tokio::test]
    async fn a_kinds_payload_survives_the_row_it_is_stored_in() {
        let (_temp, store, project) = home().await;

        for (kind, domain) in [
            (
                SiteKind::ReverseProxy {
                    upstream: "http://127.0.0.1:5173".to_owned(),
                },
                "proxy.test",
            ),
            (SiteKind::NodeApp { port: 3000 }, "node.test"),
            (SiteKind::Static, "static.test"),
        ] {
            let created = create(
                &store,
                &NewSite {
                    project_id: project,
                    doc_root: String::new(),
                    kind: kind.clone(),
                    https_enabled: true,
                    domains: vec![domain.to_owned()],
                    services: Vec::new(),
                },
            )
            .await
            .expect("a site");

            assert_eq!(by_domain(&store, domain).await.unwrap().unwrap().kind, kind);
            assert_eq!(created.kind, kind);
        }
    }
}
