//! The domain: what a project, site, runtime and service *are*.
//!
//! Storage and platform access arrive as injected traits, so every rule in here — version
//! resolution, config rendering, blueprint diffing — is testable without touching the machine.
//! Modules are organised by capability (`sites/`, `runtimes/`, `certs/`), never by layer.
//!
//! `core` never depends on `daemon`.

#![warn(missing_docs)]

use std::path::{Path, PathBuf};

use mixengine_platform::Host;

pub mod blueprints;
pub mod certs;
pub mod config;
pub mod domains;
pub mod elevation;
pub mod extensions;
pub mod generate;
pub mod hosts;
pub mod index;
pub mod install;
pub mod jobs;
pub mod manifest;
pub mod metrics;
pub mod packages;
pub mod paths;
pub mod projects;
pub mod resolve;
pub mod runtimes;
pub mod services;
pub mod shims;
pub mod sites;
pub mod store;
pub mod updates;

pub use config::Config;
pub use paths::Paths;
pub use store::Store;

/// An opened MixEngine home: the user's preferences and the directory layout they produce.
#[derive(Debug, Clone)]
pub struct Home {
    /// What the user asked for in `config.toml`.
    pub config: Config,
    /// Where everything lives, with any `[paths]` override already applied.
    pub paths: Paths,
}

/// Open the MixEngine home directory: resolve it, read its configuration, create what is missing.
///
/// This is the first thing a daemon or CLI process does, and the only place the four steps are
/// ordered — they depend on each other. `config.toml` lives *inside* the root, so the root has to
/// be resolved and created before the configuration that may relocate `runtimes/` or `data/` can
/// be read at all.
///
/// `root_override` is `MIXENGINE_HOME` (or `--home`), already read at `main`; `None` means "use
/// what this OS considers the right place". Running it twice is a no-op — every step is
/// idempotent, which is what makes `mix doctor` able to reuse it.
///
/// # Errors
///
/// [`Error::Platform`] when the OS cannot say where user data belongs and no override was given,
/// or when a directory cannot be made private; [`Error::EmptyHome`] when the override is present
/// but empty; [`Error::Config`] when `config.toml` does not parse; and [`Error::Io`] when a
/// directory cannot be created.
pub fn open_home(root_override: Option<&Path>, host: &dyn Host) -> Result<Home> {
    let root = paths::resolve_root(root_override, host)?;
    paths::create_dir(&root)?;

    let config = config::load_or_create(&root.join(config::FILE_NAME))?;
    let paths = Paths::new(root, &config.paths);
    paths.bootstrap(host)?;

    Ok(Home { config, paths })
}

/// Failure of a domain operation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The requested entity does not exist.
    #[error("no such {kind}: {id}")]
    NotFound {
        /// The kind of entity, named as its RPC namespace: `"site"`, `"project"`, `"runtime"`,
        /// `"service"`, `"job"`, `"domain"`, `"blueprint"`, `"extension"`.
        ///
        /// Not free text. The daemon turns this into the hint `mix <kind> list`, which is a
        /// command only because the namespaces in `.claude/architecture/daemon-and-ipc.md` are
        /// also the nouns the CLI uses — a `kind` invented outside that list would send the user
        /// to a command that does not exist.
        kind: &'static str,
        /// The identifier that was looked up.
        id: String,
    },

    /// A file or directory under `MIXENGINE_HOME` could not be touched.
    ///
    /// The path is part of the message because "permission denied" on its own has never helped
    /// anybody: on a `[paths]` override pointing at an unmounted disk, the path *is* the answer.
    // The OS error is the `#[source]`, not part of this message: `anyhow` in the binaries and the
    // wire error at the daemon boundary both walk the chain, and a message that repeats its own
    // cause prints it twice.
    #[error("cannot {action} {}", path.display())]
    Io {
        /// What was being attempted, e.g. `"create"`.
        action: &'static str,
        /// The path it was attempted on.
        path: PathBuf,
        /// The underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// A blueprint manifest does not parse.
    ///
    /// No path: what failed to parse is the `manifest_toml` column, or a string on its way into it,
    /// and naming the rendered file beside it would point at something that is a copy rather than
    /// the thing that is wrong (D7).
    #[error("that blueprint is not a manifest this build can read")]
    BlueprintManifest {
        /// The parse failure, which carries the line and the column.
        #[source]
        source: toml::de::Error,
    },

    /// A blueprint was written by a build whose format this one does not know.
    ///
    /// Refused rather than half-read: a manifest whose unknown sections were skipped would apply as
    /// something other than what its author wrote down.
    #[error("the blueprint {name} is schema {schema}, which this build does not read")]
    UnknownBlueprintSchema {
        /// The blueprint's own name, where the file got that far.
        name: String,
        /// The schema it declares.
        schema: u32,
    },

    /// A project holds more than one site, and a blueprint describes one.
    ///
    /// Refused rather than reduced: capturing the first site would lose the others without saying
    /// so, and `[[sites]]` is a widening of the manifest format rather than something to guess at
    /// now. The domains are here because "this project has two sites" sends somebody hunting.
    #[error("{project} has {} sites ({}), and a blueprint describes one", domains.len(), domains.join(", "))]
    ProjectHasSeveralSites {
        /// The project's name.
        project: String,
        /// Each site's primary domain.
        domains: Vec<String>,
    },

    /// A blueprint name is not a slug, and a slug is what a filename stem can be made of.
    ///
    /// The refusal is the security boundary: the name is joined onto `blueprints/`, so `../../x`
    /// would write outside the home. Nothing about the join makes it safe — this does.
    #[error("{name} cannot be a blueprint name: {reason}")]
    InvalidBlueprintName {
        /// What was asked for.
        name: String,
        /// Which rule it broke, in the words the user is shown.
        reason: &'static str,
    },

    /// Something is already filed under that name.
    #[error("a blueprint called {name} is already here")]
    BlueprintExists {
        /// The slug.
        name: String,
    },

    /// A database or account name is not one this build will put in a statement.
    ///
    /// **The refusal is the boundary** — roadmap task **T77a**. Every statement quotes its
    /// identifiers and nothing escapes them, which is safe for exactly one reason: this refused
    /// every character that could end a quoted identifier.
    #[error("{name} cannot be a database or account name: {reason}")]
    InvalidDatabaseName {
        /// What was asked for.
        name: String,
        /// Which rule it broke, in the words the user is shown.
        reason: &'static str,
    },

    /// An account of this name is on the server and MixEngine holds no credential for it.
    ///
    /// **A keyring entry is the deed of ownership** — the T77a design, D3. Without this refusal,
    /// "make sure the account exists" would be an `ALTER USER` that silently resets the password of
    /// an account somebody else made.
    #[error(
        "an account called {user} already exists on {service}, and MixEngine has no credential for \
         it"
    )]
    AccountNotOurs {
        /// Which instance.
        service: String,
        /// Which account.
        user: String,
    },

    /// This package has no databases to make.
    #[error("{package} has no databases: nothing on it can be created that way")]
    NoDatabaseVocabulary {
        /// The `packages.name` that was asked.
        package: String,
    },

    /// A `blueprints` row holds a source word this build does not know.
    ///
    /// Unreachable through our own writes, so it means a hand-edited database or a row written by a
    /// build that knew a fourth source — and answering a listing with a guess about where a
    /// blueprint came from is the wrong direction to be wrong in, since T78a's trust marking is
    /// what reads it.
    #[error("the blueprint {name} is stored as {value}, which is not a source")]
    UnknownBlueprintSource {
        /// The slug.
        name: String,
        /// What is in the column.
        value: String,
    },

    /// A blueprint's signature does not verify against the gallery key — roadmap task **T78a**.
    ///
    /// Not a refusal on its own: an import whose signature does not check out lands *untrusted*
    /// rather than being thrown away, because a stale signature is a fact about the file and not a
    /// reason to lose it. The variant exists so that what happened can be said, in the log and in a
    /// client's message, rather than folded into a silent `false`.
    #[error("this blueprint is not signed by the gallery key")]
    BlueprintSignature {
        /// What the verifier said.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The gallery key compiled into this binary is not a public key.
    ///
    /// [`Error::IndexKey`]'s twin: a broken build and nothing a user can act on, reported rather
    /// than unwrapped because nothing in this crate panics.
    #[error("this build's blueprint gallery key is not a valid minisign key")]
    BlueprintKey {
        /// What the parser said.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// An `extension.toml` does not parse — roadmap task **T80**.
    ///
    /// **The path is here**, unlike [`Error::BlueprintManifest`], because there is a file to point
    /// at: an extension is read off disk and never out of a column.
    #[error("{path} is not an extension manifest this build can read")]
    ExtensionManifest {
        /// The file that was read.
        path: String,
        /// The parse failure, which carries the line and the column.
        #[source]
        source: toml::de::Error,
    },

    /// One entry of the published registry is not a manifest this build can read — roadmap task
    /// **T81**.
    ///
    /// **No path, where [`Error::ExtensionManifest`] has one**: this arrived inside a signed
    /// document, so there is no file to point somebody at. It is also the one manifest failure that
    /// is usually *not* shown to anybody — the listing skips such an entry and counts it, because
    /// one entry a newer build published should cost that entry and not the whole registry.
    #[error("a registry entry is not an extension manifest this build can read")]
    ExtensionEntry {
        /// The parse failure.
        #[source]
        source: Box<serde_json::Error>,
    },

    /// Something is already installed under that id — roadmap task **T81**.
    ///
    /// Named rather than left to the primary key, on [`Error::PackageAlreadyRecorded`]'s reasoning:
    /// the collision is a real case and the sentence a person needs names the extension and the way
    /// out, which is `mix extension uninstall` and not a constraint.
    #[error("{id} is already installed")]
    ExtensionAlreadyInstalled {
        /// What is already there.
        id: String,
    },

    /// An `extensions` row holds something this build cannot read back.
    ///
    /// [`Error::UnreadableServiceRow`]'s twin one table across, and it exists for the same reason:
    /// a hand-edited database, or one written by a later MixEngine, is answered by naming the row
    /// and the column rather than by a failure somewhere downstream that mentions neither.
    #[error("the {column} of extension {extension} cannot be read: {value}")]
    UnreadableExtensionRow {
        /// Which extension's row.
        extension: String,
        /// Which column.
        column: &'static str,
        /// What it held.
        value: String,
    },

    /// A `services` row points at an extension that runs no process — roadmap task **T81**.
    ///
    /// Unreachable through this build's own writes, since only a `service` extension is given a
    /// row. Named rather than defaulted, on [`Error::UnreadableServiceRow`]'s reasoning: a hand-
    /// edited database is answered by saying what is wrong with it.
    #[error("{id} is a {kind} extension and runs no process")]
    ExtensionNotAService {
        /// Which extension.
        id: String,
        /// What it actually is.
        kind: &'static str,
    },

    /// Nothing is published for this machine — roadmap task **T81**.
    ///
    /// A state rather than a fault: the extension exists and was simply not built for this OS and
    /// architecture, so the message names what *was* built rather than implying something is
    /// broken.
    #[error("{id} publishes no artifact for this machine; it has: {}", targets.join(", "))]
    ExtensionNoArtifact {
        /// Which extension.
        id: String,
        /// The targets it does publish.
        targets: Vec<String>,
    },

    /// A `web-app` administers a database and this home runs none of the engines it named — roadmap
    /// task **T82**.
    ///
    /// **Refused before the download**, for [`Error::RuntimeUnresolved`]'s reason: an install whose
    /// stated effect cannot happen is worse than one that never started. Like that variant, the
    /// message names what would satisfy it rather than implying something is broken — nothing is.
    #[error(
        "{id} administers a database and this machine runs none of: {}",
        engines.join(", ")
    )]
    ExtensionNoDatabase {
        /// Which extension.
        id: String,
        /// The engines it can administer, in the order it declared them.
        engines: Vec<String>,
    },

    /// A site naming the php-fpm pool that belongs to an extension — roadmap task **T82a**, its
    /// design's D5.
    ///
    /// **Raised by [`crate::sites`] rather than by the daemon**, because `blueprint.apply` and
    /// `domain.add` reach `sites::update` without going through a CLI, and a refusal they could
    /// cross is no refusal — which is T81b's D6 arriving at a second field.
    ///
    /// What it protects is the whole reason a `web-app` has a pool of its own: that process holds a
    /// database superuser's password, read from the keyring at spawn, and a project's PHP inside it
    /// could read the same variable.
    #[error("{pool} belongs to the {extension} extension and serves nothing else")]
    ExtensionPoolNotShared {
        /// The pool that was named.
        pool: String,
        /// The extension it belongs to.
        extension: String,
    },

    /// An extension written by a build whose format this one does not know.
    ///
    /// Refused rather than half-read, for [`Error::UnknownBlueprintSchema`]'s reason: a manifest
    /// whose unknown sections were skipped would install as something other than what its author
    /// wrote down.
    #[error("the extension {id} is schema {schema}, which this build does not read")]
    UnknownExtensionSchema {
        /// The extension's own id, where the file got that far.
        id: String,
        /// The schema it declares.
        schema: u32,
    },

    /// A table that belongs to a different kind of extension.
    ///
    /// Refused rather than ignored: a `[service]` table under `kind = "desktop-app"` is somebody
    /// who believes their extension will be supervised, and a key silently dropped is a belief
    /// nothing corrects.
    #[error("a {kind} extension has no [{table}] table, and {id} declares one")]
    ExtensionTableUnexpected {
        /// Whose manifest.
        id: String,
        /// What it said it is.
        kind: &'static str,
        /// The table it should not have.
        table: &'static str,
    },

    /// A kind with none of its own table.
    #[error("a {kind} extension needs a [{table}] table, and {id} has none")]
    ExtensionTableMissing {
        /// Whose manifest.
        id: String,
        /// What it said it is.
        kind: &'static str,
        /// The table it is missing.
        table: &'static str,
    },

    /// A field in an `extension.toml` says something the format does not allow.
    ///
    /// **One variant for every such refusal** — an address written out, a path that does not grow
    /// from a placeholder, an unknown placeholder, a stop command, a `web-app` asking for the LAN.
    /// What whoever wrote the file needs is the field and the sentence; a variant per rule would be
    /// a vocabulary to keep in step with the rules rather than with the file.
    #[error("{id}: {field} {reason}")]
    ExtensionField {
        /// Whose manifest.
        id: String,
        /// Which field, spelled the way the file spells it.
        field: String,
        /// What is wrong with it.
        reason: String,
    },

    /// An extension id a compiled-in recipe already claims.
    ///
    /// A `service` extension's process is named by its id, so this one would be two definitions of
    /// one service. Said here rather than discovered when T81 writes the row.
    #[error("{id} is the name of a service MixEngine already defines")]
    ExtensionIdTaken {
        /// The id.
        id: String,
    },

    /// A rendered extension manifest is not a usable service.
    ///
    /// Reported against the extension rather than against a spec nobody wrote: the file is the
    /// thing its author can fix.
    #[error("{id} does not describe a service that could run")]
    ExtensionSpec {
        /// Whose manifest.
        id: String,
        /// What `ServiceSpec::validate` said.
        #[source]
        source: mixengine_proto::SpecError,
    },

    /// A `minisign.pub` handed to the generator is not a public key file — roadmap task **T81a**.
    ///
    /// Distinct from [`Error::RegistryKeyMismatch`] because nothing was compared: `minisign -G`
    /// writes an untrusted comment and one key line, and a file with any other number of them is
    /// one nothing can be said about.
    #[error("{path}: expected an untrusted comment and one key line, found {lines}")]
    RegistryPublicKeyShape {
        /// The file that was read.
        path: String,
        /// How many non-empty lines it held.
        lines: usize,
    },

    /// The key the packaging repository would sign with is not the key this build checks against —
    /// roadmap task **T81a**, its design's D3.
    ///
    /// The load-bearing refusal of the whole publishing chain. A signature made with a key no
    /// installed MixEngine accepts is worse than no signature, because it looks published — so a
    /// half-finished key rotation is a red run rather than a document at a stable URL that nothing
    /// can read. Rotating the index key is an application release: the MixEngine carrying the new
    /// key goes out first.
    #[error(
        "{path} is not the key this build checks against:
    committed: {committed}
    compiled: {compiled}"
    )]
    RegistryKeyMismatch {
        /// The file that was read.
        path: String,
        /// What that file holds.
        committed: String,
        /// [`index::PUBLIC_KEY`], which is what an installed MixEngine verifies against.
        compiled: &'static str,
    },

    /// A manifest file is not named after the id it declares — roadmap task **T81a**.
    ///
    /// The one rule the registry generator adds that [`extensions::manifest::read`] cannot have,
    /// because that reads one file and knows nothing about the directory around it. It is also what
    /// makes a repeated id impossible: a directory holds one `mailpit.toml`, so `<id>.toml` is the
    /// roster's uniqueness and not only its tidiness.
    #[error("{path} declares the id {id}, so it should be named {id}.toml")]
    ExtensionFileName {
        /// The file as it is named.
        path: String,
        /// The id it declares.
        id: String,
    },

    /// A generated registry holds an entry the build that generated it cannot read back — roadmap
    /// task **T81a**.
    ///
    /// T81's D4 makes an unreadable entry survivable on a user's machine on purpose, because an
    /// entry a *newer* build published should cost that entry and nothing else. Here it can only
    /// mean the generator is older than its own inputs, and the honest place to stop is before the
    /// signature rather than on somebody's machine.
    #[error("{count} generated entries cannot be read back by the build that made them")]
    RegistryUnreadable {
        /// How many.
        count: usize,
    },

    /// `config.toml` is not valid.
    ///
    /// Unknown keys count: a silently ignored typo is a setting the user believes is in effect.
    #[error("{} is not valid configuration", path.display())]
    Config {
        /// The configuration file that failed to parse.
        path: PathBuf,
        /// The parse failure, which carries the line, the column and the accepted keys.
        #[source]
        source: toml::de::Error,
    },

    /// The database could not be opened or read.
    ///
    /// Shaped like [`Error::Io`] and for the same reason: the path is the answer often enough —
    /// a `[paths]` root on a disk nobody mounted, a home directory copied from another account —
    /// that leaving it out would waste the user's afternoon.
    #[error("cannot {action} the database at {}", path.display())]
    Database {
        /// What was being attempted, e.g. `"open"`.
        action: &'static str,
        /// The database file.
        path: PathBuf,
        /// What SQLite said.
        #[source]
        source: sqlx::Error,
    },

    /// The copy that has to exist before a migration could not be written.
    ///
    /// Its own variant rather than an [`Error::Io`] because of what happens next: the upgrade is
    /// abandoned. A migration is the one operation here that can destroy declared state, and
    /// running it without the copy that undoes it is not a degraded mode, it is a gamble.
    #[error("cannot copy the database to {}", path.display())]
    Backup {
        /// Where the copy was going.
        path: PathBuf,
        /// What SQLite said.
        #[source]
        source: sqlx::Error,
    },

    /// A migration failed while running, which means the SQL in this build is wrong.
    ///
    /// Not something a user can act on — the database is left as the failed migration's
    /// transaction found it, and the fix is a new release.
    #[error("cannot bring the database at {} up to date", path.display())]
    Migration {
        /// The database file.
        path: PathBuf,
        /// Which migration, and how it failed.
        #[source]
        source: sqlx::migrate::MigrateError,
    },

    /// The database on disk was written by a build whose migrations are not this build's.
    ///
    /// A newer MixEngine that has since been downgraded, a file copied from another machine, or an
    /// upgrade that stopped half way. Distinct from [`Error::Migration`] because the user *can* act
    /// on it: the copy taken before the last upgrade is sitting next to it.
    #[error("the database at {} was written by a different version of MixEngine", path.display())]
    IncompatibleDatabase {
        /// The database file.
        path: PathBuf,
        /// Which version does not line up, and how.
        #[source]
        source: sqlx::migrate::MigrateError,
    },

    /// A service was asked to make a move its state machine does not have.
    ///
    /// A bug in the caller rather than a condition to recover from — every legal edge is in
    /// [`mixengine_proto::ServiceState::can_become`], and code that asks for one that is not there
    /// has lost track of what it is supervising. Reported instead of silently written, because a
    /// state machine that accepts anything is a status display that means nothing.
    #[error("{service} cannot go from {from} to {to}")]
    IllegalTransition {
        /// The service that was asked.
        service: String,
        /// Where it actually is.
        from: mixengine_proto::ServiceState,
        /// Where the caller wanted it.
        to: mixengine_proto::ServiceState,
    },

    /// The state changed between being read and being written.
    ///
    /// The assertion behind [`services::transition`]'s compare-and-swap. Two supervisors reaching
    /// the same service at once — a health check going `Degraded` while a user's stop arrives — are
    /// serialised by the `BEGIN IMMEDIATE` that transaction opens with, so the second one reads what
    /// the first committed and judges its move against that. This error is what says that stopped
    /// being true: something wrote the row from outside that transaction, and the transition the
    /// caller computed was based on a state that is no longer there.
    #[error("the state of {service} changed while it was being moved from {expected}")]
    StateRaced {
        /// The service that was being moved.
        service: String,
        /// What it had been when the decision was made.
        expected: mixengine_proto::ServiceState,
    },

    /// A `services` row holds a state this build does not recognise.
    ///
    /// Unreachable through our own writes — the column is `CHECK`ed against the same closed list
    /// [`mixengine_proto::ServiceState`] is — so it means a database edited by hand, or one written
    /// by a version that knew a state this one does not.
    #[error("the state of {service} is stored as {value}, which is not a service state")]
    UnknownServiceState {
        /// The service whose row cannot be read.
        service: String,
        /// The word that is in the column.
        value: String,
    },

    /// A `services` row holds a value this build cannot read back.
    ///
    /// [`Error::UnreadableRuntimeRow`]'s sibling on the other table, and reached through the same
    /// two doors: a database edited by hand, or a row written by a build that knew more than this
    /// one. `id` has no `CHECK` that says it is a [`mixengine_proto::ServiceId`] and `port` none
    /// that says it fits in sixteen bits, so the reader is the only thing that refuses.
    #[error("the {column} of the service {service} is stored as {value}, which cannot be read")]
    UnreadableServiceRow {
        /// The service whose row cannot be read.
        service: String,
        /// Which column.
        column: &'static str,
        /// What is in it, or why it was refused.
        value: String,
    },

    /// A `*_json` column of a `services` row does not hold the document it is supposed to.
    ///
    /// The one thing SQLite cannot constrain, as at [`Error::UnreadableJobRow`]: the column is TEXT,
    /// and no `CHECK` can say it is an object of overrides or a set of resource limits.
    #[error("the {column} of the service {service} is not a document this build can read")]
    UnreadableServiceDocument {
        /// The service whose row cannot be read.
        service: String,
        /// Which column.
        column: &'static str,
        /// How it failed to parse.
        #[source]
        source: serde_json::Error,
    },

    /// A `services` row belongs to a package this build has no recipe for.
    ///
    /// What a service *is* — the binary, the template, the ready check — is compiled in, so a row
    /// naming something else cannot be started, configured or listed. Reached by a home whose
    /// database was written by a newer MixEngine, and by every home for a service an extension
    /// declared and then went away.
    #[error(
        "nothing in this build knows how to run {package}, which the service {service} belongs to \
         (it knows: {})",
        if known.is_empty() { "nothing yet".to_owned() } else { known.join(", ") }
    )]
    NoRecipe {
        /// The service that cannot be generated.
        service: String,
        /// The `packages.name` there is no recipe for.
        package: String,
        /// What this build does have recipes for, in the order a listing shows them.
        known: Vec<String>,
    },

    /// An override names a setting its service does not have.
    ///
    /// Refused rather than ignored, which is `config.toml`'s rule ([`Error::Config`]) one directory
    /// down: a silently dropped override is a setting the user believes is in effect.
    #[error(
        "{service} has no setting called {key} (it has: {})",
        if known.is_empty() { "none".to_owned() } else { known.join(", ") }
    )]
    UnknownSetting {
        /// The service the override was written against.
        service: String,
        /// The key that is not one of its settings.
        key: String,
        /// The keys that are, in the order a listing shows them.
        known: Vec<String>,
    },

    /// An override is the right key and the wrong shape.
    ///
    /// `"port": "3306"` — a number written as a string — is the one that actually happens, and it
    /// is worth its own message because both halves look correct on their own.
    #[error("the {key} of {service} has to be {expected}, and is {found}")]
    SettingType {
        /// The service the override was written against.
        service: String,
        /// Which setting.
        key: String,
        /// What the recipe declared it as.
        expected: &'static str,
        /// What the override offered.
        found: &'static str,
    },

    /// An override is the right key, and the right shape, and still cannot be used.
    ///
    /// `"admin_port": 70000` is a whole number exactly as the recipe declared it, and is not a port.
    /// [`Error::SettingType`] cannot say that — it names two *shapes*, and both of these are the
    /// same one — so the distinction is what each message can carry: that one says what the value is
    /// instead of, and this one says what is wrong with the value itself.
    ///
    /// Raised from a recipe rather than from the merge, because what a value is allowed to be is
    /// knowledge about the service and not about the type: 70000 is not a port, and 3 is not a
    /// number of megabytes for an InnoDB buffer pool.
    #[error("the {key} of {service} cannot be {value}: {reason}")]
    SettingValue {
        /// The service the override was written against.
        service: String,
        /// Which setting, as the recipe declares it.
        key: &'static str,
        /// What the override offered, as it would be written back.
        value: String,
        /// Why it cannot be used, as a clause completing the sentence.
        reason: &'static str,
    },

    /// A template this build ships does not render.
    ///
    /// Ours and not the user's: templates are compiled in, and an override cannot make one
    /// syntactically invalid. What it *can* do is reach a branch nothing had exercised, which is why
    /// the message names the file rather than only the service.
    #[error("the {file} MixEngine generates for {service} could not be rendered")]
    TemplateBroken {
        /// The service being configured.
        service: String,
        /// Which of its files, as the recipe names it.
        file: &'static str,
        /// What the template engine said. Boxed to keep this enum small, as at
        /// [`Error::IndexTransport`].
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// A generated configuration was refused by the program that has to read it.
    ///
    /// **Nothing was installed.** The rendering is judged in a staging directory — `caddy validate`,
    /// `nginx -t` — so the configuration that is live is the last one that worked, and the service
    /// reading it has not been disturbed.
    #[error("{} was refused by {}: {detail}", path.display(), checker.display())]
    ConfigRejected {
        /// The staged file the checker was pointed at.
        path: PathBuf,
        /// The program that refused it.
        checker: PathBuf,
        /// What it said, or that it said nothing.
        detail: String,
    },

    /// A recipe produced a specification that is not runnable.
    ///
    /// A bug in this build rather than anything a user wrote — a program that is not absolute, a
    /// grace period of zero — and reported rather than unwrapped because nothing in this crate
    /// panics. An override *can* reach it, since a recipe builds its spec out of settings, which is
    /// why the service is named.
    #[error("the configuration generated for {service} does not describe a service that can run")]
    Unrunnable {
        /// The service being configured.
        service: String,
        /// Which field, and why.
        #[source]
        source: mixengine_proto::SpecError,
    },

    /// Something tried to move a job that is already over.
    ///
    /// **The ordinary case rather than a corruption**, which is why it is its own variant and not an
    /// [`Error::IllegalJobTransition`]: a job's work is a cancellable task, so a producer that was
    /// cancelled mid-download and reports its last progress on the way out arrives here, and so does
    /// the work finishing at the same instant a `job.cancel` lands. What the caller is told is which
    /// ending got there first, because that is the one that stands.
    #[error("job #{job} has already {state}")]
    JobEnded {
        /// The job.
        job: i64,
        /// What it ended as.
        state: mixengine_proto::JobState,
    },

    /// A job was asked to make a move its state machine does not have.
    ///
    /// A bug in the caller, on [`Error::IllegalTransition`]'s own terms — and unreachable while
    /// every ending is reachable from `running`, which is why it is asserted rather than assumed.
    #[error("job #{job} cannot go from {from} to {to}")]
    IllegalJobTransition {
        /// The job.
        job: i64,
        /// Where it is.
        from: mixengine_proto::JobState,
        /// Where the caller wanted it.
        to: mixengine_proto::JobState,
    },

    /// A `jobs` row holds a state this build does not recognise.
    ///
    /// Unreachable through our own writes — the column is `CHECK`ed against the same closed list
    /// [`mixengine_proto::JobState`] is — so it means a database edited by hand, or one written by a
    /// version that knew a state this one does not. [`Error::UnknownServiceState`]'s sibling.
    #[error("the state of job #{job} is stored as {value}, which is not a job state")]
    UnknownJobState {
        /// The job whose row cannot be read.
        job: i64,
        /// The word that is in the column.
        value: String,
    },

    /// A `jobs` row holds a kind that is not a name.
    ///
    /// `jobs.kind` is deliberately not `CHECK`ed — the set grows with every phase and every
    /// extension — so unlike the state, this one has no constraint behind it and the reader is the
    /// only thing that refuses.
    #[error("the kind of job #{job} is stored as {value}, which is not a job kind")]
    UnknownJobKind {
        /// The job whose row cannot be read.
        job: i64,
        /// The word that is in the column.
        value: String,
    },

    /// A `jobs` column holds a document that does not parse.
    ///
    /// The one thing SQLite cannot constrain: `result_json` is TEXT, and no `CHECK` can say it is a
    /// [`mixengine_proto::JobOutcome`]. Naming the column is what makes the row findable.
    #[error("job #{job} has a {column} this build cannot read")]
    UnreadableJobRow {
        /// The job whose row cannot be read.
        job: i64,
        /// Which column.
        column: &'static str,
        /// How it failed to parse.
        #[source]
        source: serde_json::Error,
    },

    /// A job's outcome could not be written as JSON.
    ///
    /// Unreachable — a [`mixengine_proto::JobOutcome`] is one of ours, and the only value in it that
    /// this crate did not construct came out of a JSON document itself. Reported rather than
    /// unwrapped because nothing in this crate panics.
    #[error("what job #{job} produced could not be stored")]
    JobOutcomeUnwritable {
        /// The job.
        job: i64,
        /// How it failed to serialise.
        #[source]
        source: serde_json::Error,
    },

    /// The package index could not be fetched.
    ///
    /// **`document` is what this family gained in T81, and it is why there is no second family.**
    /// Two signed documents are published — the package index and the extension registry — and they
    /// fail in exactly these five ways. Duplicating the variants would duplicate every `match` that
    /// reads them; naming the document in the message is the whole of the difference a reader needs.
    ///
    /// **Not always fatal**, and the only place in this enum where that is true of the error itself
    /// rather than of what the caller does with it: [`index::Client::catalogue`] constructs this,
    /// looks for a cached index, and returns the cache instead if there is one. It reaches a user
    /// only when there is no cache at all.
    #[error("cannot reach the {document} at {url}")]
    IndexTransport {
        /// Which signed document this is about — see [`index::Document::LABEL`].
        document: &'static str,
        /// What was being fetched — the document or its signature, which are separate requests and
        /// separate ways to fail.
        url: String,
        /// What the HTTP client said. Boxed to keep this enum small: a `reqwest::Error` is several
        /// times the size of every other variant here.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The package index is not signed by the key this build trusts.
    ///
    /// The one failure that cannot happen by accident. A truncated download does not produce a valid
    /// signature over different bytes; a mirror serving somebody else's index does.
    #[error("the {document} at {url} is not signed by this build's key")]
    IndexSignature {
        /// Which signed document this is about — see [`index::Document::LABEL`].
        document: &'static str,
        /// Where the document came from — a URL, or the cache file that was found to be tampered
        /// with.
        url: String,
        /// What the verifier said. Boxed for [`Error::IndexTransport`]'s reason.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The index verified, and then did not parse.
    ///
    /// Which means *we* published something malformed: the signature already established the
    /// document is ours. Distinct from [`Error::IndexSignature`] because the two send whoever reads
    /// the message to entirely different places.
    #[error("the {document} at {url} is signed but unreadable")]
    IndexUnreadable {
        /// Which signed document this is about — see [`index::Document::LABEL`].
        document: &'static str,
        /// Where the document came from.
        url: String,
        /// How it failed to parse.
        #[source]
        source: serde_json::Error,
    },

    /// The index is a document version this build does not know how to read.
    ///
    /// A MixEngine older than the index it is pointed at. The fix is an application update, and
    /// saying so is better than the field-by-field confusion a best-effort parse would produce.
    #[error("the {document} at {url} is schema {found}; this build reads schema {expected}")]
    IndexSchema {
        /// Which signed document this is about — see [`index::Document::LABEL`].
        document: &'static str,
        /// Where the document came from.
        url: String,
        /// What it says it is.
        found: u32,
        /// What this build can read.
        expected: u32,
    },

    /// A timestamp handed to a parser is not the one shape this product writes — roadmap task
    /// **T81a**.
    ///
    /// Only reachable through [`FromStr`](std::str::FromStr), which is the publishing pipeline's
    /// door onto [`index::Timestamp`]; a timestamp arriving *inside* a document is refused by
    /// `Deserialize` instead, with the message serde needs. Two doors onto one parser, because the
    /// two callers need different sentences — this one is a person who mistyped a shell argument.
    #[error("{text:?} is not a UTC RFC 3339 second, e.g. 2026-08-14T06:55:12Z")]
    Timestamp {
        /// What was handed over.
        text: String,
    },

    /// The server offered an index older than the one already cached.
    ///
    /// Every index we ever published is validly signed, so the signature cannot tell an old one from
    /// the current one — which makes replaying a copy from before a security release a real move
    /// rather than a theoretical one. `generated_at` is what separates them, and the cached document
    /// is kept.
    #[error(
        "the {document} at {url} went backwards: it says {offered}, the cached copy says {cached}"
    )]
    IndexRolledBack {
        /// Which signed document this is about — see [`index::Document::LABEL`].
        document: &'static str,
        /// Where the older document came from.
        url: String,
        /// When the cached document was generated.
        cached: String,
        /// When the offered one was.
        offered: String,
    },

    /// The public key compiled into this binary is not a public key.
    ///
    /// A broken build and nothing a user can act on. Reported rather than unwrapped because nothing
    /// in this crate panics.
    #[error("this build's package index key is not a valid minisign key")]
    IndexKey {
        /// What the parser said.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// An artifact could not be fetched.
    ///
    /// [`Error::IndexTransport`]'s sibling one layer down, and unlike it this one is **always
    /// fatal**: there is no cached copy of a runtime to fall back to, so the only thing left to say
    /// is that the download did not happen.
    #[error("cannot download {url}")]
    ArtifactTransport {
        /// What was being fetched.
        url: String,
        /// What the HTTP client said. Boxed to keep this enum small, as at [`Error::IndexTransport`].
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The transfer ended before the artifact did, after every attempt to resume it.
    ///
    /// What is on disk is **kept**: it is a prefix of the file that was wanted, and asking again is
    /// meant to continue from it rather than start over.
    #[error("{url} stopped after {received} of {expected} bytes")]
    ArtifactIncomplete {
        /// What was being fetched.
        url: String,
        /// How large the signed index says it is.
        expected: u64,
        /// How much arrived.
        received: u64,
    },

    /// The server offered more bytes than the index says the artifact has.
    ///
    /// Refused while it is happening rather than after, because the alternative to a bound here is
    /// filling a disk to discover that the checksum does not match.
    #[error("{url} is larger than the {expected} bytes the index declares")]
    ArtifactTooLarge {
        /// What was being fetched.
        url: String,
        /// How large the signed index says it is.
        expected: u64,
    },

    /// A download does not hash to what the signed index promised.
    ///
    /// The download is deleted, which
    /// [security-model.md](../../../.claude/architecture/security-model.md) requires and which is
    /// also what stops a `.part` that can never verify from being resumed forever. Whether this is
    /// a corrupted transfer or a mirror serving something else, the next step is the same one.
    #[error("{url} does not match the checksum the index publishes for it")]
    ArtifactChecksum {
        /// What was being fetched.
        url: String,
        /// The hash the index publishes.
        expected: String,
        /// The hash of what arrived.
        found: String,
    },

    /// The index names an archive in a shape this build cannot unpack.
    ///
    /// A MixEngine older than the pipeline that packed it, which the update is the fix for. Refused
    /// before the download rather than after, so it costs a round trip and not an artifact.
    #[error("{url} is not an archive this build can unpack")]
    ArtifactFormat {
        /// What was being fetched.
        url: String,
    },

    /// An archive could not be read.
    ///
    /// Reached after the checksum has already agreed, so the bytes *are* the ones we published —
    /// which makes this a packaging failure of ours rather than a transfer failure, exactly as
    /// [`Error::IndexUnreadable`] is to [`Error::IndexSignature`].
    #[error("cannot unpack {}", archive.display())]
    ArchiveUnreadable {
        /// The downloaded file.
        archive: PathBuf,
        /// What the container or the decompressor said. Boxed for [`Error::IndexTransport`]'s reason.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// An archive contains an entry that names a path outside where it is being unpacked.
    ///
    /// The oldest attack there is against an installer, and one that a correct signature and a
    /// matching checksum do nothing about: both say the archive is the one we published, and neither
    /// says what is inside it. Nothing partial is left behind — the staging directory this was being
    /// written into is removed.
    #[error("{} contains {entry}, which is not inside it", archive.display())]
    UnsafeArchiveEntry {
        /// The downloaded file.
        archive: PathBuf,
        /// The entry that named somewhere else.
        entry: String,
    },

    /// An archive does not contain something its index entry says it provides.
    ///
    /// A packaging bug found at install time instead of at the moment somebody needed the binary.
    #[error("{url} does not contain the {executable} it lists")]
    MissingFromArtifact {
        /// What was being installed.
        url: String,
        /// The name the index publishes it under.
        executable: String,
        /// Where inside the archive it was supposed to be.
        path: String,
    },

    /// The artifact was unpacked and then would not run on this machine.
    ///
    /// **The one failure a checksum cannot see.** A hash proves the bytes are ours; it says nothing
    /// about a missing VC++ redistributable, a glibc older than the build's floor, or an image the
    /// loader refuses. Found while the install is still in its staging directory, so nothing that
    /// does not work is ever renamed into place.
    #[error("{} does not run on this machine: {detail}", program.display())]
    SmokeTestFailed {
        /// Which binary was run.
        program: PathBuf,
        /// What happened when it was — an exit status with the first line of its complaint, the
        /// operating system's refusal to start it, or a timeout.
        detail: String,
    },

    /// Something is already installed where this one was going.
    ///
    /// An install never mutates a version that is already there: a runtime directory is immutable
    /// once it exists, which is what lets a project pin one and be sure of what it pinned.
    #[error("{} is already installed", path.display())]
    AlreadyInstalled {
        /// Where the install was going.
        path: PathBuf,
    },

    /// A runtime of this kind and version is already written down.
    ///
    /// Distinct from [`Error::AlreadyInstalled`], which is about a *directory* that is already
    /// there: this one is the row, and the two are separate because the ordering
    /// [`runtimes`] follows deliberately allows a directory with no row — an install that landed
    /// and whose row could not be written is a repair, and the repair is asking again.
    #[error("{kind} {version} is already installed")]
    AlreadyRecorded {
        /// Which language.
        kind: mixengine_proto::RuntimeKind,
        /// Which version.
        version: mixengine_proto::PackageVersion,
    },

    /// An extension that is compiled into this build was asked to be turned off.
    ///
    /// Not a rewritten file that quietly does nothing: `opcache` is static on the Unix cells and a
    /// DLL on Windows, so the same request is answerable on one machine and not on the other, and
    /// what it would take here is a different build rather than a different setting.
    #[error("{name} is compiled into {kind} {version} and cannot be turned off")]
    ExtensionCompiledIn {
        /// Which language.
        kind: mixengine_proto::RuntimeKind,
        /// Which version.
        version: mixengine_proto::PackageVersion,
        /// The extension that was asked about.
        name: String,
    },

    /// A service with this id, or this instance of this package, already exists.
    ///
    /// Two unique constraints and one variant, because what a person did wrong is the same in both
    /// cases: `services.id` and `UNIQUE (package_id, instance_name)` both mean the service they
    /// asked for is already here.
    #[error("{service} already exists")]
    ServiceAlreadyDeclared {
        /// Which service.
        service: mixengine_proto::ServiceId,
    },

    /// Another service already points at this data directory — roadmap task **T36**.
    ///
    /// Two instances of one server are two data directories, and the layout the generator derives
    /// guarantees that on its own. This is the case it cannot guarantee: a caller that named the
    /// path itself, twice. Refused where the row is written, because two servers over one set of
    /// files is a cost paid in the user's data rather than in a start that fails.
    #[error("{holder} already keeps its data in {path}")]
    DataDirectoryTaken {
        /// The directory both services were pointed at.
        path: String,

        /// The id of the service that got there first, as its row spells it.
        holder: String,
    },

    /// Nothing free was found near the port a recipe asked for — roadmap task **T34c**.
    ///
    /// **A bounded search, and running out of it is an error rather than a longer loop.** A machine
    /// on which sixty-four consecutive ports above 3306 are all held is not a machine one more
    /// probe would help on: something is wrong with it that a person has to look at, and a service
    /// quietly landing on a number three hundred away from the one its product is documented under
    /// would hide that.
    #[error("no free port between {preferred} and {last}")]
    PortsExhausted {
        /// The port the recipe asked for.
        preferred: u16,

        /// The highest port the search reached.
        last: u16,
    },

    /// A package of this name and version is already written down.
    ///
    /// [`Error::AlreadyRecorded`]'s sibling one table across, separate because the two name
    /// different things: that one carries a [`RuntimeKind`](mixengine_proto::RuntimeKind), and the
    /// set of packages is open where the set of runtimes is closed.
    #[error("{package} {version} is already installed")]
    PackageAlreadyRecorded {
        /// Which package.
        package: String,
        /// Which version.
        version: mixengine_proto::PackageVersion,
    },

    /// A `packages` row, or a `services.id` joined to one, holds a value this build cannot read
    /// back.
    ///
    /// [`Error::UnreadableRuntimeRow`]'s sibling, and one variant for every column for the same
    /// reason: what a reader can do about them is identical and what it needs to say is which one.
    #[error("a packages row holds a {column} this build cannot read: {value}")]
    UnreadablePackageRow {
        /// Which column.
        column: &'static str,
        /// What is in it.
        value: String,
    },

    /// A `runtime_installs` row holds a value this build cannot read back.
    ///
    /// One variant for four columns, unlike the `jobs` table's pair, because what a reader can do
    /// about them is identical and what it needs to say is which column: `kind` has a `CHECK` behind
    /// it and the other three have nothing, so a row written by a build that knew a fifth channel —
    /// or edited by hand — arrives here naming the field rather than the table.
    #[error("a runtime_installs row holds a {column} this build cannot read: {value}")]
    UnreadableRuntimeRow {
        /// Which column.
        column: &'static str,
        /// What is in it.
        value: String,
    },

    /// A `mixengine.toml` in the user's repository does not parse.
    ///
    /// [`Error::Config`]'s sibling one directory out, and refused for the same reason: an unknown
    /// key inside `[runtimes]` is a pin naming a language MixEngine does not manage, which would do
    /// nothing at all while looking exactly like a pin that does not work. Unknown *sections* are
    /// allowed through — the file also declares a site and its services, which are Phase 4's.
    #[error("{} is not a valid project manifest", path.display())]
    Manifest {
        /// The manifest that failed to parse.
        path: PathBuf,
        /// The parse failure, which carries the line, the column and the accepted keys.
        #[source]
        source: toml::de::Error,
    },

    /// A `mixengine.toml` that parses but could not be edited in place.
    ///
    /// [`Error::Manifest`]'s sibling on the write path, and separate from it because the two are
    /// different accusations: the first says the user's file is wrong, and this says this build
    /// could not put something into a file that is right. The reason is carried as text rather than
    /// as the editor's own error type, so the shape of a dependency does not become part of this
    /// enum.
    #[error("{} could not be edited: {reason}", path.display())]
    ManifestEdit {
        /// The manifest that could not be edited.
        path: PathBuf,
        /// What the editor said about it.
        reason: String,
    },

    /// A directory that has to be absolute was not.
    ///
    /// Version resolution walks upwards from it, so a relative path would be walked from wherever
    /// the *daemon* was started — a directory belonging to nobody's project, silently producing a
    /// plausible answer about the wrong tree.
    #[error("{} is not an absolute directory", path.display())]
    NotAbsolute {
        /// What was offered.
        path: PathBuf,
    },

    /// A `projects` row holds a value this build cannot read back.
    ///
    /// [`Error::UnreadableRuntimeRow`]'s sibling, and reached through the same two doors: a database
    /// edited by hand, or a row written by a build that knew more than this one. `runtime_pins_json`
    /// is TEXT with no `CHECK` available to it, so the reader is the only thing that refuses — but
    /// only for the language being asked about, since a pin naming a fifth one must not stop this
    /// build resolving PHP.
    #[error("the project at {root} has a {column} this build cannot read: {value}")]
    UnreadableProjectRow {
        /// The project's root directory, which is how a person finds the row.
        root: String,
        /// Which column.
        column: &'static str,
        /// What is in it.
        value: String,
    },

    /// A project name that cannot be one.
    ///
    /// Refused rather than corrected, because a name is a handle: it is typed on a command line,
    /// shown in a listing, and T39a takes a site's default domain from it — so a name silently
    /// changed on the way in is a name that does not work where the user next types it.
    #[error("{name} cannot be a project name: {because}")]
    InvalidProjectName {
        /// What was offered.
        name: String,
        /// Which rule it broke, as a phrase finishing "cannot be a project name: …".
        because: &'static str,
    },

    /// A domain that cannot be one.
    ///
    /// Refused rather than corrected, on [`Error::InvalidProjectName`]'s reasoning: a domain is
    /// typed into a browser, and one silently changed on the way in is one that does not work
    /// where the user next types it. Case is the single exception — DNS has never been
    /// case-sensitive, so lowercasing is normalisation rather than correction.
    #[error("{domain} cannot be a domain: {because}")]
    InvalidDomain {
        /// What was offered.
        domain: String,
        /// Which rule it broke, as a phrase finishing "cannot be a domain: …".
        because: &'static str,
    },

    /// A TLD this home does not manage.
    ///
    /// Public suffixes are refused rather than served: `.dev` and `.app` are HSTS-preloaded, so a
    /// site on one would be a browser refusing plain HTTP before any of this was consulted.
    #[error("{domain} is on .{tld}, which is a public TLD this home will not answer for")]
    UnmanagedTld {
        /// What was offered.
        domain: String,
        /// The last label, without its dot.
        tld: String,
    },

    /// `.local`, without the acknowledgement it needs.
    ///
    /// mDNS territory (RFC 6762): it works until somebody plugs in a printer. Allowed, because a
    /// person who knows that is entitled to it — and never by default.
    #[error("{domain} is on .local, which belongs to mDNS")]
    RiskyTld {
        /// What was offered.
        domain: String,
    },

    /// A domain another site already answers to.
    ///
    /// `site_domains_domain` is `UNIQUE` — one domain is one site, primary or alias — and this
    /// names the site holding it, which the index cannot. Without the name the answer would be
    /// "taken" with nowhere to go and look.
    #[error("{domain} already belongs to {holder}")]
    DomainTaken {
        /// The domain that is claimed.
        domain: String,
        /// The primary domain of the site holding it.
        holder: String,
    },

    /// The only domain a site has.
    ///
    /// `0001_initial.sql` records "at least one" as an invariant this layer upholds, because SQLite
    /// has no deferred constraint to express it with. This is that invariant being upheld: a site
    /// with no name is one nothing can reach and nothing can render.
    #[error("{domain} is the only domain its site has")]
    LastDomain {
        /// The domain that was to go.
        domain: String,
    },

    /// A site's primary domain, which `domain.remove` does not get to change.
    ///
    /// The primary decides the site's canonical URL and, from phase 5, the name on its certificate.
    /// Promoting another domain in its place would change what the site *is* under a verb that says
    /// "remove a domain" — a larger act than the one asked for (T46 design, D3).
    #[error("{domain} is its site's primary domain")]
    PrimaryDomain {
        /// The domain that was to go.
        domain: String,
    },

    /// A doc root that is not inside the project it belongs to.
    ///
    /// Refused rather than stored: a site whose files are outside its project's root is one no
    /// renderer can express, and `project.update { root }` would move the project out from under
    /// it.
    #[error("{doc_root} is not inside {root}")]
    DocRootOutsideProject {
        /// What was offered.
        doc_root: String,
        /// The project's root, as the filesystem spells it.
        root: String,
    },

    /// A `sites` row this build cannot read.
    #[error("the {column} of site {site} is not something this build can read: {value}")]
    UnreadableSiteRow {
        /// The rowid, which is the only handle a broken row has.
        site: i64,
        /// Which column.
        column: &'static str,
        /// What was in it.
        value: String,
    },

    /// A directory that is already a project.
    ///
    /// `projects.root_path` is `UNIQUE` — one directory is one project — and this names the project
    /// holding it, which the unique index cannot. A root *inside* another project's root is not
    /// this: the walk takes the nearest, so nesting has a defined answer.
    #[error("{root} is already the project {holder}")]
    ProjectRootTaken {
        /// The directory, spelled the way the filesystem spells it.
        root: String,
        /// The project that got there first.
        holder: String,
    },

    /// A project name that is already registered.
    ///
    /// The other unique column, and the one whose repair is different: a name is not freed by
    /// moving a directory, only by renaming or deleting the project that holds it.
    #[error("a project called {name} is already registered")]
    ProjectNameTaken {
        /// The name that is taken.
        name: String,
    },

    /// Nothing installed satisfies the version this directory asks for.
    ///
    /// **Never resolved against the index**, which is what makes this an error rather than a
    /// download: a `cd` into a directory must not start one. What the daemon turns it into is
    /// `dependency_missing` with [`resolve::install_command`] as the hint.
    #[error("no installed {kind} matches {constraint}, asked for by {origin}")]
    RuntimeUnresolved {
        /// Which language.
        kind: mixengine_proto::RuntimeKind,
        /// What was asked for.
        constraint: mixengine_proto::VersionConstraint,
        /// Where it was asked from, as a phrase completing "asked for by …".
        origin: String,
    },

    /// Nothing asked for a version, and the kind has no default to fall back on.
    ///
    /// Either nothing of this kind is installed at all, or the version that was the default has
    /// been uninstalled — [`runtimes::forget`] promotes nothing in its place, deliberately, so this
    /// is the state a home is left in and the message has to be able to say so.
    #[error("no {kind} version is installed as the default")]
    NoDefaultRuntime {
        /// Which language.
        kind: mixengine_proto::RuntimeKind,
    },

    /// The version resolved, and it publishes no executable under that name.
    ///
    /// Two different disappointments with one message, deliberately, because the person reading it
    /// cannot tell them apart and does not need to: a `pecl` that this build of PHP genuinely does
    /// not ship, and a runtime installed before `provides_json` existed, whose map is empty. Both
    /// are answered by naming what the runtime *does* publish — an empty list being the second case,
    /// stated rather than explained.
    #[error(
        "{kind} {version} publishes no executable called {executable} (it has: {})",
        if known.is_empty() { "nothing recorded".to_owned() } else { known.join(", ") }
    )]
    RuntimeProvidesNothing {
        /// Which language.
        kind: mixengine_proto::RuntimeKind,
        /// Which version.
        version: mixengine_proto::PackageVersion,
        /// The name that was looked up.
        executable: String,
        /// What it does publish, in the order a listing shows them.
        known: Vec<String>,
    },

    /// A recipe asked the install behind its service for an executable it does not publish.
    ///
    /// Distinct from [`Error::RuntimeProvidesNothing`], which is the shim's question — *which file
    /// is `php`* — asked of a runtime by kind and version. This one is asked by a **recipe**, of
    /// whatever the service's row points at, and it names the service because that is what the
    /// person reading has in their hand. The usual cause is an artifact packed without the SAPI the
    /// recipe needs: a PHP whose `provides` has `php` and no `php-fpm`.
    #[error(
        "{service} runs out of an install that publishes no executable called {executable} (it has: {})",
        if known.is_empty() { "nothing recorded".to_owned() } else { known.join(", ") }
    )]
    ServiceProvidesNothing {
        /// Which service.
        service: String,
        /// The name the recipe looked up.
        executable: String,
        /// What the install does publish, in the order a listing shows them.
        known: Vec<String>,
    },

    /// An install stopped because it was asked to.
    ///
    /// Not a failure of anything, and the daemon turns it into a cancelled job rather than a failed
    /// one. The partial download is kept: somebody who cancels at sixty percent and asks again has
    /// not asked for those bytes to be thrown away.
    #[error("the install was cancelled")]
    InstallCancelled,

    /// A set of service specs does not form a dependency graph.
    ///
    /// Transparent because [`services::GraphError`] already says everything there is to say and
    /// names the services it is about — wrapping it in a sentence of ours would only push the useful
    /// half one level further down a cause chain.
    #[error(transparent)]
    Graph(#[from] services::GraphError),

    /// The shim binary is not beside the program that went looking for it.
    ///
    /// A broken installation and nothing a user did: a release ships `mixengined` and
    /// `mixengine-shim` in one directory, so this means half of one was copied somewhere, or a
    /// development tree was built without `-p mixengine-shim`. Its own variant rather than an
    /// [`Error::Io`] because there is no operation to name — nothing was attempted.
    #[error("the shim binary is missing from {}", path.display())]
    ShimMissing {
        /// Where it was expected to be.
        path: PathBuf,
    },

    /// `mixengine-elevate` is not beside the program that went looking for it.
    ///
    /// [`Error::ShimMissing`]'s sibling and the same broken installation: a release ships
    /// `mixengined` and `mixengine-elevate` in one directory. It is **not** a reason to refuse to
    /// start — a daemon with no helper beside it supervises every service in this home perfectly
    /// well — and it is answered at `elevation.grant`, where somebody can act on it.
    #[error("the elevation helper is missing from {}", path.display())]
    ElevateMissing {
        /// Where it was expected to be.
        path: PathBuf,
    },

    /// `mixengine-elevate` is installed where it belongs, and that file is not an administrator's.
    ///
    /// **The one state where this refuses rather than falling back** — the T85 design, D5. A copy
    /// beside the program is used when nothing is installed, because that is a development tree and
    /// a machine before its first prompt. A copy that *is* installed and can be written by an
    /// ordinary account is not the same thing at all: it is precisely the arrangement the
    /// root-owned directory exists to prevent, and running it anyway would be doing as root exactly
    /// what somebody set up.
    ///
    /// Not [`Error::ElevateMissing`]'s sibling in code either: that one is a broken installation and
    /// the fix is a reinstall, this one is a machine somebody has arranged and the fix is to find
    /// out who. Reported through `elevation.status`' `reason`, so it is on the screen before
    /// anybody clicks Allow.
    #[error("the elevation helper at {} is not an administrator's: {why}", path.display())]
    ElevateUntrusted {
        /// The installed helper this is about.
        path: PathBuf,
        /// Which check it failed, phrased for a person.
        why: String,
    },

    /// A batch with no operations in it.
    ///
    /// The helper refuses one outright — no response file, exit 65 — because giving an empty request
    /// a meaning of its own would be a second way to ask for the report that arrives with every
    /// answer. Refused here so the message says what happened rather than what the exit code was.
    #[error("an elevation request has to carry at least one operation")]
    ElevateRequestEmpty,

    /// The helper ran and left nothing beside the request.
    ///
    /// **A state and not an impossibility.** `ElevationOutcome::Completed` means the helper ran, not
    /// that it wrote a report: a process that died before writing one is exactly this, on every
    /// system, because a crash is not a per-OS event.
    #[error("the elevation helper left no report beside {}", path.display())]
    ElevateReportMissing {
        /// Where one would have been.
        path: PathBuf,
    },

    /// The report is there and is not a document this build can read.
    ///
    /// The response is deliberately tolerant of fields it does not know — the helper is excluded
    /// from auto-update, so one newer than the daemon is routine — which makes this a file that is
    /// not the helper's answer at all.
    #[error("the elevation helper's report at {} cannot be read", path.display())]
    ElevateReportUnreadable {
        /// The report.
        path: PathBuf,
        /// How it failed to parse.
        #[source]
        source: serde_json::Error,
    },

    /// The report is readable and is not an answer to the request it sits beside.
    ///
    /// The nonce, the protocol, or the number of outcomes. Its own variant because none of the three
    /// is a parse failure and all three mean the same thing to a caller: nothing here can be applied
    /// to the queue.
    #[error("the elevation helper's report at {} does not answer this request: {why}", path.display())]
    ElevateReportMismatched {
        /// The report.
        path: PathBuf,
        /// Which of the three checks it failed.
        why: String,
    },

    /// A privileged operation could not be encoded.
    ///
    /// Unreachable: a [`mixengine_proto::privileged::PrivilegedOp`] is one of ours and holds nothing
    /// serde can refuse. Its own variant rather than an `expect`, on
    /// [`Error::JobOutcomeUnwritable`]'s rule — nothing in this crate panics.
    #[error("a privileged operation could not be written down")]
    OpUnwritable {
        /// How it failed to encode.
        #[source]
        source: serde_json::Error,
    },

    /// A certificate could not be made.
    ///
    /// Generating a key pair or signing with it, which fails only when the machine's own crypto
    /// does — a kernel with no CSPRNG, a build whose backend refused the curve. **Not** the error
    /// for a certificate that is *there* and unusable: that is a state rather than a failed
    /// operation, and it travels as [`mixengine_proto::CaState`] because it is a fact about the
    /// home and not about this call.
    #[error("cannot {action} {subject}")]
    Certificate {
        /// What was being attempted, e.g. `"sign"`.
        action: &'static str,
        /// What it was being attempted on, phrased for a person.
        subject: String,
        /// The underlying failure.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// `MIXENGINE_HOME` (or `--home`) was given, but empty.
    ///
    /// Distinct from "not given": the user meant to point somewhere and the value went missing on
    /// the way, so the platform default would be the one place they did *not* ask for.
    #[error("MIXENGINE_HOME is empty — unset it to use this platform's default location")]
    EmptyHome,

    /// The OS refused to answer a question only it can answer.
    #[error(transparent)]
    Platform(#[from] mixengine_platform::Error),
}

/// Result of a domain operation.
pub type Result<T> = std::result::Result<T, Error>;
