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
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

use clap::{Parser, Subcommand};
use mixengine_platform::ipc::Endpoint;
use mixengine_proto::{
    AnswerSubject, AutostartReport, BlueprintApplied, BlueprintApply, BlueprintApplyResponse,
    BlueprintCapture, BlueprintImport, BlueprintList, BlueprintPlan, BlueprintSummary,
    BundleReport, CaRotateReport, CaStatus, CaUninstallReport, CertIssue, CertIssueReport,
    CertStatusQuery, CertStatusReport, DaemonShutdown, DaemonStatus, DatabaseAccount,
    DatabaseClientQuery, DatabaseClientReport, DatabaseCreate, DatabaseHandoff, DatabaseOpen,
    DiagnosticsBundle, Disposition, DoctorRepair, DoctorReport, DomainAdd, DomainRemove,
    DomainStatusQuery, DomainStatusReport, ElevationDrop, ElevationStatus, Error, ErrorCode,
    ExtensionCatalogue, ExtensionChange, ExtensionChoice, ExtensionConsent, ExtensionId,
    ExtensionInspect, ExtensionInspection, ExtensionInstall, ExtensionList, ExtensionOrigin,
    ExtensionPlan, ExtensionPlanRequest, ExtensionRemoval, ExtensionTarget, ExtensionUninstall,
    IdleReport, InstalledExtensions, JobFilter, JobId, JobList, JobOutcome, JobQuery, JobState,
    JobSummary, JobWait, LogFrame, MetricsFrame, MetricsHistory, Millis, MismatchAnswer,
    PackageCatalogue, PackageFilter, PackageList, PackageRemoval, PackageTarget, PackageVersion,
    PathReport, PendingOpId, PlanAction, Priority, ProjectCreate, ProjectDetail, ProjectExport,
    ProjectList, ProjectQuery, ProjectRef, ProjectRemoval, ProjectUpdate, RepairReport,
    ResolvedRuntime, ResourceLimits, RuntimeCatalogue, RuntimeFilter, RuntimeKind, RuntimeList,
    RuntimeQuestion, RuntimeRemoval, RuntimeSummary, RuntimeTarget, RuntimeUninstall,
    ScaffoldConsent, ServiceCreate, ServiceCreation, ServiceDelete, ServiceId, ServiceIdleSet,
    ServiceLimitsReport, ServiceLimitsSet, ServiceList, ServiceQuery, ServiceRemoval,
    ServiceSummary, ServiceTarget, ServiceWalk, SignatureCheck, SiteCreate, SiteCreation,
    SiteDetail, SiteKind, SiteList, SiteListQuery, SiteQuery, SiteRef, SiteRemoval, SiteShare,
    SiteSharing, SiteState, SiteUpdate, Timestamp, VersionAnswer, VersionConstraint, rpc,
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

    /// Write down what a project is made of, and see what applying that somewhere else would do.
    Blueprint {
        #[command(subcommand)]
        command: BlueprintCommand,
    },

    /// Read an `extension.toml` without installing anything.
    Extension {
        #[command(subcommand)]
        command: ExtensionCommand,
    },

    /// Make a database on one of this home's database servers, and an account that reaches it.
    #[command(visible_alias = "db")]
    Database {
        #[command(subcommand)]
        command: DatabaseCommand,
    },

    /// Show what MixEngine is costing this machine: CPU and memory, per service and for the daemon.
    ///
    /// One reading and out by default. `--watch` opens the live stream, which is also what puts the
    /// daemon on its one-second rate — it samples once a minute when nobody is looking.
    Metrics {
        /// Keep printing, a block per reading, until interrupted.
        #[arg(long)]
        watch: bool,

        /// Read the recorded history instead, starting this far back: `30m`, `2h`, `1d`.
        #[arg(long, conflicts_with = "watch")]
        since: Option<String>,

        /// One subject only. Omit for every service and the daemon.
        #[arg(long, value_name = "SERVICE", value_parser = service_id)]
        service: Option<ServiceId>,

        /// Machine-readable output.
        #[arg(long)]
        json: bool,
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

        /// Write one diagnostics archive and print where it went.
        ///
        /// Everything a bug report needs in one file: the findings above, this daemon's status,
        /// what this machine is, and the tail of the log — with whatever was deliberately left out
        /// named beside them.
        #[arg(long, conflicts_with = "repair")]
        bundle: bool,

        /// Copy the archive here as well. Only with `--bundle`.
        #[arg(long, requires = "bundle", value_name = "FILE")]
        out: Option<PathBuf>,
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

    /// Start this home's daemon when you log in, or stop doing that.
    Autostart {
        #[command(subcommand)]
        command: AutostartCommand,
    },

    /// See what needs an administrator's permission, ask for it once, or forget it.
    Elevation {
        #[command(subcommand)]
        command: ElevationCommand,
    },

    /// Look at the certificate authority this home signs its sites with.
    Cert {
        #[command(subcommand)]
        command: CertCommand,
    },
}

/// `mix cert …` — one subcommand per `cert.*` method, and nothing that is not one.
///
/// **`ca-status` and not `status`.** `.claude/features/tls.md` gives the short name to the per-site
/// diagnostics with a live TLS handshake, which is roadmap task **T53**, and names this command's
/// siblings `ca-uninstall` and `ca-rotate`. Taking the short name here would mean renaming it later,
/// or giving one command two unrelated jobs.
#[derive(Debug, Subcommand)]
enum CertCommand {
    /// Say what this home's certificate authority is: its name, its fingerprint, how long it has.
    ///
    /// **Not whether this machine trusts it.** That is a question about the operating system's own
    /// certificate stores rather than about the authority, this build does not yet ask it, and
    /// nothing printed here implies an answer to it.
    // Roadmap task T49 is what will answer it, and `mix cert ca-install` is where that will live.
    // Kept out of the help text above on purpose: a task number means nothing to whoever typed
    // `--help`, and clap prints every line of a doc comment.

    /// Give a site the certificate its names need, or every HTTPS site one.
    ///
    /// Idempotent: a certificate that still covers the right names, has more than thirty days left
    /// and was signed by the authority this home has now is left exactly as it is.
    Issue {
        /// One site, by any of its domains. Every HTTPS site when this is left out.
        #[arg(long, value_name = "DOMAIN")]
        site: Option<String>,
    },
    /// Say whether each site's padlock is green, by asking the server rather than the disk.
    ///
    /// Opens a real TLS connection to this home's front end for every site and reports the
    /// certificate it presents — which is the only thing a browser ever sees, and the only way to
    /// notice a server still holding a certificate that was replaced underneath it.
    ///
    /// Reads only. Nothing is issued, nothing is installed and nothing is reloaded.
    Status {
        /// One site, by any of its domains. Every site when this is left out.
        #[arg(long, value_name = "DOMAIN")]
        site: Option<String>,
    },
    CaStatus,

    /// Replace this home's certificate authority with a new one.
    ///
    /// Destructive: every browser holding a cached chain under the old authority stops accepting
    /// it, and every site's certificate is reissued. Nothing is replaced unless this machine can be
    /// made to trust the new one — declining the prompt leaves this home exactly as it was.
    CaRotate {
        /// Answer the confirmation in advance, for a script with nobody at the keyboard.
        #[arg(long)]
        yes: bool,

        /// Start the work and print the job, rather than waiting for it to finish.
        #[arg(long = "no-wait")]
        no_wait: bool,
    },

    /// Take this home's certificate authority out of every store that trusts it.
    ///
    /// Leaves the certificate and its key on disk, and leaves every site's certificate alone —
    /// `mix doctor --repair` puts the trust back. Removing it from the system store needs an
    /// administrator; the browser databases do not.
    CaUninstall {
        /// Answer the confirmation in advance, for a script with nobody at the keyboard.
        #[arg(long)]
        yes: bool,

        /// Start the work and print the job, rather than waiting for it to finish.
        #[arg(long = "no-wait")]
        no_wait: bool,
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
        ///
        /// `id` spelled out because the flattened project argument is also called `name`, and clap
        /// refuses two arguments under one id — it did so at *parse* time, so `mix project update
        /// blog --name blogging` panicked instead of running. Found by T77's
        /// `every_command_is_one_clap_can_build`, which is now what stops the next one.
        #[arg(long, id = "new_name", value_name = "NAME")]
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

    /// Hold this project's services out of idle shutdown while you are working on it.
    ///
    /// A verb of its own rather than a flag on `update`, because it is a thing you do to a project
    /// for an afternoon and not part of what the project *is*.
    ///
    /// It reaches the PHP pool this project's sites name. It does not yet reach the database they
    /// query — nothing in MixEngine records which database a project uses.
    #[command(name = "keep-warm")]
    KeepWarm {
        #[command(flatten)]
        project: WhichProject,

        /// Stop keeping it warm.
        #[arg(long)]
        off: bool,
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

/// `mix extension …` — one subcommand per `extension.*` method this build has.
#[derive(Debug, clap::Subcommand)]
enum ExtensionCommand {
    /// Say what installing this extension here would produce.
    Inspect {
        /// The extension's directory, or its `extension.toml`.
        path: PathBuf,
    },

    /// What this home has installed.
    List,

    /// What the signed registry publishes.
    Available,

    /// Say what installing one would do, and change nothing.
    Plan {
        /// The extension's id in the registry.
        #[arg(conflicts_with = "path")]
        id: Option<String>,

        /// A directory to read instead of the registry. **Nothing vouches for one of these.**
        #[arg(long)]
        path: Option<PathBuf>,
    },

    /// Install one.
    Install {
        /// The extension's id in the registry.
        #[arg(conflicts_with = "path")]
        id: Option<String>,

        /// A directory to install instead of a registry entry. **Nothing vouches for one of
        /// these**, and the row records it as unsigned for as long as it stays installed.
        #[arg(long)]
        path: Option<PathBuf>,

        /// Install without asking about what it declares.
        #[arg(long)]
        yes: bool,

        /// Answer with the job rather than waiting for it.
        #[arg(long)]
        no_wait: bool,
    },

    /// Remove one.
    Uninstall {
        /// Which extension.
        id: String,

        /// Delete its data directory as well.
        ///
        /// **Kept when this is absent**, which is the answer that can be undone.
        #[arg(long)]
        delete_data: bool,
    },

    /// Start the service an extension runs as.
    Start {
        /// Which extension.
        id: String,
    },

    /// Stop it.
    Stop {
        /// Which extension.
        id: String,
    },
}

/// `mix blueprint …` — one subcommand per `blueprint.*` method this build has.
///
/// `export` and `delete` are deliberately absent: a blueprint's rendering is already on disk at
/// `blueprints/<slug>.toml`, so exporting one is copying a file rather than a daemon method, and
/// nothing has asked to delete one.
#[derive(Debug, Subcommand)]
enum BlueprintCommand {
    /// Write down what a project is made of.
    Capture {
        /// What to file it under: lower-case letters, digits and hyphens.
        ///
        /// Positional rather than `--name`, because the flattened project argument is already
        /// called `name` and clap refuses two arguments under one id — found by running the command
        /// rather than by a test, which is why it is worth a sentence here.
        #[arg(value_name = "NAME")]
        name: String,

        /// Which project. Defaults to whichever project the current directory is in.
        #[arg(long, value_name = "PROJECT")]
        project: Option<String>,

        /// What it is for.
        #[arg(long, value_name = "TEXT")]
        description: Option<String>,

        /// Replace the blueprint already filed under this name.
        #[arg(long)]
        overwrite: bool,
    },

    /// Take in a blueprint somebody else wrote.
    ///
    /// **What arrives without a signature the gallery key vouches for is untrusted for good** —
    /// nothing raises that afterwards, and it is what decides how loudly its `[scaffold]` command
    /// has to be agreed to before it runs.
    Import {
        /// The manifest to read.
        #[arg(value_name = "FILE")]
        file: PathBuf,

        /// What to file it under. Defaults to the file's own name, without `.toml`.
        #[arg(long, value_name = "NAME")]
        name: Option<String>,

        /// The detached signature to check it against. Defaults to `<FILE>.minisig` if that exists.
        #[arg(long, value_name = "FILE")]
        signature: Option<PathBuf>,

        /// Replace the blueprint already filed under that name.
        #[arg(long)]
        overwrite: bool,
    },

    /// Every blueprint this home holds.
    List,

    /// What applying one would do.
    Apply {
        /// Which blueprint.
        #[arg(value_name = "BLUEPRINT")]
        blueprint: String,

        /// What the new project is called, and what `{project}` becomes.
        #[arg(long, value_name = "NAME")]
        project: String,

        /// Where it goes. Defaults to `<current directory>/<project>`.
        #[arg(long, value_name = "DIR")]
        path: Option<PathBuf>,

        /// Stop after planning, and print the plan.
        ///
        /// Sent as it is typed rather than insisted on here: whether this build can carry an apply
        /// out is the daemon's to say, and a client that refused to ask would be holding a rule of
        /// its own.
        #[arg(long)]
        dry_run: bool,

        /// Answer every version question by installing what the blueprint asks for.
        #[arg(long, conflicts_with = "use_installed")]
        install_missing: bool,

        /// Answer every version question by using what this machine already has.
        #[arg(long)]
        use_installed: bool,

        /// Run the blueprint's own `[scaffold]` command without asking first.
        ///
        /// For a blueprint the gallery signed. An unsigned one takes the other flag, and neither
        /// covers the other: a script that runs somebody's unsigned command should say so on the
        /// line that does it.
        #[arg(long, conflicts_with = "run_untrusted_scaffold")]
        run_scaffold: bool,

        /// Run an **untrusted** blueprint's own `[scaffold]` command without asking first.
        ///
        /// Nothing vouches for what this runs. The command is still printed before it starts.
        #[arg(long)]
        run_untrusted_scaffold: bool,

        /// Spend the one elevation prompt at the end without asking first.
        #[arg(long)]
        grant: bool,
    },
}

/// `mix database …` — one subcommand per `database.*` method this build has.
///
/// `list` and `drop` are deliberately absent: nothing has asked for either, and dropping a database
/// is a decision with data behind it rather than the other half of a pair.
#[derive(Debug, Subcommand)]
enum DatabaseCommand {
    /// Make a database and the account that reaches it.
    ///
    /// The instance is started if it is not running. Nothing prints the password: it is put in this
    /// machine's credential store, and what is printed is where.
    Create {
        /// Which instance: `mariadb@main`, `postgres@shop`.
        #[arg(value_name = "SERVICE", value_parser = service_id)]
        service: ServiceId,

        /// The database's name.
        #[arg(long, value_name = "NAME")]
        name: String,

        /// The account's name. The database's own when nobody says.
        #[arg(long, value_name = "ACCOUNT")]
        user: Option<String>,
    },

    /// Where this instance could be opened, and with what.
    ///
    /// Reads only: starts nothing, opens nothing. "Not installed" is an answer, not a failure.
    Client {
        /// Which instance: `mariadb@main`, `redis@main`.
        #[arg(value_name = "SERVICE", value_parser = service_id)]
        service: ServiceId,
    },

    /// Open this instance in the installed desktop database client.
    ///
    /// The instance is started if it is not running. The account's password is read from this
    /// machine's credential store at that moment and handed to the client in its own environment —
    /// never printed, never put in an argument. Exits 1 when no client is installed, and says what
    /// to install.
    Open {
        /// Which instance.
        #[arg(value_name = "SERVICE", value_parser = service_id)]
        service: ServiceId,

        /// The account to sign in as. The server's administrator when nobody says.
        #[arg(long, value_name = "ACCOUNT")]
        user: Option<String>,

        /// A database to open at.
        #[arg(long, value_name = "NAME")]
        database: Option<String>,
    },
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

    /// Let the local network reach this site, and print a QR code for it.
    ///
    /// This site only: every other site keeps answering on loopback alone. The certificate gains
    /// the LAN address, and one administrator prompt asks for the firewall rule.
    Share {
        #[command(flatten)]
        site: WhichSite,

        /// Which network to share on, by the name this machine gives it.
        ///
        /// Needed only where more than one is up — MixEngine refuses to choose rather than putting
        /// a site on a network you did not mean, and names the candidates when it does.
        #[arg(long, value_name = "NAME")]
        interface: Option<String>,

        /// How long to share for: `30s`, `90m`, `2h`, `1d`, or a bare number of seconds.
        ///
        /// Measured from when the share began, so asking for a length shorter than the site has
        /// already been shared for is refused rather than ending it on the spot. Off by default: a
        /// share with no `--for` lasts until you unshare it or this machine leaves the network.
        #[arg(long = "for", value_name = "LENGTH", value_parser = for_seconds)]
        r#for: Option<u64>,
    },

    /// Take it back off the local network.
    ///
    /// Removes the firewall rule, rebinds to loopback and reissues the certificate without the
    /// address. A site that is not shared is left as it is.
    Unshare {
        #[command(flatten)]
        site: WhichSite,
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

/// `mix autostart …` — one subcommand per `autostart.*` method.
///
/// **None of the three takes an argument**, for `mix path`'s reason above: there is one entry per
/// user and one home this can be about, and an argument would be a command for registering
/// arbitrary programs to run at somebody's login.
///
/// **`mix autostart` and not `mix daemon autostart`.** `daemon.*` is about the daemon that is
/// running; a logon task, a LaunchAgent and a systemd user unit outlive every daemon that ever
/// registered them.
#[derive(Debug, Subcommand)]
enum AutostartCommand {
    /// Say whether this home's daemon starts when you log in.
    Status,

    /// Register it.
    ///
    /// Does **not** start the daemon: there is one running, and it is the one answering this. What
    /// it changes is what happens at your next login. Idempotent, and it says which of the two it
    /// did.
    Enable,

    /// Remove it.
    ///
    /// Does **not** stop the daemon that is running — turning off "start at login" is not a request
    /// to lose the daemon you are using.
    Disable,
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

    /// What a job printed — roadmap task **T78a**.
    ///
    /// **Only a job that runs somebody else's program prints anything**, which today is an apply
    /// running a blueprint's own `[scaffold]` command. Everything else a job does is reported as
    /// progress and as its result, and this answers nothing for those rather than pretending output
    /// was lost.
    ///
    /// The lines live in memory for as long as the daemon keeps the job's log, so this is what to
    /// read while one runs rather than a record to come back to a week later.
    Logs {
        /// The job, as `mix job list` numbers them.
        #[arg(value_name = "JOB")]
        job: i64,

        /// Keep printing as the job prints.
        #[arg(long, short = 'f')]
        follow: bool,

        /// How many of the lines already printed to begin with.
        #[arg(long, short = 'n', value_name = "COUNT", default_value_t = 200)]
        lines: usize,
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

    /// What this service may take, and what this machine will actually enforce of it.
    ///
    /// With no subcommand: read it. `set` replaces it, `clear` removes it.
    Limits {
        /// The service to read or cap.
        #[arg(value_name = "SERVICE", value_parser = service_id)]
        service: ServiceId,

        #[command(subcommand)]
        command: Option<LimitsCommand>,
    },

    /// When this service is stopped for being unused, and what is holding it open.
    ///
    /// With no flag: read it. One of the three flags replaces it.
    ///
    /// Nothing idles by default in this build: a stopped service stays stopped until you start it,
    /// so switching this on is a choice you make per service.
    Idle {
        /// The service to read or set.
        #[arg(value_name = "SERVICE", value_parser = service_id)]
        service: ServiceId,

        /// Stop it once nothing has used it for this long — `30m`, `2h`, `90m`.
        #[arg(long, value_name = "DURATION", group = "idle_change", value_parser = idle_after)]
        after: Option<u32>,

        /// Never stop it for being unused, whatever a later release makes the default.
        #[arg(long, group = "idle_change")]
        never: bool,

        /// Go back to whatever its recipe wants, which in this build is never.
        #[arg(long, group = "idle_change")]
        default: bool,
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

/// What can be done to a service's limits.
#[derive(Debug, clap::Subcommand)]
enum LimitsCommand {
    /// Replace every limit on this service.
    ///
    /// **Every field, not only the ones named.** A flag left out is that field's default — uncapped,
    /// or ordinary priority — so `set --cpu 50` clears a memory ceiling that was there. That is
    /// deliberate: composing a partial change would mean reading the current value and merging it,
    /// which is business logic a client may not hold. What this does instead is print all three
    /// fields of the result, so a cleared limit is on the screen.
    Set {
        /// A ceiling on CPU, as a percentage of one core. Left out: uncapped.
        #[arg(long, value_name = "PERCENT")]
        cpu: Option<u8>,

        /// A ceiling on memory, in megabytes. Left out: uncapped.
        #[arg(long, value_name = "MB")]
        memory: Option<u32>,

        /// How this service competes for CPU.
        #[arg(long, value_name = "PRIORITY", default_value = "normal")]
        priority: PriorityArg,
    },

    /// Remove every limit from this service.
    ///
    /// A named operation rather than a `set` with three absent flags, so that "uncap this" is
    /// something a person can type rather than something they have to infer.
    Clear,
}

/// [`Priority`] on a command line.
///
/// Its own type because `clap::ValueEnum` cannot be derived for a type in another crate, and because
/// the words a person types are this crate's to choose.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum PriorityArg {
    /// Competes with everything else the user is running.
    Normal,

    /// Yields to foreground work.
    Background,
}

impl From<PriorityArg> for Priority {
    fn from(arg: PriorityArg) -> Self {
        match arg {
            PriorityArg::Normal => Self::Normal,
            PriorityArg::Background => Self::Background,
        }
    }
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

/// A duration from the command line, as a whole number of minutes.
///
/// **Refused rather than rounded when it is not one.** `services.idle_minutes` stores minutes, so
/// `--after 90s` would have to become either one minute or two, and a setting that quietly becomes
/// something else is worse than one that says it cannot. `Millis::parse` is the daemon's own
/// syntax — `30m`, `2h`, `500ms` — rather than a second reading of it here.
///
/// Zero is refused too, and it has its own flag: `--after 0m` reads as *stop it immediately* and
/// means the opposite, so `--never` is what a person types for that.
fn idle_after(value: &str) -> Result<u32, String> {
    let millis = mixengine_proto::Millis::parse(value)
        .ok_or_else(|| format!("{value:?} is not a duration — write it as `30m`, `2h` or `90m`"))?;

    if millis.is_zero() {
        return Err(
            "an idle policy of zero would stop the service on the next sweep; `--never` is how you              switch idle stopping off"
                .to_owned(),
        );
    }

    let minutes = millis.0 / 60_000;

    if minutes * 60_000 != millis.0 {
        return Err(format!(
            "{value:?} is not a whole number of minutes, and that is what MixEngine stores — write              it as minutes or hours"
        ));
    }

    u32::try_from(minutes).map_err(|_| format!("{value:?} is longer than MixEngine can store"))
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
        Command::Blueprint { command } => {
            blueprint(command, &endpoint, autostart.as_ref(), args.json).await
        }
        Command::Extension { command } => {
            extension(command, &endpoint, autostart.as_ref(), args.json).await
        }
        Command::Database { command } => {
            database(command, &endpoint, autostart.as_ref(), args.json).await
        }
        Command::Metrics {
            watch,
            since,
            service,
            json,
        } => {
            metrics(
                &endpoint,
                autostart.as_ref(),
                watch,
                since.as_deref(),
                service.as_ref(),
                args.json || json,
            )
            .await
        }
        Command::Doctor {
            repair,
            yes,
            no_wait,
            bundle: wanted,
            out,
        } => match (repair, wanted) {
            (true, _) => self_repair(&endpoint, autostart.as_ref(), args.json, yes, no_wait).await,
            (false, true) => bundle(&endpoint, autostart.as_ref(), args.json, out.as_deref()).await,
            (false, false) => doctor(&endpoint, autostart.as_ref(), args.json).await,
        },
        Command::Domain { command } => {
            domain(command, &endpoint, autostart.as_ref(), args.json).await
        }
        Command::Service { command } => {
            service(command, &endpoint, autostart.as_ref(), args.json).await
        }
        Command::Job { command } => job(command, &endpoint, autostart.as_ref(), args.json).await,
        Command::Path { command } => path(command, &endpoint, autostart.as_ref(), args.json).await,
        Command::Autostart { command } => {
            autostart_entry(command, &endpoint, autostart.as_ref(), args.json).await
        }
        Command::Elevation { command } => {
            elevation(command, &endpoint, autostart.as_ref(), args.json).await
        }
        Command::Cert { command } => cert(command, &endpoint, autostart.as_ref(), args.json).await,
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
                // `mix project update` changes what a project *is*; keeping it warm is a thing you
                // do to it while you work, and has its own verb.
                keep_warm: None,
            };
            let detail: ProjectDetail =
                ask(&mut client, rpc::method::PROJECT_UPDATE, encode(&update)).await?;
            emit(&rendered(json, &detail, || render::project_detail(&detail)))?;
        }

        ProjectCommand::KeepWarm { project, off } => {
            let update = ProjectUpdate {
                project: which(project)?,
                name: None,
                root: None,
                pins: None,
                keep_warm: Some(!off),
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
/// `mix metrics` — roadmap task **T71**.
///
/// Three readings of one namespace and one command, because they are three tenses of one question:
/// what is it costing now (`metrics.snapshot`), what has it been costing (`metrics.history`), and
/// what is it costing from here on (`GET /metrics`).
///
/// **`--watch` is written out as it arrives**, on `mix service logs --follow`'s reasoning: a stream
/// has no last message, and a buffer that filled until it ended would print nothing at all. It is
/// also the only thing in this repository that opens that route, which is what keeps the daemon's
/// one-second rate exercised rather than merely implemented.
async fn metrics(
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    watch: bool,
    since: Option<&str>,
    service: Option<&ServiceId>,
    json: bool,
) -> Result<ExitCode, Error> {
    let subject = service.map(|id| format!("service:{id}"));
    let mut client = Client::connect(endpoint, autostart).await?;

    if watch {
        return watch_metrics(&mut client, json).await;
    }

    if let Some(since) = since {
        let history: MetricsHistory = ask(
            &mut client,
            rpc::method::METRICS_HISTORY,
            encode(&serde_json::json!({
                "subject": subject,
                "since": since_moment(since)?,
            })),
        )
        .await?;

        emit(&rendered(json, &history, || {
            render::metrics_history(&history, SystemTime::now())
        }))?;

        return Ok(ExitCode::SUCCESS);
    }

    let frame: MetricsFrame = ask(
        &mut client,
        rpc::method::METRICS_SNAPSHOT,
        encode(&serde_json::json!({})),
    )
    .await?;

    // Narrowed here rather than by the daemon: `metrics.snapshot` takes no parameters, because one
    // reading measures every subject anyway and a filter on the wire would be a second way of asking
    // for the same pass.
    let frame = match subject {
        None => frame,
        Some(wanted) => MetricsFrame {
            samples: frame
                .samples
                .into_iter()
                .filter(|sample| sample.subject.to_string() == wanted)
                .collect(),
            ..frame
        },
    };

    emit(&rendered(json, &frame, || render::metrics_frame(&frame)))?;

    Ok(ExitCode::SUCCESS)
}

/// `mix metrics --watch`: print each reading as it arrives, until interrupted.
async fn watch_metrics(client: &mut Client, json: bool) -> Result<ExitCode, Error> {
    let mut stream = client.stream("/metrics").await?;

    while let Some(frame) = stream.next::<MetricsFrame>().await? {
        emit(&rendered(json, &frame, || {
            format!(
                "{}
",
                render::metrics_frame(&frame)
            )
        }))?;
    }

    Ok(ExitCode::SUCCESS)
}

/// How far back `--since 30m` reaches, as a moment this machine's clock names.
///
/// **Resolved here rather than sent as a duration**, because the API takes moments: a client that
/// sent "30m" would be asking the daemon to apply its own clock to a word, and the two clocks are
/// the same one — the endpoint is a local socket.
fn since_moment(value: &str) -> Result<Timestamp, Error> {
    let (count, unit) = value.split_at(
        value
            .find(|character: char| !character.is_ascii_digit())
            .ok_or_else(|| since_refusal(value))?,
    );

    let count: i64 = count.parse().map_err(|_| since_refusal(value))?;

    let millis = match unit {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return Err(since_refusal(value)),
    };

    let Timestamp(now) = Timestamp::from_system_time(SystemTime::now());

    Ok(Timestamp(
        now.saturating_sub(count.saturating_mul(millis).abs()),
    ))
}

/// What `--since` says when it cannot read what was typed.
fn since_refusal(value: &str) -> Error {
    Error::new(
        ErrorCode::InvalidArgument,
        format!("`--since {value}` is not a length of time"),
    )
    .with_hint("write it as a number and one of s, m, h, d — for example `--since 2h`")
}

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

/// `mix doctor --bundle` — roadmap task **T93**.
///
/// **This one exits zero when the archive was written**, unlike bare `mix doctor`, whose exit code
/// is the report. The deliverable of this command is the file: a bundle is taken *because*
/// something is wrong, so a non-zero exit every time would make the ordinary success read as a
/// failure to the person watching their terminal and to whatever wrapped it. What the exit code
/// answers here is "did I get the archive"; the answer to "is this machine well" is inside it,
/// where the person asking will be looking.
async fn bundle(
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
    out: Option<&Path>,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    let report: BundleReport = ask(
        &mut client,
        rpc::method::DAEMON_BUNDLE,
        encode(&DiagnosticsBundle::default()),
    )
    .await?;

    // **The copy is the client's and never the daemon's.** A destination on the method would be a
    // way for any local caller to have the daemon write a file anywhere that daemon can reach — so
    // the archive lands in the home, and moving it out is done by whoever asked, with their own
    // permissions.
    if let Some(destination) = out {
        std::fs::copy(&report.path, destination).map_err(|source| {
            Error::new(
                ErrorCode::Io,
                format!(
                    "the bundle was written to {} but could not be copied to {}: {source}",
                    report.path,
                    destination.display()
                ),
            )
        })?;
    }

    emit(&rendered(json, &report, || render::bundle(&report, out)))?;

    Ok(ExitCode::SUCCESS)
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

        SiteCommand::Share {
            site,
            interface,
            r#for,
        } => {
            let request = SiteShare {
                site: which_site(site)?,
                interface,
                for_seconds: r#for,
            };
            let sharing: SiteSharing =
                ask(&mut client, rpc::method::SITE_SHARE, encode(&request)).await?;
            emit(&rendered(json, &sharing, || render::site_shared(&sharing)))?;
        }

        SiteCommand::Unshare { site } => {
            let query = SiteQuery {
                site: which_site(site)?,
            };
            ask::<()>(&mut client, rpc::method::SITE_UNSHARE, encode(&query)).await?;
            emit(&rendered(json, &(), || {
                "no longer shared on the local network\n".to_owned()
            }))?;
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

/// `mix extension …` — read a manifest and say what installing it would produce.
async fn extension(
    command: ExtensionCommand,
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    match command {
        ExtensionCommand::Inspect { path } => {
            // Resolved here, because the daemon has no idea what directory this process is in and
            // a relative path sent as it was typed would be read against the wrong one.
            let asked = ExtensionInspect {
                path: here(Some(path))?.display().to_string(),
            };

            let inspection: ExtensionInspection =
                ask(&mut client, rpc::method::EXTENSION_INSPECT, encode(&asked)).await?;

            emit(&rendered(json, &inspection, || {
                render::extension_inspection(&inspection)
            }))?;
        }

        ExtensionCommand::List => {
            let list: InstalledExtensions =
                ask(&mut client, rpc::method::EXTENSION_LIST, encode(&())).await?;

            emit(&rendered(json, &list, || {
                render::installed_extensions(&list)
            }))?;
        }

        ExtensionCommand::Available => {
            let catalogue: ExtensionCatalogue =
                ask(&mut client, rpc::method::EXTENSION_AVAILABLE, encode(&())).await?;

            emit(&rendered(json, &catalogue, || {
                render::extension_catalogue(&catalogue)
            }))?;
        }

        ExtensionCommand::Plan { id, path } => {
            let asked = ExtensionPlanRequest {
                source: origin(id, path)?,
            };
            let plan: ExtensionPlan =
                ask(&mut client, rpc::method::EXTENSION_PLAN, encode(&asked)).await?;

            emit(&rendered(json, &plan, || render::extension_plan(&plan)))?;
        }

        ExtensionCommand::Install {
            id,
            path,
            yes,
            no_wait,
        } => {
            let source = origin(id, path)?;

            // **The plan is read before anything is installed, and the consent names it** — the
            // T81 design's D2 and D9. Two calls rather than one because that is what makes the
            // question answerable: the daemon has no keyboard, and the permissions a person is
            // agreeing to arrive with the listing rather than with the artifact.
            let plan: ExtensionPlan = ask(
                &mut client,
                rpc::method::EXTENSION_PLAN,
                encode(&ExtensionPlanRequest {
                    source: source.clone(),
                }),
            )
            .await?;

            if !yes && !agreed_to_install(&plan, json)? {
                return Ok(ExitCode::FAILURE);
            }

            let asked = ExtensionInstall {
                consent: ExtensionConsent {
                    id: plan.id.clone(),
                    version: plan.version.clone(),
                    signed: plan.signed,
                    network: plan.permissions.network,
                },
                source,
            };

            let started: JobSummary =
                ask(&mut client, rpc::method::EXTENSION_INSTALL, encode(&asked)).await?;

            if no_wait {
                emit(&rendered(json, &started, || render::job_status(&started)))?;
                return Ok(ExitCode::SUCCESS);
            }

            let finished = follow(&mut client, started, json).await?;
            emit(&rendered(json, &finished, || render::job_status(&finished)))?;

            let succeeded = render::job_succeeded(&finished);

            // **Said again where a person ends up** — roadmap task **T84**, the design's D2.
            // Installing a `desktop-app` writes a row and an empty directory, because MixEngine
            // finds an application somebody else installed rather than installing one; `--yes`
            // skipped the plan's render, and this is exactly the case somebody needs a sentence
            // about.
            let absent = matches!(
                plan.client,
                Some(mixengine_proto::DesktopPresence::NotInstalled { .. })
            );
            if succeeded && !json && absent {
                let homepage = plan.homepage.as_deref().unwrap_or("its homepage");
                let _ = writeln!(
                    std::io::stderr(),
                    "{} is not on this machine yet — MixEngine finds it rather than installing it. \
                     Get it at {homepage}",
                    plan.name
                );
            }

            return Ok(match succeeded {
                true => ExitCode::SUCCESS,
                false => ExitCode::FAILURE,
            });
        }

        ExtensionCommand::Uninstall { id, delete_data } => {
            let asked = ExtensionUninstall {
                id: extension_id(&id)?,
                delete_data,
            };
            let removal: ExtensionRemoval = ask(
                &mut client,
                rpc::method::EXTENSION_UNINSTALL,
                encode(&asked),
            )
            .await?;

            emit(&rendered(json, &removal, || {
                render::extension_removal(&removal)
            }))?;
        }

        ExtensionCommand::Start { id } => {
            let asked = ExtensionTarget {
                id: extension_id(&id)?,
            };
            let walk: ServiceWalk =
                ask(&mut client, rpc::method::EXTENSION_START, encode(&asked)).await?;

            emit(&rendered(json, &walk, || {
                render::service_walk(render::Walked::Start, &walk)
            }))?;
        }

        ExtensionCommand::Stop { id } => {
            let asked = ExtensionTarget {
                id: extension_id(&id)?,
            };
            let walk: ServiceWalk =
                ask(&mut client, rpc::method::EXTENSION_STOP, encode(&asked)).await?;

            emit(&rendered(json, &walk, || {
                render::service_walk(render::Walked::Stop, &walk)
            }))?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// Which of the two an `install` or a `plan` names, refusing neither and both.
fn origin(id: Option<String>, path: Option<PathBuf>) -> Result<ExtensionOrigin, Error> {
    match (id, path) {
        // Resolved here, because the daemon has no idea what directory this process is in and a
        // relative path sent as it was typed would be read against the wrong one.
        (None, Some(path)) => Ok(ExtensionOrigin::Path {
            path: here(Some(path))?.display().to_string(),
        }),

        (Some(id), None) => Ok(ExtensionOrigin::Registry {
            id: extension_id(&id)?,
        }),

        _ => Err(Error::new(
            ErrorCode::InvalidArgument,
            "name an extension from the registry, or --path a directory",
        )),
    }
}

/// An id the wire will accept, refused here rather than by the daemon.
fn extension_id(given: &str) -> Result<ExtensionId, Error> {
    ExtensionId::parse(given).map_err(|source| {
        Error::new(
            ErrorCode::InvalidArgument,
            format!("{given} is not an extension id: {source}"),
        )
    })
}

/// Ask about what an extension declares, and answer whether to go on.
///
/// **What it prints is what the daemon will be told was shown.** `permissions.services` is a
/// disclosure and not a boundary (ADR 0014), and the rendering says so — an extension runs as this
/// account, so what it may reach is what this account may reach.
fn agreed_to_install(plan: &ExtensionPlan, json: bool) -> Result<bool, Error> {
    if json {
        return Err(unanswered());
    }

    match confirm::ask(&format!(
        "{}
install it? [y/N] ",
        render::extension_plan(plan)
    )) {
        confirm::Answer::Yes => Ok(true),

        confirm::Answer::No => {
            let _ = writeln!(std::io::stderr(), "nothing was installed");
            Ok(false)
        }

        confirm::Answer::Unanswerable => Err(unanswered()),
    }
}

/// `mix blueprint …` — capture one, list them, see what applying one would do.
async fn blueprint(
    command: BlueprintCommand,
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    match command {
        BlueprintCommand::Capture {
            name,
            project,
            description,
            overwrite,
        } => {
            let capture = BlueprintCapture {
                project: which(WhichProject { name: project })?,
                name,
                description,
                overwrite,
            };
            let summary: BlueprintSummary = ask(
                &mut client,
                rpc::method::BLUEPRINT_CAPTURE,
                encode(&capture),
            )
            .await?;
            emit(&rendered(json, &summary, || {
                render::blueprint_captured(&summary)
            }))?;
        }

        BlueprintCommand::Import {
            file,
            name,
            signature,
            overwrite,
        } => {
            // Resolved here, because the daemon has no idea what directory this process is in and a
            // relative path sent as it was typed would be read against the wrong one.
            let import = BlueprintImport {
                path: here(Some(file))?.display().to_string(),
                signature: match signature {
                    Some(path) => Some(here(Some(path))?.display().to_string()),
                    None => None,
                },
                name,
                overwrite,
            };

            let summary: BlueprintSummary =
                ask(&mut client, rpc::method::BLUEPRINT_IMPORT, encode(&import)).await?;

            emit(&rendered(json, &summary, || {
                render::blueprint_imported(&summary)
            }))?;
        }

        BlueprintCommand::List => {
            let list: BlueprintList = ask(&mut client, rpc::method::BLUEPRINT_LIST, None).await?;
            emit(&rendered(json, &list, || render::blueprint_list(&list)))?;
        }

        BlueprintCommand::Apply {
            blueprint,
            project,
            path,
            dry_run,
            install_missing,
            use_installed,
            run_scaffold,
            run_untrusted_scaffold,
            grant,
        } => {
            // `<cwd>/<project>` when nobody named a directory: the new project is a new directory,
            // and the one this process is in is where a person expects it to appear.
            let root = match path {
                Some(path) => here(Some(path))?,
                None => here(None)?.join(&project),
            };

            let mut apply = BlueprintApply {
                blueprint,
                project,
                root: root.display().to_string(),
                dry_run: true,
                answers: Vec::new(),
                // Filled in below, once the plan says whether there is a command to agree to and
                // who wrote it — roadmap task **T78a**.
                scaffold: None,
            };

            // **The plan comes first either way** (the T78 design, D6). A dry run stops here; a real
            // apply needs it because the questions are in it, and a daemon has no keyboard to ask
            // them with.
            let planned: BlueprintApplyResponse =
                ask(&mut client, rpc::method::BLUEPRINT_APPLY, encode(&apply)).await?;

            let BlueprintApplyResponse::Planned { plan } = &planned else {
                return Err(Error::new(
                    ErrorCode::Internal,
                    "the daemon answered a dry run with something other than a plan",
                ));
            };

            emit(&rendered(json, plan, || render::blueprint_plan(plan)))?;

            if dry_run {
                return Ok(ExitCode::SUCCESS);
            }

            let Some(answers) = answered(plan, install_missing, use_installed, json)? else {
                // Cancelling is an answer and not a failure: nothing was asked of the machine, and
                // the same command works when the person has decided.
                return Ok(ExitCode::SUCCESS);
            };

            // **The command is shown and agreed to, every apply** — roadmap task **T78a**. The
            // consent carries the command it was given about, so a blueprint that changed between
            // this plan and the apply below cannot be run under it.
            let consent = agreed_to_scaffold(plan, run_scaffold, run_untrusted_scaffold, json)?;

            apply.dry_run = false;
            apply.answers = answers;
            apply.scaffold = consent;

            let started: BlueprintApplyResponse =
                ask(&mut client, rpc::method::BLUEPRINT_APPLY, encode(&apply)).await?;

            let BlueprintApplyResponse::Started { job } = started else {
                return Err(Error::new(
                    ErrorCode::Internal,
                    "the daemon answered an apply with something other than a job",
                ));
            };

            // **A second connection, because two streams cannot share one** — roadmap task
            // **T78a**. The job's own progress comes back on this client's `job.wait`, and what the
            // blueprint's command prints comes down `GET /logs/job/<id>` for as long as it runs. A
            // log that cannot be opened is not worth failing an apply over: the work goes on and the
            // outcome still says what happened.
            let watching = watch_job_log(endpoint, autostart, job.id, json).await;

            let finished = follow(&mut client, job, json).await?;

            if let Some(watching) = watching {
                watching.abort();
            }
            emit(&rendered(json, &finished, || render::job_status(&finished)))?;

            let mut a_step_failed = false;

            if let Some(JobOutcome::Succeeded { result }) = &finished.outcome
                && let Ok(applied) = serde_json::from_value::<BlueprintApplied>(result.clone())
            {
                emit(&rendered(json, &applied, || {
                    render::blueprint_applied(&applied)
                }))?;

                a_step_failed = render::blueprint_had_a_failed_step(&applied);
            }

            if !render::job_succeeded(&finished) {
                return Ok(ExitCode::FAILURE);
            }

            // **The job succeeded and a step did not** — roadmap task **T78a**, its design's D7. The
            // apply applied everything it was asked to; what failed is the blueprint's own command,
            // and a shell that chained on this has to hear it.
            if a_step_failed {
                return Ok(ExitCode::FAILURE);
            }

            // **The client is what spends the prompt** (D10): the apply queued the hosts entries and
            // the daemon never raises a dialog on its own initiative, so the last thing this command
            // does is offer the one prompt that makes the new site reachable.
            return granted(&mut client, grant, json).await;
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// `mix database …` — make a database and the account that reaches it, or hand it to a client.
///
/// **The password is never printed**, and that is the whole shape of these commands: what comes
/// back is the address the credential is stored under, because a password on a terminal is a
/// password in scrollback, in a tmux buffer and in a CI log. Handing one to a program that needs it
/// is `open` (roadmap task **T83**), and the daemon puts it in that program's environment alone.
async fn database(
    command: DatabaseCommand,
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    match command {
        DatabaseCommand::Create {
            service,
            name,
            user,
        } => {
            let create = DatabaseCreate {
                service,
                database: name,
                user,
            };
            let account: DatabaseAccount =
                ask(&mut client, rpc::method::DATABASE_CREATE, encode(&create)).await?;

            emit(&rendered(json, &account, || {
                render::database_created(&account)
            }))?;
        }

        DatabaseCommand::Client { service } => {
            let report: DatabaseClientReport = ask(
                &mut client,
                rpc::method::DATABASE_CLIENT,
                encode(&DatabaseClientQuery { service }),
            )
            .await?;

            emit(&rendered(json, &report, || {
                render::database_client(&report)
            }))?;
        }

        DatabaseCommand::Open {
            service,
            user,
            database,
        } => {
            let handoff: DatabaseHandoff = ask(
                &mut client,
                rpc::method::DATABASE_OPEN,
                encode(&DatabaseOpen {
                    service,
                    user,
                    database,
                }),
            )
            .await?;

            emit(&rendered(json, &handoff, || {
                render::database_opened(&handoff)
            }))?;

            // A client that did not open is exit 1 for `mix service start`'s reason: `mix database
            // open db && …` is a sentence about a client having opened. The answer is a state and
            // was printed as one; the code is what a script reads.
            if handoff.launched.is_none() {
                return Ok(ExitCode::from(1));
            }
        }
    }

    Ok(ExitCode::SUCCESS)
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

/// `mix autostart …`: one call, one rendering — roadmap task **T85b**.
///
/// No exit code of its own, on `mix path`'s rule: each of these either did what it said or failed
/// outright, and a machine with no mechanism at all is something to report rather than a failure of
/// the command.
///
/// **The `autostart` parameter is not what this command is about.** It is the client's own — the
/// thing that starts a daemon when none answers, in [`crate::autostart`] — and every command in this
/// file takes it under that name. The two meet here and nowhere else, which is why this function is
/// `autostart_entry` and the parameter is left alone.
async fn autostart_entry(
    command: AutostartCommand,
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    let (method, rendered_as) = match command {
        AutostartCommand::Status => (rpc::method::AUTOSTART_STATUS, render::Autostarted::Asked),
        AutostartCommand::Enable => (rpc::method::AUTOSTART_ENABLE, render::Autostarted::Enabled),
        AutostartCommand::Disable => (
            rpc::method::AUTOSTART_DISABLE,
            render::Autostarted::Disabled,
        ),
    };

    let report: AutostartReport = ask(&mut client, method, None).await?;
    emit(&rendered(json, &report, || {
        render::autostart_report(rendered_as, &report)
    }))?;

    Ok(ExitCode::SUCCESS)
}

/// `mix cert …`: one call, one rendering.
///
/// **Every state exits zero, including `absent` and `unusable`.** This reports; `mix doctor` is what
/// carries a verdict. A reporting command with a failing exit is one nobody can put in front of an
/// `&&` without thinking about it, and there is nothing here a person asked to change.
async fn cert(
    command: CertCommand,
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    match command {
        CertCommand::CaStatus => {
            let status: CaStatus = ask(&mut client, rpc::method::CERT_CA_STATUS, None).await?;
            emit(&rendered(json, &status, || render::ca_status(&status)))?;

            Ok(ExitCode::SUCCESS)
        }

        CertCommand::Status { site } => {
            let request = CertStatusQuery {
                site: site.map(SiteRef::Domain),
            };
            let report: CertStatusReport =
                ask(&mut client, rpc::method::CERT_STATUS, encode(&request)).await?;
            emit(&rendered(json, &report, || render::cert_status(&report)))?;

            Ok(ExitCode::SUCCESS)
        }

        CertCommand::Issue { site } => {
            let request = CertIssue {
                site: site.map(SiteRef::Domain),
            };
            let report: CertIssueReport =
                ask(&mut client, rpc::method::CERT_ISSUE, encode(&request)).await?;
            emit(&rendered(json, &report, || render::cert_issue(&report)))?;

            Ok(ExitCode::SUCCESS)
        }

        CertCommand::CaRotate { yes, no_wait } => {
            if !yes
                && !agreed(
                    &mut client,
                    "this will replace this home's certificate authority. Every site's certificate \
                     is reissued under the new one, and every browser holding a cached chain under \
                     the old one stops accepting it until it re-reads the store.",
                    json,
                )
                .await?
            {
                return Ok(ExitCode::SUCCESS);
            }

            let started: JobSummary = ask(&mut client, rpc::method::CERT_CA_ROTATE, None).await?;

            job_answering(&mut client, started, no_wait, json, |result| {
                serde_json::from_value::<CaRotateReport>(result)
                    .ok()
                    .map(|report| render::ca_rotate(&report))
            })
            .await
        }

        CertCommand::CaUninstall { yes, no_wait } => {
            if !yes
                && !agreed(
                    &mut client,
                    "this will take this home's certificate authority out of every store on this \
                     machine that trusts it. The certificate and its key stay on disk, and \
                     `mix doctor --repair` puts the trust back.",
                    json,
                )
                .await?
            {
                return Ok(ExitCode::SUCCESS);
            }

            let started: JobSummary =
                ask(&mut client, rpc::method::CERT_CA_UNINSTALL, None).await?;

            job_answering(&mut client, started, no_wait, json, |result| {
                serde_json::from_value::<CaUninstallReport>(result)
                    .ok()
                    .map(|report| render::ca_uninstall(&report))
            })
            .await
        }
    }
}

/// Say what a command is about to change, name anything else already queued, and ask.
///
/// **T64's rule, adapted to a command that queues its own work.** `mix elevation grant` can read the
/// batch before it asks, because the batch is already there; `cert.ca_uninstall` builds its own
/// batch inside the job, so what this puts in front of a person is the command's own sentence — plus
/// whatever was *already* waiting, because one grant spends one prompt on all of it and a person who
/// typed a certificate command should not discover afterwards that their hosts file moved.
async fn agreed(client: &mut Client, what: &str, json: bool) -> Result<bool, Error> {
    if json {
        return Err(unanswered());
    }

    let waiting: ElevationStatus = ask(client, rpc::method::ELEVATION_STATUS, None).await?;

    let also = match waiting.pending.is_empty() {
        true => String::new(),
        false => format!(
            "\n\nthis machine is also holding these, and one prompt covers them all:\n{}",
            render::elevation_prompt(&waiting)
        ),
    };

    match confirm::ask(&format!("{what}{also}\n\ncontinue? [y/N] ")) {
        confirm::Answer::Yes => Ok(true),

        confirm::Answer::No => {
            let _ = writeln!(std::io::stderr(), "nothing was changed");
            Ok(false)
        }

        confirm::Answer::Unanswerable => Err(unanswered()),
    }
}

/// Follow a job to its end and render whatever it produced, or print the job when it produced
/// nothing this command knows how to read.
///
/// **The result is decoded here and not in [`render::job_status`].** That function tries several
/// types in turn against one `serde_json::Value`, and T54's two reports are similar enough — an
/// `outcome` and a `status` apiece — that adding them to that chain is how a rotation gets rendered
/// as a removal. A command knows its own type.
async fn job_answering(
    client: &mut Client,
    started: JobSummary,
    no_wait: bool,
    json: bool,
    render_result: impl Fn(serde_json::Value) -> Option<String>,
) -> Result<ExitCode, Error> {
    if no_wait {
        emit(&rendered(json, &started, || render::job_status(&started)))?;
        return Ok(ExitCode::SUCCESS);
    }

    let finished = follow(client, started, json).await?;

    match finished.outcome.clone() {
        Some(JobOutcome::Succeeded { result }) => match render_result(result.clone()) {
            Some(said) => emit(&rendered(json, &result, || said))?,
            None => emit(&rendered(json, &finished, || render::job_status(&finished)))?,
        },
        _ => emit(&rendered(json, &finished, || render::job_status(&finished)))?,
    }

    Ok(match render::job_succeeded(&finished) {
        true => ExitCode::SUCCESS,
        false => ExitCode::FAILURE,
    })
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
/// The answers to a plan's version questions, or [`None`] when somebody cancelled.
///
/// **One question per mismatch, and the flags answer them all** (the T78 design, D6). A person
/// answers them one at a time; a script passes `--install-missing` or `--use-installed` and is never
/// asked. Standard input at end of file with a question outstanding is a refusal naming the two
/// flags, on `confirm.rs`' standing rule: what must not happen is a prompt nobody is there to see.
fn answered(
    plan: &BlueprintPlan,
    install_missing: bool,
    use_installed: bool,
    json: bool,
) -> Result<Option<Vec<VersionAnswer>>, Error> {
    let mut answers = Vec::new();

    for step in &plan.steps {
        let Disposition::Choice { installed, wanted } = &step.disposition else {
            continue;
        };

        let Some(subject) = subject_of(&step.action) else {
            continue;
        };

        let answer = match (install_missing, use_installed) {
            (true, _) => MismatchAnswer::Install,
            (_, true) => MismatchAnswer::UseInstalled,

            (false, false) => {
                // A `--json` run has nobody at a keyboard by construction, and a question it cannot
                // ask is one it must not answer on somebody's behalf.
                if json {
                    return Err(unanswerable_question());
                }

                match confirm::choose(&format!(
                    "{subject} {} is not installed. [i]nstall it, [u]se the installed {}, or \
                     [c]ancel? ",
                    wanted.as_str(),
                    installed.as_str()
                )) {
                    confirm::Choice::Install => MismatchAnswer::Install,
                    confirm::Choice::UseInstalled => MismatchAnswer::UseInstalled,

                    confirm::Choice::Cancel => {
                        let _ = writeln!(
                            std::io::stderr(),
                            "nothing was applied; run the same command again when you have decided"
                        );

                        return Ok(None);
                    }

                    confirm::Choice::Unanswerable => return Err(unanswerable_question()),
                }
            }
        };

        answers.push(VersionAnswer { subject, answer });
    }

    Ok(Some(answers))
}

/// What a version question is about, where the action is one that can raise one.
fn subject_of(action: &PlanAction) -> Option<AnswerSubject> {
    match action {
        PlanAction::InstallRuntime { kind, .. } => Some(AnswerSubject::Runtime { kind: *kind }),

        PlanAction::EnsureService {
            package, instance, ..
        } => service_id(&format!("{package}@{instance}"))
            .or_else(|_| service_id(package))
            .ok()
            .map(|id| AnswerSubject::Service { id }),

        _ => None,
    }
}

/// Spend the one elevation prompt an apply queued, having asked first unless told not to.
///
/// **A client is the only thing allowed to raise one** — `.claude/architecture/daemon-and-ipc.md`'s
/// rule, which the daemon's own elevation queue is built around. An apply enqueues; this is where
/// somebody says yes.
async fn granted(client: &mut Client, grant: bool, json: bool) -> Result<ExitCode, Error> {
    let waiting: ElevationStatus = ask(client, rpc::method::ELEVATION_STATUS, None).await?;

    // Nothing waiting is the ordinary end of an apply on a machine that already had the names in
    // its hosts file — or one that cannot prompt at all, which left the queue where it was.
    if waiting.pending.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }

    // **An apply that worked is not a failure because nobody was there to say yes.** The names are
    // in the daemon's queue and one command spends them, so what a `--json` run and a closed
    // standard input get is that sentence rather than an error about a question nobody heard.
    if !grant && (json || !asked_to_grant(&waiting)) {
        let _ = writeln!(
            std::io::stderr(),
            "{} still waiting; `mix elevation grant` writes them",
            waiting.pending.len()
        );

        return Ok(ExitCode::SUCCESS);
    }

    let started: JobSummary = ask(client, rpc::method::ELEVATION_GRANT, None).await?;
    let finished = follow(client, started, json).await?;
    emit(&rendered(json, &finished, || render::job_status(&finished)))?;

    Ok(match render::job_succeeded(&finished) {
        true => ExitCode::SUCCESS,
        false => ExitCode::FAILURE,
    })
}

/// Whether somebody at the keyboard said yes to spending the prompt.
///
/// Anything but a typed yes — including a closed standard input — is *not now*, which is a state the
/// same command gets out of and never an error: the queue is untouched either way.
fn asked_to_grant(waiting: &ElevationStatus) -> bool {
    matches!(
        confirm::ask(&format!(
            "{}\ngrant now? [y/N] ",
            render::elevation_prompt(waiting)
        )),
        confirm::Answer::Yes
    )
}

/// The refusal for a version question nothing could answer.
fn unanswerable_question() -> Error {
    Error::new(
        ErrorCode::InvalidArgument,
        "this blueprint asks for a version this machine does not have, and nothing answered the \
         question",
    )
    .with_hint(
        "`--install-missing` installs what it asks for; `--use-installed` takes what is here",
    )
}

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

/// Agreement to run the blueprint's own command, or [`None`] where there is nothing to agree to.
///
/// Roadmap task **T78a**, its design's D4 and D15. **The command is printed exactly as it will run**
/// — a confirmation that paraphrased would be a confirmation to something else — and the flag that
/// answers it depends on who wrote the blueprint: `--run-scaffold` for one the gallery signed, and
/// `--run-untrusted-scaffold` for one nobody vouches for. Neither covers the other, so a script that
/// runs somebody's unsigned command says so on the line that does it.
///
/// Declining is not a failure. The apply goes ahead without the command and the step comes back
/// saying it was left, which is a project a person can use and one line they can run themselves.
///
/// # Errors
///
/// `invalid_argument` when there is a question and nothing to answer it with: a `--json` run, or a
/// standard input that is closed — [`answered`]'s `Unanswerable` rule, unchanged.
fn agreed_to_scaffold(
    plan: &BlueprintPlan,
    run_scaffold: bool,
    run_untrusted_scaffold: bool,
    json: bool,
) -> Result<Option<ScaffoldConsent>, Error> {
    let Some(command) = plan.steps.iter().find_map(|step| match &step.action {
        PlanAction::RunScaffold { command } => Some(command.clone()),
        _ => None,
    }) else {
        return Ok(None);
    };

    let untrusted = !plan.trusted;

    let given = match untrusted {
        true => run_untrusted_scaffold,
        false => run_scaffold,
    };

    if given {
        return Ok(Some(ScaffoldConsent { command, untrusted }));
    }

    // The flag for the other kind of blueprint is not an answer about this one, and saying so is
    // more use than running the command or silently skipping it.
    let wrong_flag = match untrusted {
        true => run_scaffold,
        false => run_untrusted_scaffold,
    };

    if wrong_flag {
        return Err(Error::new(
            ErrorCode::InvalidArgument,
            match untrusted {
                true => format!(
                    "nothing vouches for {}, so `--run-scaffold` does not answer for it",
                    plan.blueprint
                ),
                false => format!(
                    "{} is signed, so `--run-untrusted-scaffold` does not answer for it",
                    plan.blueprint
                ),
            },
        )
        .with_hint(match untrusted {
            true => "`--run-untrusted-scaffold` runs it anyway",
            false => "`--run-scaffold` runs it",
        }));
    }

    // **Nobody to ask is not a refusal here, and that is a departure from `answered`.** A version
    // question has no safe default — the two answers leave different machines — so a `--json` run
    // with one outstanding is refused. This question does have one: not running somebody else's
    // command leaves a project that works and a line saying what was left, and there is no flag for
    // *no*, so refusing would make "apply this without its command" impossible from a script.
    if json {
        return Ok(unasked(&command, untrusted));
    }

    let vouched = vouching(untrusted, plan.signature);

    match confirm::ask(&format!(
        "{vouched} It wants to run, in the new project's directory:\n\n    {command}\n\nRun it? \
         [y/N] "
    )) {
        confirm::Answer::Yes => Ok(Some(ScaffoldConsent { command, untrusted })),

        // Declining leaves the step as a sentence and applies everything else, which is what the
        // daemon does with an apply that carries no consent at all.
        confirm::Answer::No => Ok(None),

        confirm::Answer::Unanswerable => Ok(unasked(&command, untrusted)),
    }
}

/// What a person is told about the blueprint whose command they are being asked to run.
///
/// **Which kind of untrusted, and not merely that it is** — roadmap task **T79b**. This is the
/// moment the reason is worth most: somebody is about to run a stranger's command, and "a signature
/// came with this and it is not the gallery's" is a different thing to weigh than "nobody signed
/// it". It changes what is *said* and nothing about what is allowed — one flag still answers for
/// both kinds (the T79b design's D8).
///
/// A function of its own because the question it belongs to is only asked on a real apply, with a
/// keyboard in front of it: a sentence reachable no other way is a sentence nothing can check.
fn vouching(untrusted: bool, signature: Option<SignatureCheck>) -> &'static str {
    match (untrusted, signature) {
        (false, _) => "This blueprint is signed.",

        // True of all three things the verifier folds together, which is why it does not say the
        // bytes changed: a colleague's own key and a corrupt `.minisig` land here too.
        (true, Some(SignatureCheck::Rejected)) => {
            "A signature came with this blueprint and it is not the gallery's."
        }

        // Nothing came with it, or the row is older than the reason.
        (true, _) => "Nothing vouches for this blueprint.",
    }
}

/// Say on stderr that a command was left unrun because there was nobody to ask, and leave it.
///
/// Roadmap task **T78a**. The apply goes ahead: what it makes is a project somebody can use, and
/// the step's own outcome says the command is still theirs to run.
fn unasked(command: &str, untrusted: bool) -> Option<ScaffoldConsent> {
    let flag = match untrusted {
        true => "--run-untrusted-scaffold",
        false => "--run-scaffold",
    };

    let _ = writeln!(
        std::io::stderr(),
        "mix: `{command}` was not run — nothing here could be asked. `{flag}` agrees to it."
    );

    None
}

/// Print what a job's own command says, for as long as it says anything.
///
/// Roadmap task **T78a**. A task rather than part of the wait, because the two are different
/// streams: progress comes back on `job.wait` and output comes down `GET /logs/job/<id>`, and one
/// connection cannot carry both. [`None`] when the daemon cannot be reached a second time, which is
/// a log this command does without rather than an apply it refuses to run.
async fn watch_job_log(
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    job: JobId,
    json: bool,
) -> Option<tokio::task::JoinHandle<()>> {
    let mut client = Client::connect(endpoint, autostart).await.ok()?;
    let path = format!("logs/job/{}", job.0);

    Some(tokio::spawn(async move {
        let _ = logs(&mut client, &path, 0, true, json).await;
    }))
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

        // **The one job command that is not a call** — roadmap task **T78a**: output travels on its
        // own stream, never on the event stream and never as a method's answer (ADR 0009).
        JobCommand::Logs { job, follow, lines } => {
            return logs(&mut client, &format!("logs/job/{job}"), lines, follow, json).await;
        }
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

        ServiceCommand::Limits { service, command } => {
            let report: ServiceLimitsReport = match command {
                // A read. `ServiceTarget` rather than a bare id because that is the shape every
                // other `service.*` read takes, and the daemon refuses one with no service named.
                None => {
                    let target = ServiceTarget {
                        service: Some(service.clone()),
                        wait: false,
                    };

                    ask(&mut client, rpc::method::SERVICE_LIMITS, encode(&target)).await?
                }

                Some(LimitsCommand::Set {
                    cpu,
                    memory,
                    priority,
                }) => {
                    let asked = ServiceLimitsSet {
                        service: service.clone(),
                        limits: ResourceLimits {
                            cpu_percent: *cpu,
                            memory_mb: *memory,
                            priority: Priority::from(*priority),
                        },
                    };

                    ask(&mut client, rpc::method::SERVICE_SET_LIMITS, encode(&asked)).await?
                }

                // The same method, with the value that means "nothing". One door rather than two,
                // so there is no second place for the rules to be applied differently.
                Some(LimitsCommand::Clear) => {
                    let asked = ServiceLimitsSet {
                        service: service.clone(),
                        limits: ResourceLimits::default(),
                    };

                    ask(&mut client, rpc::method::SERVICE_SET_LIMITS, encode(&asked)).await?
                }
            };

            emit(&rendered(json, &report, || render::service_limits(&report)))?;
            return Ok(ExitCode::SUCCESS);
        }

        ServiceCommand::Idle {
            service,
            after,
            never,
            default,
        } => {
            // The three flags are one `clap` group, so at most one of them is set and this is a
            // read when none is. `--default` is the absent value and `--never` is zero: the two
            // that look alike from outside and are stored differently on purpose.
            let change = match (*after, *never, *default) {
                (Some(minutes), _, _) => Some(Some(minutes)),
                (None, true, _) => Some(Some(0)),
                (None, false, true) => Some(None),
                (None, false, false) => None,
            };

            let report: IdleReport = match change {
                None => {
                    let target = ServiceTarget {
                        service: Some(service.clone()),
                        wait: false,
                    };

                    ask(&mut client, rpc::method::SERVICE_IDLE, encode(&target)).await?
                }

                Some(minutes) => {
                    let asked = ServiceIdleSet {
                        service: service.clone(),
                        minutes,
                    };

                    ask(&mut client, rpc::method::SERVICE_SET_IDLE, encode(&asked)).await?
                }
            };

            emit(&rendered(json, &report, || render::service_idle(&report)))?;
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
            return logs(
                &mut client,
                &format!("logs/service/{service}"),
                *lines,
                *follow,
                json,
            )
            .await;
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
/// `subject` is the route's two segments — `service/<id>` or `job/<id>` (roadmap task **T78a**) —
/// because what differs between the two is the path and nothing else this function does.
async fn logs(
    client: &mut Client,
    subject: &str,
    lines: usize,
    follow: bool,
    json: bool,
) -> Result<ExitCode, Error> {
    let path = format!("/{subject}?tail={lines}&follow={}", u8::from(follow));
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

/// `2h` as a number of seconds, for `--for` — roadmap task **T76**.
///
/// **Hand-written rather than a dependency.** Four suffixes and a bare number of seconds is the
/// whole grammar this flag needs, and every entry in this workspace's `Cargo.toml` carries a
/// paragraph justifying itself; a parser for `2h` cannot write one.
///
/// # Errors
///
/// The sentence a person reads, for anything that is not a positive length of time. Zero is refused
/// rather than read as "no limit": a share that ends when it begins is not a share, and a flag
/// silently ignored is the other way to be wrong.
fn for_seconds(text: &str) -> Result<u64, String> {
    let refusal = || format!("`{text}` is not a length of time — try `30s`, `90m`, `2h` or `1d`");

    let split = text
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, unit) = text.split_at(split);

    let value: u64 = digits.parse().map_err(|_| refusal())?;

    let multiplier = match unit {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        _ => return Err(refusal()),
    };

    match value.checked_mul(multiplier) {
        Some(0) | None => Err(refusal()),
        Some(seconds) => Ok(seconds),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_length_of_time_is_read_from_its_suffix() {
        assert_eq!(for_seconds("30s"), Ok(30));
        assert_eq!(for_seconds("30"), Ok(30));
        assert_eq!(for_seconds("90m"), Ok(5_400));
        assert_eq!(for_seconds("2h"), Ok(7_200));
        assert_eq!(for_seconds("1d"), Ok(86_400));
    }

    /// **Zero is refused rather than treated as "no limit".** A share that ends the instant it
    /// begins is not a share, and silently ignoring the flag would be the other way to be wrong.
    #[test]
    fn a_length_of_time_of_zero_or_a_word_is_refused() {
        assert!(for_seconds("0").is_err());
        assert!(for_seconds("0h").is_err());
        assert!(for_seconds("soon").is_err());
        assert!(for_seconds("2 hours").is_err());
        assert!(for_seconds("").is_err());
        assert!(for_seconds("-1").is_err());
    }

    /// A length large enough to overflow is a refusal and not a wrap.
    #[test]
    fn a_length_too_large_to_hold_is_refused() {
        assert!(for_seconds(&format!("{}d", u64::MAX)).is_err());
    }

    /// **Every command this binary offers is a command clap can build.**
    ///
    /// `debug_assert` is clap's own check for the mistakes a type system cannot catch — two
    /// arguments sharing an id, a positional after a variadic one — and it runs at *parse* time,
    /// which means without this test the first person to meet one is whoever typed the command.
    /// T77 met exactly that: `mix blueprint capture --name` collided with the `name` field of the
    /// flattened project argument, and every unit test in this crate passed while the command
    /// panicked on the first real run.
    #[test]
    fn every_command_is_one_clap_can_build() {
        use clap::CommandFactory as _;

        Args::command().debug_assert();
    }

    /// **The question names which kind of untrusted it is** — roadmap task **T79b**. It is asked
    /// only on a real apply, so this is where the three sentences are checked.
    #[test]
    fn the_scaffold_question_says_what_is_known_about_the_blueprint() {
        assert!(super::vouching(false, Some(SignatureCheck::Verified)).contains("is signed"));

        let rejected = super::vouching(true, Some(SignatureCheck::Rejected));
        assert!(rejected.contains("not the gallery's"), "{rejected}");

        let missing = super::vouching(true, Some(SignatureCheck::Missing));
        assert!(missing.contains("Nothing vouches"), "{missing}");

        // A row older than this task, or one whose reason this build cannot read: the sentence it
        // has always had, which says the true half of what is known.
        assert_eq!(super::vouching(true, None), missing);
    }
}
