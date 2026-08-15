//! What this build knows about running one kind of service.
//!
//! A `services` row says *that* MariaDB 11.4 is installed as `mariadb@main` on port 3306 with these
//! overrides. It cannot say what MariaDB **is**: which binary starts it, which file it reads, how to
//! tell whether it is up, what stops it cleanly. That knowledge is a [`Recipe`], and it is compiled
//! into this build rather than carried in the package index — the index describes a *download*
//! (`provides`, `requires`, a checksum), and a template that has to change with a MixEngine release
//! cannot be published by a pipeline that runs on a different schedule.
//!
//! **A recipe is looked up by `packages.name`**, which is the one thing a row and a recipe agree on.
//! `mariadb@main` and `mariadb@legacy` are two rows, two data directories and two ports; they are
//! one recipe, and the difference between them is entirely in the [`Context`] it is handed.
//!
//! **The set this build ships is [`Catalogue::builtin`], and each entry in it is a roadmap task of
//! its own** — Caddy is T31 and is in ([`recipes::caddy`](super::recipes::caddy)), php-fpm is T32,
//! MariaDB T33, PostgreSQL T34, Redis and Memcached T35 — because each is a template, a set of
//! overrides worth having and a first-start ritual, judged against the real server. What T30 owns is
//! everything around them: the merge, the render, the diff, the staging and the [`ServiceSpec`] that
//! comes out. A catalogue is a value, so a test — and a debug build with a fixture to supervise —
//! composes its own.
//!
//! [`ServiceSpec`]: mixengine_proto::ServiceSpec

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mixengine_proto::{ServiceId, ServiceSpecBuilder};
use serde::Serialize;

use super::document::{Document, Validator};
use super::settings::{Setting, Settings};
use crate::{Error, Result};

/// One file a recipe renders, and where it goes under `etc/<service-id>/`.
///
/// The source is a `&'static str` — `include_str!` for anything longer than a few lines — because a
/// template is part of the build and not part of the home: a user who could edit one would be
/// editing generated configuration by a slower route, and the file would then be a second place for
/// the truth to live. What they edit is an override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateFile {
    /// Where the rendering goes, relative to the service's configuration directory.
    pub path: &'static str,

    /// The template itself, in Jinja syntax.
    pub source: &'static str,
}

/// Everything a template and a [`Recipe`] are told about one service instance.
///
/// Assembled by [`Generator`](super::Generator) from the `services` row, the `packages` row it
/// points at and the home's layout, so that neither a template nor a recipe reads the database or
/// joins a path of its own.
///
/// The fields are `pub(super)` and there is no constructor: the only thing that may build one is
/// [`Generator`](super::Generator), which is the only thing that has read the row. A recipe reads it
/// through the accessors below.
#[derive(Debug, Clone)]
pub struct Context {
    /// Which service this is.
    pub(super) service: ServiceId,

    /// `packages.name` — the name this context's recipe was found under.
    pub(super) package: String,

    /// `packages.version`, as upstream writes it.
    pub(super) version: String,

    /// Where the package is unpacked: `packages/<name>/<version>/`.
    pub(super) install_path: PathBuf,

    /// `etc/<service-id>/`, where everything rendered goes.
    pub(super) etc: PathBuf,

    /// This instance's data directory, which is the user's and is never regenerated.
    pub(super) data: PathBuf,

    /// `run/`, for a socket or a pid file.
    pub(super) run: PathBuf,

    /// `logs/services/<service-id>/`.
    pub(super) logs: PathBuf,

    /// The port from the row, or [`None`] for a service that listens on a socket.
    pub(super) port: Option<u16>,

    /// The address it binds, `127.0.0.1` unless the row says otherwise.
    pub(super) bind: String,

    /// The recipe's defaults with the user's overrides applied.
    pub(super) settings: Settings,
}

impl Context {
    /// Which service this is.
    #[must_use]
    pub fn service(&self) -> &ServiceId {
        &self.service
    }

    /// The name this context's recipe was found under.
    #[must_use]
    pub fn package(&self) -> &str {
        &self.package
    }

    /// The installed version, as upstream writes it.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Where the package is unpacked.
    #[must_use]
    pub fn install_path(&self) -> &Path {
        &self.install_path
    }

    /// This instance's data directory.
    #[must_use]
    pub fn data(&self) -> &Path {
        &self.data
    }

    /// Where everything this recipe renders goes: `etc/<service-id>/`.
    ///
    /// It exists by the time a spec is built — [`Generator`](super::Generator) installs before it
    /// asks — which is what makes it the working directory a recipe reaches for when the service has
    /// no better one. A data directory is often the better one and is often not there yet: creating
    /// it is a first-start ritual (`mariadb-install-db`, `initdb`) and not a side effect of
    /// rendering a config file.
    #[must_use]
    pub fn etc(&self) -> &Path {
        &self.etc
    }

    /// `run/`, for a socket or a pid file.
    #[must_use]
    pub fn run(&self) -> &Path {
        &self.run
    }

    /// Where this service's output is written.
    #[must_use]
    pub fn logs(&self) -> &Path {
        &self.logs
    }

    /// The port from the row, or [`None`] for a service that listens on a socket.
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// The address it binds.
    #[must_use]
    pub fn bind(&self) -> &str {
        &self.bind
    }

    /// The recipe's defaults with the user's overrides applied.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Where a file this recipe renders ends up.
    ///
    /// What a [`Recipe::spec`] passes on a command line: the program is told to read the file the
    /// template produced, and neither half joins the path itself.
    #[must_use]
    pub fn config(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.etc.join(relative)
    }

    /// An executable inside the installed package, spelled the way this OS spells one.
    ///
    /// `caddy` here is `caddy.exe` on Windows, and a spec's `program` has to be the second on that
    /// machine or the supervisor is handed a path that does not exist. Recipes therefore never write
    /// the suffix, and no recipe carries a `#[cfg]`.
    #[must_use]
    pub fn program(&self, name: &str) -> PathBuf {
        self.install_path
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
    }

    /// This context as a template sees it.
    ///
    /// Four groups rather than one flat object, deliberately: a setting called `port` and the row's
    /// own port are different values that a template has to be able to tell apart, and flattening
    /// them would let a recipe shadow the row by declaring a setting with the wrong name.
    fn rendering(&self) -> Rendering<'_> {
        Rendering {
            service: Instance {
                id: self.service.as_str(),
                name: self.service.name(),
                instance: self.service.instance(),
                port: self.port,
                bind: &self.bind,
            },
            package: Origin {
                name: &self.package,
                version: &self.version,
                path: &self.install_path,
            },
            paths: Layout {
                etc: &self.etc,
                data: &self.data,
                run: &self.run,
                logs: &self.logs,
            },
            settings: &self.settings,
            extra: self.settings.extra(),
        }
    }
}

#[cfg(test)]
impl Context {
    /// A context for `service`, laid out under `root` as a home would lay it out.
    ///
    /// **The only thing besides [`Generator`](super::Generator) that may build one, and only in this
    /// crate's own tests.** It exists because of what a real recipe's [`Recipe::validator`] is: the
    /// service's own binary. Rendering through a generator runs `caddy validate`, so a test of the
    /// *template* would need fifty megabytes of Caddy installed to find out whether a variable name
    /// is misspelled — and would then be measuring Caddy. The real server judges the real thing in
    /// `crates/mixengine-daemon/tests/caddy.rs`; this is what keeps the cheap half cheap.
    pub(super) fn for_test(
        service: ServiceId,
        package: &str,
        root: &Path,
        port: Option<u16>,
        settings: Settings,
    ) -> Self {
        Self {
            etc: root.join("etc").join(service.as_str()),
            data: root.join("data").join(package),
            run: root.join("run"),
            logs: root.join("logs").join("services").join(service.as_str()),
            install_path: root.join("packages").join(package),
            package: package.to_owned(),
            version: "0.0.0".to_owned(),
            port,
            bind: "127.0.0.1".to_owned(),
            settings,
            service,
        }
    }
}

/// [`Context`] in the shape a template reads it.
#[derive(Debug, Serialize)]
struct Rendering<'a> {
    service: Instance<'a>,
    package: Origin<'a>,
    paths: Layout<'a>,
    settings: &'a Settings,
    /// Also `settings.extra`, and repeated at the top level because every template ends with it.
    extra: &'a str,
}

/// The `service` half of a [`Rendering`].
#[derive(Debug, Serialize)]
struct Instance<'a> {
    id: &'a str,
    name: &'a str,
    instance: Option<&'a str>,
    port: Option<u16>,
    bind: &'a str,
}

/// The `package` half.
#[derive(Debug, Serialize)]
struct Origin<'a> {
    name: &'a str,
    version: &'a str,
    path: &'a Path,
}

/// The `paths` half.
#[derive(Debug, Serialize)]
struct Layout<'a> {
    etc: &'a Path,
    data: &'a Path,
    run: &'a Path,
    logs: &'a Path,
}

/// How to configure and run one kind of service.
///
/// Implemented once per `packages.name`. Everything except [`spec`](Self::spec) has a default,
/// because a service with no configuration file of its own — Redis very nearly, Memcached entirely —
/// is a recipe that is only a command line.
pub trait Recipe: std::fmt::Debug + Send + Sync {
    /// The `packages.name` this recipe is for.
    fn package(&self) -> &'static str;

    /// Every override this recipe understands, and what each is when nobody has said.
    ///
    /// An override naming anything else is refused — see [`settings`](super::settings).
    fn settings(&self) -> &'static [Setting] {
        &[]
    }

    /// The files it renders into `etc/<service-id>/`.
    fn files(&self) -> &'static [TemplateFile] {
        &[]
    }

    /// The command that judges a rendering before it is installed, if there is one.
    ///
    /// Handed the [`Context`] because the checker is usually the service's own binary — `caddy
    /// validate`, `nginx -t` — which lives inside the package this instance was installed from.
    fn validator(&self, context: &Context) -> Option<Validator> {
        let _ = context;
        None
    }

    /// The service, as something the supervisor can run.
    ///
    /// A **builder** rather than a finished [`ServiceSpec`](mixengine_proto::ServiceSpec), because
    /// the parts of a spec that come from the row rather than from the recipe — the resource limits,
    /// today — are applied by [`Generator`](super::Generator) afterwards. A recipe that returned a
    /// finished spec could forget one, and the failure would be a limit silently not applied.
    ///
    /// # Errors
    ///
    /// Whatever this particular service cannot answer: a setting whose value is impossible, a
    /// dependency that is not a service id. A spec that does not *build* is not this method's error
    /// to report — the generator builds it.
    fn spec(&self, context: &Context) -> Result<ServiceSpecBuilder>;
}

/// The recipes a running daemon can find.
///
/// A value rather than a global, which is what makes the generator testable at all: a test composes
/// a catalogue holding one recipe of its own and never touches what this build ships.
#[derive(Debug, Clone, Default)]
pub struct Catalogue {
    recipes: BTreeMap<&'static str, Arc<dyn Recipe>>,
}

impl Catalogue {
    /// What this build knows how to run.
    ///
    /// One recipe so far, and the rest of `.claude/features/services.md`'s catalogue arrives one
    /// roadmap task at a time — php-fpm T32, MariaDB T33, PostgreSQL T34, Redis and Memcached T35 —
    /// because a template written before the server it configures is a guess nobody can check. A
    /// home whose `services` table names none of them is answered by this without a special case.
    #[must_use]
    pub fn builtin() -> Self {
        Self::default().with(Arc::new(super::recipes::Caddy))
    }

    /// The same catalogue, with `recipe` in it.
    ///
    /// A recipe for a `packages.name` that is already known **replaces** it, which is what lets a
    /// debug build put a fixture in front of a real service and a test put one in front of nothing.
    #[must_use]
    pub fn with(mut self, recipe: Arc<dyn Recipe>) -> Self {
        self.recipes.insert(recipe.package(), recipe);
        self
    }

    /// The recipe for a package, if this build has one.
    #[must_use]
    pub fn recipe(&self, package: &str) -> Option<&Arc<dyn Recipe>> {
        self.recipes.get(package)
    }

    /// Every package this catalogue can run, in name order.
    ///
    /// For the message a service belonging to something else produces.
    pub fn packages(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.recipes.keys().copied()
    }
}

/// Render every file `recipe` declares, for `context`.
///
/// # Errors
///
/// [`Error::TemplateBroken`], naming the file: a template is this build's, so this is a bug of ours
/// and not a configuration a user can fix — but which of a service's six files failed is the first
/// thing anybody needs to know.
pub(super) fn render(recipe: &dyn Recipe, context: &Context) -> Result<Vec<Document>> {
    let mut environment = minijinja::Environment::new();

    // A template that reads `{{ setings.port }}` renders an empty string by default, which is a
    // config file that is silently wrong — a port line with nothing after it. Strict makes it a
    // failure at the moment of rendering, with the name of the variable in it.
    environment.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);

    // Every one of these files is a config format where a trailing newline matters to somebody, and
    // Jinja's default is to eat the last one.
    environment.set_keep_trailing_newline(true);

    let rendering = minijinja::Value::from_serialize(context.rendering());

    recipe
        .files()
        .iter()
        .map(|file| {
            environment
                .render_str(file.source, &rendering)
                .map(|contents| Document::new(file.path, contents))
                .map_err(|source| Error::TemplateBroken {
                    service: context.service.as_str().to_owned(),
                    file: file.path,
                    source: Box::new(source),
                })
        })
        .collect()
}
