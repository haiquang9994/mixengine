//! What `mix` puts on screen.
//!
//! The two renderings are deliberately not the same information twice at different widths. `--json`
//! is a contract: whatever the daemon answered, serialised, with the client's own identity beside it
//! so a captured file says which `mix` produced it (`.claude/features/gui.md` calls this "copy
//! diagnostics"). The human one is a person's answer to "is it up, and which one am I talking to",
//! and leaves out anything they would not read.
//!
//! No colour, and no dependency for one. Nearly every line `mix` prints ends up pasted into a bug
//! report or an issue, and escape codes there are noise — the daemon makes the same call about its
//! own log file, which is coloured on stderr and never in `daemon.log`.

use std::time::SystemTime;

use mixengine_proto::{
    DaemonShutdown, DaemonStatus, DaemonVersion, ExtensionChange, ExtensionList, ExtensionSource,
    JobList, JobOutcome, JobState, JobSummary, Linkage, PROTOCOL_VERSION, PackageCatalogue,
    PackageList, PackageRemoval, PathReport, PoolOutcome, ResolvedRuntime, RuntimeCatalogue,
    RuntimeList, RuntimeRemoval, RuntimeSource, RuntimeSummary, ServiceCreation, ServiceId,
    ServiceList, ServiceRemoval, ServiceState, ServiceSummary, ServiceWalk, StateReason, Timestamp,
    Uptime,
};

/// `mix status`, for a person.
pub(crate) fn status(status: &DaemonStatus) -> String {
    let mut rendered = format!(
        "mixengined {} — running (pid {}, up {})\n",
        status.version,
        status.pid,
        uptime(status.uptime)
    );

    // The home first, because it is the single most useful line when somebody is talking to a daemon
    // they did not expect to be talking to — which is the whole reason the field exists.
    for (label, value) in [
        ("home", status.home.as_str()),
        ("endpoint", status.endpoint.as_str()),
        ("database", status.database.as_str()),
        ("protocol", &status.protocol.0.to_string()),
    ] {
        rendered.push_str(&format!("  {label:9} {value}\n"));
    }

    // Same protocol, different builds: not an error — the handshake would have refused it if it
    // were — but the explanation for a `mix` that has a command the daemon answers `not_found` to.
    if status.version != env!("CARGO_PKG_VERSION") {
        rendered.push_str(&format!(
            "  note      mix is {} and this daemon is {} — they speak the same protocol, so this \
             is a daemon that has not been restarted since the upgrade\n",
            env!("CARGO_PKG_VERSION"),
            status.version
        ));
    }

    rendered
}

/// `mix status --json`.
///
/// An envelope rather than the daemon's answer alone. The daemon half is `DaemonStatus` verbatim, so
/// `mix status --json | jq .daemon.pid` reads the field by the name the API gives it; the client
/// half is the part no daemon can report, and version skew is the first thing anybody looks for in a
/// captured diagnostic.
pub(crate) fn status_json(status: &DaemonStatus) -> serde_json::Value {
    serde_json::json!({
        "client": client(),
        "daemon": status,
    })
}

/// `mix daemon stop`, for a person.
///
/// **The headline is the daemon and the detail is the services**, indented under it, because that is
/// what was asked for: `mix service stop` reports on services and this reports on a daemon that
/// happens to have stopped some. The walk itself is rendered by [`service_walk`] rather than a
/// second time here — a service that would not stop reads the same in both places, and two renderings
/// of one failure eventually disagree about it.
///
/// A daemon with nothing to stop says only the first line. `service_walk`'s "this home declares no
/// services" is the right sentence for a command that was *about* services and the wrong one here,
/// where nothing was asked about them.
///
/// **A shutdown that could not be ordered says so before anything else**, because the two answers
/// are otherwise the same one: an empty walk from a home with nothing to stop, and an empty walk
/// from a daemon that could not work out how to stop what it had. The second is the one a user has
/// to know about — everything went down at the same moment instead of dependents first — and the
/// only reason this can say it is that [`DaemonShutdown::unordered`] carries the reason. Rendered as
/// the daemon's own sentence, hint and all, rather than reworded here: the file to fix is named in
/// it, and `mix service list` will complain about that same file in those same words.
pub(crate) fn daemon_shutdown(shutdown: &DaemonShutdown) -> String {
    let mut rendered = String::from("mixengined is stopping\n");

    if let Some(why) = &shutdown.unordered {
        rendered.push_str(
            "  the services were not stopped in dependency order — mixengined could not work one \
             out, so all of them stopped at the same time\n",
        );

        // The wire error's own `Display`, which is the message and then the hint on a line of its
        // own; each line is indented under the headline the way the walk below is.
        for line in why.to_string().lines() {
            rendered.push_str(&format!("  {line}\n"));
        }
    }

    if shutdown.services.planned.is_empty() {
        return rendered;
    }

    for line in service_walk(Walked::Stop, &shutdown.services).lines() {
        rendered.push_str(&format!("  {line}\n"));
    }

    rendered
}

/// What a walk was aiming for, in the one place the three commands differ.
///
/// `service.start`, `service.stop` and `service.restart` answer the same [`ServiceWalk`], so the
/// only thing a rendering needs from the command is the verb — and having it as a type rather than a
/// string is what stops "stopped" being printed by the one that starts things.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Walked {
    Start,
    Stop,
    Restart,
}

impl Walked {
    /// What a service that got there did.
    const fn reached(self) -> &'static str {
        match self {
            Self::Start => "started",
            Self::Stop => "stopped",
            Self::Restart => "restarted",
        }
    }

    /// What the one that did not get there failed to do.
    const fn failed(self) -> &'static str {
        match self {
            Self::Start => "failed to start",
            Self::Stop => "failed to stop",
            Self::Restart => "failed to restart",
        }
    }

    /// The verb in the present, for a walk nobody is waiting for.
    const fn ongoing(self) -> &'static str {
        match self {
            Self::Start => "starting",
            Self::Stop => "stopping",
            Self::Restart => "restarting",
        }
    }
}

/// `mix service list`, for a person.
///
/// A table because the question it answers is a comparison — which of these is up — and one block
/// per service would put the states four lines apart. `supervised` gets a column of its own rather
/// than being folded into the state: a row that says `running` with nothing supervising it is a
/// daemon that was killed, and merging the two would hide exactly the case worth seeing.
pub(crate) fn service_list(list: &ServiceList) -> String {
    if list.services.is_empty() {
        return "no services are declared in this home\n".to_owned();
    }

    let rows: Vec<[String; 5]> = list
        .services
        .iter()
        .map(|service| {
            [
                service.id.to_string(),
                state(service),
                match service.supervised {
                    true => "yes".to_owned(),
                    false => "no".to_owned(),
                },
                service
                    .pid
                    .map_or_else(|| MISSING.to_owned(), |pid| pid.to_string()),
                names(&service.depends_on),
            ]
        })
        .collect();

    table(
        ["SERVICE", "STATE", "SUPERVISED", "PID", "DEPENDS ON"],
        &rows,
    )
}

/// `mix service status <service>`, for a person.
pub(crate) fn service_status(service: &ServiceSummary) -> String {
    let mut rendered = format!("{} — {}\n", service.id, state(service));

    let mut field = |label: &str, value: &str| {
        rendered.push_str(&format!("  {label:11} {value}\n"));
    };

    field("supervised", if service.supervised { "yes" } else { "no" });

    if let Some(pid) = service.pid {
        field("pid", &pid.to_string());
    }
    if let Some(port) = service.port {
        field("port", &port.to_string());
    }
    if let Some(started) = service.last_started_at {
        // The label, not the value: `last_started_at` outlives the run it names, so a service that
        // has been stopped still has one — and `stopped` with `started 4m ago` under it reads as a
        // contradiction rather than as the history it is.
        let label = match in_the_run_it_names(service.state) {
            true => "started",
            false => "last start",
        };
        field(label, &ago(started, SystemTime::now()));
    }
    if let Some(code) = service.last_exit_code {
        field("last exit", &code.to_string());
    }
    if !service.depends_on.is_empty() {
        field("depends on", &names(&service.depends_on));
    }

    // The two states that need a sentence rather than a word, for the same reason `mix status`
    // explains a daemon from another build: neither is wrong, and neither is what a user assumes.
    if service.state.is_none() {
        field(
            "note",
            "this service is declared and has never been created, so there is nothing to start yet",
        );
    } else if !service.supervised && service.pid.is_some() {
        field(
            "note",
            "the row names a process and nothing in this daemon is supervising it — that is what a \
             daemon which was killed leaves behind",
        );
    }

    rendered
}

/// `mix service start|stop|restart`, for a person.
///
/// **The failure leads**, where everything that went right is a list underneath it. A walk of six
/// services that stopped at the fourth is read by somebody who wants the name of the one to fix,
/// and putting five lines of `started` above it is five lines between them and the answer.
pub(crate) fn service_walk(walked: Walked, walk: &ServiceWalk) -> String {
    if walk.planned.is_empty() {
        return format!(
            "nothing to {}: this home declares no services\n",
            match walked {
                Walked::Start => "start",
                Walked::Stop => "stop",
                Walked::Restart => "restart",
            }
        );
    }

    if !walk.complete {
        return format!(
            "accepted — mixengined is {} {} in the background\n",
            walked.ongoing(),
            names(&walk.planned)
        );
    }

    let Some(failure) = &walk.failed else {
        return format!("{} {}\n", walked.reached(), names(&walk.reached));
    };

    // A reason is `None` only when the failure was the daemon's own — a database that would not
    // take the write. There is nothing to render and inventing one would be worse than saying so.
    let mut rendered = match &failure.reason {
        Some(reason) => format!("{} {} — {reason}\n", failure.service, walked.failed()),
        None => format!(
            "{} {} — mixengined did not say why; logs/daemon.log has it\n",
            failure.service,
            walked.failed()
        ),
    };

    // The evidence, and the only part of a reason a client lays out itself: `StateReason`'s own
    // sentence says how many attempts there were, and these are the lines that say what went wrong.
    if let Some(StateReason::CrashLoop { tail, .. }) = &failure.reason {
        for line in tail {
            rendered.push_str(&format!("    {line}\n"));
        }
    }

    if !walk.reached.is_empty() {
        rendered.push_str(&format!(
            "  {:9} {}\n",
            walked.reached(),
            names(&walk.reached)
        ));
    }

    if !walk.blocked.is_empty() {
        rendered.push_str(&format!("  {:9} {}\n", "blocked", names(&walk.blocked)));
    }

    rendered
}

/// What is printed where a service has no value for something.
const MISSING: &str = "—";

/// What a summary says a service is doing, in one word.
fn state(service: &ServiceSummary) -> String {
    service
        .state
        .map_or_else(|| "not created".to_owned(), |state| state.to_string())
}

/// Whether the run `last_started_at` names is the one the service is still in.
///
/// Matched exhaustively on purpose, which is what [`ServiceState`] being a closed enum is for: a
/// state added later has to face this question rather than fall into a default. `Restarting` is on
/// the false side — a service waiting out a backoff has no process at all, so its last start is as
/// much history as a stopped one's.
const fn in_the_run_it_names(state: Option<ServiceState>) -> bool {
    match state {
        Some(
            ServiceState::Starting
            | ServiceState::Running
            | ServiceState::Degraded
            | ServiceState::Stopping,
        ) => true,
        Some(ServiceState::Stopped | ServiceState::Restarting | ServiceState::Failed) | None => {
            false
        }
    }
}

/// `mix runtime list`, for a person.
///
/// The default is a column rather than a mark beside the version, because the question somebody
/// scanning this asks is "which one does `php` mean" and a `*` is a footnote they have to look up.
pub(crate) fn runtime_list(list: &RuntimeList) -> String {
    if list.runtimes.is_empty() {
        return "no runtimes are installed — `mix runtime available` lists what can be\n"
            .to_owned();
    }

    let now = SystemTime::now();
    let rows: Vec<[String; 5]> = list
        .runtimes
        .iter()
        .map(|runtime| {
            [
                runtime.kind.to_string(),
                runtime.version.to_string(),
                match runtime.default {
                    true => "yes".to_owned(),
                    false => MISSING.to_owned(),
                },
                size(runtime.bytes),
                ago(runtime.installed_at, now),
            ]
        })
        .collect();

    table(
        ["RUNTIME", "VERSION", "DEFAULT", "SIZE", "INSTALLED"],
        &rows,
    )
}

/// `mix runtime ext list`, for a person.
///
/// One row per extension: what it is called, whether it can be turned off, and who decided. The last
/// column is the one the command is usually run for — *on because the build says so* and *on because
/// you turned it on* are different answers to why xdebug is loaded.
pub(crate) fn extension_list(list: &ExtensionList) -> String {
    if list.extensions.is_empty() {
        return "this build declares no extensions — nothing to turn on or off\n".to_owned();
    }

    let rows: Vec<[String; 4]> = list
        .extensions
        .iter()
        .map(|extension| {
            [
                extension.name.clone(),
                match extension.linkage {
                    Linkage::Static => "compiled in".to_owned(),
                    Linkage::Shared => "module".to_owned(),
                    _ => MISSING.to_owned(),
                },
                match extension.enabled {
                    true => "on".to_owned(),
                    false => "off".to_owned(),
                },
                match extension.source {
                    ExtensionSource::BuildDefault => "this build".to_owned(),
                    ExtensionSource::User => "you".to_owned(),
                    _ => MISSING.to_owned(),
                },
            ]
        })
        .collect();

    table(["EXTENSION", "KIND", "STATE", "DECIDED BY"], &rows)
}

/// `mix runtime ext enable` and `disable`, for a person.
///
/// Says what it deliberately did *not* do to the pool, because the alternative is a client guessing
/// from the operating system it happens to be running on.
pub(crate) fn extension_change(change: &ExtensionChange) -> String {
    let state = match change.extension.enabled {
        true => "enabled",
        false => "disabled",
    };

    let pool = match change.pool {
        PoolOutcome::Reloaded => "its pool re-read its configuration",
        PoolOutcome::RestartRequired => {
            "the running pool is still using the previous set — restart it to pick this up"
        }
        PoolOutcome::PoolNotRunning => "its pool is not running and will read this when it starts",
        _ => "what its pool did is not something this build can describe",
    };

    format!("{} {state}; {pool}\n", change.extension.name)
}

/// `mix package list`, for a person.
///
/// The last column is what a person opens this listing to find out when an uninstall was refused:
/// which services are instances of this version, and therefore what has to go first.
#[must_use]
pub(crate) fn package_list(list: &PackageList) -> String {
    if list.packages.is_empty() {
        return "no packages are installed — `mix package available` lists what can be
"
        .to_owned();
    }

    let now = SystemTime::now();
    let rows: Vec<[String; 5]> = list
        .packages
        .iter()
        .map(|package| {
            [
                package.package.clone(),
                package.version.to_string(),
                size(package.bytes),
                ago(package.installed_at, now),
                match package.services.is_empty() {
                    true => MISSING.to_owned(),
                    false => package
                        .services
                        .iter()
                        .map(|service| service.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                },
            ]
        })
        .collect();

    table(
        ["PACKAGE", "VERSION", "SIZE", "INSTALLED", "SERVICES"],
        &rows,
    )
}

/// `mix package available`, for a person.
#[must_use]
pub(crate) fn package_catalogue(catalogue: &PackageCatalogue) -> String {
    let mut rendered = String::new();

    if catalogue.stale {
        rendered.push_str(
            "this list is from a cached index — mixengined could not reach the package index, so              versions published since then are missing
",
        );
    }

    if catalogue.packages.is_empty() {
        rendered.push_str(
            "the package index offers nothing this build can run on this machine
",
        );
        return rendered;
    }

    let rows: Vec<[String; 6]> = catalogue
        .packages
        .iter()
        .map(|release| {
            [
                release.package.clone(),
                release.version.to_string(),
                release.channel.to_string(),
                size(release.bytes),
                match release.installed {
                    true => "yes".to_owned(),
                    false => MISSING.to_owned(),
                },
                release.eol.clone().unwrap_or_else(|| MISSING.to_owned()),
            ]
        })
        .collect();

    rendered.push_str(&table(
        ["PACKAGE", "VERSION", "CHANNEL", "SIZE", "INSTALLED", "EOL"],
        &rows,
    ));
    rendered
}

/// `mix package uninstall`, for a person.
#[must_use]
pub(crate) fn package_removal(removal: &PackageRemoval) -> String {
    format!(
        "removed {} {}
",
        removal.removed.package, removal.removed.version
    )
}

/// `mix service create`, for a person.
///
/// **The second paragraph is the whole reason the answer is not just the service** — roadmap task
/// **T34c**. A recipe's preferred port is the number a person has in their `.env` and in their
/// muscle memory, and a service that was quietly given the next one along would be discovered as a
/// connection that is refused, hours later. So a move is stated at the moment it happens, with as
/// much of the program that took the port as this machine would give up.
#[must_use]
pub(crate) fn service_creation(creation: &ServiceCreation) -> String {
    let mut rendered = format!(
        "created {}
",
        creation.service.id
    );

    if let Some(port) = creation.service.port {
        rendered.push_str(&format!(
            "  it listens on port {port}
"
        ));
    }

    if let Some(moved) = &creation.moved_from {
        let holder = match (&moved.program, moved.pid) {
            (Some(program), _) => format!("{program} has it"),
            (None, Some(pid)) => format!("pid {pid} has it"),
            (None, None) => "another service or program on this machine has it".to_owned(),
        };

        rendered.push_str(&format!(
            "  it asked for {} — {holder}, so it was moved
",
            moved.preferred
        ));
    }

    rendered
}

/// `mix service delete`, for a person.
///
/// **The second line is the whole reason the answer is not just the service.** A delete keeps the
/// data directory, and a person who is not told which one it was has no way to find it later — or to
/// know that deleting the service did not delete their databases.
#[must_use]
pub(crate) fn service_removal(removal: &ServiceRemoval) -> String {
    let mut rendered = format!(
        "deleted {}
",
        removal.removed.id
    );

    match &removal.data_kept {
        Some(path) => rendered.push_str(&format!(
            "  its data is kept at {path}
"
        )),
        None => rendered.push_str(
            "  it had no data directory
",
        ),
    }

    rendered
}

/// `mix runtime available`, for a person.
///
/// **The staleness is a line above the table and not a column**, because it is true of the whole
/// answer: every row came out of the same document, and repeating "from a cached index" against each
/// of forty versions would say it forty times.
pub(crate) fn runtime_catalogue(catalogue: &RuntimeCatalogue) -> String {
    let mut rendered = String::new();

    if catalogue.stale {
        rendered.push_str(
            "this list is from a cached index — mixengined could not reach the package index, so \
             versions published since then are missing\n",
        );
    }

    if catalogue.runtimes.is_empty() {
        rendered.push_str("the package index offers nothing for this machine\n");
        return rendered;
    }

    let rows: Vec<[String; 6]> = catalogue
        .runtimes
        .iter()
        .map(|release| {
            [
                release.kind.to_string(),
                release.version.to_string(),
                release.channel.to_string(),
                size(release.bytes),
                match release.installed {
                    true => "yes".to_owned(),
                    false => MISSING.to_owned(),
                },
                release.eol.clone().unwrap_or_else(|| MISSING.to_owned()),
            ]
        })
        .collect();

    rendered.push_str(&table(
        ["RUNTIME", "VERSION", "CHANNEL", "SIZE", "INSTALLED", "EOL"],
        &rows,
    ));

    rendered
}

/// One installed runtime, for a person: what `mix runtime default` answers and what a finished
/// install produced.
pub(crate) fn runtime_summary(runtime: &RuntimeSummary) -> String {
    let mut rendered = format!(
        "{} {}{}\n",
        runtime.kind,
        runtime.version,
        match runtime.default {
            true => " — the default for its kind",
            false => "",
        }
    );

    for (label, value) in [
        ("path", runtime.path.clone()),
        ("size", size(runtime.bytes)),
        ("installed", ago(runtime.installed_at, SystemTime::now())),
    ] {
        rendered.push_str(&format!("  {label:9} {value}\n"));
    }

    rendered
}

/// `mix runtime uninstall`, for a person.
///
/// The second line is the whole reason the answer is not just the runtime: a kind left with no
/// default is a kind whose shim resolves to nothing, and the person who caused it is the one who
/// should hear about it.
/// `mix runtime resolve`, for a person.
///
/// **The version is the first line and the reason is the last**, in that order because they are read
/// in that order: somebody who already knows which version they expect stops after the first line,
/// and somebody surprised by it reads on to find out which file did it. The path is between them
/// because it is what a person copies.
pub(crate) fn runtime_resolved(resolved: &ResolvedRuntime) -> String {
    let runtime = &resolved.runtime;

    let mut rendered = format!("{} {}\n", runtime.kind, runtime.version);
    rendered.push_str(&format!("  {:9} {}\n", "path", runtime.path));

    if let Some(constraint) = &resolved.constraint {
        rendered.push_str(&format!("  {:9} {constraint}\n", "asked for"));
    }

    let because = match &resolved.source {
        RuntimeSource::Explicit => "what you asked for on this command".to_owned(),
        RuntimeSource::Manifest { path } => path.clone(),
        RuntimeSource::Project { root } => format!("the project registered at {root}"),
        RuntimeSource::Default => format!(
            "the default for {} — nothing here pins a version",
            runtime.kind
        ),
    };
    rendered.push_str(&format!("  {:9} {because}\n", "chosen by"));

    rendered
}

pub(crate) fn runtime_removal(removal: &RuntimeRemoval) -> String {
    let mut rendered = format!(
        "removed {} {}\n",
        removal.removed.kind, removal.removed.version
    );

    if removal.default_cleared {
        rendered.push_str(&format!(
            "  it was the default for {}, and nothing was promoted in its place — \
             `mix runtime default {} <version>` chooses one\n",
            removal.removed.kind, removal.removed.kind
        ));
    }

    rendered
}

/// Which of the three `mix path` subcommands is being rendered.
///
/// The report they answer with is one type — the same sentence about the same directory — and what
/// differs is the first line, because "this is how things stand" and "this is what just happened"
/// are read differently even when the words after them are identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pathed {
    /// `mix path status`.
    Asked,
    /// `mix path install`.
    Installed,
    /// `mix path uninstall`.
    Uninstalled,
}

/// `mix path …`, for a person.
///
/// **The last line is the one that matters and it is about a shell that is not this one.** Nothing
/// `mix` can do changes the PATH of the terminal it was typed in — a child process cannot reach into
/// its parent's environment on any of the three systems — so an install that says nothing looks
/// exactly like one that did not work, to somebody who types `php` immediately afterwards and is
/// told there is no such command.
pub(crate) fn path_report(pathed: Pathed, report: &PathReport) -> String {
    let mut rendered = match (pathed, report.on_path) {
        (Pathed::Asked, true) => format!("{} is on this user's PATH\n", report.directory),
        (Pathed::Asked, false) => format!("{} is not on this user's PATH\n", report.directory),

        (Pathed::Installed, _) => match report.places.iter().any(|place| place.changed) {
            true => format!("{} is now on this user's PATH\n", report.directory),
            false => format!("{} was already on this user's PATH\n", report.directory),
        },

        (Pathed::Uninstalled, _) => match report.places.iter().any(|place| place.changed) {
            true => format!("{} is no longer on this user's PATH\n", report.directory),
            false => format!("{} was not on this user's PATH\n", report.directory),
        },
    };

    for place in &report.places {
        rendered.push_str(&format!(
            "  {} {}\n",
            match place.present {
                true => "in ",
                false => "not in",
            },
            place.name
        ));
    }

    if report.places.is_empty() {
        rendered.push_str("  this machine has nowhere to keep a PATH that survives a reboot\n");
    }

    rendered.push_str(&format!(
        "  {} command{} in it: {}\n",
        report.commands.len(),
        match report.commands.len() {
            1 => "",
            _ => "s",
        },
        match report.commands.is_empty() {
            true => "none — `mix path install` fills the directory".to_owned(),
            false => report.commands.join(", "),
        }
    ));

    for stale in &report.stale {
        rendered.push_str(&format!(
            "  {stale} is in that directory and answers to nothing — it could not be removed\n"
        ));
    }

    if pathed != Pathed::Asked && report.places.iter().any(|place| place.changed) {
        rendered.push_str("open a new terminal for this to take effect\n");
    }

    rendered
}

/// `mix job list`, for a person.
pub(crate) fn job_list(list: &JobList) -> String {
    if list.jobs.is_empty() {
        return "this home has run no jobs\n".to_owned();
    }

    let now = SystemTime::now();
    let rows: Vec<[String; 5]> = list
        .jobs
        .iter()
        .map(|job| {
            [
                job.id.to_string(),
                job.kind.to_string(),
                job.state.to_string(),
                format!("{}%", job.percent),
                ago(job.started_at, now),
            ]
        })
        .collect();

    table(["JOB", "KIND", "STATE", "PROGRESS", "STARTED"], &rows)
}

/// One job, for a person: what `mix job status`, `wait` and `cancel` all answer with.
///
/// **A failed job's error is rendered as the daemon wrote it**, message and hint, rather than
/// summarised here — it is the same wire error the call would have been refused with had the work
/// been short enough to do inline, and rewording it would give one failure two spellings.
pub(crate) fn job_status(job: &JobSummary) -> String {
    let mut rendered = format!("job {} — {} ({})\n", job.id, job.state, job.kind);

    let mut field = |label: &str, value: &str| {
        rendered.push_str(&format!("  {label:9} {value}\n"));
    };

    if !job.message.is_empty() {
        field("doing", &format!("{} ({}%)", job.message, job.percent));
    }
    field("started", &ago(job.started_at, SystemTime::now()));

    match &job.outcome {
        Some(JobOutcome::Failed { error }) => {
            for line in error.to_string().lines() {
                rendered.push_str(&format!("  {line}\n"));
            }
        }

        // The result belongs to the method that produced the job, so this is the one place a
        // rendering has to branch on the kind rather than on the type. `runtime.install` is the only
        // producer there is; anything else prints nothing extra rather than guessing at a shape.
        Some(JobOutcome::Succeeded { result }) => {
            if let Ok(runtime) = serde_json::from_value::<RuntimeSummary>(result.clone()) {
                for line in runtime_summary(&runtime).lines() {
                    rendered.push_str(&format!("  {line}\n"));
                }
            }
        }

        _ => {}
    }

    rendered
}

/// Whether a job that ended did what was asked, which is what an exit code is made of.
pub(crate) fn job_succeeded(job: &JobSummary) -> bool {
    job.state == JobState::Succeeded
}

/// A number of bytes, at the scale a download is read in.
///
/// Whole mebibytes, and never a fraction: what this number answers is "will this take a while and is
/// there room", and `41 MiB` answers it exactly as well as `40.7 MiB` while being a number a person
/// takes in at a glance. `--json` carries the byte count, unrounded.
fn size(bytes: u64) -> String {
    const MIB: u64 = 1 << 20;

    match bytes {
        0 => MISSING.to_owned(),
        // Anything smaller than a mebibyte would round to `0 MiB`, which reads as "nothing" for a
        // file that is really there.
        1..MIB => "< 1 MiB".to_owned(),
        _ => format!("{} MiB", bytes / MIB),
    }
}

/// A list of services, in the order the daemon gave them.
fn names(services: &[ServiceId]) -> String {
    match services.is_empty() {
        true => MISSING.to_owned(),
        false => services
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// A listing with its headings, every column as wide as its widest cell.
///
/// Generic over the number of columns rather than written once per table: four commands here answer
/// with a listing now, and the alternative is four copies of the same width calculation drifting
/// apart in how they pad and where they trim.
fn table<const N: usize>(headings: [&str; N], rows: &[[String; N]]) -> String {
    let widths: [usize; N] = std::array::from_fn(|column| {
        rows.iter()
            .map(|row| row[column].chars().count())
            .chain(std::iter::once(headings[column].chars().count()))
            .max()
            .unwrap_or_default()
    });

    let mut rendered = String::new();
    for row in std::iter::once(headings.map(str::to_owned)).chain(rows.iter().cloned()) {
        let line = row
            .iter()
            .zip(&widths)
            .map(|(cell, width)| format!("{cell:width$}"))
            .collect::<Vec<_>>()
            .join("  ");

        // Trimmed, so a table's last column carries no trailing run of spaces into whatever a
        // person pastes it into.
        rendered.push_str(line.trim_end());
        rendered.push('\n');
    }

    rendered
}

/// How long ago something happened, from this machine's clock.
///
/// **The client's own clock, and it is the daemon's too**: the endpoint is a local socket, so there
/// is exactly one clock involved. `daemon.status` carries an `Uptime` because the daemon knows how
/// long it has been up; nothing carries a "now" for a service, and asking for one would be a round
/// trip to learn what `SystemTime::now` already says.
///
/// A moment in the future — a clock moved backwards between the start and this call — reads as
/// `just now` rather than as a negative age.
fn ago(Timestamp(happened): Timestamp, now: SystemTime) -> String {
    let Timestamp(now) = Timestamp::from_system_time(now);

    match u64::try_from(now.saturating_sub(happened) / 1_000) {
        Ok(0) | Err(_) => "just now".to_owned(),
        Ok(seconds) => format!("{} ago", units(seconds)),
    }
}

/// This build of `mix`, in the shape the daemon reports itself in.
fn client() -> serde_json::Value {
    serde_json::to_value(DaemonVersion {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol: PROTOCOL_VERSION,
    })
    .expect("a DaemonVersion of two owned fields always serialises")
}

/// How long the daemon has been up, in the two units that matter at that scale.
///
/// Two and never three: "up 3d 4h" is what somebody wants from a status line, and "3d 4h 17m 6s" is
/// a number nobody reads. The exact figure is in `--json`, in seconds, unrounded.
fn uptime(Uptime(seconds): Uptime) -> String {
    units(seconds)
}

/// A number of seconds, at the scale a person reads.
///
/// Shared by [`uptime`] and [`ago`] rather than written twice: "up 13m 32s" and "started 13m 32s
/// ago" are the same rounding, and two copies of it would eventually round differently in one place.
fn units(seconds: u64) -> String {
    let (days, hours, minutes, seconds) = (
        seconds / 86_400,
        (seconds % 86_400) / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60,
    );

    match (days, hours, minutes) {
        (0, 0, 0) => format!("{seconds}s"),
        (0, 0, _) => format!("{minutes}m {seconds}s"),
        (0, _, _) => format!("{hours}h {minutes}m"),
        _ => format!("{days}d {hours}h"),
    }
}

#[cfg(test)]
mod tests {
    use mixengine_proto::ServiceState;

    use super::*;

    fn example() -> DaemonStatus {
        DaemonStatus {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol: PROTOCOL_VERSION,
            pid: 4123,
            home: "/home/dev/.local/share/mixengine".to_owned(),
            endpoint: "/home/dev/.local/share/mixengine/run/mixengined.sock".to_owned(),
            database: "/home/dev/.local/share/mixengine/mixengine.db".to_owned(),
            started_at: Timestamp(1_723_000_000_500),
            uptime: Uptime(812),
        }
    }

    #[test]
    fn the_human_rendering_leads_with_whether_it_is_up_and_which_home_it_is() {
        let rendered = status(&example());
        let mut lines = rendered.lines();

        assert_eq!(
            lines.next(),
            Some("mixengined 0.1.0 — running (pid 4123, up 13m 32s)")
        );
        assert_eq!(
            lines.next(),
            Some("  home      /home/dev/.local/share/mixengine")
        );
    }

    #[test]
    fn a_daemon_from_another_build_is_explained_rather_than_left_to_be_noticed() {
        let mut older = example();
        older.version = "0.0.9".to_owned();

        let rendered = status(&older);
        assert!(rendered.contains("has not been restarted"), "{rendered}");
        assert!(rendered.contains("0.0.9"), "{rendered}");

        // And the ordinary case says nothing, because a note on every line of a status somebody
        // reads daily is a note nobody reads.
        assert!(!status(&example()).contains("note"));
    }

    #[test]
    fn the_json_is_the_daemons_answer_untouched_under_one_key() {
        let status = example();
        let encoded = status_json(&status);

        assert_eq!(encoded["daemon"], serde_json::to_value(&status).unwrap());
        assert_eq!(encoded["client"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(encoded["client"]["protocol"], 1);
        // Unrounded, in seconds: the rendering above is for a person and this is for a program.
        assert_eq!(encoded["daemon"]["uptime"], 812);
    }

    fn id(value: &str) -> ServiceId {
        ServiceId::parse(value).expect("a valid service id")
    }

    /// A summary in the shape a given state implies: a running service has a process and a
    /// supervisor, a stopped one has neither, and one with no row has nothing at all.
    fn summary(id_value: &str, state: Option<ServiceState>) -> ServiceSummary {
        let running = state == Some(ServiceState::Running);

        ServiceSummary {
            id: id(id_value),
            state,
            supervised: running,
            pid: running.then_some(4123),
            port: None,
            last_started_at: running.then_some(Timestamp(1_723_000_000_000)),
            last_exit_code: None,
            depends_on: Vec::new(),
        }
    }

    /// A create that got the port it asked for says so, and explains nothing.
    #[test]
    fn a_service_created_on_the_port_it_wanted_is_reported_without_a_story() {
        let rendered = service_creation(&ServiceCreation {
            service: ServiceSummary {
                port: Some(3306),
                ..summary("mariadb@main", Some(ServiceState::Stopped))
            },
            moved_from: None,
        });

        assert!(rendered.contains("created mariadb@main"), "{rendered}");
        assert!(rendered.contains("port 3306"), "{rendered}");
        assert!(
            !rendered.contains("moved"),
            "nothing moved it, so nothing should say so: {rendered}"
        );
    }

    /// One that was moved names the port it wanted and the program that has it.
    ///
    /// The whole point of the field: a developer whose `.env` says 3306 finds out here rather than
    /// from a connection refused an hour later.
    #[test]
    fn a_service_moved_off_its_preferred_port_names_the_program_that_took_it() {
        let rendered = service_creation(&ServiceCreation {
            service: ServiceSummary {
                port: Some(3307),
                ..summary("mysql@main", Some(ServiceState::Stopped))
            },
            moved_from: Some(mixengine_proto::PortMoved {
                preferred: 3306,
                pid: Some(4242),
                program: Some("mysqld.exe".to_owned()),
            }),
        });

        assert!(rendered.contains("port 3307"), "{rendered}");
        assert!(rendered.contains("asked for 3306"), "{rendered}");
        assert!(rendered.contains("mysqld.exe has it"), "{rendered}");
    }

    /// A machine that will name neither the program nor the pid still says what happened.
    ///
    /// Which is the ordinary case for a port another *MixEngine* service holds: the row has it and
    /// there may be no process at all.
    #[test]
    fn a_move_with_nothing_to_name_still_says_the_port_was_taken() {
        let rendered = service_creation(&ServiceCreation {
            service: ServiceSummary {
                port: Some(3307),
                ..summary("mysql@main", Some(ServiceState::Stopped))
            },
            moved_from: Some(mixengine_proto::PortMoved {
                preferred: 3306,
                pid: None,
                program: None,
            }),
        });

        assert!(
            rendered.contains("another service or program on this machine has it"),
            "{rendered}"
        );
    }

    #[test]
    fn the_listing_is_a_table_whose_columns_line_up_whatever_the_names_are() {
        let list = ServiceList {
            services: vec![
                summary("mariadb@main", Some(ServiceState::Running)),
                ServiceSummary {
                    depends_on: vec![id("mariadb@main")],
                    ..summary("php", Some(ServiceState::Stopped))
                },
            ],
        };

        let rendered = service_list(&list);
        let lines: Vec<&str> = rendered.lines().collect();

        assert_eq!(
            lines[0],
            "SERVICE       STATE    SUPERVISED  PID   DEPENDS ON"
        );
        assert_eq!(lines[1], "mariadb@main  running  yes         4123  —");
        assert_eq!(
            lines[2],
            "php           stopped  no          —     mariadb@main"
        );
    }

    #[test]
    fn a_home_with_no_declarations_says_so_rather_than_printing_a_bare_heading() {
        assert_eq!(
            service_list(&ServiceList {
                services: Vec::new()
            }),
            "no services are declared in this home\n"
        );
    }

    #[test]
    fn a_service_that_was_never_created_is_told_apart_from_one_that_is_stopped() {
        let rendered = service_status(&summary("mailpit", None));

        assert!(rendered.starts_with("mailpit — not created"), "{rendered}");
        assert!(rendered.contains("has never been created"), "{rendered}");

        // The ordinary case says nothing extra, because a note on every status is a note nobody
        // reads — the same rule `mix status` follows for a daemon from another build.
        let stopped = service_status(&summary("mailpit", Some(ServiceState::Stopped)));
        assert!(!stopped.contains("note"), "{stopped}");
    }

    #[test]
    fn a_start_time_that_outlived_its_run_is_labelled_as_history_rather_than_as_the_present() {
        let running = service_status(&summary("mariadb@main", Some(ServiceState::Running)));
        assert!(running.contains("started"), "{running}");
        assert!(!running.contains("last start"), "{running}");

        // The same field, and the daemon keeps it across a stop on purpose — so the rendering is
        // what has to stop claiming the service is in the run it names.
        let stopped = ServiceSummary {
            last_started_at: Some(Timestamp(1_723_000_000_000)),
            ..summary("mariadb@main", Some(ServiceState::Stopped))
        };

        let rendered = service_status(&stopped);
        assert!(rendered.contains("last start"), "{rendered}");
        assert!(!rendered.contains("started"), "{rendered}");
    }

    #[test]
    fn a_row_naming_a_process_nothing_is_supervising_is_pointed_at_rather_than_smoothed_over() {
        let orphan = ServiceSummary {
            supervised: false,
            ..summary("mariadb@main", Some(ServiceState::Running))
        };

        let rendered = service_status(&orphan);
        assert!(rendered.contains("supervised  no"), "{rendered}");
        assert!(rendered.contains("daemon which was killed"), "{rendered}");
    }

    #[test]
    fn a_walk_that_reached_everything_is_one_line() {
        let walk = ServiceWalk {
            planned: vec![id("mariadb@main"), id("php-fpm@8.3")],
            complete: true,
            reached: vec![id("mariadb@main"), id("php-fpm@8.3")],
            failed: None,
            blocked: Vec::new(),
        };

        assert_eq!(
            service_walk(Walked::Start, &walk),
            "started mariadb@main, php-fpm@8.3\n"
        );
        assert_eq!(
            service_walk(Walked::Stop, &walk),
            "stopped mariadb@main, php-fpm@8.3\n"
        );
    }

    #[test]
    fn a_walk_that_stopped_leads_with_the_service_to_fix_and_shows_what_it_took_down() {
        let walk = ServiceWalk {
            planned: vec![id("db"), id("web"), id("worker")],
            complete: true,
            reached: vec![id("db")],
            failed: Some(mixengine_proto::ServiceFailure {
                service: id("web"),
                reason: Some(StateReason::CrashLoop {
                    attempts: 5,
                    window: mixengine_proto::Millis::from_secs(300),
                    tail: vec!["Address already in use".to_owned()],
                }),
            }),
            blocked: vec![id("worker")],
        };

        let rendered = service_walk(Walked::Start, &walk);
        let lines: Vec<&str> = rendered.lines().collect();

        // The name of the thing to fix is the first thing on the screen, and the evidence is
        // directly under it — five lines of `started` above both would be five lines in the way.
        assert_eq!(lines[0], "web failed to start — 5 failed starts within 5m");
        assert_eq!(lines[1], "    Address already in use");
        assert_eq!(lines[2], "  started   db");
        assert_eq!(lines[3], "  blocked   worker");
    }

    #[test]
    fn a_walk_nobody_waited_for_says_it_is_still_happening() {
        let accepted = ServiceWalk {
            planned: vec![id("db")],
            complete: false,
            reached: Vec::new(),
            failed: None,
            blocked: Vec::new(),
        };

        assert_eq!(
            service_walk(Walked::Restart, &accepted),
            "accepted — mixengined is restarting db in the background\n"
        );
    }

    #[test]
    fn a_shutdown_says_what_happened_to_the_daemon_and_puts_the_services_under_it() {
        let shutdown = DaemonShutdown {
            services: ServiceWalk {
                planned: vec![id("web"), id("db")],
                complete: true,
                reached: vec![id("web"), id("db")],
                failed: None,
                blocked: Vec::new(),
            },
            unordered: None,
        };

        assert_eq!(
            daemon_shutdown(&shutdown),
            "mixengined is stopping\n  stopped web, db\n"
        );
    }

    #[test]
    fn a_shutdown_with_nothing_to_stop_is_one_line_about_the_daemon() {
        // And specifically not `service_walk`'s "this home declares no services", which answers a
        // question about services that nobody asked here.
        let quiet = DaemonShutdown {
            services: ServiceWalk {
                planned: Vec::new(),
                complete: true,
                reached: Vec::new(),
                failed: None,
                blocked: Vec::new(),
            },
            unordered: None,
        };

        assert_eq!(daemon_shutdown(&quiet), "mixengined is stopping\n");
    }

    /// The same empty walk, and the opposite thing to say about it — which is the whole reason
    /// `unordered` is on the wire at all.
    #[test]
    fn a_shutdown_that_could_not_be_ordered_is_told_apart_from_one_with_nothing_to_stop() {
        let skipped = DaemonShutdown {
            services: ServiceWalk {
                planned: Vec::new(),
                complete: true,
                reached: Vec::new(),
                failed: None,
                blocked: Vec::new(),
            },
            unordered: Some(
                mixengine_proto::Error::new(
                    mixengine_proto::ErrorCode::Internal,
                    "cannot read the declarations in /home/dev/extensions/mailpit/extension.toml",
                )
                .with_hint("`logs/daemon.log` has the detail a report needs"),
            ),
        };

        let rendered = daemon_shutdown(&skipped);
        let lines: Vec<&str> = rendered.lines().collect();

        // The daemon still went, and that is still the headline: what follows is why the stop was
        // not the one the user was promised.
        assert_eq!(lines[0], "mixengined is stopping");
        assert!(
            lines[1].contains("were not stopped in dependency order"),
            "{rendered}"
        );
        assert_eq!(
            lines[2],
            "  cannot read the declarations in /home/dev/extensions/mailpit/extension.toml",
            "the daemon's own sentence, which names the file to fix: {rendered}"
        );
        assert_eq!(
            lines[3], "  hint: `logs/daemon.log` has the detail a report needs",
            "{rendered}"
        );
    }

    #[test]
    fn a_service_that_would_not_stop_is_named_although_the_daemon_stopped_anyway() {
        // T18's one failure: a survivor adopted from a previous daemon that will not die. The daemon
        // goes regardless — refusing would leave a user with no way out — so the report is the whole
        // of what tells them the port is still held.
        let refused = DaemonShutdown {
            services: ServiceWalk {
                planned: vec![id("db")],
                complete: true,
                reached: Vec::new(),
                failed: Some(mixengine_proto::ServiceFailure {
                    service: id("db"),
                    reason: None,
                }),
                blocked: Vec::new(),
            },
            unordered: None,
        };

        let rendered = daemon_shutdown(&refused);
        assert!(
            rendered.starts_with("mixengined is stopping\n"),
            "{rendered}"
        );
        assert!(rendered.contains("db failed to stop"), "{rendered}");
    }

    #[test]
    fn an_age_is_rounded_the_way_an_uptime_is_and_never_reads_as_negative() {
        let started = Timestamp(1_723_000_000_000);
        let now = |offset: i64| {
            std::time::UNIX_EPOCH
                + std::time::Duration::from_millis((started.0 + offset).unsigned_abs())
        };

        assert_eq!(ago(started, now(812_000)), "13m 32s ago");
        assert_eq!(ago(started, now(500)), "just now");

        // A clock that went backwards between the start and this call. Rendering "-4s ago" would
        // make a user doubt the service rather than the clock.
        assert_eq!(ago(started, now(-4_000)), "just now");
    }

    #[test]
    fn uptime_stops_at_two_units_whichever_two_they_are() {
        assert_eq!(uptime(Uptime(0)), "0s");
        assert_eq!(uptime(Uptime(59)), "59s");
        assert_eq!(uptime(Uptime(60)), "1m 0s");
        assert_eq!(uptime(Uptime(3_599)), "59m 59s");
        assert_eq!(uptime(Uptime(3_600)), "1h 0m");
        assert_eq!(uptime(Uptime(86_399)), "23h 59m");
        assert_eq!(uptime(Uptime(86_400)), "1d 0h");
        assert_eq!(uptime(Uptime(9_000_000)), "104d 4h");
    }
}
