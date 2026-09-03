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
use mixengine_proto::{ExtensionId, ServiceId, SiteKind, SiteState};
use sqlx::Sqlite;

use crate::{Error, Result, Store};

/// Who a site belongs to, which is also what gives its `doc_root` a root — roadmap task **T81b**.
///
/// **One of two, and never neither**: `0017_extension_sites.sql` holds that with a CHECK, and this
/// type is what makes it unrepresentable above the row. A project's site is rooted at
/// `projects.root_path`; an extension's at `extensions.install_dir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteOwner {
    /// A registered project, by rowid.
    Project(i64),

    /// An installed `web-app` extension.
    Extension(ExtensionId),
}

impl SiteOwner {
    /// The extension that owns this site, when one does — roadmap task **T82a**.
    #[must_use]
    pub fn extension(&self) -> Option<&ExtensionId> {
        match self {
            Self::Extension(id) => Some(id),
            Self::Project(_) => None,
        }
    }

    /// The two columns, exactly one of them set.
    fn columns(&self) -> (Option<i64>, Option<String>) {
        match self {
            Self::Project(id) => (Some(*id), None),
            Self::Extension(id) => (None, Some(id.as_str().to_owned())),
        }
    }

    /// The two columns read back, or the refusal for a row the CHECK should have made impossible.
    fn read(site: i64, project: Option<i64>, extension: Option<String>) -> Result<Self> {
        match (project, extension) {
            (Some(project), None) => Ok(Self::Project(project)),
            (None, Some(extension)) => ExtensionId::parse(extension.clone())
                .map(Self::Extension)
                .map_err(|_| Error::UnreadableSiteRow {
                    site,
                    column: "extension_id",
                    value: extension,
                }),
            (project, extension) => Err(Error::UnreadableSiteRow {
                site,
                column: "project_id",
                value: format!("{project:?} beside extension_id {extension:?}"),
            }),
        }
    }
}

/// One site, whole: the row, its ordered domains and its links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteRecord {
    /// The rowid, which stays inside this crate: the wire handle is a domain (spec D5).
    pub id: i64,

    /// Who it belongs to, and therefore what `doc_root` is relative to.
    pub owner: SiteOwner,

    /// Relative to the owner's root. `""` is the root itself.
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

    /// Where the local network reaches it, when it does — roadmap task **T74**.
    pub sharing: Option<Sharing>,
}

/// A site's LAN sharing, as the row holds it — roadmap task **T74**.
///
/// **One value or none, never three columns a reader has to agree about.** The schema enforces that
/// with a trigger (`0012_site_sharing.sql`); this type is what makes it unrepresentable in the code
/// above it, so nothing has to check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sharing {
    /// The interface, by the name the OS gives it.
    pub interface: String,

    /// The IPv4 address bound, certified and printed.
    pub address: std::net::Ipv4Addr,

    /// When sharing began.
    pub since: mixengine_proto::Timestamp,

    /// When this share ends by itself, or [`None`] for one that does not — roadmap task **T76**.
    ///
    /// **Measured from [`since`](Self::since) and stored as the instant it lands on** — the T76
    /// design, D6. A length without the start it was measured from is not a deadline, and `--for`
    /// is a property of the share rather than of the command that set it: sharing an already-shared
    /// site again neither restarts the clock nor removes the alarm.
    ///
    /// The one of the four that is optional on its own. `0013_site_sharing_until.sql` holds the
    /// other half — a site that is not shared carries no deadline either.
    pub until: Option<mixengine_proto::Timestamp>,
}

/// Everything creating a site has to write down.
///
/// Domains arrive already normalised by [`crate::domains::normalised`] — this module writes what it
/// is given, because a policy applied here as well would be a second place to change it.
#[derive(Debug, Clone)]
pub struct NewSite {
    /// Who it belongs to.
    pub owner: SiteOwner,
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

    // **A pool an extension owns serves that extension and nothing else** — roadmap task **T82a**,
    // that design's D5. Inside the transaction, so an extension installed between the read and the
    // insert cannot open a window.
    pool_is_free_for(store, &mut *tx, new.owner.extension(), &new.kind).await?;

    let (kind, pool) = columns(&new.kind);
    let config = payload(&new.kind);

    // `last_insert_rowid` rather than `RETURNING id`, on `crate::projects::create`'s precedent: the
    // column is an `INTEGER PRIMARY KEY`, which `RETURNING` types as nullable and this never is.
    let (project, extension) = new.owner.columns();

    let inserted = sqlx::query!(
        "INSERT INTO sites (project_id, extension_id, doc_root, kind, php_service_id, https_enabled,
                            config_json, state)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'enabled')",
        project,
        extension,
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
        owner: new.owner.clone(),
        doc_root: new.doc_root.clone(),
        kind: new.kind.clone(),
        https_enabled: new.https_enabled,
        state: SiteState::Enabled,
        domains: new.domains.clone(),
        services: new.services.clone(),
        // A site is created unshared, always. Sharing is a thing a person turns on afterwards for
        // a site they are looking at, and a create that could take one would be a create that could
        // open a firewall rule as a side effect of an import.
        sharing: None,
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
        "SELECT id, project_id, extension_id, doc_root, kind, php_service_id, https_enabled,
                config_json, state, shared_interface, shared_address, shared_since, shared_until
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
            owner: SiteOwner::read(row.id, row.project_id, row.extension_id)?,
            doc_root: row.doc_root,
            kind: read_kind(row.id, &row.kind, row.php_service_id, &row.config_json)?,
            https_enabled: row.https_enabled != 0,
            state: read_state(row.id, &row.state)?,
            domains,
            services,
            sharing: read_sharing(
                row.id,
                row.shared_interface,
                row.shared_address,
                row.shared_since,
                row.shared_until,
            )?,
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

/// The site an extension is served on, or [`None`] for one that has none — roadmap task **T81b**.
///
/// At most one, which `sites_one_per_extension` enforces; read back through [`records`] for
/// [`by_domain`]'s reason.
///
/// # Errors
///
/// The errors [`records`] gives.
pub async fn of_extension(store: &Store, id: &ExtensionId) -> Result<Option<SiteRecord>> {
    let id_column = id.as_str();

    let found = sqlx::query_scalar!("SELECT id FROM sites WHERE extension_id = ?", id_column)
        .fetch_optional(store.pool())
        .await
        .map_err(|source| store.failure("read", source))?;

    let Some(site) = found else {
        return Ok(None);
    };

    Ok(records(store, None)
        .await?
        .into_iter()
        .find(|record| record.id == site))
}

/// Every extension whose site is frozen on this pool — roadmap task **T81b**, the design's D9.
///
/// **Extensions only.** A project's site on the same pool is protected by its pin through
/// `projects::pins_broken_by`; this is the other half of `runtime.uninstall`'s refusal, and it
/// names the thing a person can act on.
///
/// # Errors
///
/// [`Error::Database`] when the table cannot be read, and [`Error::UnreadableSiteRow`] for an
/// `extension_id` that is not one.
pub async fn frozen_on(store: &Store, pool: &ServiceId) -> Result<Vec<ExtensionId>> {
    let pool_column = pool.as_str();

    let rows = sqlx::query!(
        r#"SELECT id, extension_id AS "extension!: String"
           FROM sites
           WHERE extension_id IS NOT NULL AND php_service_id = ?
           ORDER BY extension_id"#,
        pool_column
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    rows.into_iter()
        .map(|row| {
            ExtensionId::parse(row.extension.clone()).map_err(|_| Error::UnreadableSiteRow {
                site: row.id,
                column: "extension_id",
                value: row.extension,
            })
        })
        .collect()
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
    //
    // **The owner is read with it** — roadmap task **T82a** — because the refusal below is about who
    // this site belongs to, and a second read outside the transaction would be a second answer.
    let owner = sqlx::query_scalar!(
        r#"SELECT extension_id AS "extension: String" FROM sites WHERE id = ?"#,
        id
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(|source| store.failure("read", source))?
    .ok_or_else(|| Error::NotFound {
        kind: "site",
        id: id.to_string(),
    })?;

    let owner = owner
        .map(|value| {
            ExtensionId::parse(value.clone()).map_err(|_| Error::UnreadableSiteRow {
                site: id,
                column: "extension_id",
                value,
            })
        })
        .transpose()?;

    // **A pool an extension owns serves that extension and nothing else** — the design's D5. Here
    // as well as in [`create`], because `blueprint.apply` and `site.update` both arrive through this
    // door and neither goes through a CLI.
    if let Some(kind) = &change.kind {
        pool_is_free_for(store, &mut *tx, owner.as_ref(), kind).await?;
    }

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

/// The URL a phone opens, for a site shared at `address` and answered on `port`.
///
/// **HTTP, and http alone until T75** — the T74 design. The certificate does cover this address,
/// but a phone will not trust this home's authority until it has installed it, and the endpoint
/// that serves the authority is T75's. A URL a person is told to open must be one that opens.
///
/// The port is omitted when it is 80, because a URL a person reads off a terminal and types into a
/// phone is a URL worth keeping short.
#[must_use]
pub fn shared_url(address: std::net::Ipv4Addr, port: u16) -> String {
    match port {
        80 => format!("http://{address}"),
        other => format!("http://{address}:{other}"),
    }
}

/// The mDNS name a shared site answers to: `<slug>-mixengine.local` — roadmap task **T75**.
///
/// **One label before `.local`, and that is not a style choice.** mDNS conventions single-label host
/// names under `.local` (RFC 6762 section 3) and Windows' resolver enforces the convention: measured
/// on 2026-08-31, `blog-mixengine.local` resolved to the shared address while
/// `blog.mixengine.local` answered *DNS name does not exist* — same responder, same interface, same
/// minute. The T75 design, D1, which is where the roadmap's and the feature spec's own spelling of
/// this name was overturned.
///
/// **Not `<slug>.local`**, which would also resolve. The flat `.local` namespace is shared with
/// every printer and phone on the Wi-Fi; `-mixengine` is what makes this our name rather than a
/// claim on somebody else's.
///
/// [`None`] where the primary domain has no usable label, which is what stops this ever answering
/// `-mixengine.local`.
#[must_use]
pub fn shared_name(primary: &str) -> Option<String> {
    let label = primary.split('.').next().unwrap_or(primary);

    crate::domains::slug(label).map(|slug| format!("{slug}-mixengine.local"))
}

/// The shared site already answering to `name`, if one is — roadmap task **T75**.
///
/// **Only shared sites collide.** The name is on the network while a site is shared and nowhere
/// else, so `blog.test` and `blog.dev` are an ordinary pair right up until somebody shares the
/// second one. `except` is the site being asked about, which must not collide with itself when it
/// is re-shared or when its domains are updated.
///
/// The T75 design, D2: what comes back is the namesake's primary domain, because the refusal names
/// the site somebody has to unshare.
#[must_use]
pub fn name_taken<'a>(records: &'a [SiteRecord], except: i64, name: &str) -> Option<&'a str> {
    records
        .iter()
        .filter(|record| record.id != except && record.sharing.is_some())
        .find(|record| {
            record
                .domains
                .first()
                .and_then(|primary| shared_name(primary))
                .is_some_and(|taken| taken == name)
        })
        .and_then(|record| record.domains.first().map(String::as_str))
}

/// The port this home's front end answers HTTP on, or 80 where it declares none.
///
/// **The answer port and not the bound one** — T43, D8: on macOS a front end listens on 8080 behind
/// a packet-filter redirect, and what a browser asks for is still 80. A LAN URL is what somebody
/// types, so it carries the number they type.
///
/// # Errors
///
/// [`Error::Database`] when the tables cannot be read.
pub async fn web_port(store: &Store, catalogue: &crate::generate::Catalogue) -> Result<u16> {
    let Some(front_end) = crate::services::front_end::held_by(store, catalogue).await? else {
        return Ok(80);
    };

    let port = sqlx::query_scalar!("SELECT port FROM services WHERE id = ?", front_end)
        .fetch_optional(store.pool())
        .await
        .map_err(|source| store.failure("read", source))?
        .flatten();

    Ok(port.and_then(|port| u16::try_from(port).ok()).unwrap_or(80))
}

/// The three sharing columns as one value, or the refusal for a row the trigger should have made
/// impossible.
///
/// **An unparsable address is a refusal and not a shrug.** The alternative — treating it as not
/// shared — would silently un-share a site whose listener is up and whose firewall rule is open,
/// which is the one direction this must never fail in.
fn read_sharing(
    site: i64,
    interface: Option<String>,
    address: Option<String>,
    since: Option<i64>,
    until: Option<i64>,
) -> Result<Option<Sharing>> {
    let (Some(interface), Some(address), Some(since)) = (interface, address, since) else {
        return Ok(None);
    };

    let parsed = address
        .parse::<std::net::Ipv4Addr>()
        .map_err(|_| Error::UnreadableSiteRow {
            site,
            column: "shared_address",
            value: address.clone(),
        })?;

    Ok(Some(Sharing {
        interface,
        address: parsed,
        since: mixengine_proto::Timestamp(since),
        until: until.map(mixengine_proto::Timestamp),
    }))
}

/// Write, or clear, a site's sharing — roadmap task **T74**.
///
/// All three columns move together, which is what the trigger in `0012_site_sharing.sql` holds and
/// what [`Sharing`] makes unrepresentable otherwise. [`None`] is the unshare.
///
/// # Errors
///
/// [`Error::Database`] when the row cannot be written.
pub async fn set_sharing(store: &Store, id: i64, sharing: Option<&Sharing>) -> Result<SiteRecord> {
    let interface = sharing.map(|sharing| sharing.interface.clone());
    let address = sharing.map(|sharing| sharing.address.to_string());
    let since = sharing.map(|sharing| sharing.since.0);
    let until = sharing
        .and_then(|sharing| sharing.until)
        .map(|until| until.0);

    sqlx::query!(
        "UPDATE sites
         SET shared_interface = ?1, shared_address = ?2, shared_since = ?3, shared_until = ?4
         WHERE id = ?5",
        interface,
        address,
        since,
        until,
        id
    )
    .execute(store.pool())
    .await
    .map_err(|source| store.failure("write", source))?;

    record(store, id).await
}

/// One site by rowid, for the writers above that already know it exists.
async fn record(store: &Store, id: i64) -> Result<SiteRecord> {
    records(store, None)
        .await?
        .into_iter()
        .find(|record| record.id == id)
        .ok_or_else(|| Error::NotFound {
            kind: "site",
            id: id.to_string(),
        })
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

/// Refuse a pool that belongs to an extension to a site that is not that extension's — roadmap task
/// **T82a**, that design's D5.
///
/// **The instance half is the whole lookup.** `extensions::pools::id` composes `php-fpm@<id>`, so
/// *does this pool belong to an extension* is *is its instance an installed extension* — one query,
/// and exact because the rule is this build's own.
///
/// **Here rather than in the daemon**, because `blueprint.apply` and `domain.add` reach [`update`]
/// without going through a CLI, and a refusal they could cross is no refusal — T81b's D6 at a
/// second field.
///
/// **And it cannot be crossed by ordering.** The pool row exists only while the extension is
/// installed; a site must name a service that exists; and when the extension goes,
/// `sites.php_service_id` is `ON DELETE SET NULL`, so no site keeps a dangling name for a reinstall
/// to adopt.
async fn pool_is_free_for<'c>(
    store: &Store,
    executor: impl sqlx::SqliteExecutor<'c>,
    owner: Option<&ExtensionId>,
    kind: &SiteKind,
) -> Result<()> {
    let SiteKind::PhpFpm { pool: Some(pool) } = kind else {
        return Ok(());
    };

    let Some(instance) = pool.instance() else {
        return Ok(());
    };

    if owner.is_some_and(|owner| owner.as_str() == instance) {
        return Ok(());
    }

    let found = sqlx::query_scalar!("SELECT id FROM extensions WHERE id = ?", instance)
        .fetch_optional(executor)
        .await
        .map_err(|source| store.failure("read", source))?;

    match found {
        Some(extension) => Err(Error::ExtensionPoolNotShared {
            pool: pool.as_str().to_owned(),
            extension,
        }),
        None => Ok(()),
    }
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

    /// **A site that is not an extension's may not run on that extension's pool** — roadmap task
    /// **T82a**, that design's D5.
    ///
    /// That process holds a database superuser's password, read from the keyring at spawn; the
    /// whole point of giving a `web-app` a pool of its own is that no project's PHP is in it. Both
    /// halves are asserted together, because a refusal that also refused the extension's own site
    /// would have made the feature unbuildable rather than safe.
    #[tokio::test]
    async fn only_an_extensions_own_site_may_run_on_its_pool() {
        let (_temp, store, project) = home().await;
        an_extension_with_a_pool(&store, "phpmyadmin").await;
        let pool = ServiceId::parse("php-fpm@phpmyadmin").expect("an id");

        let refusal = create(
            &store,
            &NewSite {
                owner: SiteOwner::Project(project),
                doc_root: String::new(),
                kind: SiteKind::PhpFpm {
                    pool: Some(pool.clone()),
                },
                https_enabled: true,
                domains: vec!["blog.mixengine.test".to_owned()],
                services: Vec::new(),
            },
        )
        .await
        .expect_err("a project site on an extension's pool");

        let said = refusal.to_string();
        assert!(said.contains("php-fpm@phpmyadmin"), "{said}");
        assert!(said.contains("phpmyadmin extension"), "{said}");

        create(
            &store,
            &NewSite {
                owner: SiteOwner::Extension(
                    ExtensionId::parse("phpmyadmin").expect("an extension id"),
                ),
                doc_root: String::new(),
                kind: SiteKind::PhpFpm { pool: Some(pool) },
                https_enabled: true,
                domains: vec!["phpmyadmin.mixengine.test".to_owned()],
                services: Vec::new(),
            },
        )
        .await
        .expect("its own site names its own pool");
    }

    /// And `site.update` is held to the same rule, because `blueprint.apply` reaches it without a
    /// CLI — T81b's D6 arriving at a second field.
    #[tokio::test]
    async fn an_update_cannot_move_a_project_site_onto_an_extensions_pool() {
        let (_temp, store, project) = home().await;
        an_extension_with_a_pool(&store, "phpmyadmin").await;

        let site = create(
            &store,
            &NewSite {
                owner: SiteOwner::Project(project),
                doc_root: String::new(),
                kind: SiteKind::Static,
                https_enabled: true,
                domains: vec!["blog.mixengine.test".to_owned()],
                services: Vec::new(),
            },
        )
        .await
        .expect("a static site");

        let refusal = update(
            &store,
            site.id,
            &Change {
                kind: Some(SiteKind::PhpFpm {
                    pool: Some(ServiceId::parse("php-fpm@phpmyadmin").expect("an id")),
                }),
                ..Change::default()
            },
        )
        .await
        .expect_err("moved onto an extension's pool");

        assert!(
            refusal.to_string().contains("php-fpm@phpmyadmin"),
            "{refusal}"
        );
    }

    /// An installed extension and the php-fpm pool it owns, as an install would have left them.
    async fn an_extension_with_a_pool(store: &Store, id: &str) {
        sqlx::query(
            "INSERT INTO extensions
                 (id, name, version, kind, manifest_json, install_dir, data_dir, source, signed,
                  installed_at)
             VALUES (?1, ?1, '1.0.0', 'web-app', '{}', '/extensions/x', '/data/x', 'registry', 1,
                     '2026-09-03T00:00:00Z')",
        )
        .bind(id)
        .execute(store.pool())
        .await
        .expect("an extensions row");

        sqlx::query(
            "INSERT INTO runtime_installs
                 (kind, version, channel, install_path, installed_at, size_bytes, source_url,
                  sha256, provides_json)
             VALUES ('php', '8.3.34', 'stable', '/runtimes/php', '2026-09-03T00:00:00Z', 1,
                     'https://example.invalid/php', 'ab', '{}')",
        )
        .execute(store.pool())
        .await
        .expect("a runtime row");

        sqlx::query(
            "INSERT INTO services (id, runtime_install_id, instance_name, state)
             VALUES ('php-fpm@' || ?1, (SELECT id FROM runtime_installs LIMIT 1), ?1, 'stopped')",
        )
        .bind(id)
        .execute(store.pool())
        .await
        .expect("the extension's pool row");
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

    /// A site is created unshared, shares whole, and unshares back to nothing — roadmap task
    /// **T74**.
    #[tokio::test]
    async fn sharing_is_written_and_taken_away_as_one_value() {
        let (_temp, store, project) = home().await;

        let created = create(
            &store,
            &NewSite {
                owner: SiteOwner::Project(project),
                doc_root: String::new(),
                kind: SiteKind::Static,
                https_enabled: true,
                domains: vec!["blog.test".to_owned()],
                services: Vec::new(),
            },
        )
        .await
        .expect("a site");

        assert!(created.sharing.is_none(), "a site is created unshared");

        let sharing = Sharing {
            interface: "Wi-Fi".to_owned(),
            address: std::net::Ipv4Addr::new(192, 168, 1, 10),
            since: Timestamp(1_700_000_000_000),
            until: None,
        };

        let shared = set_sharing(&store, created.id, Some(&sharing))
            .await
            .expect("a share");

        assert_eq!(shared.sharing.as_ref(), Some(&sharing));

        // Read back through the ordinary listing rather than the writer's own answer: what a
        // consumer sees is what is on disk.
        let listed = records(&store, None).await.expect("the sites");
        assert_eq!(listed[0].sharing.as_ref(), Some(&sharing));

        let unshared = set_sharing(&store, created.id, None)
            .await
            .expect("an unshare");

        assert!(unshared.sharing.is_none());
    }

    /// A deadline rides with the rest of the sharing row and leaves with it — roadmap task **T76**.
    ///
    /// **`until` is the one of the four that is optional on its own.** A share without an expiry is
    /// the ordinary case, which is why it sits outside the trigger's all-or-nothing rule rather than
    /// inside it — and why an *unshared* site carrying one is still refused.
    #[tokio::test]
    async fn a_share_carries_a_deadline_and_loses_it_with_the_rest() {
        let (_temp, store, project) = home().await;

        let created = create(
            &store,
            &NewSite {
                owner: SiteOwner::Project(project),
                doc_root: String::new(),
                kind: SiteKind::Static,
                https_enabled: false,
                domains: vec!["blog.test".to_owned()],
                services: Vec::new(),
            },
        )
        .await
        .expect("a site");

        let sharing = Sharing {
            interface: "Wi-Fi".to_owned(),
            address: std::net::Ipv4Addr::new(192, 168, 1, 10),
            since: Timestamp(1_700_000_000_000),
            until: Some(Timestamp(1_700_000_007_200_000)),
        };

        let shared = set_sharing(&store, created.id, Some(&sharing))
            .await
            .expect("a share");
        assert_eq!(shared.sharing.as_ref(), Some(&sharing));

        // Read back through the ordinary listing: what a consumer sees is what is on disk.
        let listed = records(&store, None).await.expect("the sites");
        assert_eq!(
            listed[0].sharing.as_ref().and_then(|one| one.until),
            Some(Timestamp(1_700_000_007_200_000))
        );

        // A share with no deadline is the ordinary one, and not a half-written row.
        let forever = Sharing {
            until: None,
            ..sharing.clone()
        };
        let shared = set_sharing(&store, created.id, Some(&forever))
            .await
            .expect("a share with no deadline");
        assert_eq!(shared.sharing.as_ref(), Some(&forever));

        let unshared = set_sharing(&store, created.id, None)
            .await
            .expect("an unshare");
        assert!(unshared.sharing.is_none());
    }

    /// The trigger in `0012_site_sharing.sql`, from the side that would break it: two of the three
    /// columns set is a state no reader could make sense of, so the database refuses it outright.
    #[tokio::test]
    async fn a_half_written_share_is_refused_by_the_database() {
        let (_temp, store, project) = home().await;

        let created = create(
            &store,
            &NewSite {
                owner: SiteOwner::Project(project),
                doc_root: String::new(),
                kind: SiteKind::Static,
                https_enabled: true,
                domains: vec!["blog.test".to_owned()],
                services: Vec::new(),
            },
        )
        .await
        .expect("a site");

        let refused = sqlx::query!(
            "UPDATE sites SET shared_interface = 'Wi-Fi' WHERE id = ?",
            created.id
        )
        .execute(store.pool())
        .await;

        assert!(refused.is_err(), "the database allowed half a share");
    }

    /// The URL is what somebody types into a phone, so 80 is left off and anything else is not.
    #[test]
    fn a_shared_url_carries_the_port_only_when_it_is_not_eighty() {
        let address = std::net::Ipv4Addr::new(192, 168, 1, 10);

        assert_eq!(shared_url(address, 80), "http://192.168.1.10");
        assert_eq!(shared_url(address, 8080), "http://192.168.1.10:8080");
    }

    /// A site, its ordered domains and its links are one write.    /// A site, its ordered domains and its links are one write.
    #[tokio::test]
    async fn a_site_is_created_whole_and_read_back_whole() {
        let (_temp, store, project) = home().await;

        let created = create(
            &store,
            &NewSite {
                owner: SiteOwner::Project(project),
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
                owner: SiteOwner::Project(project),
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
                owner: SiteOwner::Project(project),
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
                owner: SiteOwner::Project(project),
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
                owner: SiteOwner::Project(project),
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
                    owner: SiteOwner::Project(project),
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

    /// The name a phone resolves — roadmap task **T75**.
    ///
    /// **One label before `.local`.** Measured on Windows: `blog-mixengine.local` resolves and
    /// `blog.mixengine.local` does not, same responder, same interface. The T75 design, D1.
    #[test]
    fn the_shared_name_is_one_label_under_local() {
        assert_eq!(
            shared_name("blog.test").as_deref(),
            Some("blog-mixengine.local")
        );
    }

    /// A hand-written domain need not already be a slug, so the label goes through the one
    /// definition of a slug this crate has.
    #[test]
    fn the_label_is_slugged_rather_than_trusted() {
        assert_eq!(
            shared_name("My_Shop.test").as_deref(),
            Some("my-shop-mixengine.local")
        );
    }

    /// A domain with no dot is its own label rather than an error.
    #[test]
    fn a_single_label_domain_is_its_own_label() {
        assert_eq!(shared_name("blog").as_deref(), Some("blog-mixengine.local"));
    }

    /// **Never `-mixengine.local`.** `slug` answers [`None`] when nothing is left, and so does this.
    #[test]
    fn a_domain_with_no_usable_label_has_no_name() {
        assert_eq!(shared_name("---.test"), None);
    }

    /// A record as `name_taken` reads one: an id, a primary domain, and whether it is shared.
    fn a_namesake(id: i64, primary: &str, shared: bool) -> SiteRecord {
        SiteRecord {
            id,
            owner: SiteOwner::Project(1),
            doc_root: String::new(),
            kind: SiteKind::Static,
            https_enabled: false,
            state: SiteState::Enabled,
            domains: vec![primary.to_owned()],
            services: Vec::new(),
            sharing: shared.then(|| Sharing {
                interface: "Wi-Fi".to_owned(),
                address: [192, 168, 1, 10].into(),
                since: Timestamp(1),
                until: None,
            }),
        }
    }

    /// Two shared sites whose first labels agree cannot both hold the name.
    #[test]
    fn a_second_shared_site_with_the_same_label_is_taken() {
        let records = vec![
            a_namesake(1, "blog.test", true),
            a_namesake(2, "blog.dev", false),
        ];

        assert_eq!(
            name_taken(&records, 2, "blog-mixengine.local"),
            Some("blog.test")
        );
    }

    /// **An unshared namesake is not a collision.** The name exists on the network only while a
    /// site is shared, so a home full of `blog.*` sites is ordinary until two are shared at once.
    #[test]
    fn an_unshared_namesake_does_not_take_the_name() {
        let records = vec![
            a_namesake(1, "blog.test", false),
            a_namesake(2, "blog.dev", false),
        ];

        assert_eq!(name_taken(&records, 2, "blog-mixengine.local"), None);
    }

    /// **A site never collides with itself**, which is what makes re-sharing and `site.update`
    /// idempotent rather than a refusal — the T75 design, D2.
    #[test]
    fn a_site_does_not_take_its_own_name() {
        let records = vec![a_namesake(1, "blog.test", true)];

        assert_eq!(name_taken(&records, 1, "blog-mixengine.local"), None);
    }

    /// **T81b, D1 and D3.** A site owned by an extension has no project, is reachable by its
    /// extension, is invisible to a project's listing, and goes when its extension goes.
    #[tokio::test]
    async fn an_extension_owns_a_site_the_way_a_project_does() {
        let (_temp, store, project) = home().await;
        let id = ExtensionId::parse("phpmyadmin").expect("an id");

        sqlx::query(
            "INSERT INTO extensions (id, name, version, kind, manifest_json, install_dir, data_dir,
                                     source, signed, installed_at)
             VALUES ('phpmyadmin', 'phpMyAdmin', '5.2.1', 'web-app', '{}', '/ext/phpmyadmin',
                     '/data/extensions/phpmyadmin', 'path', 0, '2026-09-03T00:00:00Z')",
        )
        .execute(store.pool())
        .await
        .expect("an extension row");

        let created = create(
            &store,
            &NewSite {
                owner: SiteOwner::Extension(id.clone()),
                doc_root: "app".to_owned(),
                kind: SiteKind::Static,
                https_enabled: true,
                domains: vec!["phpmyadmin.mixengine.test".to_owned()],
                services: Vec::new(),
            },
        )
        .await
        .expect("an extension-owned site");
        assert_eq!(created.owner, SiteOwner::Extension(id.clone()));

        let found = of_extension(&store, &id)
            .await
            .expect("a read")
            .expect("the site is found by its extension");
        assert_eq!(found.id, created.id);
        assert_eq!(found.domains, vec!["phpmyadmin.mixengine.test".to_owned()]);

        assert!(
            records(&store, Some(project))
                .await
                .expect("a read")
                .is_empty(),
            "a project's listing showed an extension's site"
        );
        assert_eq!(records(&store, None).await.expect("a read").len(), 1);

        sqlx::query("DELETE FROM extensions WHERE id = 'phpmyadmin'")
            .execute(store.pool())
            .await
            .expect("the delete");
        assert!(
            of_extension(&store, &id).await.expect("a read").is_none(),
            "the cascade did not take the site"
        );
    }

    /// **T81b, D9.** The extensions whose site is frozen on a pool, for `runtime.uninstall`'s
    /// refusal — and a project's site on the same pool is not one of them.
    #[tokio::test]
    async fn frozen_on_names_the_extensions_a_pool_serves() {
        let (_temp, store, project) = home().await;
        let id = ExtensionId::parse("phpmyadmin").expect("an id");
        let pool = ServiceId::parse("php-fpm@8.3.34").expect("an id");

        for statement in [
            "INSERT INTO runtime_installs (id, kind, version, channel, install_path, installed_at,
                                           size_bytes, source_url, sha256, provides_json)
             VALUES (1, 'php', '8.3.34', 'stable', '/runtimes/php/8.3.34', '2026-09-03T00:00:00Z',
                     1, 'https://example.invalid/php', 'ab', '{\"php-fpm\":\"sbin/php-fpm\"}')",
            "INSERT INTO services (id, runtime_install_id, instance_name, state, port)
             VALUES ('php-fpm@8.3.34', 1, '8.3.34', 'stopped', 9000)",
            "INSERT INTO extensions (id, name, version, kind, manifest_json, install_dir, data_dir,
                                     source, signed, installed_at)
             VALUES ('phpmyadmin', 'phpMyAdmin', '5.2.1', 'web-app', '{}', '/ext/phpmyadmin',
                     '/data/extensions/phpmyadmin', 'path', 0, '2026-09-03T00:00:00Z')",
        ] {
            sqlx::query(statement)
                .execute(store.pool())
                .await
                .unwrap_or_else(|error| panic!("{statement}: {error}"));
        }

        create(
            &store,
            &NewSite {
                owner: SiteOwner::Project(project),
                doc_root: String::new(),
                kind: SiteKind::PhpFpm {
                    pool: Some(pool.clone()),
                },
                https_enabled: true,
                domains: vec!["blog.test".to_owned()],
                services: Vec::new(),
            },
        )
        .await
        .expect("a project site");

        assert!(frozen_on(&store, &pool).await.expect("a read").is_empty());

        create(
            &store,
            &NewSite {
                owner: SiteOwner::Extension(id.clone()),
                doc_root: "app".to_owned(),
                kind: SiteKind::PhpFpm {
                    pool: Some(pool.clone()),
                },
                https_enabled: true,
                domains: vec!["phpmyadmin.mixengine.test".to_owned()],
                services: Vec::new(),
            },
        )
        .await
        .expect("an extension site");

        assert_eq!(frozen_on(&store, &pool).await.expect("a read"), vec![id]);
    }
}
