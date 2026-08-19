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

    /// What that install calls its executables, and where each one is inside the directory.
    ///
    /// `runtime_installs.provides_json`, and **empty for a service that came from a `packages`
    /// row** — see [`Context::provided`], which is the only thing that reads it.
    pub(super) provides: BTreeMap<String, String>,

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

    /// The paths this recipe computes that its own template also has to name.
    ///
    /// Filled by [`Generator`](super::Generator) from [`Recipe::endpoints`] before anything is
    /// rendered, so the file and the check the daemon makes read one value. See [`Endpoints`].
    pub(super) endpoints: Endpoints,

    /// The credentials this service's first-run ritual was given, by the key its recipe declared.
    ///
    /// **Empty everywhere except inside [`FirstRun::steps`]**, and never part of
    /// [`Context::rendering`]: a `my.cnf` with a root password in it would be a plaintext credential
    /// on disk, written by the very design that refuses one. There is a test.
    ///
    /// [`FirstRun::steps`]: super::first_run::FirstRun::steps
    pub(super) secrets: BTreeMap<String, String>,
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

    /// The executable this install publishes under `name`, wherever the publisher put it.
    ///
    /// [`program`](Self::program) is the other half of the pair and the right one for a package: it
    /// joins a name to the install path and lets this OS spell the suffix, which works because
    /// `mixengine-packages` publishes a server as one executable named after its package. **A
    /// runtime is the case where that is not true.** `php-fpm` is `sbin/php-fpm` inside a Unix
    /// build and does not exist at all inside a Windows one, where the same job is done by
    /// `php-cgi.exe` at the root — so a recipe that wrote either path down would be right on one
    /// system and wrong on the other. This looks the name up in the index's own answer, and the
    /// recorded value already carries whatever suffix it needs.
    ///
    /// # Errors
    ///
    /// [`Error::ServiceProvidesNothing`], naming the service and listing what the install does
    /// publish — which is the whole of what somebody looking at a PHP packed without a SAPI needs.
    pub fn provided(&self, name: &str) -> Result<PathBuf> {
        self.provides
            .get(name)
            .map(|relative| self.install_path.join(relative))
            .ok_or_else(|| Error::ServiceProvidesNothing {
                service: self.service.as_str().to_owned(),
                executable: name.to_owned(),
                known: self.provides.keys().cloned().collect(),
            })
    }

    /// Where this service listens on a Unix socket, if it does.
    ///
    /// Computed once by [`Recipe::endpoints`] and read back here, rather than joined again: the file
    /// and the daemon's own check have to name one path, and two places that build it are two places
    /// for it to drift.
    #[must_use]
    pub fn socket(&self) -> Option<&Path> {
        self.endpoints.socket.as_deref()
    }

    /// Where this package keeps its loadable plugins, for the one system that does not derive it.
    #[must_use]
    pub fn plugins(&self) -> Option<&Path> {
        self.endpoints.plugins.as_deref()
    }

    /// Where a credential of this service's lives inside the keyring's `mixengine` namespace.
    ///
    /// `<service-id>/<key>` — `mariadb@main/root`. The service id rather than the package name,
    /// because two instances of one server are two databases with two different passwords.
    ///
    /// **One composition, and that is the whole reason it is here.** A recipe names this entry in
    /// the [`EnvValue::Keyring`](mixengine_proto::EnvValue) its spec carries, and the daemon writes
    /// the generated value to it before the first step of the ritual runs; the failure when the two
    /// disagree is a server that starts and a client that cannot authenticate against it, reported
    /// as a service that never became ready.
    #[must_use]
    pub fn secret_address(&self, key: &str) -> String {
        format!("{}/{key}", self.service.as_str())
    }

    /// The credential this recipe declared under `key`, or an empty string when there is none.
    ///
    /// Empty rather than [`None`], because the only caller is a ritual's step builder and the only
    /// way to reach it with no secret is a recipe that declared none — which is a bug in the recipe,
    /// and shows up as a bootstrap that sets an empty password in the suite that drives a real
    /// server. A `Result` here would be a third failure path for a case a test already covers.
    #[must_use]
    pub fn secret(&self, key: &str) -> &str {
        self.secrets.get(key).map_or("", String::as_str)
    }

    /// Put the generated credentials in, which is [`FirstRun::steps`]'s doing and nobody else's.
    ///
    /// [`FirstRun::steps`]: super::first_run::FirstRun::steps
    pub(super) fn set_secrets(&mut self, secrets: BTreeMap<String, String>) {
        self.secrets = secrets;
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
                socket: self.endpoints.socket.as_deref(),
                plugins: self.endpoints.plugins.as_deref(),
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
        provides: BTreeMap<String, String>,
        port: Option<u16>,
        settings: Settings,
    ) -> Self {
        Self {
            etc: root.join("etc").join(service.as_str()),
            data: root.join("data").join(package),
            run: root.join("run"),
            logs: root.join("logs").join("services").join(service.as_str()),
            install_path: root.join("packages").join(package),
            provides,
            package: package.to_owned(),
            version: "0.0.0".to_owned(),
            port,
            bind: "127.0.0.1".to_owned(),
            settings,
            endpoints: Endpoints::default(),
            secrets: BTreeMap::new(),
            service,
        }
    }

    /// The endpoints a real render would have asked the recipe for.
    pub(super) fn with_endpoints(mut self, endpoints: Endpoints) -> Self {
        self.endpoints = endpoints;
        self
    }

    /// One credential, for the test that proves a template cannot see one.
    pub(super) fn put_secret(&mut self, key: &str, secret: &str) {
        self.secrets.insert(key.to_owned(), secret.to_owned());
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

    /// [`Endpoints::socket`], for a template that has to write it into a configuration file.
    socket: Option<&'a Path>,

    /// [`Endpoints::plugins`], likewise.
    plugins: Option<&'a Path>,
}

/// Paths a recipe computes that its own template also has to name.
///
/// Two so far, both MariaDB's, and both here for one reason: the alternative is a template joining a
/// path itself, and the failure when the file and the daemon's own check disagree is a service that
/// starts perfectly and is reported as never having come up.
#[derive(Debug, Clone, Default)]
pub struct Endpoints {
    /// Where this service listens on a Unix socket — [`None`] on a system without them, and for
    /// every service that listens on a port alone.
    pub socket: Option<PathBuf>,

    /// Where this package keeps its loadable plugins, for the one system that does not derive it.
    pub plugins: Option<PathBuf>,
}

/// How many instances of this package a home may have, which is what an id may look like.
///
/// **A recipe must answer**, which is why [`Recipe::instancing`] has no default body: the question
/// has a different answer for every server in `.claude/features/services.md`'s catalogue, and a
/// default here would be a decision made by whoever wrote this enum on behalf of a recipe nobody had
/// written yet. It is also the half of T36 that `service.create` cannot avoid — what a *second*
/// instance of one package means — while running two of them side by side stays T36's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instancing {
    /// Exactly one, and its id carries no `@`: there is one Caddy, and one active front end.
    Single,

    /// As many as are named, and every id carries one: `mariadb@main`, `mariadb@legacy`.
    Named,
}

/// Which table supplies the binary a recipe runs.
///
/// **A property of the recipe, not a rule in the daemon**, for [`Instancing`]'s reason: where
/// php-fpm's process comes from is a fact about php-fpm, and spelling it here is what lets both the
/// refusal in `service.create` and the hook that creates the pool derive from one answer instead of
/// from a string compared in two places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A `packages` row, put there by `package.install`, named by `service.create`.
    Package,

    /// A `runtime_installs` row of this kind, put there by `runtime.install` — which also creates
    /// the service, because a pool without a PHP is nothing and a PHP without a pool is a language
    /// no site can be served by. `service.create` refuses such a recipe and says which command to
    /// use instead.
    Runtime(mixengine_proto::RuntimeKind),
}

/// How to configure and run one kind of service.
///
/// Implemented once per `packages.name`. Everything except [`spec`](Self::spec) has a default,
/// because a service with no configuration file of its own — Redis very nearly, Memcached entirely —
/// is a recipe that is only a command line.
pub trait Recipe: std::fmt::Debug + Send + Sync {
    /// The `packages.name` this recipe is for.
    fn package(&self) -> &'static str;

    /// How many instances of this package a home may have. See [`Instancing`].
    fn instancing(&self) -> Instancing;

    /// Which table supplies the binary. See [`Source`].
    ///
    /// Defaulted, unlike [`instancing`](Self::instancing), because the answer *is* the same for
    /// every server the index publishes and only differs for the one recipe that runs out of a
    /// language.
    fn source(&self) -> Source {
        Source::Package
    }

    /// What proves an installed copy of this package actually runs here.
    ///
    /// Handed to [`Installer::install`](crate::install::Installer::install) after the archive is
    /// unpacked and before the staging directory is renamed into place, so a build that will not
    /// start on this machine leaves nothing behind. [`None`] for a package with nothing cheap to
    /// run — but a server almost always has one, and T20a's whole finding is that unpacking is not
    /// evidence that anything runs.
    ///
    /// The executable is named by its key in `Artifact::provides` rather than by a path: the path
    /// inside the archive belongs to whoever published it, and the name belongs to us.
    fn smoke_test(&self) -> Option<crate::install::SmokeTest> {
        None
    }

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

    /// The paths above, for the recipes that have any.
    ///
    /// Asked once by [`Generator`](super::Generator) and stored on the [`Context`], so a template
    /// and a [`spec`](Self::spec) read one answer instead of computing two.
    ///
    /// # Errors
    ///
    /// Whatever computing one costs — a socket path this kernel will not accept.
    fn endpoints(&self, context: &Context) -> Result<Endpoints> {
        let _ = context;

        Ok(Endpoints::default())
    }

    /// What must be done once, before this service is ever started — [`None`] for most.
    ///
    /// See [`first_run`](super::first_run) for the shape, and for why the credentials a ritual needs
    /// are declared here and generated by the daemon.
    fn ritual(&self) -> Option<super::first_run::Ritual> {
        None
    }
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
    /// Three recipes so far, and the rest of `.claude/features/services.md`'s catalogue arrives one
    /// roadmap task at a time — PostgreSQL T34, Redis and Memcached T35 — because a template written
    /// before the server it configures is a guess nobody can check. A home whose
    /// `services` table names none of them is answered by this without a special case.
    #[must_use]
    pub fn builtin() -> Self {
        Self::default()
            .with(Arc::new(super::recipes::Caddy))
            .with(Arc::new(super::recipes::Mariadb))
            .with(Arc::new(super::recipes::PhpFpm))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An absolute path on whichever system this is compiled for.
    const fn root() -> &'static str {
        if cfg!(windows) {
            r"C:\MixEngine"
        } else {
            "/opt/mixengine"
        }
    }

    /// **A secret never reaches a template.**
    ///
    /// [`Context::rendering`] is what a Jinja template sees, and the secret map is not part of it. A
    /// `my.cnf` with a root password in it would be a plaintext credential on disk written by the
    /// very design that refuses one, so this is a test and not a comment.
    #[test]
    fn a_template_cannot_see_a_secret() {
        let service = ServiceId::parse("mariadb@main").expect("an id");
        let settings = Settings::merge(&[], "{}", &service).expect("no settings, no overrides");
        let mut context = Context::for_test(
            service,
            "mariadb",
            Path::new(root()),
            BTreeMap::new(),
            Some(3306),
            settings,
        );
        context.put_secret("root", "hunter2");

        // Serialised through the very value the renderer hands to minijinja, so what is asserted is
        // what a template can reach and not what this test remembered to look at.
        let rendering =
            serde_json::to_string(&context.rendering()).expect("a rendering serialises");

        assert!(!rendering.contains("hunter2"), "{rendering}");
        assert!(!rendering.contains("secret"), "{rendering}");
        assert_eq!(context.secret("root"), "hunter2", "and a recipe still can");
    }
}
