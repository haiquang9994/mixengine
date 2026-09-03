//! `extension.*` — roadmap tasks **T80** and **T81**.
//!
//! **Not [`crate::php_extensions`]**, which turns a *PHP* extension on for one installed runtime.
//! These are MixEngine's own: Mailpit, phpMyAdmin, MixDB.
//!
//! A façade and nothing more — every decision here is `mixengine_core::extensions`', because
//! `CLAUDE.md` puts no business logic in a client and the daemon is the client's server rather than
//! a second place for rules. What belongs *here* is what only the daemon has: the home's paths, its
//! store, the registry client and the job runner.
//!
//! **Consent is checked here rather than trusted.** A client sends the plan it showed somebody
//! ([`ExtensionConsent`]), and this compares it against the manifest it is about to install — the
//! shape `[scaffold]` consent already has, and for its reason: the registry can be refreshed between
//! the reading and the sending, and a consent naming what was shown is the only kind that cannot be
//! spent on something else.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mixengine_core::extensions::manifest::ExtensionManifest;
use mixengine_core::extensions::registry::Registry;
use mixengine_core::extensions::store::Source;
use mixengine_core::extensions::{install, manifest, store as extension_store, uninstall};
use mixengine_core::index::Client;
use mixengine_core::{Paths, Store};
use mixengine_proto::{
    Error, ErrorCode, ExtensionCatalogue, ExtensionConsent, ExtensionId, ExtensionInspect,
    ExtensionInspection, ExtensionInstall, ExtensionOffer, ExtensionOrigin, ExtensionPlan,
    ExtensionPlanRequest, ExtensionRemoval, ExtensionSummary, ExtensionUninstall, JobKind,
    JobSummary, PortWish, ServiceId, Timestamp, rpc,
};

use crate::error::ToWire as _;

/// Where an extension's own credentials live inside the keyring's `mixengine` namespace.
///
/// `extensions/<id>/…`, beside the `<service-id>/<key>` a recipe's credential takes
/// (`generate::recipe::Context::secret_address`) — one namespace, two shapes that cannot collide
/// because a service id has no `/` in it.
const EXTENSION_SECRET_PREFIX: &str = "extensions/";

/// What `{secret}` is stored under, for the one extension-owned secret there is.
const CONFIG_SECRET_KEY: &str = "config";

/// How many characters it has — [`generate::databases::SECRET_LENGTH`]'s number, for its reason.
///
/// [`generate::databases::SECRET_LENGTH`]: mixengine_core::generate::databases::SECRET_LENGTH
const CONFIG_SECRET_LENGTH: usize = 32;

/// Everything `extension.*` needs.
#[derive(Debug)]
pub(crate) struct Extensions {
    /// The home, for the directories an install uses.
    paths: Paths,

    /// The rows.
    store: Store,

    /// The signed registry, cached under the home's cache directory.
    registry: Client<Registry>,

    /// What runs an install, which is long enough to be a job.
    jobs: Arc<crate::jobs::Jobs>,

    /// This system, for the port allocator's bind probe.
    host: Arc<dyn mixengine_platform::Host>,

    /// The sites, for what a `web-app` install does after its row — roadmap task **T81b**, the
    /// design's D7. Held rather than reached for, the reason `Domains` holds it: every path that
    /// writes a site has to ask for the hosts file, issue the certificate and regenerate, and a
    /// mechanism the caller has to remember is one the caller eventually forgets.
    sites: Arc<crate::sites::Sites>,

    /// The supervisor, for the one thing a `web-app`'s uninstall now has to do first — roadmap task
    /// **T82a**, its design's D11.
    ///
    /// `mixengine_core::extensions::uninstall`'s note says supervision belongs to the daemon and the
    /// order it walks is stop-then-this; until that task a `web-app` had no process to stop.
    services: Arc<crate::services::Registry>,
}

impl Extensions {
    /// Build it.
    ///
    /// **The registry client is built by the caller** — `main.rs`, beside the `Fetcher`, so that a
    /// compiled-in key that is not a key fails the start rather than the first install — and this
    /// is built after `Sites`, which it holds (roadmap task **T81b**).
    pub(crate) fn new(
        paths: Paths,
        store: Store,
        jobs: Arc<crate::jobs::Jobs>,
        host: Arc<dyn mixengine_platform::Host>,
        registry: Client<Registry>,
        sites: Arc<crate::sites::Sites>,
        services: Arc<crate::services::Registry>,
    ) -> Self {
        Self {
            paths,
            store,
            registry,
            jobs,
            host,
            sites,
            services,
        }
    }

    /// Read a manifest and say what installing it here would produce.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::InvalidArgument`] for a path that is not absolute, and whatever
    /// [`mixengine_core::extensions::inspect`] raises about the file itself.
    pub(crate) fn inspect(&self, asked: &ExtensionInspect) -> Result<ExtensionInspection, Error> {
        let path = absolute(&asked.path)?;

        mixengine_core::extensions::inspect(&self.paths, &path).map_err(|error| error.to_wire())
    }

    /// What this home has installed.
    ///
    /// # Errors
    ///
    /// Whatever reading the tables costs.
    pub(crate) async fn list(&self) -> Result<mixengine_proto::InstalledExtensions, Error> {
        let installed = extension_store::all(&self.store)
            .await
            .map_err(|error| error.to_wire())?;

        // One read for the whole listing: the domain of every extension-owned site, by extension —
        // roadmap task **T81b**.
        let served: std::collections::BTreeMap<ExtensionId, String> =
            mixengine_core::sites::records(&self.store, None)
                .await
                .map_err(|error| error.to_wire())?
                .into_iter()
                .filter_map(|site| match site.owner {
                    mixengine_core::sites::SiteOwner::Extension(id) => {
                        Some((id, site.domains.first().cloned().unwrap_or_default()))
                    }
                    mixengine_core::sites::SiteOwner::Project(_) => None,
                })
                .collect();

        Ok(mixengine_proto::InstalledExtensions {
            extensions: installed
                .iter()
                .map(|one| summary(one, served.get(&one.id).cloned()))
                .collect(),
        })
    }

    /// What the signed registry publishes.
    ///
    /// # Errors
    ///
    /// Whatever obtaining the registry costs when there is no usable cache either — a signature
    /// that does not verify, a document from before the cached one, a server that cannot be
    /// reached.
    pub(crate) async fn available(&self) -> Result<ExtensionCatalogue, Error> {
        let catalogue = self
            .registry
            .catalogue()
            .await
            .map_err(|error| error.to_wire())?;
        let listing = catalogue.index.listing();

        let installed = extension_store::all(&self.store)
            .await
            .map_err(|error| error.to_wire())?;

        let extensions = listing
            .extensions
            .iter()
            .map(|manifest| ExtensionOffer {
                id: manifest.extension.id.clone(),
                name: manifest.extension.name.clone(),
                version: manifest.extension.version.clone(),
                kind: manifest.extension.kind,
                description: manifest.extension.description.clone(),
                installed: installed.iter().any(|one| one.id == manifest.extension.id),
                artifact: mixengine_core::extensions::availability(manifest),
            })
            .collect();

        Ok(ExtensionCatalogue {
            extensions,
            unreadable: listing.unreadable,
            stale: catalogue.freshness.is_stale(),
        })
    }

    /// What installing something would do here.
    ///
    /// # Errors
    ///
    /// Whatever reading the manifest reports, and [`ErrorCode::NotFound`] for an id the registry
    /// does not list.
    pub(crate) async fn plan(&self, asked: &ExtensionPlanRequest) -> Result<ExtensionPlan, Error> {
        let (manifest, signed) = self.manifest(&asked.source).await?;

        let plan = install::plan(&self.store, &self.paths, &manifest, signed)
            .await
            .map_err(|error| error.to_wire())?;

        Ok(ExtensionPlan {
            id: plan.id,
            name: plan.name,
            version: plan.version,
            kind: plan.kind,
            description: plan.description,
            signed: plan.signed,
            permissions: plan.permissions,
            ports: plan.ports,
            install_dir: plan.install_dir.display().to_string(),
            data_dir: plan.data_dir.display().to_string(),
            site: plan.site.map(|site| mixengine_proto::PlannedSite {
                domain: site.domain,
                pool: site.pool,
                database: site.database,
                signs_in: site.signs_in,
            }),
        })
    }

    /// Install one, as a job.
    ///
    /// # Errors
    ///
    /// Everything [`plan`](Self::plan) reports, [`ErrorCode::PreconditionFailed`] when the consent
    /// does not describe what is about to be installed, and whatever starting a job costs.
    pub(crate) async fn install(
        self: &Arc<Self>,
        asked: &ExtensionInstall,
    ) -> Result<JobSummary, Error> {
        let (manifest, signed) = self.manifest(&asked.source).await?;

        agrees(&asked.consent, &manifest, signed)?;

        // Refused before the job exists, so a client that asked for something impossible is told
        // now rather than handed a job id that fails a second later.
        install::plan(&self.store, &self.paths, &manifest, signed)
            .await
            .map_err(|error| error.to_wire())?;

        let kind = JobKind::parse(rpc::method::EXTENSION_INSTALL)
            .expect("`extension.install` is a method name, which is what a job kind is");

        let extensions = Arc::clone(self);
        let source = asked.source.clone();

        self.jobs
            .begin(&kind, move |handle| async move {
                extensions
                    .perform(&source, &manifest, signed, &handle)
                    .await
            })
            .await
    }

    /// The work behind an install.
    async fn perform(
        &self,
        source: &ExtensionOrigin,
        manifest: &ExtensionManifest,
        signed: bool,
        handle: &crate::jobs::JobHandle,
    ) -> Result<serde_json::Value, Error> {
        let id = manifest.extension.id.as_str();
        tracing::info!(job = %handle.id(), extension = %id, "installing an extension");

        handle.progress(0, "reading the manifest").await;

        let from = match source {
            ExtensionOrigin::Path { path } => Some(absolute(path)?),
            ExtensionOrigin::Registry { .. } => None,
        };

        let source = match signed {
            true => Source::Registry,
            false => Source::Path,
        };
        let at = Timestamp::from_system_time(std::time::SystemTime::now());

        // **The front end judges the fragment before a byte is fetched** — roadmap task **T81c**,
        // the design's D5. A `[[recipe.front_end]]` this home's front end will not parse is refused
        // here, where nothing has been downloaded, unpacked or written; running it afterwards would
        // leave the choice between a wedged front end and an uninstall in an error path.
        self.would_be_served(manifest, source, at).await?;

        let installed = install::install(
            &self.store,
            &self.paths,
            self.host.as_ref(),
            install::Request {
                manifest,
                source,
                from: from.as_deref(),
                at,
            },
            handle,
        )
        .await
        .map_err(|error| error.to_wire())?;

        // **What `site.create` does after its row**, for the site the install just wrote — roadmap
        // task **T81b**, the design's D7. A `service` needs none of this: `extension.start` walks
        // `service.start` and regenerates on the way. A site has nothing to walk.
        let site = mixengine_core::sites::of_extension(&self.store, &installed.id)
            .await
            .map_err(|error| error.to_wire())?;

        if let Some(site) = &site {
            handle.progress(88, "writing its configuration").await;
            self.configure_one(&installed).await?;

            handle.progress(90, "declaring the site").await;
            self.sites.now_declares(site).await?;
        } else if carries_a_fragment(manifest) {
            // **A `recipe` extension has neither a service to start nor a site to declare, and
            // still changed what the front end reads** — roadmap task **T81c**. Nothing else on
            // this path would regenerate, so the fragment would sit in the table until the next
            // thing that happened to render.
            handle.progress(90, "regenerating the front end").await;
            self.sites.now_serves_what_it_declares().await?;
        }

        // **And every managed PHP's `conf.d`, if this extension added to it** — roadmap task
        // **T82**, found by installing the real Mailpit.
        //
        // T81 wired `[recipe] php_ini` and left it written by `refresh_all` at boot and by a runtime
        // install, neither of which happens here — so `sendmail_path` appeared only after the daemon
        // was restarted, and the acceptance criterion says *with no manual php.ini edit*, not *after
        // a restart*. This is T81c's own lesson arriving for the other half of `[recipe]`: nothing
        // else on this path would regenerate.
        if carries_php_ini(manifest) {
            handle.progress(94, "telling every managed PHP").await;
            self.refresh_ini_sets().await;
        }

        serde_json::to_value(summary(
            &installed,
            site.and_then(|site| site.domains.first().cloned()),
        ))
        .map_err(|source| {
            Error::new(
                ErrorCode::Internal,
                format!("an installed extension could not be described: {source}"),
            )
        })
    }

    /// Write every installed `web-app`'s generated configuration — roadmap task **T82**, the
    /// design's D2.
    ///
    /// **Called at boot as well as after an install**, which is what makes the file disposable
    /// rather than something written once and hoped about: a database that was re-provisioned, a
    /// port that moved and a MixEngine that changed its mind about the generated half all take
    /// effect on the next start, with no repair anybody has to know to run.
    ///
    /// **One extension's failure costs that extension and nothing else.** A boot that stopped at the
    /// first unwritable directory would leave every later `web-app` unconfigured for a reason
    /// belonging to a different one — the shape `mix extension available` already takes for an entry
    /// it cannot read.
    ///
    /// # Errors
    ///
    /// None: everything that goes wrong is logged against the extension it belongs to. The install
    /// path uses [`configure_one`](Self::configure_one), which does report.
    pub(crate) async fn configure(&self) {
        let installed = match extension_store::all(&self.store).await {
            Ok(installed) => installed,
            Err(error) => {
                tracing::warn!(%error, "the installed extensions could not be read");
                return;
            }
        };

        for one in &installed {
            if let Err(error) = self.configure_one(one).await {
                tracing::warn!(
                    extension = %one.id,
                    error = %error.message,
                    "an extension's configuration could not be written"
                );
            }
        }
    }

    /// Give every `web-app` a pool of its own, and regenerate if that changed anything — roadmap
    /// task **T82a**, its design's D10.
    ///
    /// **Called at boot, before [`configure`](Self::configure)**, so the configuration written there
    /// belongs to the pool the site is actually served on. Idempotent, which is what lets it run at
    /// every boot: it is one query on a home with no extension sites.
    ///
    /// **Reported and never fatal**, on the rule `configure` follows: a `web-app` that could not be
    /// moved onto its own pool is one command away from being reinstalled, where refusing to start
    /// would leave the user with no daemon at all.
    pub(crate) async fn ensure_pools(&self) {
        let made =
            match mixengine_core::extensions::pools::ensure(&self.store, self.host.as_ref()).await {
                Ok(made) => made,
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "could not give every web-app extension a pool of its own"
                    );
                    return;
                }
            };

        if made.is_empty() {
            return;
        }

        // The site rows moved, so what the front end is serving names a pool that is no longer
        // theirs until this runs.
        if let Err(error) = self.sites.now_serves_what_it_declares().await {
            tracing::warn!(
                error = %error.message,
                "web-app extensions were moved onto pools of their own and the front end was not \
                 told; the next `mix site` call renders it"
            );
        }
    }

    /// Stop a `web-app`'s pool before its row is deleted — roadmap task **T82a**, its design's D11.
    ///
    /// **A refusal and not a best effort.** `services::delete` does not look at a process, so a row
    /// removed from under a live php-fpm would leave a master with no configuration and nothing that
    /// knows about it — which is exactly what `service.delete`'s own first refusal exists to
    /// prevent, said here for the caller that does not go through it.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::PreconditionFailed`] naming the pool and the command that stops it, when the
    /// stop did not take.
    async fn stop_pool(&self, id: &ExtensionId) -> Result<(), Error> {
        let Some(pool) = mixengine_core::extensions::pools::of(&self.store, id)
            .await
            .map_err(|error| error.to_wire())?
        else {
            return Ok(());
        };

        if let Ok(graph) = self.services.graph().await
            && let Ok(plan) = graph.stop_plan(std::slice::from_ref(&pool))
        {
            self.services.stop(&plan).await;
        }

        let record = mixengine_core::services::record(&self.store, &pool)
            .await
            .map_err(|error| error.to_wire())?;

        if self.services.supervised().contains(&pool)
            || !matches!(
                record.state,
                mixengine_proto::ServiceState::Stopped | mixengine_proto::ServiceState::Failed
            )
        {
            return Err(Error::new(
                ErrorCode::PreconditionFailed,
                format!("{pool} is {}", record.state.as_str()),
            )
            .with_hint(format!("`mix service stop {pool}` first")));
        }

        Ok(())
    }

    /// Rewrite every installed runtime's generated `conf.d` — roadmap task **T82**.
    ///
    /// The call `main.rs` makes at boot, made again where an extension changes what belongs in it.
    /// **Reported and never fatal**, on the rule that call follows: an install that landed and then
    /// could not write one ini file is not an install to undo, and the next start rewrites it.
    async fn refresh_ini_sets(&self) {
        match mixengine_core::runtimes::extensions::refresh_all(&self.store, &self.paths).await {
            Ok(moved) if moved.is_empty() => {
                tracing::debug!("every installed runtime's conf.d was already up to date");
            }
            Ok(moved) => tracing::info!(runtimes = ?moved, "rewrote the generated conf.d"),
            Err(error) => {
                tracing::warn!(%error, "could not rebuild every installed runtime's conf.d");
            }
        }
    }

    /// Write one, or say why it was skipped.
    ///
    /// **A `web-app` whose declared database is gone is skipped and its file left alone** — the
    /// design's D4. That state is reachable only through `mix service delete <db> --force`, a person
    /// overruling the refusal the link armed; rewriting the configuration to point nowhere would
    /// make a forced delete worse in silence, and reading the old value back out of the file would
    /// be parsing a generated file into state.
    async fn configure_one(&self, installed: &extension_store::Installed) -> Result<(), Error> {
        let secret = self.config_secret(&installed.id).await?;

        let rendered = mixengine_core::extensions::config::of(&self.store, installed, &secret)
            .await
            .map_err(|error| error.to_wire())?;

        let Some(rendered) = rendered else {
            if needs_a_database(installed) {
                tracing::warn!(
                    extension = %installed.id,
                    "its database is gone, so its configuration was left as it is; \
                     `mix service create` puts one back"
                );
            }

            return Ok(());
        };

        mixengine_core::extensions::config::write(&rendered).map_err(|error| error.to_wire())
    }

    /// The stable random value behind `{secret}`, created on first use — the design's D7.
    ///
    /// **It has to be stable**: phpMyAdmin's `blowfish_secret` is what its session cookie is signed
    /// with, so a value that changed on every render would log everybody out on every regeneration.
    /// It cannot be recovered by reading the last generated file either — *never parse a generated
    /// file back into state* is the rule the whole `etc/` layout rests on — so it lives where this
    /// system keeps a secret, and `mixengine-core` never sees the keyring
    /// (`generate::databases`' D1).
    ///
    /// # Errors
    ///
    /// [`ErrorCode::Internal`] when this machine has a credential store and it refused. A machine
    /// with **no** store answers an empty secret rather than failing: a `web-app` that uses no
    /// `{secret}` would otherwise be unconfigurable on a headless Linux for a value it never asked
    /// for, and one that *does* use it is refused by the renderer, naming the field.
    async fn config_secret(&self, id: &ExtensionId) -> Result<String, Error> {
        let host = Arc::clone(&self.host);
        let key = format!("{EXTENSION_SECRET_PREFIX}{id}/{CONFIG_SECRET_KEY}");

        // The keyring blocks, and on Linux it blocks on a D-Bus round trip to a daemon that may be
        // prompting somebody to unlock it. `.claude/standards/rust.md`'s rule for anything that can
        // hang.
        let read = {
            let (host, key) = (Arc::clone(&host), key.clone());
            tokio::task::spawn_blocking(move || {
                host.keyring()
                    .secret(mixengine_platform::KEYRING_SERVICE, &key)
            })
            .await
        };

        match read {
            Ok(Ok(Some(secret))) => return Ok(secret),
            Ok(Ok(None)) => {}
            // No credential store is not a failure to configure: see this function's own note.
            Ok(Err(reason)) => {
                tracing::debug!(%id, error = %reason, "no credential store answered");
                return Ok(String::new());
            }
            Err(_) => {
                return Err(Error::new(
                    ErrorCode::Internal,
                    format!("the task reading {id}'s configuration secret did not finish"),
                ));
            }
        }

        let made = mixengine_platform::generate_secret(CONFIG_SECRET_LENGTH).map_err(|reason| {
            Error::new(
                ErrorCode::Internal,
                format!("this machine would not produce a random value: {reason}"),
            )
        })?;

        let stored = {
            let (host, key, value) = (host, key, made.clone());
            tokio::task::spawn_blocking(move || {
                host.keyring()
                    .set_secret(mixengine_platform::KEYRING_SERVICE, &key, &value)
            })
            .await
        };

        match stored {
            Ok(Ok(())) => Ok(made),
            Ok(Err(reason)) => {
                tracing::debug!(%id, error = %reason, "no credential store took the secret");
                Ok(String::new())
            }
            Err(_) => Err(Error::new(
                ErrorCode::Internal,
                format!("the task storing {id}'s configuration secret did not finish"),
            )),
        }
    }

    /// Forget the `{secret}` of an extension that is gone.
    ///
    /// Otherwise a reinstall inherits a key from something that was removed, and the entry outlives
    /// everything it was for. Idempotent, and a machine with no credential store has nothing to
    /// remove — both of which are why nothing here is reported.
    async fn forget_config_secret(&self, id: &ExtensionId) {
        let host = Arc::clone(&self.host);
        let key = format!("{EXTENSION_SECRET_PREFIX}{id}/{CONFIG_SECRET_KEY}");

        let removed = tokio::task::spawn_blocking(move || {
            host.keyring()
                .forget_secret(mixengine_platform::KEYRING_SERVICE, &key)
        })
        .await;

        if let Ok(Err(reason)) = removed {
            tracing::debug!(%id, error = %reason, "an extension's secret could not be removed");
        }
    }

    /// Would this home's front end accept what installing this would give it? — roadmap task
    /// **T81c**, the design's D5.
    ///
    /// **Here rather than in `mixengine_core::extensions::install`**, and the reason is where a
    /// [`Generator`] can be built: it needs the recipe catalogue and this system's port mapping,
    /// both of which this daemon has already assembled in [`crate::services::spec::generator`].
    /// Threading one into a core function to check one field would move that assembly into core.
    ///
    /// The `Installed` handed over is the row this install *would* write, with the ports the
    /// manifest asked for rather than the ones it will hold — which is what a judgement before the
    /// allocation can see, and is argued in [`Generator::would_serve`].
    ///
    /// # Errors
    ///
    /// [`ErrorCode::PreconditionFailed`] carrying the front end's own complaint, with the
    /// extension named: which of the two documents was refused is the first thing the reader needs.
    ///
    /// [`Generator`]: mixengine_core::generate::Generator
    /// [`Generator::would_serve`]: mixengine_core::generate::Generator::would_serve
    async fn would_be_served(
        &self,
        manifest: &ExtensionManifest,
        source: Source,
        at: Timestamp,
    ) -> Result<(), Error> {
        if !carries_a_fragment(manifest) {
            return Ok(());
        }

        let id = manifest.extension.id.clone();
        let pending = extension_store::Installed {
            install_dir: extension_store::install_dir(&self.paths, &id),
            data_dir: extension_store::data_dir(&self.paths, &id),
            ports: manifest.ports.clone(),
            manifest: manifest.clone(),
            source,
            signed: matches!(source, Source::Registry),
            installed_at: at,
            id,
        };

        crate::services::spec::generator(&self.paths, &self.store, self.host.as_ref())
            .would_serve(&pending)
            .await
            .map_err(|error| {
                Error::new(
                    ErrorCode::PreconditionFailed,
                    format!(
                        "{} declares a front-end fragment this home's front end will not accept: {error}",
                        pending.id
                    ),
                )
                .with_hint("`mix extension inspect <path>` shows the fragment as it would be rendered")
            })
    }

    /// Remove one.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::NotFound`] when nothing is installed under that id, and whatever removing it
    /// costs — including the refusal a running service holds.
    pub(crate) async fn uninstall(
        &self,
        asked: &ExtensionUninstall,
    ) -> Result<ExtensionRemoval, Error> {
        // **Stop before removing** — roadmap task **T82a**, its design's D11, and the order
        // `mixengine_core::extensions::uninstall`'s own note states for a `service` extension's
        // process. A `web-app` is served rather than run and had nothing to stop until it was given
        // a pool of its own; a row deleted from under a live php-fpm would leave a master with no
        // configuration and nothing left that knows about it.
        self.stop_pool(&asked.id).await?;

        // **Read before the row is gone, and used after it is** — roadmap task **T81c**. Whether
        // this extension put anything in the front end's configuration is a fact about its manifest,
        // and the manifest lives in the row this is about to delete.
        let manifest = extension_store::get(&self.store, &asked.id)
            .await
            .map_err(|error| error.to_wire())?
            .map(|installed| installed.manifest);
        let had_a_fragment = manifest.as_ref().is_some_and(carries_a_fragment);
        // The same question for the other half of `[recipe]` — roadmap task **T82**. Read here for
        // the reason above: the manifest lives in the row this is about to delete.
        let had_php_ini = manifest.as_ref().is_some_and(carries_php_ini);

        // **The rows go first and the regeneration second, and that order is the escape hatch** —
        // roadmap task **T81c**, the design's D6. A fragment that was accepted at install can be
        // refused later — the front-end package is upgraded, or this home switches to the other
        // front end and its fragment was never judged — and in that state nothing regenerates. The
        // way out is this call, and it works only because by the time anything renders, the row
        // carrying the fragment is not in the table. Swapping these two lines would remove the only
        // exit from a home in that state; the suites in `tests/caddy.rs` and `tests/nginx.rs` are
        // what would notice.
        let removed = uninstall::uninstall(&self.store, &self.paths, &asked.id, asked.delete_data)
            .await
            .map_err(|error| error.to_wire())?;

        // **After the row, so a failed uninstall does not lose a secret it still needs** — roadmap
        // task **T82**, the design's D7. Idempotent, so an uninstall of something that never had
        // one costs a keyring call and no error.
        self.forget_config_secret(&asked.id).await;

        // What `site.delete` does after its row — roadmap task **T81b**, the design's D8.
        if removed.site.is_some() {
            self.sites.no_longer_declares().await?;
        } else if had_a_fragment {
            self.sites.now_serves_what_it_declares().await?;
        }

        // And the line it added to every managed PHP goes with it — roadmap task **T82**. After the
        // row, so what is rewritten is a home this extension is no longer in.
        if had_php_ini {
            self.refresh_ini_sets().await;
        }

        Ok(ExtensionRemoval {
            id: removed.id,
            service: removed.service,
            data_dir_kept: removed.data_dir_kept.map(|path| path.display().to_string()),
            site: removed.site,
            pool: removed.pool,
        })
    }

    /// The service an extension runs as.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::NotFound`] when nothing is installed under that id, and
    /// [`ErrorCode::PreconditionFailed`] for a kind that runs no process — which is an answer about
    /// the extension rather than a failure of the call.
    pub(crate) async fn service_of(&self, id: &ExtensionId) -> Result<ServiceId, Error> {
        let installed = extension_store::get(&self.store, id)
            .await
            .map_err(|error| error.to_wire())?
            .ok_or_else(|| {
                Error::new(ErrorCode::NotFound, format!("{id} is not installed"))
                    .with_hint("`mix extension list` says what is")
            })?;

        match uninstall::service_of(&installed) {
            Some(service) => Ok(service),

            // A web-app is served, not run — roadmap task **T81b**, the design's D10.
            None if installed.kind() == mixengine_proto::ExtensionKind::WebApp => {
                let domain = mixengine_core::sites::of_extension(&self.store, id)
                    .await
                    .map_err(|error| error.to_wire())?
                    .and_then(|site| site.domains.first().cloned())
                    .unwrap_or_default();

                Err(served_as_a_site(id, &domain))
            }

            None => Err(Error::new(
                ErrorCode::PreconditionFailed,
                format!(
                    "{id} is a {} extension and runs no process",
                    installed.kind()
                ),
            )),
        }
    }

    /// The manifest behind a source, and whether anything vouches for it.
    async fn manifest(&self, source: &ExtensionOrigin) -> Result<(ExtensionManifest, bool), Error> {
        match source {
            ExtensionOrigin::Registry { id } => {
                let catalogue = self
                    .registry
                    .catalogue()
                    .await
                    .map_err(|error| error.to_wire())?;

                let manifest = catalogue.index.find(id.as_str()).ok_or_else(|| {
                    Error::new(ErrorCode::NotFound, format!("{id} is not in the registry"))
                        .with_hint("`mix extension available` lists what is")
                })?;

                // **Signed because the document was.** The signature covers the whole registry, so
                // an entry either arrived inside something the compiled-in key vouched for or the
                // document was refused before this line.
                Ok((manifest, true))
            }

            ExtensionOrigin::Path { path } => {
                let directory = absolute(path)?;
                let file = match directory.is_file() {
                    true => directory,
                    false => directory.join(manifest::FILE_NAME),
                };

                let text = std::fs::read_to_string(&file).map_err(|source| {
                    Error::new(
                        ErrorCode::NotFound,
                        format!("{} could not be read: {source}", file.display()),
                    )
                })?;

                let manifest = manifest::read(&file, &text).map_err(|error| error.to_wire())?;

                Ok((manifest, false))
            }
        }
    }
}

/// Whether a consent describes the manifest that is about to be installed.
///
/// **Compared rather than believed** — [`ScaffoldConsent`](mixengine_proto::ScaffoldConsent)'s rule.
/// The registry can be refreshed between the plan a person read and the install a client sent, so
/// what is checked is that the version, the signature and the reach they were shown are still the
/// ones about to be installed. Disagreement in either direction refuses.
fn agrees(
    consent: &ExtensionConsent,
    manifest: &ExtensionManifest,
    signed: bool,
) -> Result<(), Error> {
    let refuse = |what: &str| {
        Err(Error::new(
            ErrorCode::PreconditionFailed,
            format!("this is not the {what} you were shown; read the plan again"),
        )
        .with_hint("`mix extension plan` shows what would be installed now"))
    };

    if consent.id != manifest.extension.id {
        return refuse("extension");
    }

    if consent.version != manifest.extension.version {
        return refuse("version");
    }

    if consent.signed != signed {
        return refuse("signature");
    }

    if consent.network != manifest.permissions.network {
        return refuse("network reach");
    }

    Ok(())
}

/// Whether this manifest puts anything in the front end's configuration — roadmap task **T81c**.
///
/// Asked three times on two paths — before an install is judged, after one has written its rows,
/// and before an uninstall removes them — and it is one function because the three have to agree:
/// an install that regenerated where a judgement had not looked would be a fragment reaching the
/// front end unjudged, and an uninstall that did not regenerate where the install had would leave
/// the file behind.
fn carries_a_fragment(manifest: &ExtensionManifest) -> bool {
    manifest
        .recipe
        .as_ref()
        .is_some_and(|recipe| !recipe.front_end.is_empty())
}

/// Whether this extension adds anything to a managed PHP's `conf.d` — roadmap task **T82**.
///
/// [`carries_a_fragment`]'s twin, and one function for the install and the uninstall for its reason:
/// an install that regenerated where an uninstall did not would leave the line behind.
fn carries_php_ini(manifest: &ExtensionManifest) -> bool {
    manifest
        .recipe
        .as_ref()
        .is_some_and(|recipe| !recipe.php_ini.is_empty())
}

/// Whether this extension declared a database, which is what makes an unwritten configuration worth
/// a warning rather than silence — roadmap task **T82**.
fn needs_a_database(installed: &extension_store::Installed) -> bool {
    matches!(
        &installed.manifest.body,
        manifest::Body::WebApp(app) if app.config.is_some() && app.database.is_some()
    )
}

/// A `web-app` is served, not run — roadmap task **T81b**, the design's D10.
fn served_as_a_site(id: &ExtensionId, domain: &str) -> Error {
    Error::new(
        ErrorCode::PreconditionFailed,
        format!("{id} is a web-app extension and is served as a site"),
    )
    .with_hint(format!(
        "`mix site start {domain}` and `mix site stop {domain}` control it"
    ))
}

/// One installed extension as the wire describes it, with the domain it is served on where it is.
fn summary(
    installed: &mixengine_core::extensions::store::Installed,
    site: Option<String>,
) -> ExtensionSummary {
    ExtensionSummary {
        id: installed.id.clone(),
        name: installed.name().to_owned(),
        version: installed.version().clone(),
        kind: installed.kind(),
        signed: installed.signed,
        service: uninstall::service_of(installed),
        ports: installed
            .ports
            .iter()
            .map(|(name, port)| PortWish {
                name: name.clone(),
                wanted: *port,
            })
            .collect(),
        site,
    }
}

/// A path the daemon can act on: absolute, because this daemon has no idea what the client's
/// current directory is and a relative path here would be resolved against the wrong one — which
/// reads the wrong file rather than failing.
fn absolute(given: &str) -> Result<PathBuf, Error> {
    let path = Path::new(given);

    match path.is_absolute() {
        true => Ok(path.to_path_buf()),
        false => Err(Error::new(
            ErrorCode::InvalidArgument,
            format!("{given} is not an absolute path"),
        )
        .with_hint("the client resolves a path against its own directory before sending it")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **T81b, D10.** A web-app runs no process, and the refusal says what controls it instead.
    #[test]
    fn a_web_app_is_controlled_as_a_site() {
        let refusal = served_as_a_site(
            &ExtensionId::parse("phpmyadmin").expect("an id"),
            "phpmyadmin.mixengine.test",
        );

        assert_eq!(refusal.code, ErrorCode::PreconditionFailed);
        assert!(
            refusal.message.contains("served as a site"),
            "{}",
            refusal.message
        );
        assert!(
            refusal
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("mix site stop phpmyadmin.mixengine.test")),
            "{refusal:?}"
        );
    }

    /// The one thing this type decides for itself.
    #[test]
    fn a_relative_path_is_refused() {
        let outcome = absolute("mailpit");

        let error = outcome.expect_err("a relative path is not something the daemon can resolve");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    /// **A consent is spent on what it named, and on nothing else.**
    #[test]
    fn a_consent_for_another_version_is_refused() {
        let manifest = manifest::read(
            Path::new("extension.toml"),
            mixengine_testkit::extension::MAILPIT,
        )
        .expect("a fixture parses");

        let agreed = ExtensionConsent {
            id: manifest.extension.id.clone(),
            version: mixengine_proto::PackageVersion::parse("0.0.1".to_owned()).expect("a version"),
            signed: true,
            network: manifest.permissions.network,
        };

        let refusal = agrees(&agreed, &manifest, true).expect_err("refused");
        assert_eq!(refusal.code, ErrorCode::PreconditionFailed);
    }

    /// Including when what changed is whether anything vouches for it.
    #[test]
    fn a_consent_given_for_a_signed_extension_is_not_spent_on_an_unsigned_one() {
        let manifest = manifest::read(
            Path::new("extension.toml"),
            mixengine_testkit::extension::MAILPIT,
        )
        .expect("a fixture parses");

        let agreed = ExtensionConsent {
            id: manifest.extension.id.clone(),
            version: manifest.extension.version.clone(),
            signed: true,
            network: manifest.permissions.network,
        };

        let refusal = agrees(&agreed, &manifest, false).expect_err("refused");
        assert_eq!(refusal.code, ErrorCode::PreconditionFailed);
    }

    /// And the ordinary case passes.
    #[test]
    fn a_consent_naming_what_is_installed_agrees() {
        let manifest = manifest::read(
            Path::new("extension.toml"),
            mixengine_testkit::extension::MAILPIT,
        )
        .expect("a fixture parses");

        let agreed = ExtensionConsent {
            id: manifest.extension.id.clone(),
            version: manifest.extension.version.clone(),
            signed: false,
            network: manifest.permissions.network,
        };

        agrees(&agreed, &manifest, false).expect("the consent describes it");
    }
}
