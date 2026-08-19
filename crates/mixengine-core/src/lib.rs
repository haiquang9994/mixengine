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

pub mod config;
pub mod generate;
pub mod index;
pub mod install;
pub mod jobs;
pub mod packages;
pub mod paths;
pub mod resolve;
pub mod runtimes;
pub mod services;
pub mod shims;
pub mod store;

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
    /// **Not always fatal**, and the only place in this enum where that is true of the error itself
    /// rather than of what the caller does with it: [`index::Client::catalogue`] constructs this,
    /// looks for a cached index, and returns the cache instead if there is one. It reaches a user
    /// only when there is no cache at all.
    #[error("cannot reach the package index at {url}")]
    IndexTransport {
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
    #[error("the package index at {url} is not signed by this build's key")]
    IndexSignature {
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
    #[error("the package index at {url} is signed but unreadable")]
    IndexUnreadable {
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
    #[error("the package index at {url} is schema {found}; this build reads schema {expected}")]
    IndexSchema {
        /// Where the document came from.
        url: String,
        /// What it says it is.
        found: u32,
        /// What this build can read.
        expected: u32,
    },

    /// The server offered an index older than the one already cached.
    ///
    /// Every index we ever published is validly signed, so the signature cannot tell an old one from
    /// the current one — which makes replaying a copy from before a security release a real move
    /// rather than a theoretical one. `generated_at` is what separates them, and the cached document
    /// is kept.
    #[error(
        "the package index at {url} went backwards: it says {offered}, the cached copy says {cached}"
    )]
    IndexRolledBack {
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
