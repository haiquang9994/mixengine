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
pub mod index;
pub mod jobs;
pub mod paths;
pub mod services;
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

    /// A set of service specs does not form a dependency graph.
    ///
    /// Transparent because [`services::GraphError`] already says everything there is to say and
    /// names the services it is about — wrapping it in a sentence of ours would only push the useful
    /// half one level further down a cause chain.
    #[error(transparent)]
    Graph(#[from] services::GraphError),

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
