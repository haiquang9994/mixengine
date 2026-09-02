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
use mixengine_core::extensions::{
    install, manifest, registry, store as extension_store, uninstall,
};
use mixengine_core::index::Client;
use mixengine_core::{Paths, Store};
use mixengine_proto::{
    Error, ErrorCode, ExtensionCatalogue, ExtensionConsent, ExtensionId, ExtensionInspect,
    ExtensionInspection, ExtensionInstall, ExtensionOffer, ExtensionOrigin, ExtensionPlan,
    ExtensionPlanRequest, ExtensionRemoval, ExtensionSummary, ExtensionUninstall, JobKind,
    JobSummary, PortWish, ServiceId, Timestamp, rpc,
};

use crate::error::ToWire as _;

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
}

impl Extensions {
    /// Build it.
    ///
    /// # Errors
    ///
    /// Whatever building the registry client costs — a compiled-in key that is not one, which is a
    /// broken build.
    pub(crate) fn new(
        paths: Paths,
        store: Store,
        jobs: Arc<crate::jobs::Jobs>,
        host: Arc<dyn mixengine_platform::Host>,
        source: &crate::runtimes::IndexSource,
    ) -> Result<Self, Error> {
        // **The mirror that serves the package index serves this too** — T81. Derived from the same
        // setting rather than given one of its own, because the two documents are published side by
        // side under one tag and verified with one key.
        let registry = registry::client(&source.registry_url(), &source.public_key, paths.cache())
            .map_err(|error| error.to_wire())?;

        Ok(Self {
            paths,
            store,
            registry,
            jobs,
            host,
        })
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

        Ok(mixengine_proto::InstalledExtensions {
            extensions: installed.iter().map(summary).collect(),
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

        let installed = install::install(
            &self.store,
            &self.paths,
            self.host.as_ref(),
            install::Request {
                manifest,
                source: match signed {
                    true => Source::Registry,
                    false => Source::Path,
                },
                from: from.as_deref(),
                at: Timestamp::from_system_time(std::time::SystemTime::now()),
            },
            handle,
        )
        .await
        .map_err(|error| error.to_wire())?;

        serde_json::to_value(summary(&installed)).map_err(|source| {
            Error::new(
                ErrorCode::Internal,
                format!("an installed extension could not be described: {source}"),
            )
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
        let removed = uninstall::uninstall(&self.store, &self.paths, &asked.id, asked.delete_data)
            .await
            .map_err(|error| error.to_wire())?;

        Ok(ExtensionRemoval {
            id: removed.id,
            service: removed.service,
            data_dir_kept: removed.data_dir_kept.map(|path| path.display().to_string()),
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

        uninstall::service_of(&installed).ok_or_else(|| {
            Error::new(
                ErrorCode::PreconditionFailed,
                format!(
                    "{id} is a {} extension and runs no process",
                    installed.kind()
                ),
            )
        })
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

/// One installed extension as the wire describes it.
fn summary(installed: &mixengine_core::extensions::store::Installed) -> ExtensionSummary {
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
