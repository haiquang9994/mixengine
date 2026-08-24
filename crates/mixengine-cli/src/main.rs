//! `mix` — the reference client. It renders what the daemon returns and decides nothing itself.
//!
//! Every command here is one RPC and one rendering. That is the rule from
//! [CLAUDE.md](../../../CLAUDE.md) — *no business logic in clients* — and it is why this binary is
//! shaped the way it is: the only decisions it makes on its own are which home it is talking about,
//! whether to start a daemon that is not running, and how to put the answer on screen. Everything
//! else is `mixengined`'s, including the wording of every failure.
//!
//! **Failures are the wire error, always.** Whether the daemon refused a call or `mix` never
//! reached one, what comes out is a `mixengine_proto::Error` — a stable code, one sentence, and a
//! hint where there is something to do. A script gets the same object out of `--json` in both cases
//! and can branch on `code` without caring which side of the socket produced it.

mod autostart;
mod client;
mod confirm;
mod error;
mod home;
mod render;

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use mixengine_platform::ipc::Endpoint;
use mixengine_proto::{
    DaemonShutdown, DaemonStatus, DoctorRepair, DoctorReport, DomainAdd, DomainRemove,
    DomainStatusQuery, DomainStatusReport, ElevationDrop, ElevationStatus, Error, ErrorCode,
    ExtensionChange, ExtensionChoice, ExtensionList, JobFilter, JobId, JobList, JobQuery, JobState,
    JobSummary, JobWait, LogFrame, Millis, PackageCatalogue, PackageFilter, PackageList,
    PackageRemoval, PackageTarget, PackageVersion, PathReport, PendingOpId, ProjectCreate,
    ProjectDetail, ProjectExport, ProjectList, ProjectQuery, ProjectRef, ProjectRemoval,
    ProjectUpdate, RepairReport, ResolvedRuntime, RuntimeCatalogue, RuntimeFilter, RuntimeKind,
    RuntimeList, RuntimeQuestion, RuntimeRemoval, RuntimeSummary, RuntimeTarget, RuntimeUninstall,
    ServiceCreate, ServiceCreation, ServiceDelete, ServiceId, ServiceList, ServiceQuery,
    ServiceRemoval, ServiceSummary, ServiceTarget, ServiceWalk, SiteCreate, SiteCreation,
    SiteDetail, SiteKind, SiteList, SiteListQuery, SiteQuery, SiteRef, SiteRemoval, SiteState,
    SiteUpdate, VersionConstraint, rpc,
};

use autostart::Autostart;
use client::Client;

/// Command line of the client. Configuration enters the program here and is passed down; nothing
/// deeper reads the environment on its own.
#[derive(Debug, Parser)]
#[command(name = "mix", version, about = "MixEngine command line")]
struct Args {
    /// Root directory of the MixEngine installation to talk to.
    ///
    /// Defaults to the OS convention, exactly as `mixengined` resolves it — the two have to agree
    /// or they would be talking about different daemons.
    #[arg(long, global = true, env = "MIXENGINE_HOME", value_name = "DIR")]
    home: Option<PathBuf>,

    /// Emit machine-readable JSON instead of the human-facing rendering.
    #[arg(long, global = true)]
    json: bool,

    /// Fail instead of starting a daemon when none is running for this home.
    ///
    /// `mix` normally starts one, which is what makes the first command a person types work. The
    /// flag is for the caller that wants a question answered rather than a machine changed: a
    /// monitoring check, or a CI step that should not create a home as a side effect of asking
    /// whether one is there.
    #[arg(long, global = true)]
    no_autostart: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show the daemon's health, version and what it is currently running.
    Status,

    /// Control the daemon itself.
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },

    /// Install, remove and choose between language runtimes.
    Runtime {
        #[command(subcommand)]
        command: RuntimeCommand,
    },

    /// Install and remove the servers, databases and caches a service runs.
    Package {
        #[command(subcommand)]
        command: PackageCommand,
    },

    /// Register the directories this home knows about, and what they pin.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },

    /// Declare what is served out of a project's directory, and at what name.
    Site {
        #[command(subcommand)]
        command: SiteCommand,
    },

    /// Examine this machine and say what is wrong with it.
    ///
    /// Reports and repairs nothing unless `--repair` is passed. Exits non-zero when it found a
    /// problem, so a script can ask.
    Doctor {
        /// Repair everything that can be repaired, and ask for the rest.
        ///
        /// Repairs inside this home are made at once. Anything needing an administrator is queued,
        /// shown, and then granted once — one prompt for the whole batch.
        #[arg(long)]
        repair: bool,

        /// Do not ask before raising the prompt. Only with `--repair`.
        #[arg(long, requires = "repair")]
        yes: bool,

        /// Return as soon as the grant has started, rather than waiting for it. Only with
        /// `--repair`.
        #[arg(long, requires = "repair")]
        no_wait: bool,
    },

    /// Add, remove and diagnose the names this home answers for.
    Domain {
        #[command(subcommand)]
        command: DomainCommand,
    },

    /// Inspect and control the services this home declares.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },

    /// Watch the long operations this daemon is running.
    Job {
        #[command(subcommand)]
        command: JobCommand,
    },

    /// Put this home's commands on your PATH, or take them off again.
    Path {
        #[command(subcommand)]
        command: PathCommand,
    },

    /// See what needs an administrator's permission, ask for it once, or forget it.
    Elevation {
        #[command(subcommand)]
        command: ElevationCommand,
    },
}

/// `mix project …` — one subcommand per `project.*` method, and nothing that is not one.
///
/// `import` is an **alias** on `create` rather than a seventh subcommand: both produce one row, and
/// what makes a create an import is the `mixengine.toml` already lying in the directory rather than
/// a different call. An alias is the same subcommand under a second name, so the rule above holds.
#[derive(Debug, Subcommand)]
enum ProjectCommand {
    /// Register a directory as a project.
    ///
    /// With no `--name` and no `--pin`, whatever the `mixengine.toml` in that directory says is
    /// used — which is what adopting a colleague's checkout is.
    #[command(alias = "import")]
    Create {
        /// The project's root. Defaults to the current directory.
        #[arg(value_name = "DIR")]
        root: Option<PathBuf>,

        /// What to call it. Defaults to the manifest's name, then to the directory's own.
        #[arg(long, value_name = "NAME")]
        name: Option<String>,

        /// Pin a language, as `php=^8.3`. May be given more than once.
        #[arg(long = "pin", value_name = "RUNTIME=VERSION", value_parser = pin)]
        pins: Vec<(RuntimeKind, VersionConstraint)>,
    },

    /// List the projects this home has been told about.
    List,

    /// Show one, with its pins in the order they take effect.
    Show {
        #[command(flatten)]
        project: WhichProject,
    },

    /// Change a project's name, root or pins.
    ///
    /// `--pin` **replaces** every pin rather than adding to one: `--clear-pins` with no `--pin`
    /// removes them all, and leaving both out changes nothing.
    Update {
        #[command(flatten)]
        project: WhichProject,

        /// A new name.
        #[arg(long, value_name = "NAME")]
        name: Option<String>,

        /// A new root, for a repository that moved.
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,

        /// Pin a language, as `php=^8.3`. Replaces every pin the project had.
        #[arg(long = "pin", value_name = "RUNTIME=VERSION", value_parser = pin)]
        pins: Vec<(RuntimeKind, VersionConstraint)>,

        /// Remove every pin.
        #[arg(long, conflicts_with = "pins")]
        clear_pins: bool,
    },

    /// Forget a project. The directory is left exactly as it is.
    Delete {
        #[command(flatten)]
        project: WhichProject,
    },

    /// Write the project into `<root>/mixengine.toml`, keeping everything else in the file.
    Export {
        #[command(flatten)]
        project: WhichProject,
    },
}

/// Which project a command is about, which is the same question four times.
///
/// **The default is the directory you are in**, not a name this client invents: with no argument
/// `mix` sends the working directory and the daemon walks up to the nearest registered root — the
/// same walk the shim does.
#[derive(Debug, clap::Args)]
struct WhichProject {
    /// The project's name. Defaults to whichever project the current directory is in.
    #[arg(value_name = "PROJECT")]
    name: Option<String>,
}

/// `mix domain …` — one subcommand per `domain.*` method, and nothing that is not one.
#[derive(Debug, Subcommand)]
enum DomainCommand {
    /// Give a site one more name.
    ///
    /// The new name is an alias: the site's primary domain is unchanged, because that is what its
    /// canonical URL and — from the HTTPS work — its certificate are named after.
    Add {
        /// The name to add.
        #[arg(value_name = "DOMAIN")]
        domain: String,

        /// Any of the site's existing domains.
        #[arg(long, value_name = "DOMAIN")]
        site: String,

        /// Accept `.local`, which belongs to mDNS and works until somebody plugs in a printer.
        #[arg(long = "i-know")]
        accept_risky_tld: bool,
    },

    /// Take one name away.
    ///
    /// Refused for a site's last domain and for its primary; `mix site update` reorders, and the
    /// first `--domain` it is given becomes the primary.
    Remove {
        /// The name to take away. It names its own site.
        #[arg(value_name = "DOMAIN")]
        domain: String,
    },

    /// What actually happens to a name, as four facts that can fail one at a time.
    Status {
        /// One name, or every name this home declares.
        #[arg(value_name = "DOMAIN")]
        domain: Option<String>,
    },
}

/// `mix site …` — one subcommand per `site.*` method, and nothing that is not one.
#[derive(Debug, Subcommand)]
enum SiteCommand {
    /// Declare a site under a project.
    ///
    /// With nothing but a project named, whatever the `[site]` and `[[services]]` in that
    /// project's `mixengine.toml` say is used — which is what adopting a colleague's site is.
    #[command(alias = "import")]
    Create {
        /// The project. Defaults to whichever project the current directory is in.
        #[arg(long, value_name = "PROJECT")]
        project: Option<String>,

        /// A domain. The first is the primary; repeat for aliases. Defaults to `<project>.test`.
        #[arg(long = "domain", value_name = "DOMAIN")]
        domains: Vec<String>,

        /// What is served, relative to the project's root. Defaults to the root itself.
        #[arg(long, value_name = "DIR")]
        doc_root: Option<String>,

        /// What serves it.
        #[arg(long, value_enum, value_name = "KIND")]
        kind: Option<SiteKindArg>,

        /// Where a `reverse-proxy` forwards to.
        #[arg(long, value_name = "URL", required_if_eq("kind", "reverse-proxy"))]
        upstream: Option<String>,

        /// The port a `node-app` listens on.
        #[arg(long, value_name = "PORT", required_if_eq("kind", "node-app"))]
        port: Option<u16>,

        /// The php-fpm pool a `php-fpm` site uses. Defaults to whatever this directory resolves to.
        #[arg(long, value_name = "SERVICE", value_parser = service_id)]
        pool: Option<ServiceId>,

        /// A service the site declares, as `mariadb@main`. May be given more than once.
        #[arg(long = "service", value_name = "SERVICE", value_parser = service_id)]
        services: Vec<ServiceId>,

        /// Declare HTTPS for it. Phase 5 is what acts on this.
        #[arg(long)]
        https: Option<bool>,

        /// Accept a `.local` domain, which belongs to mDNS.
        #[arg(long = "i-know")]
        accept_risky_tld: bool,
    },

    /// List the sites this home has been told about.
    List {
        /// Only this project's.
        #[arg(long, value_name = "PROJECT")]
        project: Option<String>,
    },

    /// Show one, with its domains, its pool and its services.
    Show {
        #[command(flatten)]
        site: WhichSite,
    },

    /// Change what a site is.
    ///
    /// `--domain` and `--service` **replace** rather than add to what the site had: giving neither
    /// changes neither.
    Update {
        #[command(flatten)]
        site: WhichSite,

        /// A domain. The first is the primary; repeat for aliases. Replaces the whole list.
        #[arg(long = "domain", value_name = "DOMAIN")]
        domains: Vec<String>,

        /// A new doc root.
        #[arg(long, value_name = "DIR")]
        doc_root: Option<String>,

        /// A new kind.
        #[arg(long, value_enum, value_name = "KIND")]
        kind: Option<SiteKindArg>,

        /// Where a `reverse-proxy` forwards to.
        #[arg(long, value_name = "URL", required_if_eq("kind", "reverse-proxy"))]
        upstream: Option<String>,

        /// The port a `node-app` listens on.
        #[arg(long, value_name = "PORT", required_if_eq("kind", "node-app"))]
        port: Option<u16>,

        /// The php-fpm pool.
        #[arg(long, value_name = "SERVICE", value_parser = service_id)]
        pool: Option<ServiceId>,

        /// A service the site declares. Replaces the whole list.
        #[arg(long = "service", value_name = "SERVICE", value_parser = service_id)]
        services: Vec<ServiceId>,

        /// Whether HTTPS is declared.
        #[arg(long)]
        https: Option<bool>,

        /// Serve it, or stop serving it.
        #[arg(long, value_enum, value_name = "STATE")]
        state: Option<SiteStateArg>,

        /// Accept a `.local` domain.
        #[arg(long = "i-know")]
        accept_risky_tld: bool,
    },

    /// Serve this site.
    ///
    /// A flag and a re-render: the front end is told to read its configuration again. Nothing is
    /// started — a site is not a process, and the services it uses have states of their own.
    Start {
        #[command(flatten)]
        site: WhichSite,
    },

    /// Stop serving this site, keeping the declaration.
    Stop {
        #[command(flatten)]
        site: WhichSite,
    },

    /// Forget a site. The files are left exactly as they are.
    Delete {
        #[command(flatten)]
        site: WhichSite,
    },
}

/// Which site a command is about.
///
/// **The default is the directory you are in**, on [`WhichProject`]'s rule: with no argument `mix`
/// sends the working directory and the daemon walks up to the nearest registered project, then to
/// its site. A project holding several is refused there, naming them — which is a sentence this
/// client only prints.
#[derive(Debug, clap::Args)]
struct WhichSite {
    /// Any of the site's domains. Defaults to the site of whichever project you are in.
    #[arg(value_name = "DOMAIN")]
    domain: Option<String>,
}

/// What serves a site, as a person types it.
///
/// `SiteKindArg` rather than `Kind`: that name is already the runtime filter three commands take,
/// and one word meaning two things in one file is a rename waiting to go wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
enum SiteKindArg {
    /// PHP through a php-fpm pool.
    PhpFpm,
    /// Files, and nothing running.
    Static,
    /// Everything forwarded to an address you already have listening.
    ReverseProxy,
    /// A node process you run, on a port.
    NodeApp,
}

/// Whether the web server should serve a site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
enum SiteStateArg {
    /// Serve it.
    Enabled,
    /// Declare it and do not serve it.
    Disabled,
}

/// `mix path …` — one subcommand per `path.*` method.
///
/// **None of the three takes an argument.** There is one directory this can be about, `<root>/bin`,
/// and the daemon is what knows where it is — a `--dir` here would be a command for putting
/// arbitrary directories on somebody's PATH, which is not a thing MixEngine does.
#[derive(Debug, Subcommand)]
enum PathCommand {
    /// Say whether a new terminal would find this home's commands.
    Status,

    /// Fill `<root>/bin` and put it on this user's PATH.
    ///
    /// Idempotent, and it says which of the two it did: a profile that already carries the line is
    /// left exactly as it is.
    Install,

    /// Take `<root>/bin` back off this user's PATH.
    ///
    /// The commands stay in the directory — they are inside the home, and removing the home is what
    /// removes them.
    Uninstall,
}

/// `mix elevation …` — one subcommand per `elevation.*` method, and nothing that is not one.
///
/// **There is no `mix elevation enqueue`, and there will not be.** What needs an administrator's
/// permission is decided by the operation that needs it — creating a site, issuing a certificate —
/// and a command that let a person put an arbitrary privileged operation in the queue would be a
/// client deciding what runs as root.
#[derive(Debug, Subcommand)]
enum ElevationCommand {
    /// Say what is waiting for permission, and what each of them will change.
    Status,

    /// Ask once, for everything that is waiting.
    ///
    /// One prompt for the whole queue: `.claude/decisions/0005-on-demand-elevation.md` calls asking
    /// inside a loop a defect. Saying no is a normal answer — the list stays, and this command can
    /// be run again later.
    Grant {
        /// Say yes in advance, instead of being asked.
        ///
        /// What it skips is the question, never the screen: every operation and what it will change
        /// is printed either way. It exists for the caller that cannot be asked — a script, a CI
        /// step, anything with no terminal behind it — and for `--json`, which has no way to answer.
        #[arg(long)]
        yes: bool,

        /// Answer as soon as the prompt has been raised, without waiting for it.
        #[arg(long)]
        no_wait: bool,
    },

    /// Forget an operation that is waiting, so it is never asked about again.
    Drop {
        /// Which one, as `mix elevation status` numbers them.
        op: Option<i64>,

        /// Forget all of them.
        ///
        /// Its own flag rather than "drop with nothing named": emptying the queue by typing less is
        /// exactly the mistake worth making impossible.
        #[arg(long, conflicts_with = "op")]
        all: bool,
    },
}

/// `mix runtime …` — one subcommand per `runtime.*` method, and nothing that is not one.
///
/// The two listings are two commands rather than one with a flag, because they answer two different
/// questions — what is here, and what could be — and the second one reaches the network while the
/// first reads a table. A `--available` on the first would hide that difference behind a flag.
#[derive(Debug, Subcommand)]
enum RuntimeCommand {
    /// List the runtimes installed in this home.
    List(Kind),

    /// List the versions the package index offers for this machine.
    Available(Kind),

    /// Download and install one version.
    Install {
        #[command(flatten)]
        runtime: Which,

        /// Return once the daemon has accepted the install, rather than once it has finished.
        ///
        /// `mix` waits by default, because `mix runtime install php 8.3.33 && …` is a sentence about
        /// PHP being there. What comes back instead is the job, which `mix job wait` can be pointed
        /// at later.
        #[arg(long)]
        no_wait: bool,
    },

    /// Remove one installed version.
    ///
    /// Refused while a registered project pins it, naming the projects, and while the php-fpm pool
    /// that runs out of it is running. `--force` crosses the first and never the second.
    Uninstall {
        #[command(flatten)]
        runtime: Which,

        /// Remove it even though a registered project pins it.
        #[arg(long)]
        force: bool,
    },

    /// Make one installed version the one its kind resolves to.
    Default {
        #[command(flatten)]
        runtime: Which,
    },

    /// Which extensions an installed build loads.
    ///
    /// Under `runtime` rather than as `mix php ext …`, which is what
    /// `.claude/features/runtime-versions.md` wrote: a per-language command family for one language
    /// is a noun this CLI would then owe every other runtime.
    Ext {
        #[command(subcommand)]
        command: ExtCommand,
    },

    /// Say which installed version a directory uses, and why that one.
    ///
    /// The question `php -v` answers by running, asked without running anything — and the reason is
    /// the point of it: what a person wants when the version surprises them is which of the four
    /// sources decided it.
    Resolve {
        /// Which language.
        #[arg(value_name = "RUNTIME", value_parser = runtime_kind)]
        kind: RuntimeKind,

        /// Use this version or range instead of what the directory says.
        ///
        /// Exact (`8.3.33`), a series (`8.3`, `8`) or a caret (`^8.3`), resolved against what is
        /// installed and never against what could be downloaded.
        #[arg(long, value_name = "VERSION", value_parser = version_constraint)]
        version: Option<VersionConstraint>,

        /// Resolve as if this were the working directory.
        #[arg(long, value_name = "DIR")]
        cwd: Option<PathBuf>,
    },
}

/// `mix runtime ext …` — the two `runtime.*_extension*` methods, as three verbs.
///
/// Three rather than two because `enable`/`disable` is what a person types; the wire carries one
/// method with a boolean, which is the daemon's shape rather than the sentence's.
#[derive(Debug, Subcommand)]
enum ExtCommand {
    /// List what this build has, and why each is on or off.
    List(WhichPhp),

    /// Load one on every PHP process of this version.
    Enable {
        /// The extension, as the listing spells it.
        #[arg(value_name = "EXTENSION")]
        name: String,

        #[command(flatten)]
        php: WhichPhp,
    },

    /// Stop loading one.
    Disable {
        /// The extension, as the listing spells it.
        #[arg(value_name = "EXTENSION")]
        name: String,

        #[command(flatten)]
        php: WhichPhp,
    },
}

/// Which PHP a `mix runtime ext` command is about.
///
/// **The default is not this client's to invent.** With no `--php`, the version is whatever
/// `runtime.resolve` answers for this directory — the same order the shim and the GUI get, decided
/// once, in the daemon.
#[derive(Debug, clap::Args)]
struct WhichPhp {
    /// The version, exactly as it is installed. Defaults to the one `php` resolves to here.
    #[arg(long = "php", value_name = "VERSION", value_parser = runtime_version)]
    version: Option<PackageVersion>,
}

/// `mix package …` — one subcommand per `package.*` method.
///
/// **Not runtimes.** PHP and Node are `mix runtime`, which has a default version and a shim behind
/// it; these are the servers a *service* is an instance of. What a package becomes once it is
/// installed is `mix service create`.
#[derive(Debug, Subcommand)]
enum PackageCommand {
    /// List the packages installed in this home.
    List(Named),

    /// List the versions the package index offers for this machine.
    ///
    /// Only packages this build knows how to configure and run: an entry MixEngine has no recipe for
    /// would unpack into a directory nothing could start.
    Available(Named),

    /// Download and install one version.
    Install {
        #[command(flatten)]
        package: WhichPackage,

        /// Return once the daemon has accepted the install, rather than once it has finished.
        #[arg(long)]
        no_wait: bool,
    },

    /// Remove one installed version.
    ///
    /// Refused while a service is an instance of it, naming the services — `mix service delete` is
    /// what frees it, and deleting a service keeps its data directory.
    Uninstall {
        #[command(flatten)]
        package: WhichPackage,
    },
}

/// Which package a listing is about, or every package.
#[derive(Debug, clap::Args)]
struct Named {
    /// Only this package. Every one of them when it is left out.
    #[arg(long, value_name = "PACKAGE")]
    package: Option<String>,
}

/// Which package a command acts on, which is the same question twice.
#[derive(Debug, clap::Args)]
struct WhichPackage {
    /// Which package, as `mix package available` lists it.
    #[arg(value_name = "PACKAGE")]
    package: String,

    /// Which version, exactly as `mix package available` lists it.
    #[arg(value_name = "VERSION", value_parser = runtime_version)]
    version: PackageVersion,
}

/// Which kind a listing is about, or every kind.
#[derive(Debug, clap::Args)]
struct Kind {
    /// Only this language. Every one of them when it is left out.
    #[arg(long, value_name = "RUNTIME", value_parser = runtime_kind)]
    kind: Option<RuntimeKind>,
}

/// Which runtime a command acts on, which is the same question three times.
#[derive(Debug, clap::Args)]
struct Which {
    /// Which language.
    #[arg(value_name = "RUNTIME", value_parser = runtime_kind)]
    kind: RuntimeKind,

    /// Which version, exactly as `mix runtime available` lists it.
    ///
    /// Required, and deliberately not a constraint like `8.3`, even now that the daemon can read
    /// one: choosing a version from a range is *resolution*, it answers with what is installed, and
    /// none of these three commands is asking that question — an install picking `8.3`'s newest
    /// would be picking between versions none of which are here yet. `mix runtime resolve` is where
    /// a range belongs.
    #[arg(value_name = "VERSION", value_parser = runtime_version)]
    version: PackageVersion,
}

/// `mix job …` — one subcommand per `job.*` method.
#[derive(Debug, Subcommand)]
enum JobCommand {
    /// List what this home has run, newest first.
    List {
        /// Only jobs in this state.
        #[arg(long, value_name = "STATE", value_parser = job_state)]
        state: Option<JobState>,

        /// At most this many.
        #[arg(long, short = 'n', value_name = "COUNT", default_value_t = 50)]
        limit: u32,
    },

    /// Describe one job.
    Status {
        /// The job, as `mix job list` numbers them.
        #[arg(value_name = "JOB")]
        job: i64,
    },

    /// Wait for a job to finish.
    ///
    /// **Answers when the job ends or when the wait runs out**, and the second is not an error: what
    /// comes back is the job as it stands. The exit status is what a script branches on — non-zero
    /// for a job that failed, and for one that has not finished yet.
    Wait {
        /// The job to wait for.
        #[arg(value_name = "JOB")]
        job: i64,

        /// How long to wait. The daemon caps what it grants.
        #[arg(long, value_name = "SECONDS", default_value_t = 30)]
        timeout: u64,
    },

    /// Ask a running job to stop.
    ///
    /// Cancellation is cooperative, so what comes back may still say `running`: the work ends when
    /// it next looks. Cancelling a job that has already ended is not an error.
    Cancel {
        /// The job to cancel.
        #[arg(value_name = "JOB")]
        job: i64,
    },
}

/// `mix daemon …` — the daemon as a thing in itself, rather than as what answers about services.
///
/// `status` is deliberately not here and stays `mix status`: it is the first command anybody types,
/// and moving it would be renaming the one command that already exists to make room for a namespace.
#[derive(Debug, Subcommand)]
enum DaemonCommand {
    /// Stop the services this home is running, then stop the daemon.
    Stop,
}

/// `mix service …` — one subcommand per `service.*` method, and nothing that is not one.
#[derive(Debug, Subcommand)]
enum ServiceCommand {
    /// List every declared service and what it is doing.
    List,

    /// Describe one service.
    ///
    /// The id is required, where `start` and the rest take an optional one: a status with no
    /// subject is a `list` that was typed wrongly, and answering it as a list would hide that.
    Status {
        /// The service to describe.
        #[arg(value_name = "SERVICE", value_parser = service_id)]
        service: ServiceId,
    },

    /// Print what a service has been printing.
    ///
    /// The one `mix service` subcommand that is not a `service.*` method: output is a stream, and a
    /// JSON-RPC call cannot be one — see
    /// [ADR 0009](../../../.claude/decisions/0009-logs-travel-on-their-own-stream.md).
    Logs {
        /// The service to read.
        #[arg(value_name = "SERVICE", value_parser = service_id)]
        service: ServiceId,

        /// How many of the lines already printed to begin with.
        #[arg(long, short = 'n', value_name = "LINES", default_value_t = 200)]
        lines: usize,

        /// Keep printing as the service prints, rather than stopping at what it already said.
        ///
        /// Survives the service crashing and being restarted: what is being followed is the
        /// service, not one run of its process.
        #[arg(long, short)]
        follow: bool,
    },

    /// Create a service from an installed package.
    ///
    /// The part of the id before `@` is the package it is an instance of, which is why there is no
    /// separate argument for it: `mariadb@main` is an instance of `mariadb`, and a package that runs
    /// only once — Caddy — is named without an `@` at all.
    Create {
        /// The service to create.
        #[arg(value_name = "SERVICE", value_parser = service_id)]
        service: ServiceId,

        /// Which installed version of its package to run.
        #[arg(value_name = "VERSION", value_parser = runtime_version)]
        version: PackageVersion,

        /// The port it listens on. The recipe's own default when it is left out.
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,

        /// The address it binds. `127.0.0.1` when it is left out.
        #[arg(long, value_name = "ADDR")]
        bind: Option<String>,

        /// Where its data lives. The home's own layout when it is left out, and never a directory
        /// another service already keeps its data in.
        #[arg(long, value_name = "DIR")]
        data_dir: Option<String>,

        /// Start it whenever the daemon starts.
        #[arg(long)]
        autostart: bool,
    },

    /// Delete a service, keeping its data directory.
    ///
    /// Takes the row and the configuration generated from it. **Never the data** — that is somebody's
    /// databases, and the answer names the directory that was left so nobody has to go looking.
    Delete {
        /// The service to delete.
        #[arg(value_name = "SERVICE", value_parser = service_id)]
        service: ServiceId,

        /// Delete it even though a site declares it.
        #[arg(long)]
        force: bool,
    },

    /// Start a service, and everything it depends on.
    Start(Target),

    /// Stop a service, and everything that depends on it.
    Stop(Target),

    /// Stop a service and what depends on it, then start that same set again.
    Restart(Target),
}

/// What `start`, `stop` and `restart` take, which is the same question three times.
#[derive(Debug, clap::Args)]
struct Target {
    /// The service to act on. Every declared service when it is left out.
    ///
    /// Naming one does not mean acting on one — a plan is the transitive set — and what the daemon
    /// walked comes back in the answer.
    #[arg(value_name = "SERVICE", value_parser = service_id)]
    service: Option<ServiceId>,

    /// Return once the daemon has accepted the plan, rather than once it has walked it.
    ///
    /// `mix` waits by default, because `mix service start db && …` is a sentence about the database
    /// being up: an answer sent before the walk would exit `0` for a service that never came up.
    #[arg(long)]
    no_wait: bool,
}

/// A service id from the command line, refused here rather than at the daemon.
///
/// Not the client deciding anything — [`ServiceId::parse`] is the daemon's own rule, from the crate
/// that owns the vocabulary — it is only where the answer is cheapest: a typo should not start a
/// daemon and travel over a socket to be told it is a typo.
fn service_id(value: &str) -> Result<ServiceId, String> {
    ServiceId::parse(value).map_err(|error| error.to_string())
}

/// A runtime kind from the command line, refused here for [`service_id`]'s reason.
///
/// The list is in the message because this is a closed set of four and a typo is the whole of what
/// can go wrong: `mix runtime install pph 8.3.33` should say what the four are rather than send
/// somebody to `--help`.
fn runtime_kind(value: &str) -> Result<RuntimeKind, String> {
    RuntimeKind::parse(value).ok_or_else(|| {
        format!(
            "{value:?} is not a runtime MixEngine manages — it knows {}",
            RuntimeKind::ALL.map(RuntimeKind::as_str).join(", ")
        )
    })
}

/// A version from the command line. [`PackageVersion::parse`] is the daemon's own rule.
fn runtime_version(value: &str) -> Result<PackageVersion, String> {
    PackageVersion::parse(value).map_err(|error| error.to_string())
}

/// A version *or a range* from the command line, which only `mix runtime resolve` takes.
fn version_constraint(value: &str) -> Result<VersionConstraint, String> {
    VersionConstraint::parse(value).map_err(|error| error.to_string())
}

/// A job state from the command line, for `mix job list --state`.
fn job_state(value: &str) -> Result<JobState, String> {
    JobState::parse(value).ok_or_else(|| {
        format!(
            "{value:?} is not a job state — a job is {}",
            JobState::ALL.map(JobState::as_str).join(", ")
        )
    })
}

fn main() -> ExitCode {
    let args = Args::parse();
    let json = args.json;

    match run(args) {
        Ok(code) => code,
        Err(error) => {
            report(&error, json);
            ExitCode::FAILURE
        }
    }
}

/// Everything the command does, with one way out for a failure.
///
/// `current_thread`, because a client sends one request and exits: the multi-thread runtime the
/// daemon needs would be several worker threads started to wait on a single socket, paid for on
/// every `mix` invocation in a shell prompt.
#[tokio::main(flavor = "current_thread")]
async fn run(args: Args) -> Result<ExitCode, Error> {
    let host = mixengine_platform::host();
    let root = home::resolve_root(args.home.as_deref(), host.as_ref())?;
    let endpoint = home::endpoint(&root)?;

    // Prepared either way, and not because it is free: deciding *whether* to autostart here rather
    // than inside the client is what keeps "this run may start a daemon" a property of the command
    // line and not of a code path somewhere below.
    let autostart = (!args.no_autostart).then(|| Autostart::for_home(&root));

    // Dialled by the command and not here. Every command there is today needs a daemon, but that is
    // a fact about `status` rather than about `mix` — `mix doctor` (T47) has to be able to describe
    // a home that has none — and connecting above the match would have made starting one the first
    // thing every future command did, whether or not it had anything to ask.
    match args.command {
        Command::Status => status(&endpoint, autostart.as_ref(), args.json).await,
        // **Never autostarts, whatever the flags say**, and it is the one command that decides this
        // for itself: starting a daemon in order to ask it to stop is a machine left exactly as it
        // was found, one process later. A home with nothing running is told so as the wire error for
        // a daemon that is not there, which is the same sentence every other command gets.
        Command::Daemon {
            command: DaemonCommand::Stop,
        } => daemon_stop(&endpoint, args.json).await,
        Command::Runtime { command } => {
            runtime(command, &endpoint, autostart.as_ref(), args.json).await
        }
        Command::Package { command } => {
            package(command, &endpoint, autostart.as_ref(), args.json).await
        }
        Command::Project { command } => {
            project(command, &endpoint, autostart.as_ref(), args.json).await
        }
        Command::Site { command } => site(command, &endpoint, autostart.as_ref(), args.json).await,
        Command::Doctor {
            repair,
            yes,
            no_wait,
        } => match repair {
            true => self_repair(&endpoint, autostart.as_ref(), args.json, yes, no_wait).await,
            false => doctor(&endpoint, autostart.as_ref(), args.json).await,
        },
        Command::Domain { command } => {
            domain(command, &endpoint, autostart.as_ref(), args.json).await
        }
        Command::Service { command } => {
            service(command, &endpoint, autostart.as_ref(), args.json).await
        }
        Command::Job { command } => job(command, &endpoint, autostart.as_ref(), args.json).await,
        Command::Path { command } => path(command, &endpoint, autostart.as_ref(), args.json).await,
        Command::Elevation { command } => {
            elevation(command, &endpoint, autostart.as_ref(), args.json).await
        }
    }
}

/// `mix project …`: one call, one rendering.
async fn project(
    command: ProjectCommand,
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    match command {
        ProjectCommand::Create { root, name, pins } => {
            let create = ProjectCreate {
                root: here(root)?.display().to_string(),
                name,
                pins: (!pins.is_empty()).then(|| pins.into_iter().collect()),
            };
            let detail: ProjectDetail =
                ask(&mut client, rpc::method::PROJECT_CREATE, encode(&create)).await?;
            emit(&rendered(json, &detail, || render::project_detail(&detail)))?;
        }

        ProjectCommand::List => {
            let list: ProjectList = ask(&mut client, rpc::method::PROJECT_LIST, None).await?;
            emit(&rendered(json, &list, || render::project_list(&list)))?;
        }

        ProjectCommand::Show { project } => {
            let query = ProjectQuery {
                project: which(project)?,
            };
            let detail: ProjectDetail =
                ask(&mut client, rpc::method::PROJECT_SHOW, encode(&query)).await?;
            emit(&rendered(json, &detail, || render::project_detail(&detail)))?;
        }

        ProjectCommand::Update {
            project,
            name,
            root,
            pins,
            clear_pins,
        } => {
            let update = ProjectUpdate {
                project: which(project)?,
                name,
                root: root.map(|root| root.display().to_string()),
                pins: match (clear_pins, pins.is_empty()) {
                    (true, _) => Some(std::collections::BTreeMap::new()),
                    (false, true) => None,
                    (false, false) => Some(pins.into_iter().collect()),
                },
            };
            let detail: ProjectDetail =
                ask(&mut client, rpc::method::PROJECT_UPDATE, encode(&update)).await?;
            emit(&rendered(json, &detail, || render::project_detail(&detail)))?;
        }

        ProjectCommand::Delete { project } => {
            let query = ProjectQuery {
                project: which(project)?,
            };
            let removal: ProjectRemoval =
                ask(&mut client, rpc::method::PROJECT_DELETE, encode(&query)).await?;
            emit(&rendered(json, &removal, || {
                render::project_removal(&removal)
            }))?;
        }

        ProjectCommand::Export { project } => {
            let query = ProjectQuery {
                project: which(project)?,
            };
            let exported: ProjectExport =
                ask(&mut client, rpc::method::PROJECT_EXPORT, encode(&query)).await?;
            emit(&rendered(json, &exported, || {
                render::project_export(&exported)
            }))?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// A name if one was typed, and this directory if none was.
///
/// **Not a default this client invents**: the path is sent as it stands and the daemon does the
/// walking, which is the same answer the shim gets.
/// `mix site …` — ask, and render what came back.
///
/// **Nothing is decided here.** No domain is validated, no doc root is made relative and no kind is
/// defaulted: all of that is the daemon's, and a `mix` that could refuse what the GUI could not
/// would be the first bug `CLAUDE.md` names.
/// `mix doctor` — roadmap task **T47a**.
async fn doctor(
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    let report: DoctorReport = ask(
        &mut client,
        rpc::method::DAEMON_DOCTOR,
        encode(&serde_json::json!({})),
    )
    .await?;

    emit(&rendered(json, &report, || render::doctor(&report)))?;

    // **The exit code is the report and not the call.** A doctor that exits 0 because it managed to
    // ask cannot be used in a script, and the shell is where the second question gets asked.
    Ok(if report.has_a_problem() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// `mix doctor --repair` — roadmap task **T47b**.
///
/// **Two calls, and T64 is the reason.** The first repairs what needs no privilege and *queues* what
/// does; then the batch is read, shown and answered before the second raises the prompt — which is
/// the rule `mix elevation grant` obeys, over the same queue, for the same reason. `--yes` collapses
/// the two into one call by saying so on the command line, which is a person answering in advance
/// rather than a client skipping the question.
async fn self_repair(
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
    yes: bool,
    no_wait: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    let repaired: RepairReport = ask(
        &mut client,
        rpc::method::DAEMON_DOCTOR_REPAIR,
        encode(&DoctorRepair { grant: yes }),
    )
    .await?;

    emit(&rendered(json, &repaired, || render::repair(&repaired)))?;

    // **The exit code is the report and not the call**, as `mix doctor`'s is: everything found was
    // either repaired or queued, or it was not.
    let outcome = match repaired.left_something_undone() {
        true => ExitCode::FAILURE,
        false => ExitCode::SUCCESS,
    };

    let started = match repaired.granting {
        // `--yes`: the daemon raised it because the person said so before it ran.
        Some(started) => started,

        None => {
            let waiting: ElevationStatus =
                ask(&mut client, rpc::method::ELEVATION_STATUS, None).await?;

            // Nothing to grant is the ordinary end of a repair: either nothing needed an
            // administrator, or a machine that cannot prompt at all left the queue where it was.
            if waiting.pending.is_empty() {
                return Ok(outcome);
            }

            if !confirmed(&waiting, json)? {
                // Saying no is an answer and not a failure — the same rule as `mix elevation grant`.
                // Nothing was dropped, so the same command works when the person is ready.
                return Ok(outcome);
            }

            ask(&mut client, rpc::method::ELEVATION_GRANT, None).await?
        }
    };

    if no_wait {
        emit(&rendered(json, &started, || render::job_status(&started)))?;
        return Ok(outcome);
    }

    let finished = follow(&mut client, started, json).await?;
    emit(&rendered(json, &finished, || render::job_status(&finished)))?;

    // A grant that failed is a machine still holding what it held, whatever the repairs did.
    Ok(match render::job_succeeded(&finished) {
        true => outcome,
        false => ExitCode::FAILURE,
    })
}

/// `mix domain` — roadmap task **T46**.
async fn domain(
    command: DomainCommand,
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    match command {
        DomainCommand::Add {
            domain,
            site,
            accept_risky_tld,
        } => {
            let add = DomainAdd {
                site: SiteRef::Domain(site),
                domain,
                accept_risky_tld,
            };
            let detail: SiteDetail =
                ask(&mut client, rpc::method::DOMAIN_ADD, encode(&add)).await?;
            emit(&rendered(json, &detail, || render::site_detail(&detail)))?;
        }

        DomainCommand::Remove { domain } => {
            let remove = DomainRemove { domain };
            let detail: SiteDetail =
                ask(&mut client, rpc::method::DOMAIN_REMOVE, encode(&remove)).await?;
            emit(&rendered(json, &detail, || render::site_detail(&detail)))?;
        }

        DomainCommand::Status { domain } => {
            let query = DomainStatusQuery { domain };
            let report: DomainStatusReport =
                ask(&mut client, rpc::method::DOMAIN_DNS_STATUS, encode(&query)).await?;
            emit(&rendered(json, &report, || render::domain_status(&report)))?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

async fn site(
    command: SiteCommand,
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    match command {
        SiteCommand::Create {
            project,
            domains,
            doc_root,
            kind,
            upstream,
            port,
            pool,
            services,
            https,
            accept_risky_tld,
        } => {
            let create = SiteCreate {
                project: whose(project)?,
                domains: (!domains.is_empty()).then_some(domains),
                doc_root,
                kind: site_kind(kind, upstream, port, pool)?,
                services: (!services.is_empty()).then_some(services),
                https,
                accept_risky_tld,
            };
            let creation: SiteCreation =
                ask(&mut client, rpc::method::SITE_CREATE, encode(&create)).await?;
            emit(&rendered(json, &creation, || {
                render::site_detail(&creation.site)
            }))?;
        }

        SiteCommand::List { project } => {
            let query = SiteListQuery {
                project: project.map(ProjectRef::Name),
            };
            let list: SiteList = ask(&mut client, rpc::method::SITE_LIST, encode(&query)).await?;
            emit(&rendered(json, &list, || render::site_list(&list)))?;
        }

        SiteCommand::Show { site } => {
            let query = SiteQuery {
                site: which_site(site)?,
            };
            let detail: SiteDetail =
                ask(&mut client, rpc::method::SITE_SHOW, encode(&query)).await?;
            emit(&rendered(json, &detail, || render::site_detail(&detail)))?;
        }

        SiteCommand::Start { site } => {
            let query = SiteQuery {
                site: which_site(site)?,
            };
            let detail: SiteDetail =
                ask(&mut client, rpc::method::SITE_START, encode(&query)).await?;
            emit(&rendered(json, &detail, || render::site_detail(&detail)))?;
        }

        SiteCommand::Stop { site } => {
            let query = SiteQuery {
                site: which_site(site)?,
            };
            let detail: SiteDetail =
                ask(&mut client, rpc::method::SITE_STOP, encode(&query)).await?;
            emit(&rendered(json, &detail, || render::site_detail(&detail)))?;
        }

        SiteCommand::Update {
            site,
            domains,
            doc_root,
            kind,
            upstream,
            port,
            pool,
            services,
            https,
            state,
            accept_risky_tld,
        } => {
            let update = SiteUpdate {
                site: which_site(site)?,
                domains: (!domains.is_empty()).then_some(domains),
                doc_root,
                kind: site_kind(kind, upstream, port, pool)?,
                services: (!services.is_empty()).then_some(services),
                https,
                state: state.map(|state| match state {
                    SiteStateArg::Enabled => SiteState::Enabled,
                    SiteStateArg::Disabled => SiteState::Disabled,
                }),
                accept_risky_tld,
            };
            let detail: SiteDetail =
                ask(&mut client, rpc::method::SITE_UPDATE, encode(&update)).await?;
            emit(&rendered(json, &detail, || render::site_detail(&detail)))?;
        }

        SiteCommand::Delete { site } => {
            let query = SiteQuery {
                site: which_site(site)?,
            };
            let removal: SiteRemoval =
                ask(&mut client, rpc::method::SITE_DELETE, encode(&query)).await?;
            emit(&rendered(json, &removal, || render::site_removal(&removal)))?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// The four flags a kind is spelled with on a command line, as the one value the API takes.
///
/// Assembly rather than logic: which flags a kind needs is decided by clap's `required_if_eq`, and
/// what a kind *means* is the daemon's. What this does is put a tagged enum back together out of
/// the flat arguments a shell can carry.
fn site_kind(
    kind: Option<SiteKindArg>,
    upstream: Option<String>,
    port: Option<u16>,
    pool: Option<ServiceId>,
) -> Result<Option<SiteKind>, Error> {
    let missing = |flag: &str, because: &str| {
        Error::new(ErrorCode::InvalidArgument, format!("{flag} {because}"))
    };

    Ok(match kind {
        // `--pool` on its own says php-fpm without saying it, which is the only kind a pool
        // belongs to; nothing named at all leaves the whole decision to the daemon.
        None => pool.map(|pool| SiteKind::PhpFpm { pool: Some(pool) }),
        Some(SiteKindArg::PhpFpm) => Some(SiteKind::PhpFpm { pool }),
        Some(SiteKindArg::Static) => Some(SiteKind::Static),
        Some(SiteKindArg::ReverseProxy) => Some(SiteKind::ReverseProxy {
            upstream: upstream.ok_or_else(|| missing("--upstream", "says where to forward to"))?,
        }),
        Some(SiteKindArg::NodeApp) => Some(SiteKind::NodeApp {
            port: port.ok_or_else(|| missing("--port", "says where the node process listens"))?,
        }),
    })
}

/// Which site, defaulting to the directory this `mix` was run in.
fn which_site(site: WhichSite) -> Result<SiteRef, Error> {
    match site.domain {
        Some(domain) => Ok(SiteRef::Domain(domain)),
        None => Ok(SiteRef::Path(here(None)?.display().to_string())),
    }
}

/// Which project a `--project` names, defaulting to the directory this `mix` was run in.
fn whose(project: Option<String>) -> Result<ProjectRef, Error> {
    match project {
        Some(name) => Ok(ProjectRef::Name(name)),
        None => Ok(ProjectRef::Path(here(None)?.display().to_string())),
    }
}

fn which(project: WhichProject) -> Result<ProjectRef, Error> {
    match project.name {
        Some(name) => Ok(ProjectRef::Name(name)),
        None => Ok(ProjectRef::Path(here(None)?.display().to_string())),
    }
}

/// A directory argument, or the one this process is in.
fn here(given: Option<PathBuf>) -> Result<PathBuf, Error> {
    match given {
        Some(path) if path.is_absolute() => Ok(path),
        Some(path) => working_directory().map(|cwd| cwd.join(path)),
        None => working_directory(),
    }
}

/// Where this `mix` was run, which is what a project reference defaults to.
fn working_directory() -> Result<PathBuf, Error> {
    std::env::current_dir().map_err(|error| {
        Error::new(
            ErrorCode::Io,
            format!("this process has no working directory: {error}"),
        )
    })
}

/// `php=^8.3` — one pin, as a person types it.
fn pin(value: &str) -> Result<(RuntimeKind, VersionConstraint), String> {
    let (kind, constraint) = value
        .split_once('=')
        .ok_or_else(|| format!("`{value}` is not `<runtime>=<version>`"))?;

    Ok((runtime_kind(kind)?, version_constraint(constraint)?))
}

/// `mix package …`: one call, one rendering — except the install, which follows a job.
async fn package(
    command: PackageCommand,
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    match command {
        PackageCommand::List(Named { package }) => {
            let filter = PackageFilter { package };
            let list: PackageList =
                ask(&mut client, rpc::method::PACKAGE_LIST, encode(&filter)).await?;
            emit(&rendered(json, &list, || render::package_list(&list)))?;
        }

        PackageCommand::Available(Named { package }) => {
            let filter = PackageFilter { package };
            let catalogue: PackageCatalogue = ask(
                &mut client,
                rpc::method::PACKAGE_LIST_AVAILABLE,
                encode(&filter),
            )
            .await?;
            emit(&rendered(json, &catalogue, || {
                render::package_catalogue(&catalogue)
            }))?;
        }

        PackageCommand::Install { package, no_wait } => {
            let target = PackageTarget {
                package: package.package,
                version: package.version,
            };
            let started: JobSummary =
                ask(&mut client, rpc::method::PACKAGE_INSTALL, encode(&target)).await?;

            if no_wait {
                emit(&rendered(json, &started, || render::job_status(&started)))?;
                return Ok(ExitCode::SUCCESS);
            }

            let finished = follow(&mut client, started, json).await?;
            emit(&rendered(json, &finished, || render::job_status(&finished)))?;

            return Ok(match render::job_succeeded(&finished) {
                true => ExitCode::SUCCESS,
                false => ExitCode::FAILURE,
            });
        }

        PackageCommand::Uninstall { package } => {
            let target = PackageTarget {
                package: package.package,
                version: package.version,
            };
            let removal: PackageRemoval =
                ask(&mut client, rpc::method::PACKAGE_UNINSTALL, encode(&target)).await?;
            emit(&rendered(json, &removal, || {
                render::package_removal(&removal)
            }))?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// `mix path …`: one call, one rendering.
///
/// No exit code of its own — unlike `mix service start`, every one of these either did what it said
/// or failed outright, and there is no partial answer for a status to describe. A `bin/` with a
/// leftover in it is reported in the rendering and is not a failure: the commands that should be
/// there are there.
async fn path(
    command: PathCommand,
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    let (method, pathed) = match command {
        PathCommand::Status => (rpc::method::PATH_STATUS, render::Pathed::Asked),
        PathCommand::Install => (rpc::method::PATH_INSTALL, render::Pathed::Installed),
        PathCommand::Uninstall => (rpc::method::PATH_UNINSTALL, render::Pathed::Uninstalled),
    };

    let report: PathReport = ask(&mut client, method, None).await?;
    emit(&rendered(json, &report, || {
        render::path_report(pathed, &report)
    }))?;

    Ok(ExitCode::SUCCESS)
}

/// `mix elevation …`: one call, one rendering — except the grant, which is one call and a wait.
async fn elevation(
    command: ElevationCommand,
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    match command {
        ElevationCommand::Status => {
            let status: ElevationStatus =
                ask(&mut client, rpc::method::ELEVATION_STATUS, None).await?;
            emit(&rendered(json, &status, || {
                render::elevation_status(&status)
            }))?;

            Ok(ExitCode::SUCCESS)
        }

        ElevationCommand::Grant { yes, no_wait } => {
            // Roadmap task T64: what is about to be allowed is read before it is allowed. The
            // ordering is a property of the API rather than of this function — the daemon never
            // raises a prompt on its own initiative, so there is a moment between knowing the batch
            // and asking for it, and this is what happens in that moment.
            let waiting: ElevationStatus =
                ask(&mut client, rpc::method::ELEVATION_STATUS, None).await?;

            // An empty queue is `elevation.grant`'s own refusal to make, and it is left to it. What
            // is skipped is the question: there is nothing to put in front of somebody.
            if !waiting.pending.is_empty() && !yes && !confirmed(&waiting, json)? {
                // Saying no is an answer and not a failure — `.claude/decisions/0005-on-demand-
                // elevation.md`. Nothing was written and nothing was dropped, so the same command
                // works when the person is ready.
                return Ok(ExitCode::SUCCESS);
            }

            let started: JobSummary = ask(&mut client, rpc::method::ELEVATION_GRANT, None).await?;

            if no_wait {
                emit(&rendered(json, &started, || render::job_status(&started)))?;
                return Ok(ExitCode::SUCCESS);
            }

            let finished = follow(&mut client, started, json).await?;
            emit(&rendered(json, &finished, || render::job_status(&finished)))?;

            Ok(match render::job_succeeded(&finished) {
                true => ExitCode::SUCCESS,
                false => ExitCode::FAILURE,
            })
        }

        ElevationCommand::Drop { op, all } => {
            // Neither named is a person who has not decided which. Refused here rather than turned
            // into "all", which is the reading that empties a queue somebody meant to prune.
            if op.is_none() && !all {
                return Err(Error::new(
                    ErrorCode::InvalidArgument,
                    "name an operation to forget, or pass --all to forget every one of them",
                ));
            }

            let asked = ElevationDrop {
                op: op.map(PendingOpId),
            };
            let left: ElevationStatus =
                ask(&mut client, rpc::method::ELEVATION_DROP, encode(&asked)).await?;

            emit(&rendered(json, &left, || render::elevation_status(&left)))?;

            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Show what a grant would allow, ask about it, and answer whether to go on — roadmap task **T64**.
///
/// The screen is printed whichever way this ends, because it is the point: a person is being asked
/// to give an administrator's permission to a batch of operations, and the batch is what they are
/// judging. The question comes after it, never instead of it.
///
/// # Errors
///
/// When there is nobody to ask — `--json`, or a standard input at end of file. Both are refused
/// rather than assumed either way: yes would raise a dialog on a machine nobody is sitting at, and
/// no would be a decline the caller could not tell from a grant that happened.
fn confirmed(waiting: &ElevationStatus, json: bool) -> Result<bool, Error> {
    if json {
        return Err(unanswered());
    }

    match confirm::ask(&format!(
        "{}\ncontinue? [y/N] ",
        render::elevation_prompt(waiting)
    )) {
        confirm::Answer::Yes => Ok(true),

        confirm::Answer::No => {
            // On stderr, beside the question it answers. Stdout carries what a command was asked
            // for, and this run was asked for a grant that is not going to happen.
            let _ = writeln!(
                std::io::stderr(),
                "nothing was asked for; run `mix elevation grant` again when you are ready"
            );

            Ok(false)
        }

        confirm::Answer::Unanswerable => Err(unanswered()),
    }
}

/// There was nobody to put the question to.
fn unanswered() -> Error {
    Error::new(
        ErrorCode::InvalidArgument,
        "nothing answered the question, so nothing was asked of the operating system either",
    )
    .with_hint("pass `--yes` to answer in advance")
}

/// `mix runtime …`: one call, one rendering — except the install, which is one call and a wait.
async fn runtime(
    command: RuntimeCommand,
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    match command {
        RuntimeCommand::List(Kind { kind }) => {
            let filter = RuntimeFilter { kind };
            let list: RuntimeList = ask(
                &mut client,
                rpc::method::RUNTIME_LIST_INSTALLED,
                encode(&filter),
            )
            .await?;
            emit(&rendered(json, &list, || render::runtime_list(&list)))?;
        }

        RuntimeCommand::Available(Kind { kind }) => {
            let filter = RuntimeFilter { kind };
            let catalogue: RuntimeCatalogue = ask(
                &mut client,
                rpc::method::RUNTIME_LIST_AVAILABLE,
                encode(&filter),
            )
            .await?;
            emit(&rendered(json, &catalogue, || {
                render::runtime_catalogue(&catalogue)
            }))?;
        }

        RuntimeCommand::Install { runtime, no_wait } => {
            return install(&mut client, target(runtime), no_wait, json).await;
        }

        RuntimeCommand::Uninstall { runtime, force } => {
            let asked = RuntimeUninstall {
                target: target(runtime),
                force,
            };
            let removal: RuntimeRemoval =
                ask(&mut client, rpc::method::RUNTIME_UNINSTALL, encode(&asked)).await?;
            emit(&rendered(json, &removal, || {
                render::runtime_removal(&removal)
            }))?;
        }

        RuntimeCommand::Default { runtime } => {
            let summary: RuntimeSummary = ask(
                &mut client,
                rpc::method::RUNTIME_SET_DEFAULT,
                encode(&target(runtime)),
            )
            .await?;
            emit(&rendered(json, &summary, || {
                render::runtime_summary(&summary)
            }))?;
        }

        RuntimeCommand::Ext { command } => {
            let (php, choice) = match command {
                ExtCommand::List(php) => (php, None),
                ExtCommand::Enable { name, php } => (php, Some((name, true))),
                ExtCommand::Disable { name, php } => (php, Some((name, false))),
            };

            let runtime = which_php(&mut client, php).await?;

            match choice {
                None => {
                    let list: ExtensionList = ask(
                        &mut client,
                        rpc::method::RUNTIME_LIST_EXTENSIONS,
                        encode(&runtime),
                    )
                    .await?;
                    emit(&rendered(json, &list, || render::extension_list(&list)))?;
                }

                Some((name, enabled)) => {
                    let choice = ExtensionChoice {
                        runtime,
                        name,
                        enabled,
                    };
                    let change: ExtensionChange = ask(
                        &mut client,
                        rpc::method::RUNTIME_SET_EXTENSION,
                        encode(&choice),
                    )
                    .await?;
                    emit(&rendered(json, &change, || {
                        render::extension_change(&change)
                    }))?;
                }
            }
        }

        RuntimeCommand::Resolve { kind, version, cwd } => {
            let question = question(kind, version, cwd)?;
            let resolved: ResolvedRuntime =
                ask(&mut client, rpc::method::RUNTIME_RESOLVE, encode(&question)).await?;
            emit(&rendered(json, &resolved, || {
                render::runtime_resolved(&resolved)
            }))?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// `mix runtime install`: start the download, and follow it unless told not to.
///
/// **Waiting is the client's own decision and not a second API.** The daemon answers a job the
/// instant it has one, which is what keeps an eighty-megabyte download off the RPC call; what a
/// person typing this wants is for the command to end when PHP is there, and what a script wants is
/// an exit status that means it. So `mix` polls `job.wait` until the job ends — each poll is one
/// round trip over a local socket — and prints the progress it passes on **stderr**, so that stdout
/// still carries exactly one answer and `--json` still emits exactly one object.
async fn install(
    client: &mut Client,
    target: RuntimeTarget,
    no_wait: bool,
    json: bool,
) -> Result<ExitCode, Error> {
    let started: JobSummary = ask(client, rpc::method::RUNTIME_INSTALL, encode(&target)).await?;

    if no_wait {
        emit(&rendered(json, &started, || render::job_status(&started)))?;
        return Ok(ExitCode::SUCCESS);
    }

    let finished = follow(client, started, json).await?;

    emit(&rendered(json, &finished, || render::job_status(&finished)))?;

    Ok(match render::job_succeeded(&finished) {
        true => ExitCode::SUCCESS,
        false => ExitCode::FAILURE,
    })
}

/// Poll a job until it ends, saying on stderr what it is doing as that changes.
///
/// A short timeout rather than the default thirty seconds: what is being waited for is a progress
/// report, and a wait that only answers when the job *ends* would leave a person watching nothing
/// for the length of a download. The daemon caps what it grants either way, so the cost of asking
/// often is one round trip a second on a socket that is not a network.
async fn follow(client: &mut Client, started: JobSummary, json: bool) -> Result<JobSummary, Error> {
    /// How long each `job.wait` asks for.
    const POLL: Millis = Millis(1_000);

    let mut job = started;
    let mut said = String::new();

    while !job.state.is_finished() {
        // Only in the human rendering, and only when it changed: a `--json` run emits one object,
        // and a progress line repeated once a second would be noise in a terminal and a log.
        if !json && job.message != said {
            said = job.message.clone();
            report_progress(job.percent, &said);
        }

        job = ask(
            client,
            rpc::method::JOB_WAIT,
            encode(&JobWait {
                job: job.id,
                timeout: POLL,
            }),
        )
        .await?;
    }

    Ok(job)
}

/// Say where a job has got to, where it will not be mistaken for the command's answer.
fn report_progress(percent: u8, message: &str) {
    if message.is_empty() {
        return;
    }

    // Nothing to do about a stderr that will not take it — the answer the user asked for is still
    // going out on stdout. `writeln!` rather than `eprintln!`, which panics when stderr is closed.
    let _ = writeln!(std::io::stderr(), "  {percent:>3}%  {message}");
}

/// The wire shape of "which runtime", from the two arguments a person typed.
fn target(Which { kind, version }: Which) -> RuntimeTarget {
    RuntimeTarget { kind, version }
}

/// Which PHP `mix runtime ext` was told about, or the one this directory resolves to.
///
/// The fallback is a **call** and not a rule of this client's: which version a directory uses is
/// `runtime.resolve`'s answer, and a `mix` that worked it out for itself would be the second opinion
/// this whole product exists to prevent.
async fn which_php(
    client: &mut Client,
    WhichPhp { version }: WhichPhp,
) -> Result<RuntimeTarget, Error> {
    let kind = RuntimeKind::Php;

    if let Some(version) = version {
        return Ok(RuntimeTarget { kind, version });
    }

    let resolved: ResolvedRuntime = ask(
        client,
        rpc::method::RUNTIME_RESOLVE,
        encode(&question(kind, None, None)?),
    )
    .await?;

    Ok(RuntimeTarget {
        kind,
        version: resolved.runtime.version,
    })
}

/// The wire shape of "which version does this directory use", and the one place `mix` reads the
/// environment below `main`.
///
/// **It has to be read here rather than at `main`**, which is where `.claude/standards/rust.md` puts
/// configuration, and the exception is narrow enough to state exactly: the variable's *name* depends
/// on the kind the user just named — `MIXENGINE_PHP`, `MIXENGINE_NODE` — so nothing above the parse
/// knows which one to look at. The name itself is still not this client's to invent:
/// [`RuntimeKind::override_env`] is in `mixengine-proto`, so the shim and the GUI read the same one.
///
/// And it is read by *this* process rather than by the daemon on purpose. `MIXENGINE_PHP=8.1 php -v`
/// is a sentence about the shell it was typed in; a daemon consulting its own environment would
/// answer with whatever it happened to be started with, for everybody at once.
fn question(
    kind: RuntimeKind,
    version: Option<VersionConstraint>,
    cwd: Option<PathBuf>,
) -> Result<RuntimeQuestion, Error> {
    let variable = kind.override_env();

    let version = match version {
        Some(version) => Some(version),

        // An empty value is "not set", which is how a shell unsets one for a single command. Every
        // other value is meant, so one that is not a version is refused rather than skipped past —
        // a `MIXENGINE_PHP` that quietly does nothing is the exact failure this whole command
        // exists to explain.
        None => match std::env::var(variable) {
            Ok(value) if value.is_empty() => None,
            Ok(value) => Some(VersionConstraint::parse(value).map_err(|error| {
                Error::new(
                    ErrorCode::InvalidArgument,
                    format!("{variable} is set to something that is not a version: {error}"),
                )
                .with_hint(
                    "a version (`8.3.33`), a series (`8.3`) or a caret (`^8.3`) — the same forms \
                     `mixengine.toml` accepts",
                )
            })?),
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(Error::new(
                    ErrorCode::InvalidArgument,
                    format!("{variable} is set to something that is not text"),
                ));
            }
        },
    };

    Ok(RuntimeQuestion {
        kind,
        // The directory `mix` was run in, unless one was named. A process with no working directory
        // at all — a deleted one on Unix — asks the question without it rather than failing: the
        // flag and the default still answer, and the daemon says which of them did.
        cwd: cwd
            .or_else(|| std::env::current_dir().ok())
            .map(|path| path.display().to_string()),
        version,
    })
}

/// `mix job …`: one call, one rendering, and an exit status that means what a shell expects.
///
/// **A job that failed is an answer and not an error**, which is why this returns an [`ExitCode`]:
/// what happened is on stdout in both renderings, and what changes is the status — so
/// `mix job wait 3 && …` stops where a person reading the output would.
async fn job(
    command: JobCommand,
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    // **Only `wait` answers with the job's own outcome.** The other three did what they were asked
    // the moment the daemon answered — a status was reported, a cancellation was requested — and a
    // non-zero exit for `mix job status` on a job that failed yesterday would make asking about a
    // failure a failure.
    let (method, params, verdict) = match command {
        JobCommand::List { state, limit } => {
            let list: JobList = ask(
                &mut client,
                rpc::method::JOB_LIST,
                encode(&JobFilter { state, limit }),
            )
            .await?;
            emit(&rendered(json, &list, || render::job_list(&list)))?;
            return Ok(ExitCode::SUCCESS);
        }

        JobCommand::Status { job } => (
            rpc::method::JOB_STATUS,
            encode(&JobQuery { job: JobId(job) }),
            false,
        ),
        JobCommand::Cancel { job } => (
            rpc::method::JOB_CANCEL,
            encode(&JobQuery { job: JobId(job) }),
            false,
        ),
        JobCommand::Wait { job, timeout } => (
            rpc::method::JOB_WAIT,
            encode(&JobWait {
                job: JobId(job),
                timeout: Millis::from_secs(timeout),
            }),
            true,
        ),
    };

    let job: JobSummary = ask(&mut client, method, params).await?;
    emit(&rendered(json, &job, || render::job_status(&job)))?;

    // A wait that ran out is not a success either: it is what a script blocks on, and exiting zero
    // there would carry it past a download that is still running.
    Ok(match !verdict || render::job_succeeded(&job) {
        true => ExitCode::SUCCESS,
        false => ExitCode::FAILURE,
    })
}

/// `mix daemon stop`: stop the services, then the daemon.
///
/// **The answer arrives before the daemon goes**, which is what makes an exit code possible here at
/// all: the walk it carries says whether everything this home was running actually stopped, and a
/// service that refused is worth a non-zero status even though the daemon stopped regardless. What
/// happens to the connection a moment later is not this command's business — the response has been
/// read by then.
///
/// **A stop that could not be ordered is the same kind of non-zero**, and for the same reason rather
/// than by analogy: the exit code here has never meant "the daemon stopped" — it means "what was
/// asked for happened" — and what `mix daemon stop` asks for is every service stopped in dependency
/// order. A daemon that could not work one out stopped them all at the same moment instead, which is
/// the arrangement the ordering exists to prevent, and exiting `0` would carry a
/// `mix daemon stop && …` past it in silence. Both halves are still on stdout in both renderings;
/// only the status changes.
async fn daemon_stop(endpoint: &Endpoint, json: bool) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, None).await?;
    let shutdown: DaemonShutdown = ask(&mut client, rpc::method::DAEMON_SHUTDOWN, None).await?;

    emit(&rendered(json, &shutdown, || {
        render::daemon_shutdown(&shutdown)
    }))?;

    Ok(match (&shutdown.services.failed, &shutdown.unordered) {
        (None, None) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    })
}

/// `mix status`: what the daemon says about itself.
async fn status(
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;
    let status: DaemonStatus = ask(&mut client, rpc::method::DAEMON_STATUS, None).await?;

    match json {
        // The newline is here rather than in `render`, which builds a document and does not know
        // whether it is the last thing on the stream. The human rendering ends in one already.
        true => emit(&format!("{}\n", render::status_json(&status)))?,
        false => emit(&render::status(&status))?,
    }

    Ok(ExitCode::SUCCESS)
}

/// `mix service …`: one call, one rendering, and an exit code that means what a shell expects.
///
/// **A walk that failed is an answer and not an error**, which is why this returns an
/// [`ExitCode`] rather than reporting through [`report`]: a plan of six services where the fourth
/// fails leaves three running, one failed and two never tried, and all of that goes to stdout in
/// both renderings. What the failure changes is the exit status, so `mix service start db && …`
/// stops where a person reading the output would.
async fn service(
    command: ServiceCommand,
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    // The walk methods differ by one string and one verb, so they are one arm with both in it —
    // three copies of this block would be three places for the two to drift apart.
    let (method, walked, params) = match &command {
        ServiceCommand::List => {
            let list: ServiceList = ask(&mut client, rpc::method::SERVICE_LIST, None).await?;
            emit(&rendered(json, &list, || render::service_list(&list)))?;
            return Ok(ExitCode::SUCCESS);
        }

        ServiceCommand::Status { service } => {
            let query = ServiceQuery {
                service: service.clone(),
            };
            let summary: ServiceSummary =
                ask(&mut client, rpc::method::SERVICE_STATUS, encode(&query)).await?;
            emit(&rendered(json, &summary, || {
                render::service_status(&summary)
            }))?;
            return Ok(ExitCode::SUCCESS);
        }

        ServiceCommand::Create {
            service,
            version,
            port,
            bind,
            data_dir,
            autostart,
        } => {
            let create = ServiceCreate {
                id: service.clone(),
                version: version.clone(),
                port: *port,
                bind_addr: bind.clone(),
                data_dir: data_dir.clone(),
                // Only when it was asked for: `false` and "nobody said" are the same row, and
                // sending the first would put a default of ours on the wire as a decision.
                autostart: autostart.then_some(true),
                overrides: None,
            };
            let creation: ServiceCreation =
                ask(&mut client, rpc::method::SERVICE_CREATE, encode(&create)).await?;
            emit(&rendered(json, &creation, || {
                render::service_creation(&creation)
            }))?;
            return Ok(ExitCode::SUCCESS);
        }

        ServiceCommand::Delete { service, force } => {
            let asked = ServiceDelete {
                target: ServiceQuery {
                    service: service.clone(),
                },
                force: *force,
            };
            let removal: ServiceRemoval =
                ask(&mut client, rpc::method::SERVICE_DELETE, encode(&asked)).await?;
            emit(&rendered(json, &removal, || {
                render::service_removal(&removal)
            }))?;
            return Ok(ExitCode::SUCCESS);
        }

        ServiceCommand::Logs {
            service,
            lines,
            follow,
        } => {
            return logs(&mut client, service, *lines, *follow, json).await;
        }

        ServiceCommand::Start(target) => {
            (rpc::method::SERVICE_START, render::Walked::Start, target)
        }
        ServiceCommand::Stop(target) => (rpc::method::SERVICE_STOP, render::Walked::Stop, target),
        ServiceCommand::Restart(target) => (
            rpc::method::SERVICE_RESTART,
            render::Walked::Restart,
            target,
        ),
    };

    let target = ServiceTarget {
        service: params.service.clone(),
        wait: !params.no_wait,
    };

    let walk: ServiceWalk = ask(&mut client, method, encode(&target)).await?;
    emit(&rendered(json, &walk, || {
        render::service_walk(walked, &walk)
    }))?;

    Ok(match walk.failed {
        None => ExitCode::SUCCESS,
        Some(_) => ExitCode::FAILURE,
    })
}

/// `mix service logs`: what a service has printed, and what it prints next.
///
/// **Written out as it arrives rather than collected**, which is the whole difference between this
/// and every other command here: a `--follow` never has a last message, and a buffer that filled
/// until the stream ended would print nothing for as long as the service kept running.
///
/// **The text goes out exactly as the service wrote it.** No timestamp, no `[stderr]`, nothing of
/// MixEngine's — for the same reason `current.log` carries none: this is piped into `grep` by
/// somebody who greps MariaDB's log the same way, and a prefix of ours would break every one of
/// those to restate what `--json` already carries. What the human rendering does add is the one
/// thing that is not output: a gap, on stderr, where the daemon or this client fell behind and lines
/// were lost. Silence there would make a log with a hole in it look complete.
async fn logs(
    client: &mut Client,
    service: &ServiceId,
    lines: usize,
    follow: bool,
    json: bool,
) -> Result<ExitCode, Error> {
    let path = format!("/logs/{service}?tail={lines}&follow={}", u8::from(follow));
    let mut stream = client.stream(&path).await?;

    while let Some(frame) = stream.next::<LogFrame>().await? {
        match (json, &frame) {
            // Verbatim, one object per line: a script filtering on `stream` or ordering by `at`
            // needs what the human rendering deliberately drops.
            (true, _) => emit(&format!(
                "{}\n",
                serde_json::to_string(&frame).expect("a proto type always serialises")
            ))?,

            (false, LogFrame::Line(line)) => emit(&format!("{}\n", line.text))?,
            (false, LogFrame::Historic { text }) => emit(&format!("{text}\n"))?,

            (false, LogFrame::Gap { missed }) => {
                report_gap(*missed);
            }

            // A variant from a later daemon. Ignored rather than refused, which is what the wire
            // types are `non_exhaustive` for.
            (false, _) => {}
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// Say on stderr that lines were lost, so that a redirected log stays exactly the log.
fn report_gap(missed: u64) {
    let mut stderr = std::io::stderr();

    // Nothing to do about a stderr that will not take it, and nothing worth failing the command
    // over: the output the user asked for is still going out.
    let _ = writeln!(
        stderr,
        "mix: {missed} lines were dropped — this client fell behind the service"
    );
}

/// Call a method and decode what it answered.
///
/// **Decoded rather than passed through as the [`Value`](serde_json::Value) it arrived as, even for
/// `--json`.** The handshake has already established that this daemon speaks our protocol, so a
/// field this build cannot read is a bug worth reporting as one — and `--json` promising a
/// `ServiceWalk` means it has to be a `ServiceWalk` that goes out.
async fn ask<T: serde::de::DeserializeOwned>(
    client: &mut Client,
    method: &str,
    params: Option<serde_json::Value>,
) -> Result<T, Error> {
    let result = client.call(method, params).await?;

    serde_json::from_value(result).map_err(|error| {
        Error::new(
            ErrorCode::Internal,
            format!(
                "mix {} cannot read the answer to {method} from mixengined {}: {error}",
                env!("CARGO_PKG_VERSION"),
                client.daemon().version
            ),
        )
    })
}

/// The parameters of a call, as the wire carries them.
///
/// `expect` and not a failure path: every params type here is `mixengine-proto`'s and made of
/// strings, booleans and options, none of which can fail to serialise.
fn encode(params: &impl serde::Serialize) -> Option<serde_json::Value> {
    Some(serde_json::to_value(params).expect("a proto params type always serialises"))
}

/// One of the two renderings of an answer, ready to be written.
///
/// The `--json` half is the daemon's answer **verbatim**, unlike `mix status`, whose envelope exists
/// so a captured diagnostic says which `mix` produced it. A script asking about services wants
/// `.services[]` and `.failed.reason.kind` where the API names them, and the daemon's build is one
/// `mix status` away.
fn rendered(json: bool, answer: &impl serde::Serialize, human: impl FnOnce() -> String) -> String {
    match json {
        true => format!(
            "{}\n",
            serde_json::to_string(answer).expect("a proto answer type always serialises")
        ),
        false => human(),
    }
}

/// Put the command's answer on stdout.
///
/// `write!` and not `print!`, for the reason [`report`] gives for stderr — the macro panics when the
/// write fails — but the two failures it can meet are not the same failure and are not answered the
/// same way. A reader that went away, `mix status | head -1`, is not this program's problem and is
/// what every well-behaved tool exits quietly on. Anything else — a full disk, a handle closed
/// before the process started — is a command that did not deliver its answer, and says so in the
/// same wire error every other failure here uses.
///
/// Flushed explicitly, because the lock is a `LineWriter`: a rendering that reaches the buffer and
/// no further would otherwise fail on drop, where the error is discarded and this run would have
/// exited zero having printed nothing.
fn emit(rendered: &str) -> Result<(), Error> {
    let mut stdout = std::io::stdout().lock();

    stdout
        .write_all(rendered.as_bytes())
        .and_then(|()| stdout.flush())
        .or_else(|source| match source.kind() {
            std::io::ErrorKind::BrokenPipe => Ok(()),
            _ => Err(Error::new(
                ErrorCode::Io,
                format!("cannot write to stdout: {source}"),
            )),
        })
}

/// Put a failure where the person or the program running `mix` will find it.
///
/// **stderr, in both renderings.** stdout carries the command's answer and nothing else, so a script
/// that redirects it into a file gets either a status object or an empty file — never an error
/// object where a status was meant to be.
fn report(error: &Error, json: bool) {
    let mut stderr = std::io::stderr().lock();

    // The `Display` in `mixengine-proto` is the human rendering: the message, and the hint on a
    // line of its own the way `cargo` prints one. A wire error is three owned strings and cannot
    // fail to serialise, so the fallback is a formality rather than a case.
    let rendered = match json {
        true => serde_json::to_string(error).unwrap_or_else(|_| format!("error: {error}")),
        false => format!("error: {error}"),
    };

    // `writeln!` and not `eprintln!`, which panics if stderr is closed — `mix status 2>&-` in a
    // pipeline that has already gone away is not worth a panic message about a panic.
    let _ = writeln!(stderr, "{rendered}");
}
