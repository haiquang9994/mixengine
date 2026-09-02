//! Running an installed extension, out of the manifest it was installed from — roadmap task
//! **T81**, the design's D7.
//!
//! # Why an extension is a [`Recipe`] rather than a second way to build a spec
//!
//! [`Generator::prepare`](crate::generate::Generator) looks a recipe up by `packages.name` and
//! everything else a `services` row is worth — resource limits, the idle policy, the activation
//! port, the crash-loop window, log capture — is applied around whatever that lookup returns. An
//! extension has no compiled-in recipe and must not have one: a recipe is what *this build* knows,
//! and an extension is what a home installed.
//!
//! So the manifest arrives here as a recipe made at run time. The alternative was a second path
//! through `declared()` that skips recipes altogether, and it would be a second place where every
//! one of those has to be remembered — the way that is discovered is an extension quietly not
//! honouring one of them.
//!
//! What this recipe answers is deliberately almost nothing: no settings, because an extension
//! declares no overrides; no template files, because its configuration is its own; no validator,
//! because there is no rendering of ours to judge. The one thing it does is build the spec, and it
//! does that by handing the manifest back to [`crate::extensions::render`] — which T80 already
//! wrote, and which `extension.inspect` already showed somebody.

use mixengine_proto::ServiceSpecBuilder;

use crate::extensions::manifest::{Body, ExtensionManifest, ReadyTemplate};
use crate::extensions::render::{self, Context as RenderContext};
use crate::extensions::store::Installed;
use crate::generate::recipe::{Context, Instancing, Recipe};
use crate::{Error, Result};

/// The instance name an extension's service is created under, which is the extension's own id.
///
/// **One instance**, because an extension is a product somebody installed rather than a server they
/// run several of — so its `ServiceId` is the bare `mailpit` and not `mailpit@something`, and
/// `services::Declaration` already states the rule this follows: the package's own name for one
/// that exists once. `UNIQUE (extension_id, instance_name)` leaves room for the day one has
/// instances.
#[must_use]
pub fn instance_name(id: &mixengine_proto::ExtensionId) -> String {
    id.as_str().to_owned()
}

/// An installed extension, as something the supervisor can be told to run.
#[derive(Debug)]
pub struct ExtensionRecipe {
    /// The row, which carries the manifest and the ports actually allocated.
    installed: Installed,
}

impl ExtensionRecipe {
    /// Build one from a row.
    #[must_use]
    pub fn new(installed: Installed) -> Self {
        Self { installed }
    }

    /// The render context this extension is installed under.
    ///
    /// **The row's ports, not the manifest's.** `[ports]` is what an extension *asked for*; what it
    /// holds is in `extension_ports`, and rendering the wish would produce a spec that binds a port
    /// somebody else was given.
    fn context(&self) -> RenderContext {
        RenderContext {
            install_dir: self.installed.install_dir.clone(),
            data_dir: self.installed.data_dir.clone(),
            ports: self.installed.ports.clone(),
            listen: self.installed.manifest.permissions.network.listen_address(),
        }
    }
}

impl Recipe for ExtensionRecipe {
    fn package(&self) -> &str {
        self.installed.id.as_str()
    }

    fn instancing(&self) -> Instancing {
        Instancing::Single
    }

    fn spec(&self, context: &Context) -> Result<ServiceSpecBuilder> {
        let _ = context;

        let Body::Service(template) = &self.installed.manifest.body else {
            return Err(Error::ExtensionNotAService {
                id: self.installed.id.as_str().to_owned(),
                kind: self.installed.kind().as_str(),
            });
        };

        render::service_builder(&self.installed.manifest, template, &self.context())
    }
}

/// The `[ports]` name an extension's service answers on, or [`None`] where it answers on none.
///
/// **The port its readiness check names**, and that is a rule rather than a guess: `services.port`
/// has one column and an extension may hold several, so something has to decide which of them is
/// *the* address. The one a readiness check watches is the one the service is up when it answers,
/// which makes it the number worth showing in `mix service list` and the one an idle probe has to
/// probe.
///
/// **A name rather than a number**, because the two maps this is looked up in are different: the
/// manifest's `[ports]` holds what was asked for and the row holds what was allocated, and an
/// install that read the first would write down a port it did not get.
///
/// [`None`] for a readiness check that is not a port at all — an executed command, a log pattern, a
/// url this cannot decompose — because a column left empty is honest and a guessed number is a
/// service reported at an address nothing listens on.
#[must_use]
pub fn served_port_name(manifest: &ExtensionManifest) -> Option<&str> {
    let Body::Service(template) = &manifest.body else {
        return None;
    };

    let named = match &template.ready {
        ReadyTemplate::Tcp { addr, .. } => addr.as_str(),
        ReadyTemplate::Http { url, .. } => url.as_str(),
        _ => return None,
    };

    // Every address in a manifest is `{listen}:{some_port}` — a literal host is refused at parse
    // (T80's D2) — so what is read here is the placeholder, never an address.
    let name = named
        .rsplit_once(":{")
        .and_then(|(_, rest)| rest.split('}').next())?;

    manifest.ports.contains_key(name).then_some(name)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use mixengine_proto::Timestamp;

    use super::*;
    use crate::extensions::manifest;
    use crate::extensions::store::Source;

    fn manifest_of(text: &str) -> ExtensionManifest {
        manifest::read(std::path::Path::new("extension.toml"), text).expect("a fixture parses")
    }

    /// **`services.port` is the port `ready` watches** — the T81 design's D8.
    #[test]
    fn the_served_port_is_the_one_ready_watches() {
        // Mailpit's readiness check is `{listen}:{ui_port}`.
        let manifest = manifest_of(mixengine_testkit::extension::MAILPIT);

        assert_eq!(served_port_name(&manifest), Some("ui_port"));
    }

    /// A kind that runs no process answers no port rather than the first one it can find.
    #[test]
    fn something_that_runs_nothing_has_no_port() {
        let manifest = manifest_of(mixengine_testkit::extension::SENDMAIL);

        assert_eq!(served_port_name(&manifest), None);
    }

    /// **The spec renders against the ports the row holds, not the ones the manifest asked for.**
    ///
    /// An extension whose wish was taken is moved to another number, and a spec built from the
    /// manifest would send it to bind the one it did not get.
    #[test]
    fn the_spec_renders_the_ports_that_were_allocated() {
        let manifest = manifest_of(mixengine_testkit::extension::MAILPIT);
        let mut ports = manifest.ports.clone();
        ports.insert("ui_port".to_owned(), 18_025);

        let recipe = ExtensionRecipe::new(Installed {
            id: manifest.extension.id.clone(),
            install_dir: PathBuf::from("/x/extensions/mailpit"),
            data_dir: PathBuf::from("/x/data/extensions/mailpit"),
            ports,
            manifest,
            source: Source::Registry,
            signed: true,
            installed_at: Timestamp::parse_rfc3339("2026-09-02T09:00:00Z").expect("a timestamp"),
        });

        let context = recipe.context();

        assert_eq!(context.ports.get("ui_port"), Some(&18_025));
        assert_eq!(context.install_dir, PathBuf::from("/x/extensions/mailpit"));
        assert_eq!(
            context.data_dir,
            PathBuf::from("/x/data/extensions/mailpit")
        );
    }
}
