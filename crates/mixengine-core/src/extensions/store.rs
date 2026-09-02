//! The `extensions` and `extension_ports` tables — roadmap task **T81**.
//!
//! [`crate::packages`]' shape one table across: this module owns every write to both, and the
//! ordering every caller follows is the same one — put it on disk, *then* write the row; remove the
//! directory, *then* delete it.
//!
//! # What is different, and why
//!
//! **The manifest is stored, and it is the source of truth for the spec.** A `packages` row points
//! at a compiled-in recipe for the knowledge of how to run the thing; an extension carries that
//! knowledge itself, and it arrived in a document somebody consented to. So the row keeps it, and
//! **nothing re-reads `extension.toml` out of the install directory** — that file sits where a user
//! can edit it, and a manifest read back from it is one nobody agreed to.
//!
//! **What is stored is the reader's rendering, not the author's text**, which is T79's finding:
//! keeping somebody's bytes would make the file on disk, this column and what the renderer reads
//! three texts for one extension.
//!
//! **Ports are a table.** Not because an extension has many — though it does — but because
//! [`crate::services::ports::allocate`] asks the database which ports are held, and a port it
//! cannot see in SQL is one it hands out twice (the T81 design's D8).

use std::collections::BTreeMap;
use std::path::PathBuf;

use mixengine_proto::{ExtensionId, ExtensionKind, PackageVersion, Timestamp};

use crate::extensions::manifest::{self, ExtensionManifest};
use crate::{Error, Result, Store};

/// Where an extension came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The published registry, whose signature was checked when it arrived.
    Registry,

    /// A directory somebody pointed at. **Nothing vouches for one of these**, which is why it is
    /// recorded rather than inferred — see [`Installed::signed`].
    Path,
}

impl Source {
    /// How the column spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Registry => "registry",
            Self::Path => "path",
        }
    }

    /// Read it back, or [`None`] for a word this build does not know.
    #[must_use]
    pub fn parse(column: &str) -> Option<Self> {
        match column {
            "registry" => Some(Self::Registry),
            "path" => Some(Self::Path),
            _ => None,
        }
    }
}

/// One installed extension, as the database holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    /// Its id, which is also its directory name.
    pub id: ExtensionId,

    /// What it declared, canonically.
    pub manifest: ExtensionManifest,

    /// Where its own files are. Removed by an uninstall.
    pub install_dir: PathBuf,

    /// Where what it writes goes. **Outside `install_dir`** so an uninstall can keep it — the T81
    /// design's D13.
    pub data_dir: PathBuf,

    /// Where it came from.
    pub source: Source,

    /// Whether a signature covered it.
    ///
    /// Two-valued because the situation is: the registry's signature covers the whole document, so
    /// an entry either arrived inside something the compiled-in key vouched for or the document was
    /// refused entirely. `--path` is the false case, and it is false for every one of them.
    pub signed: bool,

    /// When it was installed.
    pub installed_at: Timestamp,

    /// The ports it holds, by the `[ports]` name each one was asked for under.
    pub ports: BTreeMap<String, u16>,
}

impl Installed {
    /// Its display name, out of the manifest rather than a column of its own.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.manifest.extension.name
    }

    /// Its own version.
    #[must_use]
    pub fn version(&self) -> &PackageVersion {
        &self.manifest.extension.version
    }

    /// What it is.
    #[must_use]
    pub fn kind(&self) -> ExtensionKind {
        self.manifest.extension.kind
    }
}

/// Where an extension's own files go.
#[must_use]
pub fn install_dir(paths: &crate::Paths, id: &ExtensionId) -> PathBuf {
    paths.extensions().join(id.as_str())
}

/// Where what it writes goes — deliberately not under [`install_dir`].
#[must_use]
pub fn data_dir(paths: &crate::Paths, id: &ExtensionId) -> PathBuf {
    paths.data().join("extensions").join(id.as_str())
}

/// Write down an extension that is now on disk, with the ports it holds.
///
/// One transaction, because a row whose ports did not land is a row the allocator will contradict.
///
/// # Errors
///
/// [`Error::ExtensionAlreadyInstalled`] when a row for this id already exists, and
/// [`Error::Database`] when it cannot be written.
pub async fn remember(store: &Store, installed: &Installed) -> Result<()> {
    let id = installed.id.as_str();
    let name = installed.name();
    let version = installed.version().as_str();
    let kind = installed.kind().as_str();
    let manifest = manifest::to_value(&installed.manifest).to_string();
    let install_dir = installed.install_dir.display().to_string();
    let data_dir = installed.data_dir.display().to_string();
    let source = installed.source.as_str();
    let signed = i64::from(installed.signed);
    let installed_at = installed.installed_at.to_rfc3339();

    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|source| store.failure("write", source))?;

    let inserted = sqlx::query!(
        "INSERT INTO extensions
             (id, name, version, kind, manifest_json, install_dir, data_dir, source, signed,
              installed_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (id) DO NOTHING",
        id,
        name,
        version,
        kind,
        manifest,
        install_dir,
        data_dir,
        source,
        signed,
        installed_at
    )
    .execute(&mut *transaction)
    .await
    .map_err(|source| store.failure("write", source))?;

    // `DO NOTHING` rather than letting the primary key raise, on `packages::remember`'s reasoning:
    // the collision is a real case and it deserves a sentence naming the extension rather than
    // SQLite's.
    if inserted.rows_affected() == 0 {
        return Err(Error::ExtensionAlreadyInstalled {
            id: installed.id.as_str().to_owned(),
        });
    }

    for (port_name, port) in &installed.ports {
        let port = i64::from(*port);
        sqlx::query!(
            "INSERT INTO extension_ports (extension_id, name, port) VALUES (?, ?, ?)",
            id,
            port_name,
            port
        )
        .execute(&mut *transaction)
        .await
        .map_err(|source| store.failure("write", source))?;
    }

    transaction
        .commit()
        .await
        .map_err(|source| store.failure("write", source))?;

    tracing::info!(extension = %id, %version, "an extension was recorded");

    Ok(())
}

/// Forget an extension whose directory has already gone.
///
/// Its ports go with it, by the cascade — which is what releases them back to the allocator.
///
/// # Errors
///
/// [`Error::Database`] when the row cannot be deleted, which includes a `services` row still
/// pointing at it: that foreign key is `RESTRICT`, so removing an extension out from under a
/// running service is refused rather than carried out.
pub async fn forget(store: &Store, id: &ExtensionId) -> Result<()> {
    let id = id.as_str();

    sqlx::query!("DELETE FROM extensions WHERE id = ?", id)
        .execute(store.pool())
        .await
        .map_err(|source| store.failure("delete", source))?;

    tracing::info!(extension = %id, "an extension was forgotten");

    Ok(())
}

/// One installed extension, or [`None`] where nothing is installed under that id.
///
/// # Errors
///
/// [`Error::Database`] when the tables cannot be read, and [`Error::UnreadableExtensionRow`] when a
/// row holds something this build cannot read back.
pub async fn get(store: &Store, id: &ExtensionId) -> Result<Option<Installed>> {
    Ok(all(store)
        .await?
        .into_iter()
        .find(|installed| installed.id == *id))
}

/// Every installed extension, by id.
///
/// **One query and one reading**, rather than a `get` the listing calls per row: the manifests are
/// a few kilobytes each and a home has a handful of extensions, so the join is cheaper than the
/// second code path would be to keep honest.
///
/// # Errors
///
/// As [`get`].
pub async fn all(store: &Store) -> Result<Vec<Installed>> {
    let rows = sqlx::query!(
        "SELECT id, manifest_json, install_dir, data_dir, source, signed, installed_at
           FROM extensions
          ORDER BY id"
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    let held = sqlx::query!("SELECT extension_id, name, port FROM extension_ports")
        .fetch_all(store.pool())
        .await
        .map_err(|source| store.failure("read", source))?;

    let mut installed = Vec::with_capacity(rows.len());

    for row in rows {
        let unreadable = |column: &'static str, value: String| Error::UnreadableExtensionRow {
            extension: row.id.clone(),
            column,
            value,
        };

        let id =
            ExtensionId::parse(&row.id).map_err(|source| unreadable("id", source.to_string()))?;

        let manifest = serde_json::from_str(&row.manifest_json)
            .map_err(|source| unreadable("manifest_json", source.to_string()))
            .and_then(|entry| {
                manifest::read_value(entry)
                    .map_err(|source| unreadable("manifest_json", source.to_string()))
            })?;

        let source =
            Source::parse(&row.source).ok_or_else(|| unreadable("source", row.source.clone()))?;

        let installed_at = Timestamp::parse_rfc3339(&row.installed_at)
            .ok_or_else(|| unreadable("installed_at", row.installed_at.clone()))?;

        let ports = held
            .iter()
            .filter(|port| port.extension_id == row.id)
            .map(|port| {
                u16::try_from(port.port)
                    .map(|number| (port.name.clone(), number))
                    .map_err(|_| unreadable("port", port.port.to_string()))
            })
            .collect::<Result<BTreeMap<String, u16>>>()?;

        installed.push(Installed {
            id,
            manifest,
            install_dir: PathBuf::from(row.install_dir),
            data_dir: PathBuf::from(row.data_dir),
            source,
            signed: row.signed != 0,
            installed_at,
            ports,
        });
    }

    Ok(installed)
}

/// Whether anything is installed under `id` — asked before an install, where reading the whole row
/// would be reading a manifest to throw it away.
///
/// # Errors
///
/// [`Error::Database`] when the table cannot be read.
pub async fn exists(store: &Store, id: &ExtensionId) -> Result<bool> {
    let id = id.as_str();

    let found: Option<String> = sqlx::query_scalar!("SELECT id FROM extensions WHERE id = ?", id)
        .fetch_optional(store.pool())
        .await
        .map_err(|source| store.failure("read", source))?;

    Ok(found.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&home.path().join(crate::paths::DATABASE_FILE_NAME))
            .await
            .expect("a database");
        (home, store)
    }

    fn installed_fixture(source: Source, signed: bool) -> Installed {
        let manifest = manifest::read(
            std::path::Path::new("extension.toml"),
            mixengine_testkit::extension::MAILPIT,
        )
        .expect("a fixture parses");

        Installed {
            id: manifest.extension.id.clone(),
            install_dir: PathBuf::from("/x/extensions/mailpit"),
            data_dir: PathBuf::from("/x/data/extensions/mailpit"),
            ports: manifest.ports.clone(),
            manifest,
            source,
            signed,
            installed_at: Timestamp::parse_rfc3339("2026-09-02T09:00:00Z").expect("a timestamp"),
        }
    }

    /// A row and its ports are one write, and read back as one thing.
    #[tokio::test]
    async fn an_extension_and_its_ports_are_one_write() {
        let (_home, store) = store().await;
        let installed = installed_fixture(Source::Registry, true);

        remember(&store, &installed).await.expect("a write");

        let read = get(&store, &installed.id)
            .await
            .expect("a read")
            .expect("the row");
        assert_eq!(read, installed);
        assert_eq!(read.ports.len(), 2, "Mailpit asks for a UI port and SMTP");
    }

    /// **The stored manifest is the reader's rendering** — the T81 design's D5.
    #[tokio::test]
    async fn the_stored_manifest_is_canonical() {
        let (_home, store) = store().await;
        let installed = installed_fixture(Source::Registry, true);
        remember(&store, &installed).await.expect("a write");

        let column: String =
            sqlx::query_scalar("SELECT manifest_json FROM extensions WHERE id = 'mailpit'")
                .fetch_one(store.pool())
                .await
                .expect("the column");

        assert_eq!(column, manifest::to_value(&installed.manifest).to_string());
    }

    /// A `--path` install is recorded as unsigned, and stays that way when it is read back.
    #[tokio::test]
    async fn a_path_install_is_unsigned_in_the_row() {
        let (_home, store) = store().await;
        let installed = installed_fixture(Source::Path, false);
        remember(&store, &installed).await.expect("a write");

        let read = get(&store, &installed.id)
            .await
            .expect("a read")
            .expect("the row");

        assert_eq!(read.source, Source::Path);
        assert!(!read.signed);
    }

    /// Installing over one already installed is refused by name rather than by SQLite.
    #[tokio::test]
    async fn one_id_is_installed_once() {
        let (_home, store) = store().await;
        let installed = installed_fixture(Source::Registry, true);
        remember(&store, &installed).await.expect("the first");

        let refusal = remember(&store, &installed).await.expect_err("the second");

        assert!(
            matches!(refusal, Error::ExtensionAlreadyInstalled { ref id } if id == "mailpit"),
            "{refusal}"
        );
    }

    /// **Forgetting releases the ports**, which is what makes them available to the allocator
    /// again.
    #[tokio::test]
    async fn forgetting_releases_the_ports() {
        let (_home, store) = store().await;
        let installed = installed_fixture(Source::Registry, true);
        remember(&store, &installed).await.expect("a write");

        forget(&store, &installed.id).await.expect("a delete");

        let held: i64 = sqlx::query_scalar("SELECT count(*) FROM extension_ports")
            .fetch_one(store.pool())
            .await
            .expect("a count");
        assert_eq!(held, 0);
        assert!(!exists(&store, &installed.id).await.expect("a read"));
    }

    /// Nothing installed is an empty answer rather than a failure.
    #[tokio::test]
    async fn a_home_with_no_extensions_lists_none() {
        let (_home, store) = store().await;

        assert!(all(&store).await.expect("a read").is_empty());
    }
}
