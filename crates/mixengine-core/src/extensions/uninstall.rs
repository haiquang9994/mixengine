//! Removing an installed extension — roadmap task **T81**, the design's D12.
//!
//! # The reverse of the install, and tolerant of a half-done one
//!
//! Delete the `services` row, forget the extension (which releases its ports by the cascade),
//! remove the install directory, and remove the data directory only if somebody asked. Each step
//! tolerates its predecessor having already happened, because an uninstall interrupted by a daemon
//! restart has to be able to finish rather than refuse: what would otherwise be left is an extension
//! that cannot be installed again and cannot be removed either.
//!
//! # The data directory is kept
//!
//! Unless asked otherwise. It is the one thing here a person cannot get back, and it is why the
//! layout puts it outside the install directory in the first place (D13) — an uninstall that had to
//! delete everything under one root would be one that could not make this promise.
//!
//! **Nothing here stops a process.** Supervision belongs to the daemon, so the order it walks is
//! stop-then-this; what this refuses on its own behalf is the `RESTRICT` on `services.extension_id`,
//! which is the database saying an extension is not removable while something still runs out of it.

use std::path::{Path, PathBuf};

use mixengine_proto::{ExtensionId, ExtensionKind, ServiceId};

use crate::extensions::store::{self as extension_store, Installed};
use crate::{Error, Paths, Result, Store};

/// What an uninstall did, for a client to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    /// Which extension.
    pub id: ExtensionId,

    /// The service that went with it, where there was one.
    pub service: Option<ServiceId>,

    /// The data directory that was **kept**, or [`None`] when it was deleted or never existed.
    ///
    /// Answered rather than assumed, so a client can say where somebody's captured mail still is
    /// instead of leaving them to guess.
    pub data_dir_kept: Option<PathBuf>,

    /// The domain released with it, for a `web-app` — roadmap task **T81b**.
    pub site: Option<String>,

    /// The php-fpm pool that went with it, for a `web-app` — roadmap task **T82a**.
    ///
    /// Answered so a client can say the process went too, rather than leaving a `mix service list`
    /// entry for somebody to find and wonder about.
    pub pool: Option<ServiceId>,
}

/// The service an extension runs as, or [`None`] for a kind that runs nothing.
#[must_use]
pub fn service_of(installed: &Installed) -> Option<ServiceId> {
    matches!(installed.kind(), ExtensionKind::Service).then(|| installed.id.service_id().clone())
}

/// Remove an installed extension.
///
/// # Errors
///
/// [`Error::NotFound`] when nothing is installed under `id`; [`Error::Database`] when a row cannot
/// be deleted — including the `RESTRICT` a running service holds; and [`Error::Io`] when a directory
/// cannot be removed.
pub async fn uninstall(
    store: &Store,
    paths: &Paths,
    id: &ExtensionId,
    delete_data: bool,
) -> Result<Removed> {
    let installed = extension_store::get(store, id)
        .await?
        .ok_or_else(|| Error::NotFound {
            kind: "extension",
            id: id.as_str().to_owned(),
        })?;

    let service = service_of(&installed);

    if let Some(service) = &service {
        // `delete` answers `None` for a row that is not there, which is what makes this resumable.
        crate::services::delete(store, service).await?;
        remove(&paths.etc().join(service.as_str())).await?;
        remove(&paths.service_logs(service)).await?;
    }

    // **The site goes before the row that owns it** — roadmap task **T81b**, the design's D8. The
    // cascade would take it anyway; deleting it here is what lets the answer name the domain, and
    // `of_extension` answering `None` is what makes a second run after an interruption succeed.
    let site = match crate::sites::of_extension(store, id).await? {
        Some(site) => {
            crate::sites::delete(store, site.id).await?;
            site.domains.first().cloned()
        }
        None => None,
    };

    // **After the site and before the row** — roadmap task **T82a**, that design's D11.
    // `sites.php_service_id` is `ON DELETE SET NULL`, so removing the pool first would leave the
    // site pointing at nothing for one statement, and an interruption there would leave it for
    // good. `pools::remove` answers [`None`] for a pool that is already gone, which is what lets a
    // second run after an interruption finish rather than refuse.
    //
    // **Nothing here stops it.** Supervision is the daemon's, which stops the pool before calling
    // this — the same order the module note above states for a `service` extension's process.
    let pool = crate::extensions::pools::remove(store, paths, id).await?;

    extension_store::forget(store, id).await?;
    remove(&installed.install_dir).await?;

    let data_dir_kept = match delete_data {
        true => {
            remove(&installed.data_dir).await?;
            None
        }
        false => installed.data_dir.exists().then_some(installed.data_dir),
    };

    tracing::info!(
        extension = %id,
        kept = data_dir_kept.is_some(),
        released = site.as_deref().unwrap_or("nothing"),
        pool = pool.as_ref().map(ServiceId::as_str).unwrap_or("none"),
        "an extension was uninstalled"
    );

    Ok(Removed {
        id: id.clone(),
        service,
        data_dir_kept,
        site,
        pool,
    })
}

/// Remove a directory that may not be there.
///
/// [`crate::paths::remove_dir`] since roadmap task **T82a**, which is where the second caller is:
/// two copies of "not found is fine" is one copy that eventually is not.
async fn remove(path: &Path) -> Result<()> {
    crate::paths::remove_dir(path).await
}

#[cfg(test)]
mod tests {
    use mixengine_proto::Timestamp;

    use super::*;
    use crate::extensions::install::{self, Request};
    use crate::extensions::manifest;
    use crate::extensions::store::Source;
    use crate::install::Watcher;

    struct Quiet;

    impl Watcher for Quiet {
        async fn report(&self, _percent: u8, _message: &str) {}

        fn is_cancelled(&self) -> bool {
            false
        }
    }

    /// A home with one installed extension, put there the way a real install puts it.
    async fn installed(
        text: &str,
    ) -> (
        tempfile::TempDir,
        tempfile::TempDir,
        Paths,
        Store,
        Installed,
    ) {
        let home = tempfile::tempdir().expect("a temporary directory");
        let paths = Paths::new(
            home.path().to_path_buf(),
            &crate::config::PathOverrides::default(),
        );
        let store = Store::open(paths.database_file())
            .await
            .expect("a database");

        let source = tempfile::tempdir().expect("a source directory");
        std::fs::write(source.path().join("extension.toml"), text).expect("the manifest");
        std::fs::write(source.path().join("mailpit"), b"#!/bin/true\n").expect("the program");

        let manifest =
            manifest::read(std::path::Path::new("extension.toml"), text).expect("a fixture parses");

        let installed = install::install(
            &store,
            &paths,
            &mixengine_platform::mock::Host::with_home("/mixengine"),
            Request {
                manifest: &manifest,
                source: Source::Path,
                from: Some(source.path()),
                at: Timestamp::parse_rfc3339("2026-09-02T09:00:00Z").expect("a timestamp"),
            },
            &Quiet,
        )
        .await
        .expect("the install");

        (home, source, paths, store, installed)
    }

    /// **Everything goes except the data** — the design's D12.
    #[tokio::test]
    async fn uninstalling_keeps_the_data_directory() {
        let (_home, _source, paths, store, one) =
            installed(mixengine_testkit::extension::MAILPIT).await;
        std::fs::write(one.data_dir.join("captured.db"), b"mail").expect("something worth keeping");

        let removed = uninstall(&store, &paths, &one.id, false)
            .await
            .expect("the uninstall");

        assert_eq!(
            removed.service.map(|id| id.as_str().to_owned()),
            Some("mailpit".to_owned())
        );
        assert_eq!(
            removed.data_dir_kept.as_deref(),
            Some(one.data_dir.as_path())
        );
        assert_eq!(
            removed.site, None,
            "a service extension has no site to release"
        );
        assert!(
            one.data_dir.join("captured.db").is_file(),
            "the data was deleted"
        );
        assert!(!one.install_dir.exists(), "the install directory survived");

        assert!(
            extension_store::get(&store, &one.id)
                .await
                .expect("a read")
                .is_none()
        );

        let held: i64 = sqlx::query_scalar("SELECT count(*) FROM extension_ports")
            .fetch_one(store.pool())
            .await
            .expect("a count");
        assert_eq!(held, 0, "a port outlived the extension holding it");

        let services: i64 = sqlx::query_scalar("SELECT count(*) FROM services")
            .fetch_one(store.pool())
            .await
            .expect("a count");
        assert_eq!(services, 0);
    }

    /// And goes too when that is what was asked for.
    #[tokio::test]
    async fn uninstalling_deletes_the_data_when_told_to() {
        let (_home, _source, paths, store, one) =
            installed(mixengine_testkit::extension::MAILPIT).await;

        let removed = uninstall(&store, &paths, &one.id, true)
            .await
            .expect("the uninstall");

        assert_eq!(removed.data_dir_kept, None);
        assert!(!one.data_dir.exists());
    }

    /// The ports it held are free again, which is the point of releasing them.
    #[tokio::test]
    async fn the_ports_come_back() {
        let (_home, _source, paths, store, one) =
            installed(mixengine_testkit::extension::MAILPIT).await;
        let held = *one.ports.get("ui_port").expect("a port");

        uninstall(&store, &paths, &one.id, false)
            .await
            .expect("the uninstall");

        let allocation = crate::services::ports::allocate(
            &store,
            &mixengine_platform::mock::Host::with_home("/mixengine"),
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            held,
        )
        .await
        .expect("an allocation");

        assert_eq!(
            allocation.port, held,
            "the released port was still counted as held"
        );
    }

    /// Uninstalling something that is not installed says so rather than half-doing it.
    #[tokio::test]
    async fn uninstalling_nothing_is_a_not_found() {
        let (_home, _source, paths, store, one) =
            installed(mixengine_testkit::extension::MAILPIT).await;
        uninstall(&store, &paths, &one.id, false)
            .await
            .expect("the first");

        let refusal = uninstall(&store, &paths, &one.id, false)
            .await
            .expect_err("the second");

        assert!(
            matches!(
                refusal,
                Error::NotFound {
                    kind: "extension",
                    ..
                }
            ),
            "{refusal}"
        );
    }
}
