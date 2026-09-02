//! Putting an extension on this machine — roadmap task **T81**.
//!
//! # The order, and why it is that order
//!
//! 1. Read the manifest — from the verified registry, or from a directory for `--path`.
//! 2. Refuse what this build cannot honour, **before anything is fetched**.
//! 3. Ask. [`plan`] is what a person answers, and it carries the permissions the manifest declares.
//! 4. Download, verify the SHA-256, unpack into staging, rename into place — [`crate::install`]
//!    whole, which is the same transaction a runtime install is.
//! 5. Allocate the ports, write the rows.
//!
//! **Consent comes before the download** (the T81 design's D2), and that is what carrying manifests
//! in the registry buys: the question *"this wants to reach the LAN and read your project roots —
//! go on?"* can be asked before a byte of artifact arrives. Asking afterwards is asking after doing
//! the thing somebody was about to refuse.
//!
//! # What a half-install leaves behind
//!
//! Nothing. The archive is unpacked into a staging directory beside the destination and renamed in
//! only once everything that could still refuse it has not — which is [`crate::install`]'s
//! invariant, inherited here rather than restated. The rows are written after that, so a failure
//! at any point leaves a home that has never heard of this extension.

use std::path::{Path, PathBuf};

use mixengine_proto::{
    ExtensionId, ExtensionKind, ExtensionPermissions, PackageVersion, PortWish, Timestamp,
};

use crate::extensions::manifest::{Body, ExtensionManifest};
use crate::extensions::recipe;
use crate::extensions::store::{self as extension_store, Installed, Source};
use crate::index::format::{Arch, Artifact, Os};
use crate::install::{Installer, Watcher};
use crate::services::{self, Declaration, Origin, Port};
use crate::{Error, Paths, Result, Store};

/// The `[artifact.<target>]` key for something that runs anywhere.
const ANY: &str = "any";

/// The ceiling a download is held to when a manifest declares no size.
///
/// The package index makes `size` mandatory and [`crate::install`] uses it to stop a body that
/// never ends before a disk is filled — a bound the checksum cannot provide, since it is only
/// knowable once everything has arrived. An extension manifest may omit it, because an author
/// pointing `--path` at their own directory should not have to count bytes to try something. So a
/// declared size is used as the index's is, and an absent one falls back to this: large enough for
/// anything in the plan, small enough that a stream with no end is stopped.
const UNDECLARED_SIZE_CEILING: u64 = 2 * 1024 * 1024 * 1024;

/// What somebody is shown before anything is fetched.
///
/// **`permissions.services` is a declaration and not a boundary** — [ADR
/// 0014](../../../../.claude/decisions/0014-an-extension-is-not-an-api-client.md) — and every
/// surface that renders this says so.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Which extension.
    pub id: ExtensionId,

    /// What it calls itself.
    pub name: String,

    /// Its own version.
    pub version: PackageVersion,

    /// What it is.
    pub kind: ExtensionKind,

    /// What it is for.
    pub description: String,

    /// Whether a signature covered it. `false` for every `--path` install.
    pub signed: bool,

    /// What it declares.
    pub permissions: ExtensionPermissions,

    /// The ports it asks for. **Asked for, not held** — nothing is allocated until the install runs.
    pub ports: Vec<PortWish>,

    /// Where its own files would go.
    pub install_dir: PathBuf,

    /// Where what it writes would go.
    pub data_dir: PathBuf,
}

/// Read a manifest and say what installing it here would do, without doing any of it.
///
/// # Errors
///
/// [`Error::ExtensionAlreadyInstalled`] when something is already installed under that id;
/// [`Error::ExtensionRecipeUnsupported`] for a `[recipe]` this build cannot apply;
/// [`Error::ExtensionNoArtifact`] where nothing is published for this machine; and whatever reading
/// the manifest reports.
pub async fn plan(
    store: &Store,
    paths: &Paths,
    manifest: &ExtensionManifest,
    signed: bool,
) -> Result<Plan> {
    let id = &manifest.extension.id;

    if extension_store::exists(store, id).await? {
        return Err(Error::ExtensionAlreadyInstalled {
            id: id.as_str().to_owned(),
        });
    }

    supported(manifest)?;
    artifact_for_host(manifest)?;

    Ok(Plan {
        id: id.clone(),
        name: manifest.extension.name.clone(),
        version: manifest.extension.version.clone(),
        kind: manifest.extension.kind,
        description: manifest.extension.description.clone(),
        signed,
        permissions: manifest.permissions.clone(),
        ports: manifest
            .ports
            .iter()
            .map(|(name, wanted)| PortWish {
                name: name.clone(),
                wanted: *wanted,
            })
            .collect(),
        install_dir: extension_store::install_dir(paths, id),
        data_dir: extension_store::data_dir(paths, id),
    })
}

/// Everything a manifest declares that this build cannot honour.
///
/// **A refusal rather than a field quietly ignored** — the T81 design's D10. A `[recipe] front_end`
/// fragment would have to reach both front-end templates and be revalidated against the real
/// server, and no extension in the plan asks for one; accepting it here would install a manifest
/// whose stated effect does not happen, which is the failure this codebase spends whole designs
/// avoiding.
fn supported(manifest: &ExtensionManifest) -> Result<()> {
    if manifest
        .recipe
        .as_ref()
        .is_some_and(|recipe| !recipe.front_end.is_empty())
    {
        return Err(Error::ExtensionRecipeUnsupported {
            id: manifest.extension.id.as_str().to_owned(),
            field: "recipe.front_end",
        });
    }

    Ok(())
}

/// The artifact this machine would install, or [`None`] for a kind that needs none.
///
/// **No artifact for this machine is an answer, not a failure to install** — but it is an answer
/// that stops an install, because there is nothing to put anywhere.
///
/// # Errors
///
/// [`Error::ExtensionNoArtifact`], naming what *is* published, so somebody on a machine an
/// extension was not built for is told which ones it was.
fn artifact_for_host(manifest: &ExtensionManifest) -> Result<Option<Artifact>> {
    if manifest.artifacts.is_empty() {
        return Ok(None);
    }

    let here = Os::host()
        .zip(Arch::host())
        .map(|(os, arch)| format!("{}-{}", os.as_str(), arch.as_str()));

    let published = here
        .as_ref()
        .and_then(|target| manifest.artifacts.get(target))
        .or_else(|| manifest.artifacts.get(ANY));

    let Some(declared) = published else {
        return Err(Error::ExtensionNoArtifact {
            id: manifest.extension.id.as_str().to_owned(),
            targets: manifest.artifacts.keys().cloned().collect(),
        });
    };

    // The index's own artifact type, so the download, the checksum, the unpack, the staging
    // directory and the atomic rename are the ones a runtime install goes through rather than a
    // second implementation that resembles them. What a manifest does not carry is filled in with
    // what this shape means here: no `provides`, because an extension names its program itself; and
    // a size ceiling where it declared none, see `UNDECLARED_SIZE_CEILING`.
    Ok(Some(Artifact {
        os: Os::host().unwrap_or(Os::Linux),
        arch: Arch::host().unwrap_or(Arch::X86_64),
        url: declared.url.clone(),
        sha256: declared.sha256.clone(),
        size: declared.size.unwrap_or(UNDECLARED_SIZE_CEILING),
        provides: std::collections::BTreeMap::new(),
        requires: crate::index::Requires::default(),
        extension_dir: None,
        extensions: crate::index::Extensions::default(),
    }))
}

/// One install, as the caller describes it.
///
/// A struct rather than four more parameters: `source` and `from` are two halves of one answer —
/// where this extension came from — and pulling them apart at the call site is how a registry
/// install ends up marked as a directory one.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    /// What is being installed.
    pub manifest: &'a ExtensionManifest,

    /// Where it came from, which is also what decides whether it is signed.
    pub source: Source,

    /// The directory a `--path` install copies, and [`None`] for one that downloads.
    pub from: Option<&'a Path>,

    /// When this happened.
    pub at: Timestamp,
}

/// Install an extension whose [`plan`] somebody has agreed to.
///
/// # Errors
///
/// Everything [`plan`] reports, everything [`crate::install::Installer::install`] reports, and
/// [`Error::Database`] when the rows cannot be written.
pub async fn install<W: Watcher>(
    store: &Store,
    paths: &Paths,
    host: &dyn mixengine_platform::Host,
    request: Request<'_>,
    watcher: &W,
) -> Result<Installed> {
    let Request {
        manifest,
        source,
        from,
        at,
    } = request;
    let id = &manifest.extension.id;
    let install_dir = extension_store::install_dir(paths, id);
    let data_dir = extension_store::data_dir(paths, id);

    supported(manifest)?;

    if extension_store::exists(store, id).await? {
        return Err(Error::ExtensionAlreadyInstalled {
            id: id.as_str().to_owned(),
        });
    }

    crate::paths::create_dir(paths.extensions())?;

    match from {
        // **A directory is copied rather than linked**, so that editing the source afterwards does
        // not change what is installed: the row's manifest and the files it names have to describe
        // one thing, and a symlink would let them diverge without anything to notice.
        Some(directory) => copy_into(directory, &install_dir)?,
        None => {
            if let Some(artifact) = artifact_for_host(manifest)? {
                let installer = Installer::new(paths.cache())?;
                installer
                    .install(&artifact, &install_dir, None, watcher)
                    .await?;
            } else {
                // A kind with no artifact — a `recipe`, or a `desktop-app` we only detect — still
                // gets its directory, so everything downstream can name one place.
                crate::paths::create_dir(&install_dir)?;
            }
        }
    }

    let written = write_rows(store, host, manifest, &install_dir, &data_dir, source, at).await;

    if written.is_err() {
        // The rows are the last step, so undoing them is undoing the directory: an extension whose
        // row did not land is one nothing can start, list or uninstall, and leaving the files would
        // leave `install` unable to run again for the same id.
        discard(&install_dir).await;
    }

    written
}

/// Allocate the ports and write every row, or write none of them.
///
/// **The allocation lock is held over the ports and the extension row, and released before the
/// service row is written.** It has to cover the first two together, for `services::create`'s
/// reason: two installs asking at once both read a table neither has written to and are told the
/// same number is free. It must *not* still be held for the third, because `services::create` takes
/// the same lock — and this one is not reentrant, so holding it across that call is a daemon that
/// stops answering. By then the numbers are in `extension_ports`, which is what makes them held.
async fn write_rows(
    store: &Store,
    host: &dyn mixengine_platform::Host,
    manifest: &ExtensionManifest,
    install_dir: &Path,
    data_dir: &Path,
    source: Source,
    at: Timestamp,
) -> Result<Installed> {
    let id = &manifest.extension.id;

    let installed = {
        let _allocating = crate::services::ports::hold().await;

        let mut ports = std::collections::BTreeMap::new();

        for (name, wanted) in &manifest.ports {
            let allocation = crate::services::ports::allocate(
                store,
                host,
                manifest.permissions.network.listen_address(),
                *wanted,
            )
            .await?;

            ports.insert(name.clone(), allocation.port);
        }

        let installed = Installed {
            id: id.clone(),
            manifest: manifest.clone(),
            install_dir: install_dir.to_path_buf(),
            data_dir: data_dir.to_path_buf(),
            source,
            signed: matches!(source, Source::Registry),
            installed_at: at,
            ports,
        };

        extension_store::remember(store, &installed).await?;

        installed
    };

    // The service, for the one kind that runs a process. `Port::Fixed`, because the number was
    // decided above and is already held by `extension_ports` — asking the allocator again would be
    // asking it to step over a port this extension itself holds.
    if matches!(manifest.extension.kind, ExtensionKind::Service)
        && matches!(manifest.body, Body::Service(_))
    {
        let declaration = Declaration {
            service: id.service_id().clone(),
            origin: Origin::Extension { id: id.clone() },
            instance_name: recipe::instance_name(id),
            port: recipe::served_port_name(manifest)
                .and_then(|named| installed.ports.get(named).copied())
                .map_or(Port::None, Port::Fixed),
            bind_addr: Some(manifest.permissions.network.listen_address().to_string()),
            data_dir: Some(data_dir.display().to_string()),
            autostart: false,
            overrides: "{}".to_owned(),
        };

        if let Err(refusal) = services::create(store, host, &declaration).await {
            // The extension row landed and its service did not, which would leave something listed
            // and unstartable.
            extension_store::forget(store, id).await?;
            return Err(refusal);
        }
    }

    crate::paths::create_dir(data_dir)?;

    Ok(installed)
}

/// Copy a directory tree, for `--path`.
fn copy_into(from: &Path, into: &Path) -> Result<()> {
    if into.exists() {
        return Err(Error::AlreadyInstalled {
            path: into.to_path_buf(),
        });
    }

    crate::paths::create_dir(into)?;

    let entries = std::fs::read_dir(from).map_err(|source| Error::Io {
        action: "read",
        path: from.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            action: "read",
            path: from.to_path_buf(),
            source,
        })?;

        let target = into.join(entry.file_name());

        match entry.path().is_dir() {
            true => copy_into(&entry.path(), &target)?,
            false => {
                std::fs::copy(entry.path(), &target).map_err(|source| Error::Io {
                    action: "write",
                    path: target,
                    source,
                })?;
            }
        }
    }

    Ok(())
}

/// Remove a directory, saying nothing about a failure.
///
/// The caller is already reporting something that went wrong, and "the install failed, and so did
/// tidying up after it" is not a more useful sentence than the first half.
async fn discard(path: &Path) {
    if let Err(reason) = tokio::fs::remove_dir_all(path).await
        && reason.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), error = %reason, "could not remove a half-install");
    }
}
