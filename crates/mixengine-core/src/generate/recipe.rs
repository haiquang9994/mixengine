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
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mixengine_platform::PortBinding;
use mixengine_proto::{ServiceId, ServiceSpecBuilder};
use serde::Serialize;

use super::document::{Document, Validator};
use super::served::Served;
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

    /// `etc/` itself, for the one thing a recipe needs that is not its own directory.
    ///
    /// A pool has to name the ini set of the *runtime* it runs, which is generated per version and
    /// is not a file this recipe renders — see [`crate::runtimes::extensions`].
    pub(super) etc_root: PathBuf,

    /// This instance's data directory, which is the user's and is never regenerated.
    pub(super) data: PathBuf,

    /// `run/`, for a socket or a pid file.
    pub(super) run: PathBuf,

    /// `logs/services/<service-id>/`.
    pub(super) logs: PathBuf,

    /// The port from the row, or [`None`] for a service that listens on a socket.
    pub(super) port: Option<u16>,

    /// The port the *activator* listens on for this service — roadmap task **T70**.
    ///
    /// [`None`] for a service that listens on a socket, whose activator derives its address from
    /// the service's own, and for a row written before the column existed.
    pub(super) activation_port: Option<u16>,

    /// The address it binds, `127.0.0.1` unless the row says otherwise.
    pub(super) bind: String,

    /// The recipe's defaults with the user's overrides applied.
    pub(super) settings: Settings,

    /// The paths this recipe computes that its own template also has to name.
    ///
    /// Filled by [`Generator`](super::Generator) from [`Recipe::endpoints`] before anything is
    /// rendered, so the file and the check the daemon makes read one value. See [`Endpoints`].
    pub(super) endpoints: Endpoints,

    /// What this system makes a program bind to answer on each port a front end serves.
    ///
    /// **Data and not a `#[cfg]`**, which is the whole reason it arrives here: on macOS a front end
    /// binds 8080 to answer on 80, and `mixengine-core` may not know what system it is on. Filled by
    /// [`Generator`](super::Generator) from what the platform layer says, and read by a template
    /// through the `bound` filter.
    pub(super) bindings: Vec<PortBinding>,

    /// This home's certificate authority, as the public PEM — roadmap task **T75**.
    ///
    /// **Filled by [`Generator`](super::Generator), like [`bindings`](Self::bindings)**, and for the
    /// same reason: a recipe may not go looking for it. A front end renders it into a directory of
    /// its own so that a phone can install it and trust a shared site's certificate — the T75
    /// design, D9.
    ///
    /// **The bytes and not a path**, which is the whole of that decision. `certs/ca/root.key` sits
    /// beside `certs/ca/root.crt`, so a front end pointed at the certificates directory would serve
    /// this home's signing key to the local network. Rendering a copy means the directory a front
    /// end is pointed at holds exactly one file, and that is true by construction rather than by
    /// two directives agreeing.
    ///
    /// [`None`] on a home whose authority has not been generated.
    pub(super) authority: Option<String>,

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

    /// `etc/`, the root of everything generated.
    #[must_use]
    pub fn etc_root(&self) -> &Path {
        &self.etc_root
    }

    /// This home's certificate authority as a public PEM, when it has one — roadmap task **T75**.
    #[must_use]
    pub fn authority(&self) -> Option<&str> {
        self.authority.as_deref()
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

    /// The port the activator listens on for this service, where it needs one of its own.
    ///
    /// **A second column and not `port + 1`** — roadmap task **T70**, design D3. With pools on 9000
    /// and 9001 an arithmetic rule gives the first pool's activator the second pool's own port, and
    /// what a user sees is one service refusing to bind and a conflict reported about a number
    /// nobody chose. Allocated once by [`crate::services::ports`] and never computed again.
    #[must_use]
    pub fn activation_port(&self) -> Option<u16> {
        self.activation_port
    }

    /// The address it binds.
    #[must_use]
    pub fn bind(&self) -> &str {
        &self.bind
    }

    /// The port a program must bind to answer on `answering`.
    ///
    /// Itself on every system but macOS, and on macOS itself for everything but 80 and 443. A port
    /// nothing was asked about maps to itself, which is correct rather than a fallback: the mapping
    /// is only ever about the two the operating system reserves.
    #[must_use]
    pub fn bound(&self, answering: u16) -> u16 {
        bound(&self.bindings, answering)
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
                includes: &self.endpoints.includes,
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
            etc_root: root.join("etc"),
            data: root.join("data").join(package),
            run: root.join("run"),
            logs: root.join("logs").join("services").join(service.as_str()),
            install_path: root.join("packages").join(package),
            provides,
            package: package.to_owned(),
            version: "0.0.0".to_owned(),
            port,
            activation_port: None,
            bind: "127.0.0.1".to_owned(),
            settings,
            endpoints: Endpoints::default(),
            bindings: Vec::new(),
            authority: None,
            secrets: BTreeMap::new(),
            service,
        }
    }

    /// The version of the package this instance runs, which a recipe may branch on.
    ///
    /// MySQL is why: which program bootstraps a data directory is a fact about the *line*, and a
    /// test that could not vary it could only ever exercise one of three routes.
    pub(super) fn with_version(mut self, version: &str) -> Self {
        self.version = version.to_owned();
        self
    }

    /// The activator port a real render would have read off the row.
    ///
    /// A setter rather than a seventh argument to [`for_test`](Self::for_test): all but two of this
    /// crate's recipes have no activator, and a parameter every one of them passed [`None`] to
    /// would be ten call sites edited to say nothing.
    pub(super) fn with_activation_port(mut self, port: Option<u16>) -> Self {
        self.activation_port = port;
        self
    }

    /// The endpoints a real render would have asked the recipe for.
    pub(super) fn with_endpoints(mut self, endpoints: Endpoints) -> Self {
        self.endpoints = endpoints;
        self
    }

    /// The mapping a real render would have been given.
    pub(super) fn with_bindings(mut self, bindings: Vec<PortBinding>) -> Self {
        self.bindings = bindings;
        self
    }

    /// The authority a real render would have read off this home's certificates directory.
    pub(super) fn with_authority(mut self, authority: Option<String>) -> Self {
        self.authority = authority;
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

    /// [`Endpoints::includes`], which a template reads by name: `paths.includes['mime.types']`.
    includes: &'a BTreeMap<String, PathBuf>,
}

/// Paths a recipe computes that its own template also has to name.
///
/// Three so far — two MariaDB's, one nginx's — and all of them here for one reason: the alternative
/// is a template joining a path itself, and the failure when the file and the daemon's own check
/// disagree is a service that starts perfectly and is reported as never having come up.
#[derive(Debug, Clone, Default)]
pub struct Endpoints {
    /// Where this service listens on a Unix socket — [`None`] on a system without them, and for
    /// every service that listens on a port alone.
    ///
    /// **Two recipes read this two ways, and the field promises neither.** MariaDB puts the socket
    /// *file* here, because `socket = ` in `my.cnf` names a file. PostgreSQL puts the *directory*
    /// here, because `unix_socket_directories` takes a directory and the server creates
    /// `.s.PGSQL.<port>` inside it. Each is the convention of one recipe and its own template, which
    /// is why nothing outside that pair may assume either: a caller measuring this against
    /// [`within_socket_limit`](super::recipes) would be seventeen characters optimistic about
    /// PostgreSQL, and the recipe measures the file rather than the directory for exactly that
    /// reason.
    pub socket: Option<PathBuf>,

    /// Where this package keeps its loadable plugins, for the one system that does not derive it.
    pub plugins: Option<PathBuf>,

    /// Data files out of the package's own archive that the template `include`s by absolute path,
    /// keyed by the `provides` name the index publishes them under — roadmap task **T37**.
    ///
    /// **nginx is why, and it will not be the only one.** A generated `nginx.conf` sits in
    /// `etc/nginx/` with no `conf/` beside it, so `mime.types` has to be reached where the artifact
    /// keeps it — and Phase 4's sites need `fastcgi_params` from the same place. Resolved through
    /// [`Context::provided`], so a package that publishes neither fails while the recipe is being
    /// rendered, naming what the install does provide, rather than as an `include` of a file that
    /// is not there.
    pub includes: BTreeMap<String, PathBuf>,
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

/// What a service is *for*, where two packages can be for the same thing — roadmap task **T37**.
///
/// **[`Instancing`] cannot say this**, which is why there are two enums rather than one. Instancing
/// is about a package: how many rows may name `nginx`. This is about a *job*: `.claude/features/services.md`
/// says exactly one of Caddy and Nginx is the active front end, and both of them answering
/// [`Instancing::Single`] leaves a home with one of each — two programs that both own 80 and 443 the
/// moment sites arrive.
///
/// Only the one distinction, because only one exists: every other recipe in this catalogue is a
/// server that a home may run beside any of the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The program every site on the machine is reached through. Caddy and Nginx.
    FrontEnd,

    /// Everything else: a database, a cache, a pool. As many as the home wants.
    Other,
}

/// Where one service listens, for whoever else has to point at it.
///
/// **A value and not a string**, because the two shapes are spelled differently by every program
/// that consumes one: Caddy writes a socket as `unix//run/php-fpm-8.3.sock` and nginx writes the
/// same socket as `unix:/run/php-fpm-8.3.sock`. Each front end converts this in its own recipe,
/// which is the only place that spelling is knowledge about anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Upstream {
    /// A Unix domain socket, by absolute path.
    Socket(PathBuf),

    /// A TCP address, which on Windows is what a pool has instead.
    Tcp(SocketAddr),
}

/// Both addresses a site may point at for one service — roadmap task **T70**.
///
/// **One value rather than two maps**, because a site file names them together and in one order:
/// the service first, the activator second, so that a request the service refuses is retried
/// against whatever can start it. Two maps built beside each other could disagree about which
/// service an activator belongs to; this cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstreams {
    /// Where the service itself listens.
    pub listen: Upstream,

    /// Where the activator waits on its behalf, or [`None`] for a service nothing can start by
    /// connecting to it — which is every recipe but php-fpm today.
    pub activator: Option<Upstream>,
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

    /// What this service is for, where two packages can be for the same thing. See [`Role`].
    ///
    /// Defaulted to [`Role::Other`] because that is what a server *is* unless it is one of the two
    /// front ends: a recipe added later has to opt into the exclusivity rather than remember to opt
    /// out of it.
    fn role(&self) -> Role {
        Role::Other
    }

    /// Which table supplies the binary. See [`Source`].
    ///
    /// Defaulted, unlike [`instancing`](Self::instancing), because the answer *is* the same for
    /// every server the index publishes and only differs for the one recipe that runs out of a
    /// language.
    fn source(&self) -> Source {
        Source::Package
    }

    /// The port this service would like, and [`None`] for one the daemon hands no port to.
    ///
    /// **A wish, not a reservation** — roadmap task **T34c**. It is declared here, beside the binary
    /// and the template, because which number a product is documented under is a fact about the
    /// product: 3306 for either database, 6379 for Redis. A `service.create` that had to know them
    /// would be a caller that has to know the whole catalogue, and two recipes naming 3306 would be
    /// a special case rather than the ordinary one it is —
    /// [`Port::Allocate`](crate::services::Port::Allocate) gives the first row to ask its wish and
    /// the next the first free port above.
    ///
    /// [`None`] means the daemon allocates nothing: a pool on a Unix socket, and Caddy, whose 80 and
    /// 443 are its own settings — a front end moved to 81 because something else answered on 80 is
    /// not a front end anybody asked for.
    fn preferred_port(&self) -> Option<u16> {
        None
    }

    /// How to tell that this service has nothing to do — roadmap task **T69**.
    ///
    /// **The recipe's half of an [`IdlePolicy`], and `services.idle_minutes` holds the other.** Only
    /// the recipe knows which port its pool listens on or whether it renders a status endpoint, and
    /// a user has no way to check such a value and no reason to want it different: a probe that
    /// disagrees with the program it measures is a bug here, not a preference there. How *long* a
    /// machine's owner will keep something warm is theirs, and is the column.
    ///
    /// [`None`] means never idle-stopped whatever the row says, which is both front ends' answer —
    /// the thing that starts everything else back up cannot be the thing that gets stopped.
    ///
    /// [`IdlePolicy`]: mixengine_proto::IdlePolicy
    fn idle_probe(&self, context: &Context) -> Option<mixengine_proto::IdleProbe> {
        let _ = context;
        None
    }

    /// How long this service should look idle before it is stopped, when nobody has said.
    ///
    /// **A recipe answers a number only once something can start its service again.** Stopping a
    /// service nothing can wake is a site that answers 502 for ever, so each number arrives with
    /// the task that makes its service wakeable and never before it: php-fpm names half an hour
    /// (**T70** — the request that finds the pool down is what wakes it), the databases and the
    /// caches name an hour (**T70a** — the connection that finds the server down is what wakes it),
    /// and the two front ends answer [`None`] for ever, because the thing that starts everything
    /// else back up cannot be the thing that gets stopped.
    ///
    /// It exists now rather than with T70 so that `idle_minutes` can tell *nobody said* from
    /// *somebody said no* before either is reachable. Every existing row is `NULL`; if `NULL` also
    /// meant "never", a default turned on later would either ignore the person who switched it off
    /// or fail to reach the person who never touched it, and separating them afterwards means a
    /// migration that has to guess which of the two each `NULL` was.
    fn idle_default(&self) -> Option<mixengine_proto::Millis> {
        None
    }

    /// Whether a memory watchdog may restart this service — roadmap task **T71a**.
    ///
    /// **`false`, and a recipe opts in**, because whether a program survives being restarted under
    /// memory pressure is a property of the program and not a preference about it: a php-fpm pool
    /// loses the requests in flight, which `pm.max_requests` already recycles workers underneath; a
    /// database loses a transaction; a cache loses everything somebody believes is still there.
    ///
    /// **Unlike [`idle_default`](Self::idle_default), this is not overruled by a row**, and needs no
    /// three-state column to leave room for one: nothing about it is per-home. A person's control
    /// over the watchdog is `memory_mb` itself — nothing watches a service that declared no ceiling.
    /// The day somebody wants an override, it arrives as a column whose `NULL` means *what the
    /// recipe says*, and nothing stored has to be guessed at.
    fn restart_over_memory_default(&self) -> bool {
        false
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

    /// Where this service listens, for a *different* service's configuration to point at.
    ///
    /// [`None`] for everything that nothing points at, which is every recipe but php-fpm. The pool
    /// is why this exists: `fastcgi_pass` needs `run/php-fpm-<version>.sock` on Unix and
    /// `127.0.0.1:<row port>` on Windows, and both are already computed inside that recipe's own
    /// spec. A site template that worked either of them out again would be a second copy of a rule
    /// whose whole point is that it differs per system.
    ///
    /// # Errors
    ///
    /// Whatever computing one costs — a socket path this kernel will not accept, a Windows pool
    /// whose row carries no port.
    fn upstream(&self, context: &Context) -> Result<Option<Upstream>> {
        let _ = context;

        Ok(None)
    }

    /// Where the *activator* listens for this service, for a site file to name after
    /// [`upstream`](Self::upstream) — roadmap task **T70**.
    ///
    /// [`None`] for every recipe nothing can start by connecting to it, which is the default and is
    /// most of them. A recipe that answers [`Some`] is promising two things: the address differs
    /// from its own, and it is the same address on every render — a site file that moved when a pool
    /// stopped would make each idle stop reload the front end, which is a reload storm driven by the
    /// thing that exists to save work.
    ///
    /// **[`Some`] is not a promise that the daemon is listening there.** Whether it binds is the
    /// daemon's, and depends on why the service is stopped: a service a person stopped is not one a
    /// request may start again (design D8). What this answers is only *where*.
    ///
    /// # Errors
    ///
    /// Whatever computing one costs — for a socket, a home too deeply nested for the derived path,
    /// which is nine characters longer than the service's own and can cross `sockaddr_un`'s limit on
    /// a home that was just inside it.
    fn activator(&self, context: &Context) -> Result<Option<Upstream>> {
        let _ = context;

        Ok(None)
    }

    /// Whether this recipe's activator needs a port allocated onto the row — roadmap task **T70**.
    ///
    /// **A question about the recipe and this system, never about one instance**, which is why it
    /// takes no [`Context`]: the port has to be allocated before a context exists to render with.
    /// [`activator`](Self::activator) is what says *where*; this says only *whether a number is
    /// owed*, and answers `false` for a recipe whose activator derives its address from a socket
    /// path — there is nothing to allocate and nothing to take out of circulation.
    fn activation_port_needed(&self) -> bool {
        false
    }

    /// The addresses of this service's *own* that a connection may start it at — roadmap task
    /// **T70a**, design D4.
    ///
    /// Empty by default, and empty for php-fpm on purpose. A pool has a front end in front of it,
    /// so its activator gets a permanent address of its own ([`activator`](Self::activator)) and
    /// the site file names both. A database has nothing in front of it — a client dials
    /// `127.0.0.1:3306` and nothing else will do — so the daemon binds what the service itself
    /// listens on while the service is idle-stopped, and gives it back on the start.
    ///
    /// **More than one address, because a database has more than one.** On a system with Unix
    /// sockets MariaDB answers on a port *and* on a socket in `run/`, and which of the two a
    /// client uses is that client's habit rather than a setting: a generated `.env` names the
    /// port, `mariadb` typed with no host at all names the socket. A recipe answering only the
    /// port leaves the second client hanging against an address nothing holds.
    ///
    /// **This is the one place [`activator`](Self::activator)'s permanent address does not hold**,
    /// and it cannot: the address belongs to the service, so it is bound only while nothing is
    /// serving it. What that costs is the window between the release and the service's own bind,
    /// which is the service's start time — stated in `.claude/features/resource-isolation.md`
    /// rather than hidden.
    ///
    /// # Errors
    ///
    /// Whatever computing one costs — a socket path this kernel will not accept, or a row carrying
    /// no port for a service that has nothing else to be addressed by.
    fn held_while_stopped(&self, context: &Context) -> Result<Vec<Upstream>> {
        let _ = context;

        Ok(Vec::new())
    }

    /// Directories under `etc/<service-id>/` whose contents must be exactly what
    /// [`sites`](Self::sites) and [`files`](Self::files) render into them.
    ///
    /// Anything else in one is removed by [`install`](super::document::install), in the same
    /// operation and before the same reload. Only the two front ends declare one, and each declares
    /// `sites/`: without it a deleted site keeps the file it had, and a file in that directory is a
    /// site that goes on being served.
    ///
    /// **Nothing sweeps `etc/<service-id>/` itself.** A directory belonging to a service that was
    /// deleted is `service.delete`'s problem and is not made this one's by proximity.
    fn swept(&self) -> &'static [&'static str] {
        &[]
    }

    /// The site files this service serves, if it is the one every site is reached through.
    ///
    /// Asked only of the recipe holding [`Role::FrontEnd`], and appended to the set
    /// [`files`](Self::files) rendered — **not installed by a path of its own**. That is the whole
    /// arrangement: the checker judges a staging directory, so a site file written anywhere else
    /// would be invisible to `caddy validate` and present at run time, which is the one arrangement
    /// whose correctness cannot be checked before it is live.
    ///
    /// `context` is the *front end's* own, which is where a site block gets the port to listen on
    /// and the paths its includes resolve against.
    ///
    /// # Errors
    ///
    /// [`Error::TemplateBroken`] naming the site template: a template is this build's, so a refusal
    /// here is a bug of ours rather than a configuration a user can fix.
    fn sites(&self, context: &Context, served: &[Served]) -> Result<Vec<Document>> {
        let _ = (context, served);

        Ok(Vec::new())
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
    /// Eight recipes, which is `.claude/features/services.md`'s catalogue — arrived one roadmap task
    /// at a time, because a template written before the server it configures is a guess nobody can
    /// check. A home whose `services` table names none of them is answered by this without a special
    /// case.
    #[must_use]
    pub fn builtin() -> Self {
        Self::default()
            .with(Arc::new(super::recipes::Caddy))
            .with(Arc::new(super::recipes::Memcached))
            .with(Arc::new(super::recipes::Mariadb))
            .with(Arc::new(super::recipes::Mysql))
            .with(Arc::new(super::recipes::Nginx))
            .with(Arc::new(super::recipes::PhpFpm))
            .with(Arc::new(super::recipes::Postgres))
            .with(Arc::new(super::recipes::Redis))
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

/// What a program must bind to answer on `answering`, given this system's table.
fn bound(bindings: &[PortBinding], answering: u16) -> u16 {
    bindings
        .iter()
        .find(|binding| binding.answer == answering)
        .map_or(answering, |binding| binding.bind)
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

    // What a program has to bind to answer on a port, as a filter rather than a variable: the two
    // numbers a front end maps come from two different places — `service.port` is the row's and
    // `settings.https_port` is an override — and a variable would have to be added per place.
    // Identity on every system but macOS, and identity there for everything but 80 and 443.
    let bindings = context.bindings.clone();
    environment.add_filter("bound", move |port: i64| -> i64 {
        u16::try_from(port).map_or(port, |answering| i64::from(bound(&bindings, answering)))
    });

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

    /// **Sixty minutes for the databases and the caches, and only now** — the design's D9.
    ///
    /// A default that idles a service nothing can start again is a home that changed nothing and
    /// broke, so these five answer a number in the same commit that makes them wakeable and not one
    /// commit earlier. php-fpm has answered half an hour since **T70**, and the two front ends stay
    /// [`None`] for ever: the thing that starts everything else back up cannot be the thing that
    /// gets stopped.
    #[test]
    fn a_recipe_answers_an_idle_default_once_something_can_start_it_again() {
        let catalogue = Catalogue::builtin();

        let default_of = |package: &str| {
            catalogue
                .recipe(package)
                .unwrap_or_else(|| panic!("{package} is a builtin recipe"))
                .idle_default()
        };

        for package in ["mariadb", "mysql", "postgres", "redis", "memcached"] {
            assert_eq!(
                default_of(package),
                Some(mixengine_proto::Millis::from_secs(60 * 60)),
                "{package} was not turned on by the task that made it wakeable"
            );
        }

        assert_eq!(
            default_of("php-fpm"),
            Some(mixengine_proto::Millis::from_secs(30 * 60)),
            "the pool's own default moved"
        );

        for package in ["caddy", "nginx"] {
            assert_eq!(
                default_of(package),
                None,
                "{package} starts everything else back up and must never be stopped"
            );
        }
    }

    /// Every service that listens on a port names the one its product is documented under.
    ///
    /// **A wish belongs to the recipe, not to `service.create`.** Which port MySQL would like is a
    /// fact about MySQL, and a caller that had to know 3306 would be a caller that has to know
    /// every number in the catalogue. Caddy is the deliberate exception: 80 and 443 are its own
    /// settings, and a web server renumbered to 81 because something else answered on 80 is not a
    /// web server anybody asked for.
    #[test]
    fn a_recipe_that_listens_on_a_port_says_which_one_it_would_like() {
        let catalogue = Catalogue::builtin();

        let preferred = |package: &str| {
            catalogue
                .recipe(package)
                .unwrap_or_else(|| panic!("{package} is in the catalogue"))
                .preferred_port()
        };

        assert_eq!(preferred("mariadb"), Some(3306));
        assert_eq!(
            preferred("mysql"),
            Some(3306),
            "the two databases name one number, which is what the allocation is for"
        );
        assert_eq!(preferred("postgres"), Some(5432));
        assert_eq!(preferred("redis"), Some(6379));
        assert_eq!(preferred("memcached"), Some(11211));
        assert_eq!(preferred("php-fpm"), Some(9000));
        assert_eq!(
            preferred("caddy"),
            None,
            "a front end's ports are its own settings"
        );
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
