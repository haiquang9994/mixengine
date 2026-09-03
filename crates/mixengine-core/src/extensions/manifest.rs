//! The `extension.toml` manifest — roadmap task **T80**.
//!
//! **Its own type rather than a [`ServiceSpec`]** (the T80 design, D1), and not as a matter of
//! taste: `ServiceSpec` has sixteen fields where `[service]` has four, it has no `id` — the
//! manifest's author is not naming a service when they write `program` — and every path and address
//! in the file is a *template*. `{install_dir}/mailpit` is not a `PathBuf` any check would accept
//! and `{listen}:{ui_port}` is not a `SocketAddr` at all, so a `Deserialize` that succeeded on this
//! file would be one that had stopped checking. What T77 found for `mixengine.toml`, arriving a
//! second time.
//!
//! What *is* shared is the vocabulary — [`EnvValue`], [`StopBehaviour`], [`RestartPolicy`],
//! [`Millis`], [`VersionConstraint`] — which is what [ADR 0006] asked for, and which is why the rule
//! that a spec cannot express a secret by value costs this module nothing to obey: writing a
//! `value` beside `from = "keyring"` is refused by the type, here as everywhere else.
//!
//! Rendering the templates into the spec is [`super::render`].
//!
//! [`ServiceSpec`]: mixengine_proto::ServiceSpec
//! [ADR 0006]: https://github.com/mixnz/mixengine/blob/master/.claude/decisions/0006-servicespec-in-proto-and-secret-free.md

use std::collections::BTreeMap;
use std::path::Path;

use mixengine_proto::{
    EnvValue, ExtensionId, ExtensionKind, ExtensionPermissions, FrontEndServer, Millis,
    NetworkReach, PackageVersion, ReloadBehaviour, RestartPolicy, RuntimeKind, StopBehaviour,
    VersionConstraint,
};

use crate::{Error, Result};

/// The only schema this build writes, and the highest one it reads.
pub const SCHEMA: u32 = 1;

/// What the manifest is called, inside the directory that holds an extension.
pub const FILE_NAME: &str = "extension.toml";

/// The placeholders that are not ports.
///
/// The first three are the installer's (T80). The next four arrive with the one document that has a
/// database and a secret behind it — a `[web-app.config]`, roadmap task **T82**, the design's D5 and
/// D7 — and the last is roadmap task **T82a**'s, the one placeholder that renders the *name* of an
/// environment variable rather than a value. They are reserved here and not only in the renderer,
/// because a `[ports]` key becomes a placeholder: a port called `db_host` would be a manifest whose
/// own port shadowed the address it was pointed at.
const FIXED_PLACEHOLDERS: [&str; 8] = [
    "install_dir",
    "data_dir",
    "listen",
    "db_host",
    "db_port",
    "db_user",
    "db_password_env",
    "secret",
];

/// An extension, as its manifest says it.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionManifest {
    /// The format version.
    pub schema: u32,

    /// `[extension]`.
    pub extension: Header,

    /// `[artifact.<target>]`, keyed by the target word — `windows-x86_64`, or `any` for something
    /// that runs anywhere.
    pub artifacts: BTreeMap<String, Artifact>,

    /// `[ports]` — the ports it would like, each key its own placeholder.
    ///
    /// **Its own table rather than a field inside `[service]`**, because a port is an installer's
    /// concern: by the time a spec exists it has already been told which number to use. Nothing
    /// here reserves anything — allocation is T81's.
    pub ports: BTreeMap<String, u16>,

    /// `[permissions]`.
    pub permissions: ExtensionPermissions,

    /// Whichever of the four bodies `kind` names.
    pub body: Body,

    /// `[recipe]`, which may accompany any kind (the T80 design, D7).
    pub recipe: Option<RecipeTable>,
}

/// `[extension]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    /// What it is called everywhere, and the name of its directory.
    pub id: ExtensionId,

    /// The display name.
    pub name: String,

    /// Its own version, not MixEngine's.
    pub version: PackageVersion,

    /// What it is, which decides which tables are legal.
    pub kind: ExtensionKind,

    /// What it is for, in a sentence.
    #[serde(default)]
    pub description: String,

    /// Where to read about it.
    #[serde(default)]
    pub homepage: Option<String>,
}

/// One `[artifact.<target>]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    /// Where to fetch it. **Verified in T81**, not here — T80 downloads nothing.
    pub url: String,

    /// What it must hash to.
    pub sha256: String,

    /// How large it is, in bytes.
    ///
    /// **Optional here where the package index makes it mandatory** — roadmap task **T81**. The
    /// downloader uses it to stop a body that never ends before a disk is filled, which the
    /// checksum cannot do because it is only knowable once everything has arrived. A published
    /// extension should carry one; an author pointing `--path` at their own directory should not
    /// have to count bytes to try something, so an absent size falls back to a fixed ceiling
    /// (`extensions::install::UNDECLARED_SIZE_CEILING`) rather than to no bound at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// The four bodies, one per [`ExtensionKind`].
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// A supervised process. Boxed because it is three times the size of the next variant, and
    /// an enum is as big as its largest arm wherever one is held.
    Service(Box<ServiceTemplate>),

    /// Source served on an internal domain.
    WebApp(WebApp),

    /// Something MixEngine finds rather than runs.
    DesktopApp(DesktopApp),

    /// Nothing but `[recipe]` — which is held beside this rather than inside it, because it may
    /// accompany any kind.
    Recipe,
}

/// `[service]` — a [`ServiceSpec`](mixengine_proto::ServiceSpec) with the templates still in it.
///
/// **What an extension may declare is what its program *is*, never policy about the machine** (the
/// T80 design, D9): `limits` are the machine owner's (T68), an `idle` policy on something nothing
/// can wake is a service that stops for good (T69, T70), `logs` are per-home (T16), and
/// `depends_on` is an edge into a service graph the extension cannot see and a name it would have
/// to guess. Each of those takes the builder's own default — the same answer a compiled-in recipe
/// gets when it says nothing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceTemplate {
    /// The binary. Must grow from `{install_dir}`.
    pub program: String,

    /// The working directory. Must grow from `{install_dir}` or `{data_dir}`.
    pub cwd: String,

    /// Arguments, already split — never a command line to be parsed.
    #[serde(default)]
    pub args: Vec<String>,

    /// The child's environment. [`EnvValue`] whole, so ADR 0006's secret rule is inherited rather
    /// than restated.
    #[serde(default)]
    pub env: BTreeMap<String, EnvValue>,

    /// When traffic may be routed to it.
    pub ready: ReadyTemplate,

    /// Whether it is still fine once it is ready.
    #[serde(default)]
    pub health: Option<HealthTemplate>,

    /// What to do when it exits.
    #[serde(default)]
    pub restart: RestartPolicy,

    /// How to ask it to stop. The `command` form is refused — see [`read`].
    #[serde(default)]
    pub stop: StopBehaviour,

    /// How to hand it a changed configuration. The `command` form is refused — see [`read`].
    #[serde(default)]
    pub reload: Option<ReloadBehaviour>,
}

/// `ready`, with every address and path still a template.
///
/// Mirrors [`ReadyCheck`](mixengine_proto::ReadyCheck) variant for variant. A second type rather
/// than a borrow over the first, because the two are read at different moments: this is what a
/// person wrote, and that is what will be connected to.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReadyTemplate {
    /// Connect to an address.
    Tcp {
        /// `{listen}:{<port>}`.
        addr: String,
        /// How long to keep trying.
        timeout: Millis,
    },

    /// Connect to a socket, whose path must grow from a placeholder.
    UnixSocket {
        /// The socket path.
        path: String,
        /// How long to keep trying.
        timeout: Millis,
    },

    /// Fetch a URL, whose host must be `{listen}`.
    Http {
        /// `http://{listen}:{<port>}/…`.
        url: String,
        /// The status that means ready.
        expect_status: u16,
        /// How long to keep trying.
        timeout: Millis,
    },

    /// Wait for a line in its own output.
    LogPattern {
        /// The pattern.
        regex: String,
        /// How long to keep waiting.
        timeout: Millis,
    },

    /// It is ready once it has been running for a moment.
    PidAlive {
        /// How long it has to stay up before it counts.
        settle: Millis,
    },
}

/// `health`, with its probe still a template.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct HealthTemplate {
    /// What to ask.
    pub probe: HealthProbeTemplate,

    /// How often.
    pub interval: Millis,

    /// How long each ask may take.
    pub timeout: Millis,

    /// How many failures make it degraded.
    pub failures_before_degraded: u32,

    /// How many successes bring it back.
    pub successes_before_running: u32,
}

/// What a [`HealthTemplate`] asks, with the address or path still a template.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HealthProbeTemplate {
    /// Connect to an address.
    Tcp {
        /// `{listen}:{<port>}`.
        addr: String,
    },

    /// Connect to a socket.
    UnixSocket {
        /// The socket path.
        path: String,
    },

    /// Fetch a URL.
    Http {
        /// `http://{listen}:{<port>}/…`.
        url: String,
        /// The status that means healthy.
        expect_status: u16,
    },
}

/// `[web-app]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct WebApp {
    /// The document root. Must grow from `{install_dir}`.
    pub root: String,

    /// One label, placed under the internal domain by whoever generates the site.
    pub domain: String,

    /// The one file MixEngine generates for this application, and the text it is generated from.
    ///
    /// **The text lives here rather than inside the artifact** — roadmap task **T82**, the design's
    /// D1, which is where T80's `template = "config.inc.php.tmpl"` was overturned. That field said
    /// *a file inside the extension*, and for a registry install the extension's files **are**
    /// upstream's archive, verified against a hash upstream published: there is no step between the
    /// download and the rename where a file of ours could be added without making that hash a hash
    /// of something else. Measured on the real one — phpMyAdmin's `libraries/vendor_config.php`
    /// fixes `'configFile' => ROOT_PATH . 'config.inc.php'`, with no environment override — so the
    /// destination is inside the served root and the source has nowhere else to come from.
    ///
    /// **So an upgrade does not clobber what a person changed**: the generated file is ours and the
    /// settings inside it are theirs, which is the split every other generated file here takes. A
    /// manifest carries the `@include` of the user's own file, under `{data_dir}`, which outlives an
    /// uninstall; nothing here enforces that it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<WebAppConfig>,

    /// The database this application administers, where it needs one — the T82 design's D4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<WebAppDatabase>,

    /// Which language, and which versions of it will do.
    pub runtime: WebAppRuntime,
}

/// `[web-app.config]` — roadmap task **T82**, the design's D1.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct WebAppConfig {
    /// Where it goes, **relative to [`WebApp::root`]**. Never absolute, and never climbing out —
    /// checked at parse, where every other refusal about this format lives.
    pub path: String,

    /// What is written there, with the placeholders still in it. Rendered through
    /// [`render::php_source`](super::render::php_source), whose destination knows that a `{` it does
    /// not recognise is the language's own punctuation rather than a mistake.
    pub text: String,
}

/// `[web-app.database]` — roadmap task **T82**, the design's D4.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct WebAppDatabase {
    /// The engines this application can administer, **in order of preference**.
    ///
    /// The first one with a declared service on this machine is the one it is pointed at, and a
    /// machine with none of them is told so before anything is fetched — T81b's shape for the PHP,
    /// arriving a second time for the database.
    pub engines: Vec<String>,

    /// Whether this application is handed the server's **superuser password**, in its own php-fpm
    /// pool's environment — roadmap task **T82a**, the design's D2.
    ///
    /// **A boolean, because the variable's name is not an author's to write.** It arrives in
    /// [`CREDENTIAL_ENV`](super::render::CREDENTIAL_ENV), read out of the OS keyring by the
    /// supervisor at the moment the pool is spawned, and the manifest reaches it through
    /// `{db_password_env}` rather than by spelling it — so there is no name to collide with `PATH`
    /// or with `PHP_INI_SCAN_DIR`, and therefore no check anybody could forget. That is T80's D2
    /// (an extension cannot write an address, so nothing has to refuse one) applied to a second
    /// field.
    ///
    /// **Off by default**, because a missing value is silence and silence is not consent. Every
    /// surface that shows a plan names the account this would sign in as.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub signs_in: bool,
}

/// `[web-app.runtime]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct WebAppRuntime {
    /// The language.
    pub kind: RuntimeKind,

    /// Which versions of it will do.
    ///
    /// **A constraint and not a pin**: the version a web-app runs on is MixEngine's to choose, and
    /// deliberately not the user's project's — an administrative interface that broke because
    /// somebody pinned their project to an older PHP would be a tool that fails exactly when it is
    /// needed.
    pub requires: VersionConstraint,
}

/// `[desktop-app]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopApp {
    /// The URL scheme a handoff is written to. `mixdb`.
    pub scheme: String,

    /// How to find it, per OS.
    ///
    /// **Declared here, looked up by `mixengine-platform`'s `DesktopApps`** (T83): what belongs in
    /// the manifest is the name each OS looks it up by, and the lookup is the platform layer's.
    #[serde(default)]
    pub detect: DetectHints,
}

/// `[desktop-app.detect]` — one hint per OS, each in that OS's own currency.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct DetectHints {
    /// An executable's file name — `MixDB.exe` — looked for under App Paths and then in the
    /// uninstall table's `DisplayIcon`, case-insensitively.
    ///
    /// This once said "under App Paths" alone. T83 measured that Tauri's NSIS installer writes no
    /// App Paths entry at all and does write `Uninstall\<product>\DisplayIcon`, so the lookup reads
    /// both; the hint stays a file name either way.
    #[serde(default)]
    pub windows: Option<String>,

    /// A bundle identifier, asked of Spotlight.
    #[serde(default)]
    pub macos: Option<String>,

    /// A desktop entry file name, looked for in the XDG `applications/` directories.
    #[serde(default)]
    pub linux: Option<String>,
}

impl DetectHints {
    /// The hint for the system this build was compiled for, where the manifest gives one.
    #[must_use]
    pub fn here(&self) -> Option<&str> {
        let hint = match std::env::consts::OS {
            "windows" => self.windows.as_ref(),
            "macos" => self.macos.as_ref(),
            "linux" => self.linux.as_ref(),
            _ => None,
        };

        hint.map(String::as_str)
    }
}

/// `[recipe]` — what an extension adds to what MixEngine generates.
///
/// **Two forms, and both have a consumer named in the roadmap**: `php_ini` is T82's
/// `sendmail_path`, and `front_end` is the "extra Caddy directives" the feature document names. No
/// third form until something reads one.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeTable {
    /// Settings applied to every managed PHP.
    #[serde(default)]
    pub php_ini: Vec<PhpIniEntry>,

    /// Fragments added to the front end's configuration.
    #[serde(default)]
    pub front_end: Vec<FrontEndFragment>,
}

/// One `[[recipe.php_ini]]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct PhpIniEntry {
    /// The ini key.
    pub key: String,

    /// Its value, which may carry placeholders.
    pub value: String,
}

/// One `[[recipe.front_end]]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrontEndFragment {
    /// Which front end it is written for — roadmap task **T81c**, the design's D1.
    ///
    /// **Required, and not defaulted to either one.** The two configuration languages are not
    /// interchangeable, so a fragment with no server would be accepted by the parser and be a
    /// syntax error on half the machines that installed it.
    pub server: FrontEndServer,

    /// The directives, which may carry placeholders.
    pub fragment: String,
}

/// Enough of a manifest to refuse a schema this build does not read.
///
/// Read on its own first, the shape `blueprints::manifest::read` takes, so a manifest from a newer
/// build is refused by *version* rather than by whichever unknown key it happens to contain first.
#[derive(serde::Deserialize)]
struct Versioned {
    schema: u32,
    #[serde(default)]
    extension: Option<IdOnly>,
}

impl Versioned {
    /// Whether this build reads that schema.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownExtensionSchema`], naming the extension where the header got far enough to
    /// say which one it is.
    fn readable(self) -> Result<()> {
        if self.schema > SCHEMA {
            return Err(Error::UnknownExtensionSchema {
                id: self.extension.map(|header| header.id).unwrap_or_default(),
                schema: self.schema,
            });
        }

        Ok(())
    }
}

/// The id, where the header got that far. Not [`Header`], because a newer schema may have changed
/// everything else about it.
#[derive(serde::Deserialize)]
struct IdOnly {
    #[serde(default)]
    id: String,
}

/// The whole file, before `kind` has been checked against the tables beside it.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    schema: u32,
    extension: Header,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    artifact: BTreeMap<String, Artifact>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    ports: BTreeMap<String, u16>,
    #[serde(default)]
    permissions: ExtensionPermissions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    service: Option<ServiceTemplate>,
    #[serde(default, rename = "web-app", skip_serializing_if = "Option::is_none")]
    web_app: Option<WebApp>,
    #[serde(
        default,
        rename = "desktop-app",
        skip_serializing_if = "Option::is_none"
    )]
    desktop_app: Option<DesktopApp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recipe: Option<RecipeTable>,
}

impl From<&ExtensionManifest> for Raw {
    /// **The file's shape, back out of the checked type** — roadmap task **T81**, its design's D2.
    ///
    /// A registry entry is a manifest written as JSON, and the document is worth having only if it
    /// is the shape somebody could have written: `[service]` under `service`, `[web-app]` under
    /// `web-app`. Serialising [`Body`] itself would produce a key named after a Rust variant, which
    /// round-trips perfectly and is not a manifest.
    fn from(manifest: &ExtensionManifest) -> Self {
        let (service, web_app, desktop_app) = match &manifest.body {
            Body::Service(template) => (Some((**template).clone()), None, None),
            Body::WebApp(app) => (None, Some(app.clone()), None),
            Body::DesktopApp(app) => (None, None, Some(app.clone())),
            Body::Recipe => (None, None, None),
        };

        Self {
            schema: manifest.schema,
            extension: manifest.extension.clone(),
            artifact: manifest.artifacts.clone(),
            ports: manifest.ports.clone(),
            permissions: manifest.permissions.clone(),
            service,
            web_app,
            desktop_app,
            recipe: manifest.recipe.clone(),
        }
    }
}

/// Read an `extension.toml`.
///
/// Two passes, the shape `blueprints::manifest::read` takes: the schema is read on its own first,
/// so a manifest from a newer build is refused by *version* rather than by whichever unknown key it
/// happens to contain first.
///
/// `path` names the file for the message; nothing is read off disk here.
///
/// # Errors
///
/// [`Error::ExtensionManifest`] for a parse failure; [`Error::UnknownExtensionSchema`] for a newer
/// format; [`Error::ExtensionTableUnexpected`] and [`Error::ExtensionTableMissing`] where the tables
/// do not match `kind`; [`Error::ExtensionField`] for a field the format refuses;
/// [`Error::ExtensionIdTaken`] for an id a compiled-in recipe already claims.
pub fn read(path: &Path, text: &str) -> Result<ExtensionManifest> {
    let failed = |source: toml::de::Error| Error::ExtensionManifest {
        path: path.display().to_string(),
        source,
    };

    let versioned: Versioned = toml::from_str(text).map_err(failed)?;
    versioned.readable()?;

    let raw: Raw = toml::from_str(text).map_err(failed)?;
    checked(raw)
}

/// Read one entry of the published registry — roadmap task **T81**, its design's D2.
///
/// The same manifest in the same shape, arriving as JSON instead of TOML, and checked by the same
/// code [`read`] checks a file with rather than by a second set of rules: what a `--path` install
/// refuses is what an entry refuses.
///
/// # Errors
///
/// [`Error::ExtensionEntry`] when the entry is not a manifest at all, and everything [`read`]
/// reports about one that is.
pub fn read_value(entry: serde_json::Value) -> Result<ExtensionManifest> {
    let failed = |source: serde_json::Error| Error::ExtensionEntry {
        source: Box::new(source),
    };

    let versioned: Versioned = serde_json::from_value(entry.clone()).map_err(failed)?;
    versioned.readable()?;

    let raw: Raw = serde_json::from_value(entry).map_err(failed)?;
    checked(raw)
}

/// Write a manifest as the registry publishes it, in the shape its file has.
///
/// What the `extensions.manifest_json` column holds (D5) and what T81a will publish, out of one
/// rendering rather than two.
#[must_use]
pub fn to_value(manifest: &ExtensionManifest) -> serde_json::Value {
    serde_json::to_value(Raw::from(manifest))
        .expect("a manifest is made of strings, numbers and maps")
}

/// Everything a manifest is checked for once it has been read, whichever format it arrived in.
fn checked(raw: Raw) -> Result<ExtensionManifest> {
    let id = raw.extension.id.clone();
    let kind = raw.extension.kind;

    if crate::generate::Catalogue::builtin()
        .packages()
        .any(|package| package == id.as_str())
    {
        return Err(Error::ExtensionIdTaken {
            id: id.as_str().to_owned(),
        });
    }

    let body = body(&id, kind, raw.service, raw.web_app, raw.desktop_app)?;

    for name in raw.ports.keys() {
        placeholder_name(&id, name)?;
    }

    check_service(&id, &body)?;
    check_reach(&id, kind, raw.permissions.network)?;
    check_label(&id, &body)?;
    check_config(&id, &body)?;

    Ok(ExtensionManifest {
        schema: raw.schema,
        extension: raw.extension,
        artifacts: raw.artifact,
        ports: raw.ports,
        permissions: raw.permissions,
        body,
        recipe: raw.recipe,
    })
}

/// Pair `kind` with the one table it is allowed, refusing the other two.
fn body(
    id: &ExtensionId,
    kind: ExtensionKind,
    service: Option<ServiceTemplate>,
    web_app: Option<WebApp>,
    desktop_app: Option<DesktopApp>,
) -> Result<Body> {
    let present: [(&'static str, bool); 3] = [
        ("service", service.is_some()),
        ("web-app", web_app.is_some()),
        ("desktop-app", desktop_app.is_some()),
    ];

    let own = match kind {
        ExtensionKind::Service => Some("service"),
        ExtensionKind::WebApp => Some("web-app"),
        ExtensionKind::DesktopApp => Some("desktop-app"),
        ExtensionKind::Recipe => None,
    };

    for (table, is_present) in present {
        if is_present && own != Some(table) {
            return Err(Error::ExtensionTableUnexpected {
                id: id.as_str().to_owned(),
                kind: kind.as_str(),
                table,
            });
        }
    }

    let missing = |table: &'static str| Error::ExtensionTableMissing {
        id: id.as_str().to_owned(),
        kind: kind.as_str(),
        table,
    };

    match kind {
        ExtensionKind::Service => service
            .map(|template| Body::Service(Box::new(template)))
            .ok_or_else(|| missing("service")),
        ExtensionKind::WebApp => web_app.map(Body::WebApp).ok_or_else(|| missing("web-app")),
        ExtensionKind::DesktopApp => desktop_app
            .map(Body::DesktopApp)
            .ok_or_else(|| missing("desktop-app")),
        ExtensionKind::Recipe => Ok(Body::Recipe),
    }
}

/// The rules that are about `[service]` alone.
fn check_service(id: &ExtensionId, body: &Body) -> Result<()> {
    let Body::Service(template) = body else {
        return Ok(());
    };

    let refuse = |field: &str| {
        Err(Error::ExtensionField {
            id: id.as_str().to_owned(),
            field: field.to_owned(),
            reason: "may not be a command: a second program is a second path to render, and \
                     nothing yet needs one"
                .to_owned(),
        })
    };

    if matches!(template.stop, StopBehaviour::Command { .. }) {
        return refuse("stop");
    }

    if matches!(template.reload, Some(ReloadBehaviour::Command { .. })) {
        return refuse("reload");
    }

    Ok(())
}

/// `web-app` may not ask for the LAN (the T80 design, D8).
fn check_reach(id: &ExtensionId, kind: ExtensionKind, network: NetworkReach) -> Result<()> {
    if kind == ExtensionKind::WebApp && network == NetworkReach::Lan {
        return Err(Error::ExtensionField {
            id: id.as_str().to_owned(),
            field: "permissions.network".to_owned(),
            reason: "may not be `lan` for a web-app: these are administrative interfaces onto \
                     this machine's own databases, and the difference between one of them and a \
                     site somebody chose to share is that nobody chose"
                .to_owned(),
        });
    }

    Ok(())
}

/// `[web-app].domain` is **one label** — roadmap task **T81b**, the design's D4.
///
/// T80 said so and did not check it; the site that serves the label composes
/// `<label>.mixengine.<tld>`, so a dotted value would quietly become a deeper name. Refused at parse,
/// where every other refusal about this format lives.
fn check_label(id: &ExtensionId, body: &Body) -> Result<()> {
    let Body::WebApp(app) = body else {
        return Ok(());
    };

    let refuse = |reason: String| {
        Err(Error::ExtensionField {
            id: id.as_str().to_owned(),
            field: "web-app.domain".to_owned(),
            reason,
        })
    };

    if app.domain.contains('.') {
        return refuse(
            "is one label, placed under the internal domain by the site that serves it, so it may \
             not contain `.`"
                .to_owned(),
        );
    }

    let composed = format!(
        "{}.mixengine.{}",
        app.domain,
        mixengine_proto::domains::DEFAULT_TLD
    );

    match mixengine_proto::domains::domain_syntax(&composed) {
        Some(because) => refuse(format!("does not make a domain: {because}")),
        None => Ok(()),
    }
}

/// `[web-app.config].path` stays under the root, and `[web-app.database].engines` names something —
/// roadmap task **T82**, the design's D1 and D4.
///
/// **Joined rather than rendered**, which is why this check is here and not in
/// [`render::rooted`](super::render::rooted): the path carries no placeholders and is appended to a
/// root that has already been checked, so what is left to refuse is a value that would leave that
/// root — an absolute path, or one climbing out with `..`.
fn check_config(id: &ExtensionId, body: &Body) -> Result<()> {
    let Body::WebApp(app) = body else {
        return Ok(());
    };

    let refuse = |field: &str, reason: &str| {
        Err(Error::ExtensionField {
            id: id.as_str().to_owned(),
            field: field.to_owned(),
            reason: reason.to_owned(),
        })
    };

    if let Some(config) = &app.config {
        let path = Path::new(&config.path);

        if config.path.is_empty() {
            return refuse(
                "web-app.config.path",
                "is empty, and a generated file has to be called something",
            );
        }

        if path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        }) {
            return refuse(
                "web-app.config.path",
                "must stay under `web-app.root`: it is where MixEngine writes a file for this \
                 application, and an extension reaches the paths it was handed and no others",
            );
        }
    }

    if let Some(database) = &app.database
        && database.engines.is_empty()
    {
        return refuse(
            "web-app.database.engines",
            "is empty: a web-app that declares a database says which engines it can administer, \
             and a list naming none of them declares nothing",
        );
    }

    // **A manifest that asks for a database superuser's password has to read it** — roadmap task
    // **T82a**, the design's D2. The only justification for putting that credential into a process
    // is using it, so a `signs_in` nothing reads is a person agreeing to something for nothing.
    //
    // The one rule in this format that spans two tables, and it is here rather than in the renderer
    // because a renderer only ever sees the half it was handed.
    if app
        .database
        .as_ref()
        .is_some_and(|database| database.signs_in)
        && !app
            .config
            .as_ref()
            .is_some_and(|config| config.text.contains(super::render::PASSWORD_ENV))
    {
        return Err(Error::ExtensionField {
            id: id.as_str().to_owned(),
            field: "web-app.database.signs_in".to_owned(),
            reason: format!(
                "is set and no `[web-app.config].text` reads {}: a superuser password put into a \
                 process that never reads it is consent bought for nothing",
                super::render::PASSWORD_ENV
            ),
        });
    }

    Ok(())
}

/// A `[ports]` key becomes a placeholder, so it has to be spellable as one — and it may not be one
/// of the three the renderer already answers.
fn placeholder_name(id: &ExtensionId, name: &str) -> Result<()> {
    let refuse = |reason: &str| {
        Err(Error::ExtensionField {
            id: id.as_str().to_owned(),
            field: format!("ports.{name}"),
            reason: reason.to_owned(),
        })
    };

    if name.is_empty() {
        return refuse("is empty, and a port's name is its placeholder");
    }

    if FIXED_PLACEHOLDERS.contains(&name) {
        return refuse("is already a placeholder that means something else");
    }

    if !name.starts_with(|character: char| character.is_ascii_lowercase()) {
        return refuse("must start with a lowercase letter, because it is a placeholder name");
    }

    if !name.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
    }) {
        return refuse("may only contain lowercase letters, digits and `_`");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A registry entry is a manifest in JSON, and it is read by the same code that reads the
    /// file** — roadmap task **T81**, its design's D2.
    ///
    /// The document the registry publishes holds manifests rather than pointers to them, so a
    /// `--path` install and a registry install are proved by one parse. That only holds if what is
    /// written is the *file's* shape: an enum serialised by its Rust variant would round-trip
    /// perfectly and produce a document nobody could write by hand or check against a manifest.
    #[test]
    fn a_manifest_round_trips_through_json_in_the_file_s_own_shape() {
        let file = parse(mixengine_testkit::extension::MAILPIT).expect("mailpit parses");

        let json = to_value(&file);
        let again = read_value(json.clone()).expect("the entry reads back");

        assert_eq!(file, again);

        let object = json.as_object().expect("an object");
        assert!(
            object.contains_key("service"),
            "the table `kind` names: {json}"
        );
        assert!(object.contains_key("ports"), "{json}");
        assert!(
            object.contains_key("artifact"),
            "`[artifact.<target>]`, spelled as the file spells it: {json}"
        );
        assert!(
            !object.contains_key("body"),
            "a Rust enum leaked into the document: {json}"
        );
    }

    /// An entry from a schema this build does not read is refused by *version*, exactly as a file
    /// from one is — before anything decides which unknown key to complain about.
    #[test]
    fn an_entry_from_a_newer_schema_is_refused_by_version() {
        let mut json = to_value(&parse(mixengine_testkit::extension::MAILPIT).expect("parses"));
        json["schema"] = serde_json::Value::from(SCHEMA + 1);

        let refusal = read_value(json).expect_err("refused");

        assert!(
            matches!(refusal, Error::UnknownExtensionSchema { ref id, schema }
                if id == "mailpit" && schema == SCHEMA + 1),
            "{refusal}"
        );
    }

    /// The four fixtures are the four kinds, and each one parses into the body its `kind` names.
    #[test]
    fn every_kind_reads() {
        let mailpit = parse(mixengine_testkit::extension::MAILPIT).expect("mailpit parses");
        assert_eq!(mailpit.extension.kind, ExtensionKind::Service);
        assert!(matches!(mailpit.body, Body::Service(_)));

        let phpmyadmin =
            parse(mixengine_testkit::extension::PHPMYADMIN).expect("phpmyadmin parses");
        assert!(matches!(phpmyadmin.body, Body::WebApp(_)));

        let mixdb = parse(mixengine_testkit::extension::MIXDB).expect("mixdb parses");
        assert!(matches!(mixdb.body, Body::DesktopApp(_)));

        let sendmail = parse(mixengine_testkit::extension::SENDMAIL).expect("sendmail parses");
        assert!(matches!(sendmail.body, Body::Recipe));
    }

    /// **D7.** Mailpit is a supervised service *and* a php.ini change, in one extension, because
    /// two extensions for one product would be two things to install and uninstall in step.
    #[test]
    fn a_service_may_also_carry_a_recipe() {
        let mailpit = parse(mixengine_testkit::extension::MAILPIT).expect("parses");

        let recipe = mailpit.recipe.expect("mailpit carries a recipe");

        assert_eq!(recipe.php_ini.len(), 1);
        assert_eq!(recipe.php_ini[0].key, "sendmail_path");
    }

    /// A table belonging to another kind is somebody who believes their extension will be
    /// supervised. It is refused, not ignored.
    #[test]
    fn a_table_from_another_kind_is_refused() {
        let text = with_body(
            "desktop-app",
            "[desktop-app]\nscheme = \"probe\"\n\n[service]\nprogram = \"{install_dir}/x\"\ncwd = \"{data_dir}\"\nready = { type = \"pid_alive\", settle = \"1s\" }\n",
        );

        assert!(matches!(
            parse(&text),
            Err(Error::ExtensionTableUnexpected { .. })
        ));
    }

    /// And a kind with none of its own table is a manifest that says nothing about what it is.
    #[test]
    fn a_kind_without_its_table_is_refused() {
        let text = with_body("service", "");

        assert!(matches!(
            parse(&text),
            Err(Error::ExtensionTableMissing { .. })
        ));
    }

    /// Refused rather than half-read, for `UnknownBlueprintSchema`'s reason: a manifest whose
    /// unknown sections were skipped would install as something other than what its author wrote.
    #[test]
    fn a_newer_schema_is_refused() {
        let text = with_body("recipe", "[[recipe.php_ini]]\nkey = \"a\"\nvalue = \"b\"\n")
            .replace("schema = 1", "schema = 2");

        assert!(matches!(
            parse(&text),
            Err(Error::UnknownExtensionSchema { schema: 2, .. })
        ));
    }

    /// **D9.** A stop command is a second program to render, and a second place to repeat the
    /// path rule, for something no planned extension needs.
    #[test]
    fn a_stop_command_is_refused() {
        let text = with_body(
            "service",
            "[service]\nprogram = \"{install_dir}/x\"\ncwd = \"{data_dir}\"\nready = { type = \"pid_alive\", settle = \"1s\" }\nstop = { type = \"command\", program = \"{install_dir}/stop\", args = [], grace = \"5s\" }\n",
        );

        assert!(matches!(parse(&text), Err(Error::ExtensionField { .. })));
    }

    /// A key in `[ports]` becomes a placeholder, so it has to be spellable as one.
    #[test]
    fn a_port_name_must_be_a_placeholder_name() {
        let text = with_body(
            "service",
            "[ports]\n\"ui port\" = 8025\n\n[service]\nprogram = \"{install_dir}/x\"\ncwd = \"{data_dir}\"\nready = { type = \"pid_alive\", settle = \"1s\" }\n",
        );

        assert!(matches!(parse(&text), Err(Error::ExtensionField { .. })));
    }

    /// **D8.** These are administrative interfaces onto the machine's own databases. The
    /// difference between them and a site somebody chose to share is that nobody chose.
    #[test]
    fn a_web_app_may_not_ask_for_the_lan() {
        let text = mixengine_testkit::extension::PHPMYADMIN
            .replace("network = \"loopback\"", "network = \"lan\"");

        assert!(matches!(parse(&text), Err(Error::ExtensionField { .. })));
    }

    /// An id a compiled-in recipe already claims would be two definitions of one service.
    #[test]
    fn an_id_a_recipe_claims_is_refused() {
        let text = with_body("recipe", "[[recipe.php_ini]]\nkey = \"a\"\nvalue = \"b\"\n")
            .replace("id = \"probe\"", "id = \"mariadb\"");

        assert!(matches!(parse(&text), Err(Error::ExtensionIdTaken { .. })));
    }

    /// **T81c, D1.** A fragment says which configuration language it is written in, because a
    /// Caddyfile fragment in an `nginx.conf` is a syntax error and there is no answer to *"which of
    /// the two did the author mean"* that is not a guess.
    #[test]
    fn a_front_end_fragment_names_its_server() {
        let text = with_body(
            "recipe",
            "[[recipe.front_end]]\nserver = \"nginx\"\nfragment = \"map $a $b { default 0; }\"\n\n[[recipe.front_end]]\nserver = \"caddy\"\nfragment = \"(probe) { respond 204 }\"\n",
        );

        let manifest = parse(&text).expect("a manifest naming its servers");
        let recipe = manifest.recipe.expect("a recipe");

        assert_eq!(recipe.front_end.len(), 2);
        assert_eq!(recipe.front_end[0].server, FrontEndServer::Nginx);
        assert_eq!(recipe.front_end[1].server, FrontEndServer::Caddy);
    }

    /// A fragment with no server is a fragment that is a syntax error in one of the two places it
    /// could go.
    #[test]
    fn a_fragment_with_no_server_is_refused() {
        let text = with_body("recipe", "[[recipe.front_end]]\nfragment = \"a\"\n");

        assert!(matches!(parse(&text), Err(Error::ExtensionManifest { .. })));
    }

    /// And a third front end is a recipe, a package and a variant, in the task that adds one —
    /// never a string a manifest may write.
    #[test]
    fn a_server_this_build_does_not_ship_is_refused() {
        let text = with_body(
            "recipe",
            "[[recipe.front_end]]\nserver = \"apache\"\nfragment = \"a\"\n",
        );

        assert!(matches!(parse(&text), Err(Error::ExtensionManifest { .. })));
    }

    /// An unknown key is a setting somebody believes they made.
    #[test]
    fn an_unknown_key_is_refused() {
        let text = with_body(
            "recipe",
            "[[recipe.php_ini]]\nkey = \"a\"\nvalue = \"b\"\nnote = \"c\"\n",
        );

        assert!(matches!(parse(&text), Err(Error::ExtensionManifest { .. })));
    }

    /// **T81b, D4.** `[web-app].domain` is one label placed under `mixengine.test`; a dotted one
    /// would have become `pma.tools.mixengine.test`, and an uppercase one is not a label this
    /// system's domain syntax accepts.
    #[test]
    fn a_web_app_domain_is_one_label() {
        for (domain, expected) in [
            ("pma.tools", "may not contain `.`"),
            ("PMA", "lowercase ASCII"),
        ] {
            let text = with_body(
                "web-app",
                &format!(
                    "[web-app]\nroot = \"{{install_dir}}/app\"\ndomain = \"{domain}\"\n\n[web-app.runtime]\nkind = \"php\"\nrequires = \"^8.1\"\n"
                ),
            );

            let refusal = parse(&text).expect_err(domain);
            assert!(
                matches!(refusal, Error::ExtensionField { ref field, ref reason, .. }
                    if field == "web-app.domain" && reason.contains(expected)),
                "{domain}: {refusal}"
            );
        }

        let fine = with_body(
            "web-app",
            "[web-app]\nroot = \"{install_dir}/app\"\ndomain = \"pma\"\n\n[web-app.runtime]\nkind = \"php\"\nrequires = \"^8.1\"\n",
        );
        assert!(parse(&fine).is_ok());
    }

    /// **Every fixture is a manifest the roster publishes** — roadmap task **T82**, the design's
    /// D10.
    ///
    /// The claim `extension.rs` opens with is only worth having if something checks it, and what a
    /// published entry has to survive is this parse: `mixnz/mixengine-packages` renders each of
    /// these through [`read`] and refuses the run on anything this refuses.
    #[test]
    fn every_shipped_manifest_parses() {
        use mixengine_testkit::extension;

        for (name, text) in [
            ("mailpit", extension::MAILPIT),
            ("phpmyadmin", extension::PHPMYADMIN),
            ("adminer", extension::ADMINER),
            ("mixdb", extension::MIXDB),
            ("sendmail", extension::SENDMAIL),
        ] {
            let manifest = parse(text).unwrap_or_else(|error| panic!("{name}: {error}"));

            // And it survives the round trip a registry entry takes, which is how it is published.
            let again = read_value(to_value(&manifest))
                .unwrap_or_else(|error| panic!("{name} as an entry: {error}"));

            assert_eq!(manifest, again, "{name}");
        }
    }

    /// **Adminer is the one whose artifact is not an archive** — the design's D3 — and the one whose
    /// generated file is the application's entry point rather than a configuration beside it.
    #[test]
    fn adminer_is_one_file_and_a_generated_entry_point() {
        let manifest = parse(mixengine_testkit::extension::ADMINER).expect("adminer parses");

        let artifact = manifest.artifacts.get("any").expect("published anywhere");
        assert!(
            artifact.url.ends_with(".php"),
            "the distribution is one file: {}",
            artifact.url
        );

        let Body::WebApp(app) = &manifest.body else {
            panic!("a web-app");
        };
        let config = app.config.as_ref().expect("a generated file");

        assert_eq!(config.path, "index.php");
        assert!(
            config.text.contains("include __DIR__"),
            "a relative include is a bet on the working directory: {}",
            config.text
        );
    }

    /// A `web-app` with whatever tables the caller wants beside `[web-app]`.
    fn a_web_app(extra: &str) -> String {
        with_body(
            "web-app",
            &format!(
                "[web-app]\nroot = \"{{install_dir}}/pma\"\ndomain = \"pma\"\n\n\
                 [web-app.runtime]\nkind = \"php\"\nrequires = \"^8.0\"\n\n{extra}"
            ),
        )
    }

    /// **The text is the manifest's, because the artifact is upstream's** — roadmap task **T82**,
    /// the design's D1.
    ///
    /// For a registry install the extension's files *are* the published archive, verified against a
    /// hash its publisher wrote down. There is no step between the download and the rename where a
    /// file of ours could be added without making that hash a hash of something else — so the text
    /// of the one file MixEngine generates travels in the manifest.
    #[test]
    fn a_web_app_carries_the_text_of_its_configuration_and_the_database_it_administers() {
        let manifest = parse(&a_web_app(
            "[web-app.config]\npath = \"config.inc.php\"\n\
             text = \"<?php $cfg['t'] = '{data_dir}';\"\n\n\
             [web-app.database]\nengines = [\"mariadb\", \"mysql\"]\n",
        ))
        .expect("it parses");

        let Body::WebApp(app) = &manifest.body else {
            panic!("a web-app");
        };

        let config = app.config.as_ref().expect("a configuration");
        assert_eq!(config.path, "config.inc.php");
        assert!(config.text.contains("{data_dir}"), "{}", config.text);
        assert_eq!(
            app.database.as_ref().expect("a database").engines,
            ["mariadb", "mysql"]
        );
    }

    /// A generated file goes under the root the manifest declared, and `..` is how one would reach
    /// outside it — [`rooted`](super::super::render::rooted)'s rule, checked here because this path
    /// is joined rather than rendered.
    #[test]
    fn a_configuration_path_may_not_climb_out_of_the_root() {
        for path in ["../evil.php", "/etc/passwd", "a/../../b.php", ""] {
            let text = a_web_app(&format!(
                "[web-app.config]\npath = \"{path}\"\ntext = \"<?php\"\n"
            ));

            let refusal = parse(&text).expect_err("it is refused");

            assert!(
                matches!(&refusal, Error::ExtensionField { field, .. } if field == "web-app.config.path"),
                "{path}: {refusal}"
            );
        }
    }

    /// A `web-app` that declares a database says which engines it can administer, and an empty list
    /// says nothing at all.
    #[test]
    fn a_declared_database_names_at_least_one_engine() {
        let refusal = parse(&a_web_app("[web-app.database]\nengines = []\n"))
            .expect_err("an empty list is refused");

        assert!(
            matches!(&refusal, Error::ExtensionField { field, .. } if field == "web-app.database.engines"),
            "{refusal}"
        );
    }

    /// **The four placeholders T82 adds join the three that were already reserved.**
    ///
    /// A `[ports]` key becomes a placeholder, so a port called `db_host` would be a manifest whose
    /// own port shadowed the address it was pointed at.
    #[test]
    fn a_port_may_not_be_named_after_a_placeholder_that_already_means_something() {
        for name in FIXED_PLACEHOLDERS {
            let text = with_body("recipe", &format!("[ports]\n{name} = 1234\n"));

            assert!(parse(&text).is_err(), "{name} is already taken");
        }
    }

    /// [`read`] against a file name a message can name.
    fn parse(text: &str) -> Result<ExtensionManifest> {
        read(Path::new("probe").join(FILE_NAME).as_path(), text)
    }

    /// A manifest with a header of the given kind and whatever body the caller wants.
    fn with_body(kind: &str, body: &str) -> String {
        format!(
            "schema = 1\n\n[extension]\nid = \"probe\"\nname = \"Probe\"\nversion = \"1.0.0\"\nkind = \"{kind}\"\n\n{body}"
        )
    }

    /// **A manifest that asks for a superuser password has to read it** — roadmap task **T82a**,
    /// the design's D2. Consent that buys nothing is consent nobody should be asked for, and this
    /// is the one rule in this format that spans two tables.
    #[test]
    fn signing_in_without_reading_the_password_is_refused() {
        let refusal = parse(&signing_in("signs_in = true", "$cfg['h'] = '{db_host}';"))
            .expect_err("a manifest that never reads the password");

        let said = refusal.to_string();
        assert!(said.contains("web-app.database.signs_in"), "{said}");
        assert!(said.contains("db_password_env"), "{said}");
    }

    /// And one that does is accepted, with the flag on the parsed body.
    #[test]
    fn signing_in_is_read_off_the_database_table() {
        let manifest = parse(&signing_in(
            "signs_in = true",
            "$cfg['p'] = getenv('{db_password_env}');",
        ))
        .expect("it parses");

        let Body::WebApp(app) = &manifest.body else {
            panic!("a web-app");
        };
        assert!(app.database.as_ref().expect("a database").signs_in);
    }

    /// The default is off: a missing value is silence, and silence is not consent — which is the
    /// rule [`NetworkReach`](mixengine_proto::NetworkReach) already states for the other field a
    /// person is shown before an install.
    #[test]
    fn a_database_that_says_nothing_does_not_sign_in() {
        let manifest = parse(&signing_in("", "$cfg['h'] = '{db_host}';")).expect("it parses");

        let Body::WebApp(app) = &manifest.body else {
            panic!("a web-app");
        };
        assert!(!app.database.as_ref().expect("a database").signs_in);
    }

    /// A `[ports]` key may not shadow the placeholder that renders the variable's name.
    #[test]
    fn a_port_cannot_be_called_after_the_password_variable() {
        let refusal = parse(&with_body(
            "service",
            "[ports]\ndb_password_env = 8025\n\n[service]\nprogram = \"{install_dir}/p\"\n\
             cwd = \"{data_dir}\"\nready = { type = \"pid_alive\", settle = \"1s\" }\n",
        ))
        .expect_err("a port shadowing a fixed placeholder");

        assert!(refusal.to_string().contains("db_password_env"));
    }

    /// A `web-app` with one line of `[web-app.database]` and one of configuration text.
    fn signing_in(database_line: &str, config_line: &str) -> String {
        with_body(
            "web-app",
            &format!(
                "[web-app]\nroot = \"{{install_dir}}/pma\"\ndomain = \"pma\"\n\n\
                 [web-app.database]\nengines = [\"mariadb\"]\n{database_line}\n\n\
                 [web-app.runtime]\nkind = \"php\"\nrequires = \"^8.0\"\n\n\
                 [web-app.config]\npath = \"config.inc.php\"\ntext = \"\"\"\n\
                 <?php\n{config_line}\n\"\"\"\n"
            ),
        )
    }
}
