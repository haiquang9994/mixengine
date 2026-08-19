//! The `packages` table, and where on disk a server, a database or a cache is unpacked.
//!
//! [`crate::runtimes`] one table across, and deliberately the same shape: this module owns every
//! write to `packages` and nothing else, and the ordering every caller follows is the same one —
//! install into place, *then* write the row; remove the directory, *then* delete the row. The
//! reasoning is [`crate::runtimes`]' own and is not repeated here.
//!
//! # What is different, and why
//!
//! **There is no default version.** A runtime has one because a program called `php` has to resolve
//! to something with nobody present to ask; a service instance names its version in its own row, so
//! there is nothing left to decide. That removes the transaction [`crate::runtimes::remember`]
//! needs — a single insert is atomic on its own.
//!
//! **A row can be held.** `services.package_id` is `ON DELETE RESTRICT`, so a package with an
//! instance is one nothing may remove; [`holders`] is what turns that constraint into a sentence
//! naming the services, and [`PackageSummary::services`] is what puts it in the listing rather than
//! only in the failure.
//!
//! **What knows how to *run* a package is not here.** Which executable proves an install works is
//! [`Recipe::smoke_test`](crate::generate::Recipe::smoke_test) — a fact about Caddy, held beside the
//! template that configures it — where the runtime equivalent is a `match` in
//! [`crate::runtimes::smoke_test`] because a runtime has no recipe to hold it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mixengine_proto::{PackageSummary, PackageVersion, ServiceId, Timestamp, VersionError};

use crate::{Error, Paths, Result, Store};

/// Where one version of one package lives.
///
/// `packages/<name>/<version>/`, which is the layout [`Context::install_path`] has always read, and
/// the reason a version is a validated path component ([`PackageVersion`]) rather than a string:
/// this is a `join` and not an escaping problem.
///
/// [`Context::install_path`]: crate::generate::Context::install_path
#[must_use]
pub fn directory(paths: &Paths, package: &str, version: &PackageVersion) -> PathBuf {
    paths.packages().join(package).join(version.as_str())
}

/// Everything a finished install has to write down.
///
/// Taken as one value rather than as six arguments, on
/// [`runtimes::Installation`](crate::runtimes::Installation)'s reasoning: most of them come straight
/// off the artifact that was installed, and a caller assembling them in the wrong order would
/// produce a row that is wrong rather than one that fails to insert.
#[derive(Debug, Clone)]
pub struct Installation {
    /// Which package, by the name a recipe is found under.
    pub package: String,

    /// Which version.
    pub version: PackageVersion,

    /// Where it landed — [`directory`]'s answer, passed in rather than recomputed so the row names
    /// the directory that was actually renamed into place.
    pub path: PathBuf,

    /// How large the archive was, as the index declared it and the download proved it.
    pub bytes: u64,

    /// Which URL it came from, kept so a support conversation can ask "from where".
    pub url: String,

    /// The hash the index published for it, kept for the same reason.
    pub sha256: String,
}

/// Write down a package that is now on disk.
///
/// # Errors
///
/// [`Error::PackageAlreadyRecorded`] when a row for this name and version already exists, and
/// [`Error::Database`] when it cannot be written.
pub async fn remember(
    store: &Store,
    installation: &Installation,
    at: Timestamp,
) -> Result<PackageSummary> {
    let (package, version) = (&installation.package, installation.version.as_str());
    let installed_at = at.to_rfc3339();
    let path = installation.path.display().to_string();
    // The column is INTEGER and the value is a count of bytes: a size that does not fit an `i64` is
    // an artifact of nine million terabytes, so the saturation is a formality rather than a case.
    let bytes = i64::try_from(installation.bytes).unwrap_or(i64::MAX);

    let inserted = sqlx::query!(
        "INSERT INTO packages
             (name, version, install_path, installed_at, source_url, sha256, size_bytes)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (name, version) DO NOTHING",
        package,
        version,
        path,
        installed_at,
        installation.url,
        installation.sha256,
        bytes
    )
    .execute(store.pool())
    .await
    .map_err(|source| store.failure("write", source))?;

    // `DO NOTHING` rather than letting the unique index raise, on
    // [`runtimes::remember`](crate::runtimes::remember)'s reasoning: the collision is a real case —
    // two clients asking for the same install at the same moment — and it deserves a sentence
    // naming the version rather than SQLite's.
    if inserted.rows_affected() == 0 {
        return Err(Error::PackageAlreadyRecorded {
            package: installation.package.clone(),
            version: installation.version.clone(),
        });
    }

    tracing::info!(%package, %version, "a package was recorded");

    Ok(PackageSummary {
        package: installation.package.clone(),
        version: installation.version.clone(),
        path,
        installed_at: at,
        bytes: installation.bytes,
        // Nothing can be an instance of a version that did not exist a moment ago.
        services: Vec::new(),
    })
}

/// Forget a package whose directory has already gone.
///
/// # Errors
///
/// [`Error::NotFound`] when there is no such row, [`Error::Database`] when it cannot be written —
/// which includes the foreign key refusing while a service is still an instance of it, a case
/// callers are expected to have reported through [`holders`] first.
pub async fn forget(store: &Store, package: &str, version: &PackageVersion) -> Result<()> {
    let version_column = version.as_str();

    let removed = sqlx::query!(
        "DELETE FROM packages WHERE name = ? AND version = ?",
        package,
        version_column
    )
    .execute(store.pool())
    .await
    .map_err(|source| store.failure("write", source))?;

    if removed.rows_affected() == 0 {
        return Err(missing(package, version));
    }

    tracing::info!(%package, %version_column, "a package was forgotten");

    Ok(())
}

/// Every installed package, optionally of one name.
///
/// Ordered by name and then by the version string, on
/// [`runtimes::records`](crate::runtimes::records)' reasoning: a listing is a table somebody scans
/// for a row they already have in mind, and the order that makes a row findable is the one the eye
/// can predict.
///
/// # Errors
///
/// [`Error::UnreadablePackageRow`] when a row holds something this build cannot read back, and
/// [`Error::Database`] when the table cannot be read.
pub async fn records(store: &Store, package: Option<&str>) -> Result<Vec<PackageSummary>> {
    // One query and a grouped read rather than a listing plus a lookup for each row: the second is
    // N+1 against a table whose whole purpose here is to be joined.
    //
    // The same value bound twice rather than a numbered parameter, on `runtimes::records`'
    // precedent: `?1` is valid SQLite and it is the sqlx macro that has an opinion about it.
    let rows = sqlx::query!(
        "SELECT p.name, p.version, p.install_path, p.installed_at, p.size_bytes, s.id AS service
         FROM packages p
         LEFT JOIN services s ON s.package_id = p.id
         WHERE (? IS NULL OR p.name = ?)
         ORDER BY p.name, p.version, s.id",
        package,
        package
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    // A `BTreeMap` rather than a fold over the rows in order: the ordering above already groups
    // them, and a map says so without depending on it.
    let mut listed: BTreeMap<(String, String), PackageSummary> = BTreeMap::new();

    for row in rows {
        let key = (row.name.clone(), row.version.clone());

        let entry = match listed.entry(key) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => entry.insert(summary(
                row.name,
                row.version,
                row.install_path,
                &row.installed_at,
                row.size_bytes,
            )?),
        };

        if let Some(service) = row.service {
            entry
                .services
                .push(ServiceId::parse(service.clone()).map_err(|_| {
                    Error::UnreadablePackageRow {
                        column: "services.id",
                        value: service,
                    }
                })?);
        }
    }

    Ok(listed.into_values().collect())
}

/// One installed package.
///
/// # Errors
///
/// [`Error::NotFound`] when it is not installed, [`Error::UnreadablePackageRow`] when the row cannot
/// be read back, and [`Error::Database`] when the table cannot be read.
pub async fn record(
    store: &Store,
    package: &str,
    version: &PackageVersion,
) -> Result<PackageSummary> {
    records(store, Some(package))
        .await?
        .into_iter()
        .find(|summary| &summary.version == version)
        .ok_or_else(|| missing(package, version))
}

/// The services that are instances of one installed version, in [`ServiceId`] order.
///
/// **What an uninstall refuses over.** `services.package_id` is `ON DELETE RESTRICT`, so the
/// alternative to asking is a foreign-key failure whose message names a constraint rather than the
/// three services a person has to delete first.
///
/// # Errors
///
/// [`Error::UnreadablePackageRow`] when a `services.id` cannot be read back as one, and
/// [`Error::Database`] when the table cannot be read.
pub async fn holders(
    store: &Store,
    package: &str,
    version: &PackageVersion,
) -> Result<Vec<ServiceId>> {
    let version_column = version.as_str();

    let rows = sqlx::query_scalar!(
        "SELECT s.id FROM services s
         JOIN packages p ON p.id = s.package_id
         WHERE p.name = ? AND p.version = ?
         ORDER BY s.id",
        package,
        version_column
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    rows.into_iter()
        .map(|id| {
            ServiceId::parse(id.clone()).map_err(|_| Error::UnreadablePackageRow {
                column: "services.id",
                value: id,
            })
        })
        .collect()
}

/// Remove an installed package's directory, if it is there at all.
///
/// **Before the row, never after**, which is [`crate::runtimes::discard`]'s ordering and its
/// reasoning: a directory that could not be removed leaves a row that still describes it, and asking
/// again repeats exactly this.
///
/// # Errors
///
/// [`Error::Io`] naming the directory, which on Windows most often means a process is still running
/// out of it.
pub async fn discard(path: &Path) -> Result<()> {
    crate::runtimes::discard(path).await
}

/// The failure of looking one up, named the way the wire mapping expects.
fn missing(package: &str, version: &PackageVersion) -> Error {
    Error::NotFound {
        kind: "package",
        id: format!("{package} {version}"),
    }
}

/// One row, as the type a client renders, with no services attached yet.
fn summary(
    package: String,
    version: String,
    path: String,
    installed_at: &str,
    bytes: i64,
) -> Result<PackageSummary> {
    let unreadable = |column: &'static str, value: &str| Error::UnreadablePackageRow {
        column,
        value: value.to_owned(),
    };

    Ok(PackageSummary {
        version: PackageVersion::parse(version.clone())
            .map_err(|VersionError { value, .. }| unreadable("version", &value))?,
        installed_at: Timestamp::parse_rfc3339(installed_at)
            .ok_or_else(|| unreadable("installed_at", installed_at))?,
        // A negative size is not expressible through any write of ours and would be a hand-edited
        // row; read as zero rather than refused, because the number is a display value and refusing
        // the whole listing over it would hide the package it belongs to.
        bytes: u64::try_from(bytes).unwrap_or(0),
        package,
        path,
        services: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Timestamp = Timestamp(1_760_000_000_000);

    async fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&home.path().join(crate::paths::DATABASE_FILE_NAME))
            .await
            .expect("a database");
        (home, store)
    }

    fn version(text: &str) -> PackageVersion {
        PackageVersion::parse(text).expect("a valid version")
    }

    fn installation(package: &str, text: &str) -> Installation {
        Installation {
            package: package.to_owned(),
            version: version(text),
            path: PathBuf::from("/home/packages").join(package).join(text),
            bytes: 41_000_000,
            url: format!("https://example.invalid/{package}-{text}.tar.zst"),
            sha256: "00".to_owned(),
        }
    }

    /// Declare `service` as an instance of an already-recorded package.
    async fn instantiate(store: &Store, service: &str, package: &str, version: &str) {
        sqlx::query(
            "INSERT INTO services (id, package_id, instance_name, state)
             VALUES (?, (SELECT id FROM packages WHERE name = ? AND version = ?), ?, 'stopped')",
        )
        .bind(service)
        .bind(package)
        .bind(version)
        .bind(service)
        .execute(store.pool())
        .await
        .expect("a services row");
    }

    /// `packages/<name>/<version>` — the layout `Context::install_path` has always read.
    #[test]
    fn a_package_lands_under_its_name_and_version() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let paths = Paths::new(
            home.path().to_path_buf(),
            &crate::config::PathOverrides::default(),
        );

        assert_eq!(
            directory(&paths, "caddy", &version("2.11.4")),
            paths.packages().join("caddy").join("2.11.4")
        );
    }

    /// What was written is what comes back, including the services holding it — which is none.
    #[tokio::test]
    async fn a_recorded_package_is_listed_with_nothing_holding_it() {
        let (_home, store) = store().await;

        let written = remember(&store, &installation("caddy", "2.11.4"), NOW)
            .await
            .expect("a package is recorded");

        assert_eq!(written.package, "caddy");
        assert_eq!(written.services, Vec::<ServiceId>::new());
        assert_eq!(
            records(&store, None).await.expect("a listing"),
            vec![written]
        );
    }

    /// The same version twice is two clients asking at once, and deserves a sentence naming it
    /// rather than SQLite's unique-index error.
    #[tokio::test]
    async fn recording_a_package_twice_is_refused_by_name() {
        let (_home, store) = store().await;

        remember(&store, &installation("caddy", "2.11.4"), NOW)
            .await
            .expect("the first");
        let error = remember(&store, &installation("caddy", "2.11.4"), NOW)
            .await
            .expect_err("the second");

        assert!(format!("{error}").contains("2.11.4"), "{error}");
    }

    /// What an uninstall has to refuse over, and the reason [`PackageSummary`] carries it.
    #[tokio::test]
    async fn a_package_names_the_services_that_are_instances_of_it() {
        let (_home, store) = store().await;
        remember(&store, &installation("caddy", "2.11.4"), NOW)
            .await
            .expect("recorded");
        instantiate(&store, "caddy", "caddy", "2.11.4").await;

        let held = holders(&store, "caddy", &version("2.11.4"))
            .await
            .expect("a lookup");

        assert_eq!(held, vec![ServiceId::parse("caddy").expect("an id")]);
        assert_eq!(
            record(&store, "caddy", &version("2.11.4"))
                .await
                .expect("the record")
                .services,
            held,
            "a listing says what is holding a package, not only a failure"
        );
    }

    /// A name nobody installed is a `NotFound` rather than an empty success.
    #[tokio::test]
    async fn forgetting_a_package_that_was_never_here_says_so() {
        let (_home, store) = store().await;

        let error = forget(&store, "caddy", &version("2.11.4"))
            .await
            .expect_err("nothing to forget");

        assert!(
            matches!(
                error,
                Error::NotFound {
                    kind: "package",
                    ..
                }
            ),
            "{error}"
        );
    }
}
