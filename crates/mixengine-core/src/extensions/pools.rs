//! The php-fpm pool a `web-app` extension's site runs on — roadmap task **T82a**.
//!
//! # Why a pool of its own, for every `web-app` and not only the ones that ask for a password
//!
//! The design's D1. A manifest field must not decide whether an extension gets a process of its
//! own: one that grew `signs_in` in a later release would, on the next install, quietly stop
//! sharing a pool and start owning one — a change to what runs on the machine, arriving from a
//! document rather than from a decision.
//!
//! And the isolation belongs to the *kind*. A `web-app` is an administrative interface onto this
//! machine's own databases, served on a runtime MixEngine picks rather than the user's; today it
//! competes for the five workers of the `[www]` pool with every site somebody is actually
//! developing. That is true whether or not a credential is involved.
//!
//! What it costs is one php-fpm master per installed `web-app`, and that is bounded by machinery
//! that already exists: a pool is idle-stopped after half an hour ([`Recipe::idle_default`], T70's
//! D9) and woken by the request that needs it.
//!
//! # The id is derived and the row is confirmed
//!
//! [`id`] composes `php-fpm@<extension-id>`, which is the rule a `service` extension's process
//! already follows one step along — there the process *is* `<id>`. [`of`] reads the row back before
//! anything names it, because T81b paid once already for the difference between formatting an id
//! and having one.
//!
//! # And it is not a pool anybody else may name
//!
//! [`crate::sites`] refuses it to a site that is not this extension's — the design's D5 — because a
//! project's PHP inside this process would be a project's PHP able to read a database superuser's
//! password.
//!
//! [`Recipe::idle_default`]: crate::generate::recipe::Recipe::idle_default

use std::collections::BTreeMap;

use mixengine_proto::{ExtensionId, PackageVersion, RuntimeKind, ServiceId, SiteKind};

use crate::generate::recipe::Recipe as _;
use crate::services::{Declaration, Origin, Port};
use crate::sites::SiteOwner;
use crate::{Error, Paths, Result, Store};

/// The variable a `web-app` declaring `signs_in` finds the database superuser's password in — the
/// design's D2.
///
/// **A constant and not a manifest field.** T80's D2 made an address unwritable so that there would
/// be no check to forget; the same move here means an author cannot collide with `PATH`, with
/// `PHP_INI_SCAN_DIR`, or with the two variables a Windows pool is configured through. A manifest
/// reaches it through `{db_password_env}` rather than by spelling it, so the name lives in one
/// place in one repository and a manifest published from the other keeps working the day it
/// changes.
pub const CREDENTIAL_ENV: &str = "MIXENGINE_DB_PASSWORD";

/// The `packages.name` half of a pool's id, which is also the recipe it is found under.
const POOL: &str = crate::generate::recipes::php_fpm::PACKAGE;

/// What this extension's pool is called.
///
/// # Errors
///
/// [`Error::ExtensionField`] naming `extension.id` when the composed id is longer than
/// [`ServiceId::MAX_LEN`] — an extension id of 57 characters or more, whose pool nothing could
/// name. Refused here, which is before anything is fetched, rather than at the insert.
pub fn id(extension: &ExtensionId) -> Result<ServiceId> {
    ServiceId::parse(format!("{POOL}@{extension}")).map_err(|_| Error::ExtensionField {
        id: extension.as_str().to_owned(),
        field: "extension.id".to_owned(),
        reason: format!(
            "is too long for a web-app: its pool is `{POOL}@<id>`, and a service id is at most {} \
             characters",
            ServiceId::MAX_LEN
        ),
    })
}

/// The pool row this extension owns, or [`None`] when there is not one.
///
/// **Read back rather than formatted**, which is [`id`]'s own note said as a call: a site naming a
/// service the front end cannot find is a site that is silently not served.
///
/// # Errors
///
/// [`Error::ExtensionField`] for an id with no possible pool, and [`Error::Database`] when the
/// table cannot be read.
pub async fn of(store: &Store, extension: &ExtensionId) -> Result<Option<ServiceId>> {
    let wanted = id(extension)?;
    let column = wanted.as_str();

    let found = sqlx::query_scalar!("SELECT id FROM services WHERE id = ?", column)
        .fetch_optional(store.pool())
        .await
        .map_err(|source| store.failure("read", source))?;

    Ok(found.map(|_| wanted))
}

/// Write it, on the installed PHP the site was resolved against.
///
/// **`Origin::Runtime` and not `Origin::Extension`**, and the choice is the schema's: the process is
/// `php-fpm` out of an installed PHP, so `runtime_installs`' `ON DELETE RESTRICT` is what stops that
/// PHP being removed out from under it — which an `extension_id` parent would not do. What ties the
/// row to the extension is [`id`], the same way a `service` extension's process is tied to it by
/// being called after it.
///
/// The port is the recipe's wish on the systems where a pool listens on TCP and nothing on the ones
/// where it listens on a socket — [`crate::services::pools::ensure`]'s rule, read here because the
/// same decision has to be made for the same reason: a port is *allocated* when the row is written
/// rather than derived at every start.
///
/// **The caller must not be holding the port allocator's lock**: [`crate::services::create`] takes
/// it, and it is not reentrant — which T81 found by writing a daemon that stopped answering rather
/// than one that failed.
///
/// # Errors
///
/// Whatever [`crate::services::create`] reports — including [`Error::NotFound`] for a runtime that
/// is not installed and [`Error::ServiceAlreadyDeclared`] for a pool that is already there.
pub async fn create(
    store: &Store,
    host: &dyn mixengine_platform::Host,
    extension: &ExtensionId,
    version: &PackageVersion,
) -> Result<ServiceId> {
    let service = id(extension)?;

    // `cfg!` as a *value*, so both arms compile everywhere — `recipes::php_fpm::listens_on_tcp`'s
    // rule, and the same expression `services::pools::ensure` reads. The number itself is the
    // recipe's wish rather than one written here, for that function's reason: what a service would
    // like is a fact about the service.
    let port = match cfg!(unix) {
        true => Port::None,
        false => match crate::generate::recipes::PhpFpm.preferred_port() {
            Some(preferred) => Port::Allocate { preferred },
            None => Port::None,
        },
    };

    crate::services::create(
        store,
        host,
        &Declaration {
            service: service.clone(),
            origin: Origin::Runtime {
                kind: RuntimeKind::Php,
                version: version.clone(),
            },
            instance_name: extension.as_str().to_owned(),
            port,
            bind_addr: None,
            data_dir: None,
            // **Not on by default**, for the reason a shared pool is not: the request that needs it
            // starts it, through the activator T70 built, and a machine that boots with two
            // administrative interfaces running is a machine spending memory on nobody.
            autostart: false,
            overrides: "{}".to_owned(),
        },
    )
    .await?;

    Ok(service)
}

/// Take it away with the extension, and answer what went.
///
/// Tolerant of its own predecessor having run, on [`crate::extensions::uninstall`]'s rule: an
/// uninstall interrupted by a daemon restart has to be able to finish rather than refuse.
///
/// **Nothing here stops a process.** Supervision belongs to the daemon, which stops the pool before
/// calling this — the design's D11, and the order that module's note already states for a `service`
/// extension.
///
/// # Errors
///
/// [`Error::Database`] when the row cannot be removed, and [`Error::Io`] when a generated directory
/// cannot be.
pub async fn remove(
    store: &Store,
    paths: &Paths,
    extension: &ExtensionId,
) -> Result<Option<ServiceId>> {
    let Some(service) = of(store, extension).await? else {
        return Ok(None);
    };

    crate::services::delete(store, &service).await?;
    crate::paths::remove_dir(&paths.etc().join(service.as_str())).await?;
    crate::paths::remove_dir(&paths.service_logs(&service)).await?;

    Ok(Some(service))
}

/// Give every `web-app` extension's site a pool of its own, and say which were made.
///
/// **Idempotent, and run at boot as well as after an install** — the design's D10, which is
/// [`crate::services::pools::ensure`]'s note at a second site and for its reasons. It is what gives
/// a `web-app` installed before this task a pool of its own with no data migration, and what
/// repairs a home whose pool row somebody deleted by hand.
///
/// **The runtime comes from the pool the site already names**, which is the honest source: T81b
/// froze a PHP into that row at install, and re-resolving `[web-app.runtime].requires` here would
/// silently move an application onto a newer PHP because a daemon restarted. A site naming no pool
/// at all — the state a forced `runtime.uninstall` leaves — is skipped with a line naming what would
/// fix it, because there is nothing to read a runtime out of.
///
/// **A no-op on a home that is already right**, which is what lets it run at boot: one query on a
/// home with no extension sites at all.
///
/// # Errors
///
/// [`Error::Database`] when the tables cannot be read or written, and whatever [`create`] reports
/// for a row that cannot be written.
pub async fn ensure(store: &Store, host: &dyn mixengine_platform::Host) -> Result<Vec<ServiceId>> {
    let mut made = Vec::new();

    for site in crate::sites::records(store, None).await? {
        let SiteOwner::Extension(owner) = &site.owner else {
            continue;
        };

        let SiteKind::PhpFpm { pool: named } = &site.kind else {
            continue;
        };

        let wanted = id(owner)?;

        if named.as_ref() == Some(&wanted) {
            continue;
        }

        let Some(named) = named else {
            tracing::warn!(
                extension = %owner,
                "this extension's site names no pool, so there is no runtime to give it one of its \
                 own out of; `mix extension uninstall` and installing it again is the way back"
            );
            continue;
        };

        let column = named.as_str();
        let version = sqlx::query_scalar!(
            "SELECT r.version
             FROM services s
             JOIN runtime_installs r ON r.id = s.runtime_install_id
             WHERE s.id = ?",
            column
        )
        .fetch_optional(store.pool())
        .await
        .map_err(|source| store.failure("read", source))?;

        let Some(version) = version.and_then(|value| PackageVersion::parse(&value).ok()) else {
            tracing::warn!(
                extension = %owner,
                pool = %named,
                "this extension's site names a pool that runs out of no installed runtime, so it \
                 was left where it is"
            );
            continue;
        };

        if of(store, owner).await?.is_none() {
            create(store, host, owner, &version).await?;
        }

        crate::sites::update(
            store,
            site.id,
            &crate::sites::Change {
                kind: Some(SiteKind::PhpFpm {
                    pool: Some(wanted.clone()),
                }),
                ..crate::sites::Change::default()
            },
        )
        .await?;

        made.push(wanted);
    }

    if !made.is_empty() {
        tracing::info!(pools = ?made, "web-app extensions were given pools of their own");
    }

    Ok(made)
}

/// A credential a service's processes are handed at spawn — the design's D4.
///
/// **An address and never a value.** `mixengine-core` has no business reaching a credential store
/// (`generate::databases`' D1); what travels here is what the supervisor will look up, so a
/// [`Context`](crate::generate::recipe::Context) holding one may be `Debug`-printed exactly as it
/// always could.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    /// The variable it arrives in — [`CREDENTIAL_ENV`].
    pub env: String,

    /// The keyring service name: [`mixengine_platform::KEYRING_SERVICE`].
    pub keyring_service: String,

    /// The entry within it — `<service-id>/<administrator>`, which is what
    /// [`Context::secret_address`](crate::generate::recipe::Context::secret_address) composes for
    /// the database's own recipe and what `services::databases` wrote it under.
    pub keyring_key: String,

    /// The service that credential opens.
    ///
    /// Carried so the pool can declare an edge to it (the design's D7): a pool started before the
    /// database has ever run finds no entry at all, because the entry is written by that database's
    /// first run.
    pub database: ServiceId,
}

/// Which pools in this home carry a credential, by pool.
///
/// **Resolved once per generation walk, off the link and never off `engines`.** The link is the
/// `site_service_links` row T82 froze at install; re-resolving the preference order at render time
/// would re-point an application at a different server than the one it was installed against, which
/// is why the pool is frozen too (T81b's D5).
///
/// **It fails closed.** A pool a site that is not its extension's also names carries nothing, and
/// says so: that state is unreachable through [`crate::sites`]' refusal, and these few lines are the
/// difference between a bug there and a disclosure here.
///
/// # Errors
///
/// [`Error::Database`] when the tables cannot be read, and whatever reading a `sites` row reports.
pub async fn credentials(store: &Store) -> Result<BTreeMap<ServiceId, Credential>> {
    use crate::extensions::manifest::Body;

    let installed: BTreeMap<String, crate::extensions::store::Installed> =
        crate::extensions::store::all(store)
            .await?
            .into_iter()
            .map(|one| (one.id.as_str().to_owned(), one))
            .collect();

    let mut carried: BTreeMap<ServiceId, Credential> = BTreeMap::new();
    let mut shared: Vec<ServiceId> = Vec::new();

    for site in crate::sites::records(store, None).await? {
        let SiteKind::PhpFpm { pool: Some(pool) } = &site.kind else {
            continue;
        };

        let SiteOwner::Extension(owner) = &site.owner else {
            // A site that is not an extension's, on a pool something else is carrying a credential
            // for, is the state `sites` refuses. Noted rather than trusted: what it costs is the
            // credential, not the site.
            shared.push(pool.clone());
            continue;
        };

        let Some(one) = installed.get(owner.as_str()) else {
            continue;
        };

        let Body::WebApp(app) = &one.manifest.body else {
            continue;
        };

        if !app
            .database
            .as_ref()
            .is_some_and(|database| database.signs_in)
        {
            continue;
        }

        // The first linked service that *is* a database. A link to something else — a cache a
        // manifest declared — is not an error; it is a link to something else.
        for service in &site.services {
            if let Some(endpoint) = crate::extensions::database::endpoint(store, service).await? {
                carried.insert(
                    pool.clone(),
                    Credential {
                        env: CREDENTIAL_ENV.to_owned(),
                        keyring_service: mixengine_platform::KEYRING_SERVICE.to_owned(),
                        keyring_key: format!("{}/{}", endpoint.service, endpoint.user),
                        database: endpoint.service,
                    },
                );
                break;
            }
        }
    }

    for pool in shared {
        if carried.remove(&pool).is_some() {
            tracing::warn!(
                %pool,
                "a site that is not this pool's extension names it, so its credential is not being \
                 passed; `mix site update` moves that site to the shared pool"
            );
        }
    }

    Ok(carried)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule, spelled once and asserted once.
    #[test]
    fn a_pool_is_named_after_the_extension_that_owns_it() {
        let extension = ExtensionId::parse("phpmyadmin").expect("an id");

        assert_eq!(
            id(&extension).expect("a pool id").as_str(),
            "php-fpm@phpmyadmin"
        );
    }

    /// An id too long for a service id is refused here, which is before anything is fetched.
    #[test]
    fn an_extension_whose_pool_cannot_be_named_is_refused_by_name() {
        let extension = ExtensionId::parse("a".repeat(60)).expect("a long but legal id");

        let refusal = id(&extension).expect_err("its pool id is over the limit");

        let said = refusal.to_string();
        assert!(said.contains("extension.id"), "{said}");
        assert!(said.contains("64"), "{said}");
    }

    /// Nothing is answered for an extension with no pool, and the *row* is what decides — not the
    /// rule that composes the name.
    #[tokio::test]
    async fn a_pool_is_answered_from_the_row_and_not_from_the_rule() {
        let home = tempfile::tempdir().expect("a temporary home");
        let store = Store::open(&home.path().join("mixengine.db"))
            .await
            .expect("a store");
        let extension = ExtensionId::parse("phpmyadmin").expect("an id");

        assert_eq!(of(&store, &extension).await.expect("a lookup"), None);
    }
}
