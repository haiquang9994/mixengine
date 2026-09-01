//! `blueprint.*`: what a project is made of, written down and read back.
//!
//! Roadmap task **T77**. [`crate::projects`]' shape one namespace across, and like it there is no
//! index, no fetcher and no job here — a capture is rows and one rendered file inside this home.
//!
//! # Applying happens elsewhere
//!
//! What is here is the *planning*: [`Blueprints::planned`] is the one path a dry run and a real
//! apply both take. Carrying the plan out is `crate::api::apply` — a private module, so this is a
//! name rather than a link — because every action in a plan is a capability `Api` holds and this
//! type holds none of them (the T78 design, D1).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mixengine_core::blueprints::manifest::BlueprintManifest;
use mixengine_core::blueprints::{capture, manifest, plan, store as filed, trust};
use mixengine_core::{Paths, Store, projects};
use mixengine_proto::{
    BlueprintApply, BlueprintCapture, BlueprintImport, BlueprintList, BlueprintPlan,
    BlueprintSource, BlueprintSummary, Error, ErrorCode, ProjectRef, Timestamp,
};

use crate::error::ToWire as _;

/// Everything `blueprint.*` needs: the rows, and the directory a rendering goes in.
#[derive(Debug)]
pub(crate) struct Blueprints {
    /// Where a blueprint is written down.
    store: Store,

    /// Where its rendering goes.
    paths: Paths,

    /// This build's version, for a captured manifest's provenance.
    version: &'static str,
}

impl Blueprints {
    /// The one of these the API holds.
    pub(crate) fn new(store: &Store, paths: &Paths, version: &'static str) -> Arc<Self> {
        Arc::new(Self {
            store: store.clone(),
            paths: paths.clone(),
            version,
        })
    }

    /// `blueprint.capture` — write down what a project is made of.
    ///
    /// # Errors
    ///
    /// `not_found` for a project nothing is registered as; `invalid_argument` for a name that
    /// cannot be a slug; `already_exists` for a name already taken without `overwrite`; `conflict`
    /// for a project holding more than one site.
    pub(crate) async fn capture(
        &self,
        asked: &BlueprintCapture,
    ) -> Result<BlueprintSummary, Error> {
        let project = self.expect(&asked.project).await?;

        // Refused before anything is read, because the name is what the rendering is filed under and
        // a capture that did the work and then failed on the name would be work thrown away.
        let slug = filed::validated_slug(&asked.name).map_err(|error| error.to_wire())?;

        let manifest = capture::capture(
            &self.store,
            &capture::Asked {
                project: &project,
                name: &slug,
                description: asked.description.as_deref().unwrap_or_default(),
                // A compile-time constant rather than a call into the operating system, which is
                // what `crate::diagnostics` already reads for the same field.
                os: std::env::consts::OS,
                version: self.version,
                created_at: &now(),
            },
        )
        .await
        .map_err(|error| error.to_wire())?;

        filed::save(
            &self.store,
            &self.paths,
            &manifest,
            &slug,
            BlueprintSource::Captured,
            // A capture is this machine's own, so there is nobody else to vouch for — roadmap task
            // **T78a**, its design's D1. It cannot carry a `[scaffold]` either way: capture never
            // writes one.
            true,
            asked.overwrite,
        )
        .await
        .map_err(|error| {
            error.to_wire().with_hint(
                "`mix blueprint capture --overwrite` replaces the one already filed under that name",
            )
        })
    }

    /// `blueprint.import` — take in a blueprint somebody else wrote.
    ///
    /// Roadmap task **T78a**, its design's D3. **The signature decides the trust, and nothing else
    /// ever does** (D1): it is checked here, once, against the bytes on disk, and the answer becomes
    /// a column no later call raises. The rendering written beside the row is not the signed
    /// artifact and is never checked again — D16, and the round-trip test that keeps the two
    /// identical in practice.
    ///
    /// A signature that does not verify is **not** a refusal. A file whose signature is stale is
    /// still a file its owner may want; what it loses is the right to have its `[scaffold]` offered
    /// without the louder gesture, which is exactly what the untrusted marking is for.
    ///
    /// # Errors
    ///
    /// `invalid_argument` for a path that is not absolute, a file that cannot be read or is not a
    /// manifest, and a name that cannot be a slug; `already_exists` for a name already taken
    /// without `overwrite`.
    pub(crate) async fn import(&self, asked: &BlueprintImport) -> Result<BlueprintSummary, Error> {
        let path = absolute(&asked.path)?;

        let document = tokio::fs::read(&path).await.map_err(|source| {
            Error::new(
                ErrorCode::InvalidArgument,
                format!("{} could not be read: {source}", path.display()),
            )
        })?;

        let text = String::from_utf8(document.clone()).map_err(|_| {
            Error::new(
                ErrorCode::InvalidArgument,
                format!("{} is not text, so it is not a manifest", path.display()),
            )
        })?;

        let manifest = manifest::read(&text).map_err(|error| error.to_wire())?;

        let trusted = self.vouched_for(asked, &path, &document).await?;

        // **The file's own name, not the manifest's** — roadmap task **T79a**, its design's D10.
        // `[blueprint] name` is display text and the gallery is what proves it: `Static site` and
        // `Next.js` are good names for a person and cannot be slugs at all. Every rendering this
        // product writes is `<slug>.toml`, so the stem is what carries a blueprint's name from one
        // machine to another — and it is still `validated_slug` that decides whether it may.
        let slug = match &asked.name {
            Some(given) => given.clone(),
            None => file_stem(&path)?,
        };

        filed::save(
            &self.store,
            &self.paths,
            &manifest,
            &slug,
            BlueprintSource::Imported,
            trusted,
            asked.overwrite,
        )
        .await
        .map_err(|error| {
            error.to_wire().with_hint(
                "`mix blueprint import --overwrite` replaces the one already filed under that name",
            )
        })
    }

    /// Whether the gallery signed these bytes.
    ///
    /// The signature named in the request, or the `<file>.minisig` beside it, or nothing — and
    /// nothing is the ordinary case for a blueprint a colleague sent, which is why it is an answer
    /// rather than an error.
    async fn vouched_for(
        &self,
        asked: &BlueprintImport,
        path: &Path,
        document: &[u8],
    ) -> Result<bool, Error> {
        let beside = || {
            let mut name = path.as_os_str().to_owned();
            name.push(".minisig");
            let beside = PathBuf::from(name);

            beside.is_file().then_some(beside)
        };

        let Some(file) = (match &asked.signature {
            Some(given) => Some(absolute(given)?),
            None => beside(),
        }) else {
            return Ok(false);
        };

        let signature = tokio::fs::read_to_string(&file).await.map_err(|source| {
            Error::new(
                ErrorCode::InvalidArgument,
                format!("{} could not be read: {source}", file.display()),
            )
        })?;

        match trust::verify(document, &signature, trust::PUBLIC_KEY) {
            Ok(()) => Ok(true),

            // Said rather than swallowed, and not raised: what the file loses is the trust, not the
            // import (D3).
            Err(error) => {
                tracing::info!(
                    path = %path.display(),
                    %error,
                    "a blueprint's signature did not verify against the gallery key; it is imported untrusted"
                );

                Ok(false)
            }
        }
    }

    /// `blueprint.list` — every blueprint this home holds.
    ///
    /// # Errors
    ///
    /// `internal` when a row holds a source this build does not know.
    pub(crate) async fn list(&self) -> Result<BlueprintList, Error> {
        let blueprints = filed::records(&self.store, &self.paths)
            .await
            .map_err(|error| error.to_wire())?;

        Ok(BlueprintList { blueprints })
    }

    /// The manifest a request names and the plan it implies — **the one path a dry run and a real
    /// apply both take**.
    ///
    /// One function rather than two, because the feature's acceptance criterion is that `--dry-run`
    /// matches what the real run performs, and two planning paths are two chances for them to
    /// disagree. Both halves are answered because the executor needs both, and reading the row a
    /// second time would be two chances to read two different things.
    ///
    /// # Errors
    ///
    /// `not_found` for a blueprint nothing is filed under; `invalid_argument` for a root that is not
    /// absolute; the wire error of a table that cannot be read.
    pub(crate) async fn planned(
        &self,
        asked: &BlueprintApply,
    ) -> Result<(BlueprintManifest, BlueprintPlan), Error> {
        let root = absolute(&asked.root)?;
        let filed = filed::filed_of(&self.store, &asked.blueprint)
            .await
            .map_err(|error| {
                error
                    .to_wire()
                    .with_hint("`mix blueprint list` shows what does exist")
            })?;

        let plan = plan::plan(
            &self.store,
            &asked.blueprint,
            &filed,
            &asked.project,
            &root,
            &asked.answers,
        )
        .await
        .map_err(|error| error.to_wire())?;

        Ok((filed.manifest, plan))
    }

    /// The project a reference names, or the refusal that says which kind of miss it was.
    async fn expect(&self, reference: &ProjectRef) -> Result<projects::ProjectRecord, Error> {
        projects::find(&self.store, reference)
            .await
            .map_err(|error| error.to_wire())?
            .ok_or_else(|| {
                let (said, hint) = match reference {
                    ProjectRef::Name(name) => (
                        format!("no such project: {name}"),
                        "`mix project list` shows what does exist".to_owned(),
                    ),
                    ProjectRef::Path(path) => (
                        format!("no project is registered at or above {path}"),
                        format!("`mix project create {path}` registers it"),
                    ),
                };

                Error::new(ErrorCode::NotFound, said).with_hint(hint)
            })
    }
}

/// The moment a capture happened, as this schema writes one down.
///
/// `to_rfc3339` for the reason `0001_initial.sql` gives: a moment a person reads is text, and this
/// one is read back by nobody.
fn now() -> String {
    Timestamp::from_system_time(std::time::SystemTime::now()).to_rfc3339()
}

/// The slug a file carries in its own name — roadmap task **T79a**, its design's D10.
///
/// # Errors
///
/// `invalid_argument` for a path with no filename, or one whose stem is not UTF-8: neither is a
/// name a slug could be taken from, and `--name` is what says so instead.
fn file_stem(path: &Path) -> Result<String, Error> {
    path.file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidArgument,
                format!("{} has no filename to file it under", path.display()),
            )
            .with_hint("`mix blueprint import --name <NAME>` says what to file it under")
        })
}

/// A root the daemon can act on: absolute, because this daemon has no idea what the client's
/// current directory is and a relative path here would be resolved against the wrong one.
fn absolute(given: &str) -> Result<PathBuf, Error> {
    let path = Path::new(given);

    match path.is_absolute() {
        true => Ok(path.to_path_buf()),
        false => Err(Error::new(
            ErrorCode::InvalidArgument,
            format!("{given} is not an absolute path"),
        )
        .with_hint("the client resolves a root against its own directory before sending it")),
    }
}
