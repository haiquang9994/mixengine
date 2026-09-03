//! Extensions — roadmap task **T80**.
//!
//! **Not [`crate::runtimes::extensions`]**, which is about a PHP extension being switched on for
//! one installed runtime. These are MixEngine's own: Mailpit, phpMyAdmin, MixDB.
//!
//! T80 reads a manifest and renders it into the thing that would run. Nothing here installs, stores
//! or starts anything — that is T81, which is deliberately handed a format already proved to make
//! sense rather than one it discovers is wrong five actions into an install.

pub mod config;
pub mod database;
pub mod install;
pub mod manifest;
pub mod pools;
pub mod recipe;
pub mod registry;
pub mod render;
pub mod store;
pub mod uninstall;

use std::path::Path;

use mixengine_proto::{
    ArtifactAvailability, DesktopAppSummary, ExtensionInspection, PortWish, RecipeAddition,
    WebAppSummary,
};

use crate::index::format::{Arch, Os};
use crate::{Error, Paths, Result};

use manifest::{Body, ExtensionManifest, RecipeTable};
use render::Context;

/// The `[artifact.<target>]` key for something that runs anywhere.
const ANY: &str = "any";

/// Read the manifest at `path` and say what installing it here would produce.
///
/// `path` is the directory holding `extension.toml`; the file itself is accepted too, because that
/// is what a person types.
///
/// **It renders rather than reporting.** The context it builds is the one an install on this
/// machine would build — `install_dir` under `extensions/<id>`, the ports from `[ports]`, `{listen}`
/// from `permissions.network` — and the answer carries the spec that came out of it. That is
/// `blueprint.apply --dry-run`'s position: a plan is worth having because it was computed.
///
/// # Errors
///
/// [`Error::Io`] where the file cannot be read, and every refusal [`manifest::read`] and
/// [`render::service_spec`] raise.
pub fn inspect(paths: &Paths, path: &Path) -> Result<ExtensionInspection> {
    let file = if path.is_file() {
        path.to_path_buf()
    } else {
        path.join(manifest::FILE_NAME)
    };

    let text = std::fs::read_to_string(&file).map_err(|source| Error::Io {
        action: "read",
        path: file.clone(),
        source,
    })?;

    let read = manifest::read(&file, &text)?;
    let context = Context::planned(paths, &read);

    let runs = match &read.body {
        Body::Service(template) => Some(render::service_spec(&read, template, &context)?),
        _ => None,
    };

    let serves = match &read.body {
        Body::WebApp(app) => Some(WebAppSummary {
            root: render::rooted(
                &read.extension.id,
                "web-app.root",
                &app.root,
                &[render::INSTALL_DIR],
                &context,
            )?
            .display()
            .to_string(),
            domain: app.domain.clone(),
            runtime: app.runtime.kind,
            requires: app.runtime.requires.clone(),
        }),
        _ => None,
    };

    let opens = match &read.body {
        Body::DesktopApp(app) => Some(DesktopAppSummary {
            scheme: app.scheme.clone(),
            detect: app.detect.here().map(str::to_owned),
        }),
        _ => None,
    };

    let extends = additions(&read, &context)?;

    Ok(ExtensionInspection {
        artifact: availability(&read),
        ports: read
            .ports
            .iter()
            .map(|(name, wanted)| PortWish {
                name: name.clone(),
                wanted: *wanted,
            })
            .collect(),
        install_dir: context.install_dir.display().to_string(),
        data_dir: context.data_dir.display().to_string(),
        id: read.extension.id.clone(),
        name: read.extension.name.clone(),
        version: read.extension.version.clone(),
        kind: read.extension.kind,
        description: read.extension.description.clone(),
        homepage: read.extension.homepage.clone(),
        permissions: read.permissions.clone(),
        runs,
        serves,
        opens,
        extends,
    })
}

/// What `[recipe]` adds, rendered.
///
/// **Held to the same address rule as `[service]`**: a `sendmail_path` naming a host would reach
/// past this machine exactly as an argument would, and it is written by the same author on the same
/// afternoon.
fn additions(read: &ExtensionManifest, context: &Context) -> Result<Vec<RecipeAddition>> {
    let Some(RecipeTable { php_ini, front_end }) = &read.recipe else {
        return Ok(Vec::new());
    };

    let id = &read.extension.id;
    let mut additions = Vec::with_capacity(php_ini.len() + front_end.len());

    for entry in php_ini {
        let field = format!("recipe.php_ini.{}", entry.key);
        additions.push(RecipeAddition::PhpIni {
            key: entry.key.clone(),
            value: render::value(id, &field, &entry.value, context)?,
        });
    }

    for (index, entry) in front_end.iter().enumerate() {
        let field = format!("recipe.front_end[{index}]");
        additions.push(RecipeAddition::FrontEnd {
            server: entry.server,
            fragment: render::fragment(id, &field, &entry.fragment, context, entry.server.into())?,
        });
    }

    Ok(additions)
}

/// Whether this machine has an artifact to install.
///
/// `pub` since T81: a listing of what the registry publishes answers the same question about an
/// entry nobody has inspected, and two functions deciding it would be two answers.
#[must_use]
pub fn availability(read: &ExtensionManifest) -> ArtifactAvailability {
    if read.artifacts.is_empty() {
        return ArtifactAvailability::NotRequired;
    }

    let here = Os::host()
        .zip(Arch::host())
        .map(|(os, arch)| format!("{}-{}", os.as_str(), arch.as_str()));

    let published = here
        .and_then(|target| read.artifacts.get(&target))
        .or_else(|| read.artifacts.get(ANY));

    match published {
        Some(artifact) => ArtifactAvailability::Published {
            url: artifact.url.clone(),
            sha256: artifact.sha256.clone(),
        },
        None => ArtifactAvailability::OtherTargets {
            targets: read.artifacts.keys().cloned().collect(),
        },
    }
}

#[cfg(test)]
mod tests {
    use mixengine_proto::ExtensionKind;

    use super::*;
    use crate::config::PathOverrides;

    /// Inspect answers what *would run*, not what the file says.
    #[test]
    fn inspect_renders_a_service() {
        let home = tempfile::tempdir().expect("a directory");
        let paths = Paths::new(home.path().to_path_buf(), &PathOverrides::default());
        let directory = paths.extensions().join("mailpit");
        std::fs::create_dir_all(&directory).expect("a directory");
        std::fs::write(
            directory.join(manifest::FILE_NAME),
            mixengine_testkit::extension::MAILPIT,
        )
        .expect("written");

        let inspection = inspect(&paths, &directory).expect("inspects");

        assert_eq!(inspection.id.as_str(), "mailpit");
        assert_eq!(inspection.kind, ExtensionKind::Service);

        let spec = inspection.runs.expect("a service renders one");
        assert!(spec.args().contains(&"127.0.0.1:8025".to_owned()));

        assert_eq!(
            inspection.ports,
            vec![
                PortWish {
                    name: "smtp_port".to_owned(),
                    wanted: 1025
                },
                PortWish {
                    name: "ui_port".to_owned(),
                    wanted: 8025
                },
            ]
        );
        assert_eq!(inspection.extends.len(), 1);
    }

    /// A kind with nothing to supervise does not get a spec invented for it. And the path may be
    /// the file itself, because that is what a person types.
    #[test]
    fn inspect_invents_no_spec() {
        let home = tempfile::tempdir().expect("a directory");
        let paths = Paths::new(home.path().to_path_buf(), &PathOverrides::default());
        std::fs::create_dir_all(paths.extensions()).expect("a directory");
        let file = paths.extensions().join("mixdb.toml");
        std::fs::write(&file, mixengine_testkit::extension::MIXDB).expect("written");

        let inspection = inspect(&paths, &file).expect("inspects");

        assert!(inspection.runs.is_none());
        assert!(inspection.opens.is_some());
        assert!(matches!(
            inspection.artifact,
            ArtifactAvailability::NotRequired
        ));
    }

    /// An artifact published for other systems is a **state**, not a failure — the shape T83 gives
    /// "MixDB is not installed".
    #[test]
    fn an_artifact_for_another_machine_is_a_state() {
        let home = tempfile::tempdir().expect("a directory");
        let paths = Paths::new(home.path().to_path_buf(), &PathOverrides::default());
        let directory = paths.extensions().join("mailpit");
        std::fs::create_dir_all(&directory).expect("a directory");
        // **Every target renamed, counted rather than listed.** The fixture is the manifest that
        // shipped (T82), so its target list is upstream's to change — a test naming three of them
        // failed the day Mailpit published six, which is a fixture doing its job and an assertion
        // that was measuring the wrong thing.
        let text = mixengine_testkit::extension::MAILPIT
            .replace("[artifact.windows-", "[artifact.plan9w-")
            .replace("[artifact.macos-", "[artifact.plan9m-")
            .replace("[artifact.linux-", "[artifact.plan9l-");
        let published = text.matches("[artifact.").count();
        std::fs::write(directory.join(manifest::FILE_NAME), text).expect("written");

        let inspection = inspect(&paths, &directory).expect("inspects");

        let ArtifactAvailability::OtherTargets { targets } = inspection.artifact else {
            panic!("nothing here is published for this machine");
        };
        assert_eq!(targets.len(), published);
        assert!(published >= 3, "a fixture with no targets proves nothing");
    }

    /// A recipe value is held to the address rule too — the same author, the same afternoon.
    #[test]
    fn a_recipe_may_not_write_an_address() {
        let home = tempfile::tempdir().expect("a directory");
        let paths = Paths::new(home.path().to_path_buf(), &PathOverrides::default());
        let directory = paths.extensions().join("sendmail-to-mailpit");
        std::fs::create_dir_all(&directory).expect("a directory");
        let text = mixengine_testkit::extension::SENDMAIL.replace(
            "value = \"{install_dir}/sendmail.sh\"",
            "value = \"{install_dir}/sendmail.sh --host 127.0.0.1\"",
        );
        std::fs::write(directory.join(manifest::FILE_NAME), text).expect("written");

        let outcome = inspect(&paths, &directory);

        assert!(
            matches!(outcome, Err(Error::ExtensionField { .. })),
            "{outcome:?}"
        );
    }
}
