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
/// `*.test, *.localhost`, which is how a wildcard route reads in a sentence.
///
/// An empty list cannot reach this — the mode is `hosts_only` when nothing is routed — but it is
/// answered rather than left to render as nothing, because a status line that trails off is worse
/// than one that says the awkward thing.
fn patterns(tlds: &[String]) -> String {
    if tlds.is_empty() {
        return "no names".to_owned();
    }

    tlds.iter()
        .map(|tld| format!("*.{tld}"))
        .collect::<Vec<_>>()
        .join(", ")
}

use mixengine_proto::{
    Action, ApiAccess, ArtifactAvailability, BlueprintApplied, BlueprintList, BlueprintPlan,
    BlueprintSummary, BrowserDatabase, Browsers, BundleReport, CaRotateReport, CaState, CaStatus,
    CaUninstallReport, CertIssueReport, CertProblem, CertState, CertStatusReport, DaemonShutdown,
    DaemonStatus, DaemonVersion, DatabaseAccount, Disposition, DnsMode, DoctorReport,
    DomainStatusReport, ElevationStatus, Enforcement, ExtensionCatalogue, ExtensionChange,
    ExtensionInspection, ExtensionKind, ExtensionList, ExtensionPlan, ExtensionRemoval,
    ExtensionSource, FilesystemReach, GrantOutcome, Handshake, IdleExemption, IdleProbe,
    IdleReport, IdleSource, InstalledExtensions, IssueOutcome, JobList, JobOutcome, JobState,
    JobSummary, Linkage, Made, MemoryMeasure, MemoryWatchdog, MetricsFrame, MetricsHistory,
    NetworkReach, Outcome, PROTOCOL_VERSION, PackageCatalogue, PackageList, PackageRemoval,
    PackageVersion, PathReport, PinSource, PlanAction, PlanStep, PoolOutcome, Priority,
    ProjectDetail, ProjectExport, ProjectList, ProjectRemoval, RecipeAddition, RepairReport,
    ResolvedRuntime, RotateOutcome, RuntimeCatalogue, RuntimeList, RuntimeRemoval, RuntimeSource,
    RuntimeSummary, ServiceCreation, ServiceId, ServiceLimitsReport, ServiceList, ServiceRemoval,
    ServiceState, ServiceSummary, ServiceWalk, SignatureCheck, SiteDetail, SiteKind, SiteList,
    SiteOwner, SiteRemoval, SiteSharing, StateReason, StepResult, Timestamp, Trust,
    UninstallOutcome, Unusable, Uptime, Verdict, WhenExceeded, privileged::ElevationOutcome,
};

/// `mix cert ca-status`, for a person.
///
/// **The trust line prints what the daemon said and never a word more.** T48 left it off entirely,
/// because there was no such fact in the answer and a line implying one would be the client
/// inventing something — `CLAUDE.md`'s "a client only renders what the daemon returns". T49a put the
/// fact in the answer, including the case where the daemon could not find out, which prints as
/// exactly that rather than as "no".
///
/// **And it says which store**, because "trusted" and "in this machine's own store, for every
/// account on it" are different claims and only the second is what happened.
/// `mix cert issue`, for a person — roadmap task **T50**.
///
/// **A line per site and never a count.** "3 certificates issued" is a number nobody can act on;
/// the domain is what somebody opens in a browser, and the reason a refusal gives is the only part
/// of this output that ever needs doing something about.
pub(crate) fn cert_issue(report: &CertIssueReport) -> String {
    if report.sites.is_empty() {
        return "  no site in this home declares HTTPS\n".to_owned();
    }

    report
        .sites
        .iter()
        .map(|site| match (&site.outcome, &site.state) {
            (IssueOutcome::Issued {}, CertState::Present { cert }) => format!(
                "  {}  issued — {} days, {} name(s)\n",
                site.domain,
                cert.days_left,
                cert.sans.len()
            ),
            (IssueOutcome::Reused {}, CertState::Present { cert }) => format!(
                "  {}  unchanged — {} days left\n",
                site.domain, cert.days_left
            ),
            // Roadmap task **T52**: a site that declares no HTTPS asked for nothing, and printing
            // it as "not issued" reads as a fault where there is none.
            (IssueOutcome::NotWanted { because }, _) => {
                format!("  {}  nothing to do — {because}\n", site.domain)
            }
            (IssueOutcome::Refused { because }, _) => {
                format!("  {}  not issued — {because}\n", site.domain)
            }
            // A written or reused certificate that does not read back is a state nothing should
            // produce, and printing it as success would hide exactly the case worth seeing.
            (_, state) => format!("  {}  unclear — {state:?}\n", site.domain),
        })
        .collect()
}

/// `mix cert status`, for a person — roadmap task **T53**.
///
/// **The command to run is written here and not by the daemon.** The answer carries a
/// [`CertProblem`], which is a name for a condition; turning that into `mix cert issue --site …` is
/// this client's job, because a graphical client renders a button for the same condition and would
/// have no use for a sentence telling its user to open a terminal.
///
/// **Two lines per site and not one**, unlike [`cert_issue`]: what is on the wire and what is on
/// the disk are two facts, and the whole point of this command is the case where they disagree.
pub(crate) fn cert_status(report: &CertStatusReport) -> String {
    if report.sites.is_empty() {
        return "  no site in this home
"
        .to_owned();
    }

    report
        .sites
        .iter()
        .map(|site| {
            let mut lines = format!(
                "  {}
",
                site.domain
            );

            lines.push_str(&match &site.handshake {
                Handshake::NotAsked {} => "    served over HTTP only
"
                .to_owned(),
                Handshake::NotServed { because } => {
                    format!(
                        "    not served over TLS — {because}
"
                    )
                }
                Handshake::Failed { because } => format!(
                    "    the handshake failed — {because}
"
                ),
                Handshake::Presented { cert, trust } => format!(
                    "    presented {} — {} days, {} name(s), {}
",
                    short(&cert.fingerprint),
                    cert.days_left,
                    cert.sans.len(),
                    match trust {
                        Verdict::Trusted {} => "trusted by this home's authority".to_owned(),
                        Verdict::Rejected { because } => format!("not trusted — {because}"),
                    }
                ),
            });

            if let Some(problem) = site.problem {
                lines.push_str(&format!(
                    "    {}
",
                    advice(&site.domain, problem)
                ));
            }

            lines
        })
        .collect()
}

/// The first sixteen characters of a fingerprint, which is what a person compares by eye.
///
/// The whole hash is in `--json` for anything that compares by machine.
fn short(fingerprint: &str) -> &str {
    &fingerprint[..fingerprint.len().min(16)]
}

/// What to do about a condition, in this client's own words.
fn advice(domain: &str, problem: CertProblem) -> String {
    match problem {
        CertProblem::NoCertificate | CertProblem::NamesDiffer | CertProblem::Expiring => {
            format!("run `mix cert issue --site {domain}`")
        }
        CertProblem::NotServed => {
            "start this home's front end — `mix service list` says which it is".to_owned()
        }
        CertProblem::ServedCertificateDiffers => {
            "the running server is holding an older certificate — restart this home's front end"
                .to_owned()
        }
        CertProblem::NotTrusted => {
            "this was not signed by this home's authority — `mix cert ca-status` says which              authority this home has"
                .to_owned()
        }
        // `CertProblem` is `#[non_exhaustive]`: a variant added by a newer daemon reaches an older
        // `mix`, and printing nothing at all would be worse than saying there is something to look
        // at.
        _ => "run `mix doctor`".to_owned(),
    }
}

pub(crate) fn ca_status(status: &CaStatus) -> String {
    let mut rendered = certificate(&status.state);

    rendered.push_str(&match &status.trust {
        Trust::Installed { store } => format!(
            "  trusted    yes — in {store}
"
        ),
        Trust::NotInstalled { because } => format!(
            "  trusted    no — {because}
"
        ),
        Trust::NoStore { because } => format!(
            "  trusted    n/a — {because}
"
        ),
        Trust::Unknown { because } => format!(
            "  trusted    unknown — {because}
"
        ),
    });

    // **A line per database, and never a summary count.** "2 of 3" is a number nobody can act on;
    // the path is what a person opens and the owner is what tells them which browser to restart.
    rendered.push_str(&match &status.browsers {
        Browsers::Reached { databases } if databases.is_empty() => {
            "  browsers   none found — Firefox and Chrome keep certificate databases of their own,              and this machine has none
"
            .to_owned()
        }
        Browsers::Reached { databases } => databases.iter().map(browser).collect::<String>(),
        Browsers::NoTool { because } => format!(
            "  browsers   not asked — {because}
"
        ),
        Browsers::NotSearched { because } => format!(
            "  browsers   n/a — {because}
"
        ),
        Browsers::Unknown { because } => format!(
            "  browsers   unknown — {because}
"
        ),
    });

    rendered
}

/// `mix cert ca-rotate` — roadmap task **T54**.
///
/// **What is left comes from the status, because that is the measurement**; the outcome supplies the
/// reason a measurement cannot give.
pub(crate) fn ca_rotate(report: &CaRotateReport) -> String {
    let mut rendered = match &report.outcome {
        RotateOutcome::Rotated {} => {
            let mut said = format!(
                "this home has a new certificate authority\n{} site certificate(s) were reissued under it\n",
                report.sites.len()
            );

            // **The one thing a rotation can leave behind and not otherwise mention.** A previous
            // authority that could not be read cannot be named for removal — T49a's D5 forbids
            // guessing — so the old certificate is still in the store, and saying nothing here
            // would read as a clean rotation.
            if report.previous.is_none() {
                said.push_str(
                    "the previous certificate was left in this machine's trust store: it could not\nbe read, and nothing is removed that cannot be named\n",
                );
            }

            said
        }

        RotateOutcome::NotCommitted { because } => format!(
            "nothing was changed: {because}\n\nrun `mix cert ca-status` to see what this machine holds now\n"
        ),

        RotateOutcome::NothingToRotate { because } => {
            format!("{because}\n\nrun `mix doctor --repair` to make one\n")
        }

        // `RotateOutcome` is `#[non_exhaustive]`: a variant a newer daemon knows reaches an older
        // `mix`, and printing nothing would be worse than saying the question was asked.
        _ => "this home's certificate authority was asked about\n".to_owned(),
    };

    rendered.push('\n');
    rendered.push_str(&ca_status(&report.status));
    rendered
}

/// `mix cert ca-uninstall` — roadmap task **T54**.
///
/// **What is left comes from the status, because that is the measurement**; the outcome supplies the
/// reason a measurement cannot give. So the two halves cannot disagree: there is only one reading.
pub(crate) fn ca_uninstall(report: &CaUninstallReport) -> String {
    let mut rendered = match &report.outcome {
        UninstallOutcome::Removed {} => {
            "this home's certificate authority was taken out of every store that held it
the certificate and its key are still on disk — `mix doctor --repair` puts the trust back
"
            .to_owned()
        }
        UninstallOutcome::PartlyRemoved { because } => format!(
            "some of it is still there: {because}

run `mix elevation grant` if a prompt was refused, or `mix cert ca-status` to look again
"
        ),
        UninstallOutcome::NothingToRemove { because } => format!("{because}\n"),
        // `UninstallOutcome` is `#[non_exhaustive]`: a variant a newer daemon knows reaches an older
        // `mix`, and printing nothing would be worse than saying the question was asked.
        _ => "this home's certificate authority was asked about\n".to_owned(),
    };

    rendered.push('\n');
    rendered.push_str(&ca_status(&report.status));
    rendered
}

/// One database's line.
fn browser(database: &BrowserDatabase) -> String {
    let verdict = if database.installed {
        "yes".to_owned()
    } else {
        match &database.because {
            Some(because) => format!("no — {because}"),
            None => "no".to_owned(),
        }
    };

    format!(
        "  browsers   {verdict} — {} ({})
",
        database.path, database.owner
    )
}

/// The authority itself, which is the half T48 built.
fn certificate(state: &CaState) -> String {
    match state {
        // Reachable, and worth a sentence rather than an empty screen: a start whose generation
        // failed warns into the daemon's log and carries on, so this is what the next question gets.
        CaState::Absent {} => "  authority  none — one is made when the daemon starts
"
        .to_owned(),

        CaState::Unusable { because } => {
            format!(
                "  authority  unusable — {}
",
                unusable(*because)
            )
        }

        CaState::Present { ca } => {
            let mut rendered = format!(
                "  authority  {}
",
                ca.subject
            );
            rendered.push_str(&format!(
                "  sha256     {}
",
                ca.fingerprint
            ));

            // Negative rather than clamped: an expired authority is a true state, and a screen that
            // said "in -3 days" — or silently "in 0 days" — would be hiding the one thing worth
            // acting on.
            rendered.push_str(&if ca.days_left < 0 {
                format!(
                    "  expired    {} days ago
",
                    ca.days_left.abs()
                )
            } else {
                format!(
                    "  expires    in {} days
",
                    ca.days_left
                )
            });

            rendered
        }
    }
}

/// Each way of being unusable, in a sentence. **Not what to do about it**: the reason is a fact
/// about this home, and the remedy is `mix cert ca-rotate`, which T54 builds.
fn unusable(because: Unusable) -> &'static str {
    match because {
        Unusable::KeyMissing => "the certificate is here and its private key is not",
        Unusable::CertificateMissing => "the private key is here and the certificate is not",
        Unusable::KeyUnreadable => "the private key is not one this build can read",
        Unusable::CertificateUnreadable => "the certificate is not a certificate",
        Unusable::KeyAndCertificateDisagree => {
            "the certificate and the private key are not each other's"
        }
    }
}

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

    // **The names line, and it is one line whichever mechanism is running** — roadmap task T44.
    // The mode alone is not the sentence somebody needs: what a hosts-only home loses is wildcards,
    // and what it wants to know is why, so both travel with it.
    rendered.push_str(&match status.dns.mode {
        // The TLDs are named rather than counted, because from T45 on a home can have wildcards for
        // some of its names and not others — `.local` is never routed — and "wildcards work" would
        // be true and useless to somebody whose `.local` site had just stopped resolving.
        DnsMode::Dns => format!(
            "  names     DNS on {} — wildcards for {}\n",
            status.dns.listening.as_deref().unwrap_or("loopback"),
            patterns(&status.dns.wildcards)
        ),
        DnsMode::HostsOnly => format!(
            "  names     hosts file — no wildcards{}\n",
            status
                .dns
                .because
                .as_deref()
                .map(|because| format!(" ({because})"))
                .unwrap_or_default()
        ),
    });

    if status.elevation.elevated {
        rendered.push_str(
            "  note      this daemon holds an administrative token — every service it supervises \
             inherits it\n",
        );
    }

    // Degraded is this number and nothing else — there is no flag on the wire and none here.
    if status.elevation.pending > 0 {
        rendered.push_str(&format!(
            "  waiting   {} for permission — `mix elevation status` says what they are\n",
            operations(status.elevation.pending)
        ));
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

/// `mix elevation status`, for a person.
///
/// **The list is the point, and it comes before any offer to raise a prompt.** T64 is what turns
/// this into the screen that explains every operation and what it will literally change *before*
/// somebody is asked to allow it; what is here already prints the operations' own descriptions,
/// because the daemon renders them and a client that composed its own would be composing the
/// sentence a person judges the change by.
pub(crate) fn elevation_status(status: &ElevationStatus) -> String {
    let mut rendered = waiting(status);

    if let Some(last) = &status.last {
        rendered.push_str(&format!("  last      {}\n", grant(last)));
    }

    if status.elevated {
        rendered.push_str(
            "  note      this daemon holds an administrative token; every service it supervises \
             inherits it\n",
        );
    }

    match (&status.reason, status.pending.is_empty()) {
        // The reason is the answer, and on Linux it is a command to type. Printed whether or not
        // anything is waiting: a machine that cannot elevate is worth knowing about before the first
        // site is created rather than after.
        (Some(reason), _) => rendered.push_str(&format!("  cannot    {reason}\n")),

        (None, false) => rendered.push_str(
            "\n`mix elevation grant` asks once for all of them; `mix elevation drop` forgets one\n",
        ),

        (None, true) => {}
    }

    rendered
}

/// Everything that is waiting, and what each one will change: the part of the screen that is the
/// same whether it is being reported or being asked about.
///
/// The description is the operation's own — `PrivilegedOp::describe`, rendered by the daemon into
/// `PendingOp::description`. A client that composed its own sentence here would be composing the
/// one a person judges the change by, and it would be the sentence most able to disagree with what
/// is actually applied.
fn waiting(status: &ElevationStatus) -> String {
    let mut rendered = match status.pending.len() {
        0 => "nothing is waiting for permission\n".to_owned(),
        waiting => format!("{} for permission\n", operations(waiting)),
    };

    for pending in &status.pending {
        rendered.push_str(&format!(
            "  {:<4} {} — {}\n",
            pending.id,
            pending.op.name(),
            pending.description
        ));
    }

    rendered
}

/// The screen `mix elevation grant` shows **before** it raises anything — roadmap task **T64**.
///
/// The same list [`elevation_status`] prints, and then the one thing that screen cannot say: a
/// prompt is about to appear, it will appear once, and it will name this program. What is
/// deliberately absent is the advice [`elevation_status`] ends with — a person reading this is
/// already running `mix elevation grant`.
pub(crate) fn elevation_prompt(status: &ElevationStatus) -> String {
    let helper = status.helper.as_deref().unwrap_or("mixengine-elevate");

    format!(
        "{}\nyour operating system will ask once, for all of them, to allow\n  {helper}\n",
        waiting(status)
    )
}

/// What one grant did, in a line.
fn grant(outcome: &GrantOutcome) -> String {
    let what = match &outcome.outcome {
        // A choice and not a failure — ADR 0005. The word carries that, and nothing here adds to it.
        ElevationOutcome::Declined => "declined".to_owned(),
        ElevationOutcome::Unavailable { reason } => format!("could not be raised — {reason}"),
        ElevationOutcome::Completed => format!(
            "{} applied, {} still waiting",
            outcome.applied, outcome.still_pending
        ),
    };

    format!("job {} — {what}", outcome.job)
}

/// "1 operation is" / "3 operations are", so a sentence built from a count reads.
fn operations(count: usize) -> String {
    match count {
        1 => "1 operation is waiting".to_owned(),
        many => format!("{many} operations are waiting"),
    }
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

            if let Ok(outcome) = serde_json::from_value::<GrantOutcome>(result.clone()) {
                rendered.push_str(&format!("  {}\n", grant(&outcome)));
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

/// `mix metrics` — what everything is costing right now.
///
/// **A dash where a CPU figure could not be taken, and never a zero.** A group measured for the
/// first time has no difference to report yet, and printing `0.0%` there would say a service is
/// idling in the second it is most expensive.
pub(crate) fn metrics_frame(frame: &MetricsFrame) -> String {
    if frame.samples.is_empty() {
        return "nothing could be measured
"
        .to_owned();
    }

    let rows: Vec<[String; 4]> = frame
        .samples
        .iter()
        .map(|sample| {
            [
                sample.subject.to_string(),
                percent(sample.cpu_percent),
                memory(sample.rss_bytes),
                sample.processes.to_string(),
            ]
        })
        .collect();

    table(["SUBJECT", "CPU", "MEMORY", "PROCESSES"], &rows)
}

/// `mix metrics --since` — the per-minute history, oldest first.
///
/// **`SAMPLES` is on the table rather than only in `--json`.** A minute made of one reading and one
/// made of sixty are both averages, and a person comparing two rows has to be able to see which is
/// which. A row is only ever missing because nothing was measured that minute — the service was
/// stopped, or the machine was asleep — so the gaps are part of the answer.
///
/// **The minute is printed as an age rather than as a clock time**, which is [`ago`]'s rule and this
/// workspace's: turning epoch milliseconds into 14:03 needs a civil calendar, and nothing here has
/// one. `--json` carries the millisecond, which is what a chart wants anyway.
pub(crate) fn metrics_history(history: &MetricsHistory, now: SystemTime) -> String {
    if history.minutes.is_empty() {
        return format!(
            "no readings in that window — this home keeps {} hours of them
",
            history.retention_hours
        );
    }

    let rows: Vec<[String; 6]> = history
        .minutes
        .iter()
        .map(|minute| {
            [
                ago(minute.minute, now),
                minute.subject.to_string(),
                percent(minute.cpu_avg),
                percent(minute.cpu_peak),
                memory(minute.rss_peak),
                minute.samples.to_string(),
            ]
        })
        .collect();

    table(
        [
            "MINUTE",
            "SUBJECT",
            "CPU AVG",
            "CPU PEAK",
            "MEMORY PEAK",
            "SAMPLES",
        ],
        &rows,
    )
}

/// A percentage of one core, or a dash where no figure was taken.
fn percent(cpu: Option<f32>) -> String {
    cpu.map_or_else(|| MISSING.to_owned(), |cpu| format!("{cpu:.1}%"))
}

/// Resident bytes, at the scale a person reads memory in.
///
/// Mebibytes with one decimal, unlike [`size`]: a download is tens or hundreds of them and a service
/// is often under ten, where whole numbers would round php-fpm and Redis to the same figure.
fn memory(bytes: u64) -> String {
    #[expect(clippy::cast_precision_loss, reason = "one decimal place of mebibytes")]
    let mib = bytes as f64 / (1u64 << 20) as f64;

    format!("{mib:.1} MiB")
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

/// How long until something happens, from this machine's clock — roadmap task **T76**.
///
/// [`ago`]'s mirror, sharing [`units`] with it so that "in 1h 58m" and "1h 58m ago" round the same
/// way. A moment already gone reads as `any moment now` rather than as a negative wait: the loop
/// that ends a share runs on a period, so a deadline can be a few seconds past while the share is
/// still up, and that is a wait rather than a fault.
fn in_time(Timestamp(happens): Timestamp, now: SystemTime) -> String {
    let Timestamp(now) = Timestamp::from_system_time(now);

    match u64::try_from(happens.saturating_sub(now) / 1_000) {
        Ok(0) | Err(_) => "any moment now".to_owned(),
        Ok(seconds) => format!("in {}", units(seconds)),
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

/// `mix project list` — every registered project, and whether it has a manifest.
pub(crate) fn project_list(list: &ProjectList) -> String {
    if list.projects.is_empty() {
        return "no projects are registered — `mix project create <dir>` adds one\n".to_owned();
    }

    let mut out = format!("{:<24}  {:<9}  {}\n", "PROJECT", "MANIFEST", "ROOT");

    for project in &list.projects {
        out.push_str(&format!(
            "{:<24}  {:<9}  {}\n",
            project.name,
            if project.manifest.is_some() {
                "yes"
            } else {
                "—"
            },
            project.root
        ));
    }

    out
}

/// `mix project show` — one project, and what each pin actually resolves to.
///
/// The **source** column is the whole value of the rendering: a pin read from the manifest outranks
/// the row, so a person looking at a version they did not expect is looking for which of the two is
/// in charge.
pub(crate) fn project_detail(detail: &ProjectDetail) -> String {
    let mut out = format!(
        "{}\n  root      {}\n  created   {}\n",
        detail.project.name, detail.project.root, detail.project.created_at
    );

    if let Some(manifest) = &detail.project.manifest {
        out.push_str(&format!("  manifest  {manifest}\n"));
    }

    if detail.pins.is_empty() {
        out.push_str("\nno runtimes are pinned\n");
        return out;
    }

    out.push_str(&format!(
        "\n{:<8}  {:<10}  {:<10}  {}\n",
        "RUNTIME", "PINNED", "RESOLVES", "FROM"
    ));

    for pin in &detail.pins {
        let from = match &pin.source {
            PinSource::Registered => "this home".to_owned(),
            PinSource::Manifest { path } => path.clone(),
        };

        out.push_str(&format!(
            "{:<8}  {:<10}  {:<10}  {}\n",
            pin.kind.as_str(),
            pin.constraint.as_str(),
            pin.resolved.as_ref().map_or("—", PackageVersion::as_str),
            from
        ));
    }

    for hint in detail.pins.iter().filter_map(|pin| pin.hint.as_ref()) {
        out.push_str(&format!("\n{hint}\n"));
    }

    out
}

/// `mix project delete` — and the directory it did not touch.
pub(crate) fn project_removal(removal: &ProjectRemoval) -> String {
    let mut out = format!("{} is no longer registered\n", removal.removed.name);
    out.push_str(&format!("  the directory is kept: {}\n", removal.root_kept));

    if let Some(manifest) = &removal.manifest_kept {
        out.push_str(&format!("  so is its manifest:   {manifest}\n"));
    }

    out
}

/// `mix project export` — which file, and whether it had to be made.
pub(crate) fn project_export(exported: &ProjectExport) -> String {
    match exported.created {
        true => format!("wrote {}\n", exported.path),
        false => format!(
            "updated {} — everything else in it is untouched\n",
            exported.path
        ),
    }
}

/// `mix site list` — every site, and what serves it.
pub(crate) fn site_list(list: &SiteList) -> String {
    if list.sites.is_empty() {
        return "no sites are declared — `mix site create` adds one\n".to_owned();
    }

    let mut out = format!(
        "{:<28}  {:<14}  {:<9}  {}\n",
        "DOMAIN", "KIND", "STATE", "OWNER"
    );

    for site in &list.sites {
        out.push_str(&format!(
            "{:<28}  {:<14}  {:<9}  {}\n",
            site.domain,
            kind_word(&site.kind),
            site.state.as_str(),
            owner_word(&site.owner)
        ));
    }

    out
}

/// Who a site belongs to, as a column reads it — roadmap task **T81b**.
fn owner_word(owner: &SiteOwner) -> String {
    match owner {
        SiteOwner::Project { name } => name.clone(),
        SiteOwner::Extension { id } => format!("extension {id}"),
    }
}

/// The word a person typed for a kind, which is the word the wire uses.
fn kind_word(kind: &SiteKind) -> &'static str {
    match kind {
        SiteKind::PhpFpm { .. } => "php-fpm",
        SiteKind::Static => "static",
        SiteKind::ReverseProxy { .. } => "reverse-proxy",
        SiteKind::NodeApp { .. } => "node-app",
    }
}

/// `daemon.doctor`, as a person reads it — roadmap task **T47a**.
///
/// **Every check gets a line, including the ones that found nothing.** A doctor that printed only
/// faults would leave a person unsure it looked, which is the whole reason the report carries what
/// was examined rather than only what was wrong.
///
/// The word in the margin is the outcome and the indented line under it is the daemon's own
/// sentence — this client writes none of its own, on the standing rule that a client renders what
/// the daemon returns.
pub(crate) fn doctor(report: &DoctorReport) -> String {
    let mut out = String::new();

    for check in &report.checks {
        let (mark, because) = match &check.outcome {
            Outcome::Ok {} => ("ok     ", None),
            Outcome::Note { because } => ("note   ", Some(because)),
            Outcome::Problem { because, .. } => ("PROBLEM", Some(because)),
            Outcome::Skipped { because } => ("skipped", Some(because)),
        };

        out.push_str(&format!("{mark}  {}\n", check.name));

        if let Some(because) = because {
            out.push_str(&format!("         {because}\n"));
        }
    }

    out
}

/// `daemon.doctor_repair`, as a person reads it — roadmap task **T47b**.
///
/// **The same three margins as [`doctor`]**, so the two read as one tool: what was done, what is
/// waiting, and what nothing could be done about. A `PROBLEM` here means the same thing it means
/// there — something is wrong and this build cannot fix it.
///
/// A repair that found nothing prints a sentence rather than nothing at all, for `doctor`'s reason
/// one document along: silence cannot be told apart from a command that did not run.
pub(crate) fn repair(report: &RepairReport) -> String {
    if report.actions.is_empty() {
        return "nothing to repair\n".to_owned();
    }

    let mut out = String::new();

    for action in &report.actions {
        let (mark, sentence) = match &action.outcome {
            Action::Repaired { what } => ("repaired", what),
            Action::Enqueued { what } => ("waiting ", what),
            Action::Untouched { because } => ("PROBLEM ", because),
        };

        out.push_str(&format!("{mark}  {}\n", action.name));
        out.push_str(&format!("          {sentence}\n"));
    }

    out
}

/// `daemon.bundle`, as a person reads it — roadmap task **T93**.
///
/// **The omissions are printed and not summarised.** They are the half a person will not otherwise
/// know to ask about, and a bundle whose gaps live only in a JSON field is a bundle whose gaps get
/// discovered by whoever opens it three days later, looking for the file that is not there.
pub(crate) fn bundle(report: &BundleReport, copied_to: Option<&std::path::Path>) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "wrote  {}
",
        report.path
    ));
    out.push_str(&format!(
        "       {}, {} file(s)
",
        size(report.bytes),
        report.members.len()
    ));

    if let Some(destination) = copied_to {
        out.push_str(&format!(
            "copied {}
",
            destination.display()
        ));
    }

    if !report.omitted.is_empty() {
        out.push_str(
            "
not included
",
        );
        for left in &report.omitted {
            out.push_str(&format!(
                "       {} — {}
",
                left.name, left.because
            ));
        }
    }

    out
}

/// `domain.dns_status`, as a person reads it — roadmap task **T46**.
///
/// **One column per fact, because the four fail independently.** A single "works / does not" column
/// would be exactly the derivation the report exists to prevent; the sentence under a failing row is
/// the thing a person acts on, and it is the daemon's sentence rather than this client's.
pub(crate) fn domain_status(report: &DomainStatusReport) -> String {
    if report.domains.is_empty() {
        return "no domains declared
"
        .to_owned();
    }

    let width = report
        .domains
        .iter()
        .map(|row| row.domain.len())
        .max()
        .unwrap_or_default();

    let mut out = String::new();

    for row in &report.domains {
        let resolved = if row.resolves_to.is_empty() {
            "does not resolve".to_owned()
        } else {
            row.resolves_to
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };

        out.push_str(&format!(
            "{:width$}  {}  {}  {}  {}
",
            row.domain,
            if row.site.is_some() {
                "declared"
            } else {
                "unknown "
            },
            if row.hosts_entry { "hosts" } else { "     " },
            if row.wildcard { "wildcard" } else { "        " },
            resolved,
        ));

        if let Some(because) = &row.because {
            out.push_str(&format!(
                "{:width$}  {because}
",
                ""
            ));
        }
    }

    out
}

/// `mix site show` — one site, and the two answers about its pool.
///
/// The **pool** lines are the whole value of the rendering: a site's pool is frozen at create while
/// the shell in the same directory keeps following the default, so somebody looking at a PHP
/// version they did not expect is looking for which of the two is in charge.
pub(crate) fn site_detail(detail: &SiteDetail) -> String {
    let mut out = format!(
        "{}\n  owner     {}\n  kind      {}\n  root      {}\n  doc root  {}{}\n  https     {}\n  \
         state     {}\n",
        detail.site.domain,
        owner_word(&detail.site.owner),
        kind_word(&detail.site.kind),
        detail.root,
        detail.doc_root_full,
        if detail.doc_root_exists {
            ""
        } else {
            "  (not there yet)"
        },
        detail.site.https,
        detail.site.state.as_str()
    );

    match &detail.site.kind {
        SiteKind::ReverseProxy { upstream } => {
            out.push_str(&format!("  upstream  {upstream}\n"));
        }
        SiteKind::NodeApp { port } => out.push_str(&format!("  port      {port}\n")),
        SiteKind::PhpFpm { .. } | SiteKind::Static => {}
    }

    if let Some(pool) = &detail.pool {
        out.push_str(&format!(
            "  pool      {}\n",
            pool.declared
                .as_ref()
                .map_or("— the service it named is gone", ServiceId::as_str)
        ));

        if pool.declared != pool.resolved {
            out.push_str(&format!(
                "  resolves  {} — this directory resolves to a different PHP than the site was \
                 declared with\n",
                pool.resolved.as_ref().map_or("—", ServiceId::as_str)
            ));
        }
    }

    if let Some(sharing) = &detail.site.sharing {
        out.push_str(&format!(
            "  shared    {} on {}\n",
            sharing.url, sharing.interface
        ));

        if let Some(until) = sharing.until {
            out.push_str(&format!(
                "  ends      {}\n",
                in_time(until, SystemTime::now())
            ));
        }
    }

    if detail.domains.len() > 1 {
        out.push_str(&format!("\naliases: {}\n", detail.domains[1..].join(", ")));
    }

    if !detail.services.is_empty() {
        out.push_str(&format!("\n{:<24}  {}\n", "SERVICE", "STATE"));

        for link in &detail.services {
            out.push_str(&format!(
                "{:<24}  {}\n",
                link.service.as_str(),
                link.state.as_str()
            ));
        }
    }

    out
}

/// `mix site delete` — what was freed, and what was not touched.
pub(crate) fn site_removal(removal: &SiteRemoval) -> String {
    let mut out = format!("{} is no longer declared\n", removal.removed.domain);

    out.push_str(&format!(
        "  the files are kept:    {}\n",
        removal.doc_root_kept
    ));
    out.push_str(&format!(
        "  free for another site: {}\n",
        removal.domains_released.join(", ")
    ));

    out
}

/// When a service is stopped for being unused, and what is holding it open right now.
///
/// **Four answers, and never fewer.** A service that stays running does so for one of four reasons
/// that look identical from outside — nothing idles it, somebody switched idling off for it,
/// something running depends on it, or a project is being kept warm. Two of those are settings and
/// two are not, so a rendering that showed only the policy would send half of the people who read it
/// to change something that was never the cause. This is `mix domain status`' rule from T46, applied
/// to a smaller question.
pub(crate) fn service_idle(report: &IdleReport) -> String {
    let mut rendered = format!("{}\n", report.service);

    let policy = match &report.policy {
        Some(policy) => format!("after {}", policy.after),
        None => "never".to_owned(),
    };

    let source = match report.source {
        IdleSource::Row => "set for this service",
        IdleSource::Never => "switched off for this service",
        IdleSource::Recipe => "the default for this kind of service",
        // The state of every service in this build, and it is worth spelling out rather than
        // leaving as a blank: nothing is wrong, the feature simply has no default yet.
        IdleSource::Unset => "no default yet — nothing idles this",
        // Asked for, and nothing to measure it with. The line above still says "never", which is
        // what happens; this says why, which is what a person can act on.
        IdleSource::Unmeasurable => "asked for, but this service has nothing to measure",
        // A newer daemon distinguishing something this build does not. The policy line above is
        // still true, so what is lost is the provenance and not the answer.
        _ => "for a reason this version of `mix` does not know",
    };

    rendered.push_str(&idle_line("idle stop", &policy, source));

    if let Some(policy) = &report.policy {
        rendered.push_str(&idle_line("measured by", &probe(&policy.probe), ""));
    }

    for exemption in &report.exempt {
        let held = match exemption {
            IdleExemption::DependentRunning { service } => {
                format!("{service} is running and depends on it")
            }
            IdleExemption::ProjectKeptWarm { project } => {
                format!("the project {project} is being kept warm")
            }
            // A newer daemon knows a reason this build does not. Named as one rather than dropped:
            // what the reader needs is that *something* holds it open, and a blank line would say
            // the opposite.
            _ => "something this version of `mix` does not know about".to_owned(),
        };

        rendered.push_str(&idle_line("held open by", &held, ""));
    }

    rendered
}

/// How a probe is described to somebody who did not choose it.
///
/// A probe comes from the recipe rather than from the person reading this, so it is written as what
/// is being watched and not as the variant's name.
fn probe(probe: &IdleProbe) -> String {
    match probe {
        IdleProbe::Connections { port } => format!("connections to port {port}"),
        IdleProbe::HttpCounter { url, field } => format!("`{field}` at {url}"),
        IdleProbe::FastCgiStatus { socket, path } => {
            format!("`{path}` at {}", socket.display())
        }
        _ => "something this version of `mix` does not know about".to_owned(),
    }
}

/// One line of [`service_idle`], laid out as [`limit_line`] lays its own out.
fn idle_line(field: &str, value: &str, note: &str) -> String {
    format!("  {field:<13} {value:<24} {note}\n")
        .trim_end()
        .to_owned()
        + "\n"
}

/// What a service may take, and what this machine will actually do about each of it.
///
/// **The number and the verdict on one line, always.** A ceiling of 512 MB means one thing where it
/// is a commit charge enforced by a failed allocation and another where it is charged pages enforced
/// by the OOM killer — and a third where it is stored and enforced by nothing at all. Printing the
/// number alone would be telling a third of the truth.
///
/// **And every field, always, including the ones that are unset.** This is where `service.set_limits`
/// pays for taking the whole value rather than a patch: `mix service limits web set --cpu 50` clears
/// a memory ceiling that was there, and the only thing that keeps that from being a surprise is that
/// the cleared field is on the screen a line below the one that was set.
pub(crate) fn service_limits(report: &ServiceLimitsReport) -> String {
    let mut rendered = format!(
        "{}
",
        report.service
    );

    rendered.push_str(&limit_line(
        "cpu",
        &report.limits.cpu_percent.map_or_else(
            || "uncapped".to_owned(),
            |percent| format!("{percent}% of one core"),
        ),
        &enforcement(
            &report.support.cpu,
            report.support.memory_measure,
            false,
            report.limits.cpu_percent.is_some(),
        ),
    ));

    rendered.push_str(&limit_line(
        "memory",
        &report
            .limits
            .memory_mb
            .map_or_else(|| "uncapped".to_owned(), |mb| format!("{mb} MB")),
        &enforcement(
            &report.support.memory,
            report.support.memory_measure,
            true,
            report.limits.memory_mb.is_some(),
        ),
    ));

    rendered.push_str(&watchdog_line(report.watchdog));

    rendered.push_str(&limit_line(
        "priority",
        match report.limits.priority {
            Priority::Normal => "normal",
            Priority::Background => "background",
        },
        match report.support.priority {
            true => "enforced",
            false => "not enforced here",
        },
    ));

    rendered.push_str(&format!(
        "
cpu is a percentage of one core; this machine has {} of them
",
        report.support.cores
    ));

    rendered
}

/// The line about what is watching a ceiling this machine cannot hold — task **T71a**.
///
/// **Both numbers and the ending, or nothing at all.** A client that printed only the restart would
/// say nothing about the services most worth saying something about: a database over its ceiling is
/// warned about and deliberately left alone, and a person who saw no line would think nothing was
/// watching. Empty for [`None`], which is a machine that enforces the ceiling itself or a service
/// that declared none — in both cases there is no loop to describe.
fn watchdog_line(watchdog: Option<MemoryWatchdog>) -> String {
    let Some(watchdog) = watchdog else {
        return String::new();
    };

    let minutes = watchdog.after_minutes;

    let ending = if watchdog.restarts {
        format!("restarted after {minutes} minutes over it")
    } else {
        "warned about; this service is not restarted automatically".to_owned()
    };

    format!("  {:<9} {:<18} {ending}\n", "watchdog", "checked a minute")
}

// One field: what was asked for, and what happens to it here.
fn limit_line(field: &str, asked: &str, verdict: &str) -> String {
    format!("  {field:<9} {asked:<18} {verdict}\n")
}

/// What this machine does with one field, in words rather than in an enum's name.
///
/// **The tense depends on whether a ceiling is actually set**, and getting that wrong is a real way
/// to mislead: a field nobody has capped that reads *"enforced — at the ceiling, the service is
/// killed"* names a ceiling that does not exist. So an uncapped field is written conditionally, which
/// also makes this line useful *before* somebody sets one — it is where they find out what the
/// number would mean here.
///
/// `measured` is only true for the memory line: it is what the *number* counts, and a CPU percentage
/// counts the same thing everywhere.
fn enforcement(
    enforcement: &Enforcement,
    measure: MemoryMeasure,
    measured: bool,
    capped: bool,
) -> String {
    match enforcement {
        Enforcement::Hard { when } => {
            let ending = match when {
                WhenExceeded::AllocationFails => "the next allocation fails",
                WhenExceeded::Killed => "the service is killed",
            };

            let counts = counted(measure, measured);

            match capped {
                true => format!("enforced —{counts} at the ceiling, {ending}"),
                false => format!("would be enforced —{counts} at a ceiling, {ending}"),
            }
        }

        // The permanent fact: this operating system has no such mechanism, and none is coming.
        Enforcement::Unsupported => match capped {
            true => "stored, not enforced — this system has no such limit".to_owned(),
            false => "this system has no such limit".to_owned(),
        },

        // The fixable one, in the platform's own words, because they were written for this line.
        Enforcement::Unavailable { why } => match capped {
            true => format!("stored, not enforced — {why}"),
            false => format!("could not be enforced — {why}"),
        },

        // **Watched rather than capped** — roadmap task T71a. Deliberately not the word "enforced":
        // the service may go over this number and keep running. What happens after it does is per
        // service rather than per machine, so it is the `watchdog` line below this one and not this
        // sentence. The `why` is carried where the platform gave one, which is a machine somebody
        // could start differently; macOS gives none and none is printed.
        Enforcement::Advisory { why } => {
            let counts = counted(measure, measured);

            let opening = match capped {
                true => format!("watched, not capped —{counts}"),
                false => format!("would be watched, not capped —{counts}"),
            };

            match why {
                Some(why) => format!("{opening} {why}"),
                None => format!("{opening} this system has no hard cap to give"),
            }
        }

        // A variant this build of the client has never heard of. The rest of the line is still true,
        // and saying so beats printing a word that was invented after this binary was compiled.
        _ => "this client does not know what this machine does with it".to_owned(),
    }
}

/// What the memory number counts here, as a clause to drop into a longer sentence.
///
/// Empty for every field but memory: a CPU percentage counts the same thing everywhere, and the
/// clause would be noise on the line that carries it.
fn counted(measure: MemoryMeasure, measured: bool) -> String {
    if !measured {
        return String::new();
    }

    match measure {
        MemoryMeasure::Commit => " counts committed memory;".to_owned(),
        MemoryMeasure::ChargedPages => " counts resident memory and page cache;".to_owned(),

        // Named as an overestimate on the line itself, because it is one: shared pages are counted
        // once per process, so a pool and its workers add up to more than they occupy.
        MemoryMeasure::Resident => {
            " counts resident memory, shared pages once per process;".to_owned()
        }

        _ => String::new(),
    }
}

/// What `mix site share` prints: where the site is, and a code a phone can point at.
///
/// **The QR is drawn here and the URL is answered by the daemon** - the T74 design, D10. A terminal
/// is one client's rendering of one string; a graphical client draws its own from the same string,
/// and the daemon knows about neither.
pub(crate) fn site_shared(sharing: &SiteSharing) -> String {
    let mut out = format!(
        "shared on the local network\n  url        {}\n  interface  {} ({})\n",
        sharing.url, sharing.interface, sharing.address
    );

    // **The name is printed, and whether anything answers for it is printed beside it** - roadmap
    // task T75. A name that resolves nowhere looks exactly like one that does until somebody types
    // it into a phone, so a home that could not bind UDP 5353 says so here rather than there.
    if let Some(name) = sharing.name.as_deref() {
        match sharing.advertised {
            true => out.push_str(&format!("  name       {name}\n")),
            false => out.push_str(&format!(
                "  name       {name} (not being advertised: this home could not answer mDNS)\n"
            )),
        }
    }

    out.push_str(&format!("  authority  {}\n", sharing.ca_url));

    // **Printed only when there is one** — roadmap task T76. A share with no deadline is the
    // ordinary case, and a line reading "ends  never" is a line every reader has to skip.
    if let Some(until) = sharing.until {
        out.push_str(&format!(
            "  ends       {}\n",
            in_time(until, SystemTime::now())
        ));
    }

    // **The QR carries the address and not the name** - the T75 design, D11. Android's resolver
    // does not answer `.local` for a browser, so a code carrying the name would be a broken URL for
    // a large share of the phones this feature exists for.
    if let Some(code) = qr(&sharing.url) {
        out.push('\n');
        out.push_str(&code);
    }

    out.push_str(
        "\nover http: a phone does not trust this home's certificate authority until it has \
         installed it. Open the authority URL on the device, then, on iOS, turn it on under \
         Settings > General > About > Certificate Trust Settings\n",
    );

    out
}

/// The URL as a QR code, in half-height blocks, or [`None`] where it will not encode.
///
/// **Never an error.** An address and a port is thirty characters at most, so nothing a home
/// produces comes close to the limit - but a code that would not fit is still no reason to fail a
/// share that already worked. The URL above it is the answer; the code is the convenience.
fn qr(url: &str) -> Option<String> {
    let code = qrcode::QrCode::new(url.as_bytes()).ok()?;

    Some(
        code.render::<qrcode::render::unicode::Dense1x2>()
            .quiet_zone(true)
            .build(),
    )
}

/// `mix database create` — what was made, and where the credential is.
///
/// **It says where the password lives and never what it is** — the T77a design, D11. Two lines,
/// aligned on the noun, in the words this file uses everywhere else rather than a glyph.
pub(crate) fn database_created(account: &DatabaseAccount) -> String {
    let word = |made: Made| match made {
        Made::Created => "created",
        Made::Existing => "already existed",
    };

    format!(
        "database {} {} on {}\naccount  {} {}, password in the keyring at {}",
        account.database,
        word(account.made.database),
        account.service,
        account.user,
        word(account.made.user),
        account.secret,
    )
}

/// `mix blueprint capture` — what was written down, and where to read it.
pub(crate) fn blueprint_captured(summary: &BlueprintSummary) -> String {
    format!(
        "captured {} from this project
  {}
",
        summary.slug, summary.file
    )
}

/// `mix blueprint import` — what was taken in, and whether anything vouched for it.
///
/// **The trust is said on the way in**, roadmap task **T78a**: it is decided here once and never
/// again, so this line is the only moment a person is told what they now have.
///
/// **And which kind of untrusted it is** — roadmap task **T79b**. A file that came with nothing and
/// a file whose signature did not verify are both untrusted and are not the same event: only the
/// second is what the gallery key exists to catch, and it used to arrive here as the first one's
/// sentence.
pub(crate) fn blueprint_imported(summary: &BlueprintSummary) -> String {
    let vouched = match (summary.trusted, summary.signature) {
        (true, _) => "signed by the gallery key",

        // True of all three things the verifier folds together — a manifest edited after it was
        // signed, a signature from another key, and a file that is not a signature at all. Saying
        // "the bytes changed" would accuse the second and third of the first.
        (false, Some(SignatureCheck::Rejected)) => {
            "untrusted: a signature came with it, and it is not the gallery's"
        }

        (false, Some(SignatureCheck::Missing)) => {
            "untrusted: nothing came with it to vouch for it, and nothing will"
        }

        // A row written before T79b, or one whose reason this build cannot read: the sentence this
        // line has always had, which says the true half of what is known.
        (false, _) => "untrusted: nothing vouches for it, and nothing will",
    };

    format!(
        "imported {} — {vouched}
  {}
",
        summary.slug, summary.file
    )
}

/// `mix extension inspect` — what a manifest declares, and what installing it here would produce.
///
/// **Three things a person reads off this**, in this order: what it is, what it would run, and what
/// it asked for. The last is the one a line could mislead about, so it says *asked for* — a port
/// here is a wish, and allocation is not something T80 does at all.
///
/// The permission lines say which of them are boundaries. `network` and `filesystem` are enforced
/// by the manifest format itself; `services` is a declaration, and reads as one, because an
/// extension runs as this account and could ignore any token it was handed (ADR 0014).
pub(crate) fn extension_inspection(inspection: &ExtensionInspection) -> String {
    let mut out = format!(
        "{} {} — {}\n  {}\n",
        inspection.id,
        inspection.version,
        inspection.name,
        match inspection.kind {
            ExtensionKind::Service => "a program MixEngine would supervise",
            ExtensionKind::WebApp => "source MixEngine would serve on an internal domain",
            ExtensionKind::DesktopApp =>
                "an application MixEngine would find and hand something to",
            ExtensionKind::Recipe => "configuration MixEngine would merge into what it generates",
        }
    );

    if !inspection.description.is_empty() {
        out.push_str(&format!("  {}\n", inspection.description));
    }
    if let Some(homepage) = &inspection.homepage {
        out.push_str(&format!("  {homepage}\n"));
    }

    out.push_str(&format!(
        "\nreaches      {}\n",
        match inspection.permissions.network {
            NetworkReach::Loopback => "this machine only, on 127.0.0.1",
            NetworkReach::Lan =>
                "every interface, on 0.0.0.0 — reachable from other machines on this network",
        }
    ));

    if !inspection.permissions.filesystem.is_empty() {
        let paths: Vec<&str> = inspection
            .permissions
            .filesystem
            .iter()
            .map(|reach| match reach {
                FilesystemReach::OwnData => "its own installation and data directories",
                FilesystemReach::ProjectRootsRead => {
                    "reading project roots (declared; this build grants nothing for it)"
                }
            })
            .collect();
        out.push_str(&format!("paths        {}\n", paths.join(", ")));
    }

    if !inspection.permissions.services.is_empty() {
        let calls: Vec<&str> = inspection
            .permissions
            .services
            .iter()
            .map(|access| match access {
                ApiAccess::Read => "read",
                ApiAccess::Write => "change",
            })
            .collect();
        out.push_str(&format!(
            "api          says it would {} what MixEngine knows about services — a declaration \
             shown to you, not a permission MixEngine enforces\n",
            calls.join(" and ")
        ));
    }

    out.push_str(&match &inspection.artifact {
        ArtifactAvailability::Published { url, .. } => format!("artifact     {url}\n"),
        ArtifactAvailability::OtherTargets { targets } => format!(
            "artifact     none for this machine; published for {}\n",
            targets.join(", ")
        ),
        ArtifactAvailability::NotRequired => {
            "artifact     none — it downloads nothing\n".to_owned()
        }
    });

    out.push_str(&format!(
        "install dir  {}\ndata dir     {}\n",
        inspection.install_dir, inspection.data_dir
    ));

    if let Some(spec) = &inspection.runs {
        out.push_str(&format!(
            "\nit would run\n  program  {}\n  cwd      {}\n",
            spec.program().display(),
            spec.cwd().display()
        ));
        if !spec.args().is_empty() {
            out.push_str(&format!("  args     {}\n", spec.args().join(" ")));
        }
    }

    if let Some(site) = &inspection.serves {
        out.push_str(&format!(
            "\nit would serve\n  root     {}\n  domain   {}\n  runtime  {} {}\n",
            site.root, site.domain, site.runtime, site.requires
        ));
    }

    if let Some(app) = &inspection.opens {
        out.push_str(&format!("\nit would open\n  scheme   {}://\n", app.scheme));
        out.push_str(&match &app.detect {
            Some(hint) => format!("  found by {hint}\n"),
            None => "  found by nothing this manifest declares for this system\n".to_owned(),
        });
    }

    if !inspection.ports.is_empty() {
        out.push_str("\nports asked for, and not held — allocation happens at install\n");
        for port in &inspection.ports {
            out.push_str(&format!("  {:<10} {}\n", port.name, port.wanted));
        }
    }

    if !inspection.extends.is_empty() {
        out.push_str("\nit would also add\n");
        for addition in &inspection.extends {
            out.push_str(&match addition {
                RecipeAddition::PhpIni { key, value } => format!("  php.ini  {key} = {value}\n"),
                // **The server is named rather than folded away** — roadmap task T81c. A home
                // running the other front end renders nothing for this entry, and a reader who
                // cannot see which one it is for cannot tell that from a line that took effect.
                RecipeAddition::FrontEnd { server, fragment } => {
                    format!("  frontend ({}) {fragment}\n", server.package())
                }
            });
        }
    }

    out
}

/// `mix blueprint list` — every blueprint this home holds.
pub(crate) fn blueprint_list(list: &BlueprintList) -> String {
    if list.blueprints.is_empty() {
        return "no blueprints have been captured — `mix blueprint capture --name <name>` writes one
"
            .to_owned();
    }

    let mut out = format!(
        "{:<24}  {:<9}  {:<12}  {}
",
        "BLUEPRINT", "SOURCE", "TRUST", "DESCRIPTION"
    );

    for blueprint in &list.blueprints {
        out.push_str(&format!(
            "{:<24}  {:<9}  {:<12}  {}
",
            blueprint.slug,
            blueprint.source.as_str(),
            // A word rather than a colour, because `--json` carries the same fact and a listing
            // that only said it in ANSI would say it to nobody in a pipe.
            //
            // Three words rather than two — roadmap task **T79b**. `mismatched` is the one worth
            // scanning a table for: somebody signed that file, and this is not what they signed.
            match (blueprint.trusted, blueprint.signature) {
                (true, _) => "signed",
                (false, Some(SignatureCheck::Missing)) => "unsigned",
                (false, Some(SignatureCheck::Rejected)) => "mismatched",
                (false, _) => "untrusted",
            },
            match blueprint.description.is_empty() {
                true => "—",
                false => blueprint.description.as_str(),
            }
        ));
    }

    out
}

/// `mix blueprint apply --dry-run` — every action, in the order it would happen.
///
/// **Words rather than a column of glyphs.** Nothing else in this file marks a line with `✓` or
/// `✗`, and a non-ASCII status column is one more thing to be wrong on a Windows console — while
/// the words are the vocabulary [`Disposition`] already has.
///
/// The elevation sentence is gathered to the end and said **once** (the T77 design, D11): what a
/// person needs to know before they start is that they will be asked for a password, not which of
/// six lines asks for it.
pub(crate) fn blueprint_plan(plan: &BlueprintPlan) -> String {
    let mut out = format!(
        "Plan: {} into project {} at {}

",
        plan.blueprint, plan.project, plan.root
    );

    for step in &plan.steps {
        out.push_str(&format!(
            "  {:<11} {}
",
            disposition_word(&step.disposition),
            step_said(step)
        ));
    }

    if plan.steps.iter().any(|step| step.elevates) {
        out.push_str(
            "
applying this asks for elevation once, to write the hosts file
",
        );
    }

    out
}

/// `mix blueprint apply` — what the apply did, step by step.
///
/// **Every step, including the ones that needed nothing**: a second apply whose every line says
/// *already true* is what tells a person the first one finished, and a rendering that hid them would
/// hide exactly that.
///
/// What did **not** run is gathered to the end and said in full, because a `[scaffold]` command is
/// the one line somebody has to act on themselves.
pub(crate) fn blueprint_applied(applied: &BlueprintApplied) -> String {
    let mut out = format!(
        "Applied {} as {} at {}\n\n",
        applied.blueprint, applied.project, applied.root
    );

    for step in &applied.steps {
        out.push_str(&format!(
            "  {:<11} {}\n",
            match &step.result {
                StepResult::Done => "done",
                StepResult::AlreadyTrue => "already",
                StepResult::NotRun { .. } => "not run",
                // **A step that ran and did not succeed** — roadmap task **T78a**. Told apart from
                // one nothing attempted, because what a person does next differs: a failure is
                // theirs to read, and a skip is theirs to decide about.
                StepResult::Failed { .. } => "failed",
                _ => "unknown",
            },
            action_said(&step.action)
        ));
    }

    for step in &applied.steps {
        match &step.result {
            StepResult::NotRun { why } | StepResult::Failed { why } => {
                out.push_str(&format!("\n{why}\n"));
            }

            _ => {}
        }
    }

    out
}

/// Whether any step of an apply ran and failed — roadmap task **T78a**.
///
/// **What the exit status is read off.** The job succeeded: the apply did everything it was asked
/// and the report is complete, and the command's own exit code is the command's news (the T78a
/// design, D7). A shell still has to hear it, and this is where it does.
pub(crate) fn blueprint_had_a_failed_step(applied: &BlueprintApplied) -> bool {
    applied
        .steps
        .iter()
        .any(|step| matches!(step.result, StepResult::Failed { .. }))
}

/// The one word a disposition is printed as.
fn disposition_word(disposition: &Disposition) -> &'static str {
    match disposition {
        Disposition::Satisfied => "installed",
        Disposition::Create => "create",
        Disposition::Choice { .. } => "asks",
        Disposition::Confirm { .. } => "confirm",
        Disposition::Blocked { .. } => "blocked",
        Disposition::Unsupported { .. } => "unsupported",
        // A disposition a later build added. Printed as something rather than hidden, because a
        // step nobody can see is a step nobody can decide about.
        _ => "unknown",
    }
}

/// What one step says about itself, including the reason where it has one.
fn step_said(step: &PlanStep) -> String {
    let said = action_said(&step.action);

    match &step.disposition {
        Disposition::Choice { installed, .. } => {
            format!("{said} — {} is installed", installed.as_str())
        }
        Disposition::Blocked { reason } | Disposition::Unsupported { reason } => {
            format!("{said} — {reason}")
        }
        _ => said,
    }
}

/// What one action says about itself, whatever became of it.
///
/// Split from [`step_said`] so that a plan and the report of an apply say the same words about the
/// same action — two renderings would be two vocabularies for one list.
fn action_said(action: &PlanAction) -> String {
    match action {
        PlanAction::RegisterProject { name, root, pins } => {
            // The pins are on this line rather than folded into the runtime steps below, because
            // they are what the *project* will ask for from now on — which is a different fact from
            // what is being installed, and the one a person is really applying a blueprint for.
            let asks = match pins.is_empty() {
                true => String::new(),
                false => format!(
                    ", asking for {}",
                    pins.iter()
                        .map(|(kind, wanted)| format!("{} {}", kind.as_str(), wanted.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };

            format!("project {name} at {root}{asks}")
        }
        PlanAction::InstallRuntime { kind, wanted } => {
            format!("{} {}", kind.as_str(), wanted.as_str())
        }
        PlanAction::InstallPackage { package, wanted } => match wanted {
            Some(wanted) => format!("{package} {}", wanted.as_str()),
            None => package.clone(),
        },
        PlanAction::EnsureService {
            package,
            instance,
            version,
            dedicated,
        } => format!(
            "{package} {}@{instance}{}",
            version
                .as_ref()
                .map(|version| format!("{} ", version.as_str()))
                .unwrap_or_default(),
            match dedicated {
                true => ", this project's own",
                false => ", reusing the shared instance",
            }
        ),
        PlanAction::CreateDatabase { database, user, .. } => {
            format!("database {database}, user {user}")
        }
        PlanAction::CreateSite {
            kind,
            doc_root,
            https,
        } => format!(
            "site {} at {}{}",
            site_kind_word(kind),
            match doc_root.is_empty() {
                true => "the project root",
                false => doc_root.as_str(),
            },
            match https {
                true => ", https",
                false => "",
            }
        ),
        PlanAction::AddDomain { domain, primary } => match primary {
            true => format!("domain {domain}"),
            false => format!("domain {domain}, an alias"),
        },
        PlanAction::IssueCertificate { domains } => {
            format!("certificate for {}", domains.join(", "))
        }
        // **The line says how far this reaches.** Extension choices belong to an installed runtime,
        // so this changes the PHP every project on this machine runs — which belongs here, at the
        // moment somebody is deciding, rather than in documentation.
        PlanAction::SetPhpExtension { runtime, name } => format!(
            "php extension {name} — changes PHP {} for every project here",
            runtime.as_str()
        ),
        PlanAction::RunScaffold { command } => format!("run `{command}`"),
        _ => "something this build cannot describe".to_owned(),
    }
}

/// The word a site kind is printed as in a plan.
fn site_kind_word(kind: &SiteKind) -> &'static str {
    match kind {
        SiteKind::PhpFpm { .. } => "php-fpm",
        SiteKind::Static => "static",
        SiteKind::ReverseProxy { .. } => "reverse-proxy",
        SiteKind::NodeApp { .. } => "node-app",
    }
}

/// `mix extension list` — roadmap task **T81**.
///
/// **The `TRUST` column is T79b's, one table across.** A blueprint says `signed` / `unsigned` /
/// `mismatched`; an extension has two answers, because the registry's signature covers the whole
/// document — an entry either arrived inside something the compiled-in key vouched for, or the
/// document was refused before anything was installed. What is left is `--path`, which nothing
/// vouches for and which stays marked for as long as it is installed.
pub(crate) fn installed_extensions(list: &InstalledExtensions) -> String {
    if list.extensions.is_empty() {
        return "nothing is installed — `mix extension available` lists what could be\n".to_owned();
    }

    let rows: Vec<[String; 7]> = list
        .extensions
        .iter()
        .map(|one| {
            [
                one.id.to_string(),
                one.version.to_string(),
                one.kind.as_str().to_owned(),
                match one.signed {
                    true => "signed".to_owned(),
                    false => "unsigned".to_owned(),
                },
                one.service
                    .as_ref()
                    .map_or_else(|| "—".to_owned(), ToString::to_string),
                one.site.clone().unwrap_or_else(|| "—".to_owned()),
                match one.ports.is_empty() {
                    true => "—".to_owned(),
                    false => one
                        .ports
                        .iter()
                        .map(|port| format!("{}={}", port.name, port.wanted))
                        .collect::<Vec<_>>()
                        .join(" "),
                },
            ]
        })
        .collect();

    table(
        ["ID", "VERSION", "KIND", "TRUST", "SERVICE", "SITE", "PORTS"],
        &rows,
    )
}

/// `mix extension available`.
///
/// **Empty and old are two answers, not one.** A registry that lists nothing — which is what
/// **T81a** publishes until the first manifests land — is a complete answer, and telling that
/// person to update sends them after a listing no version of MixEngine would show them. The
/// sentence about what this build cannot read belongs to the other empty: one where every entry
/// there is an entry this build had to drop.
pub(crate) fn extension_catalogue(catalogue: &ExtensionCatalogue) -> String {
    let mut out = match (catalogue.extensions.is_empty(), catalogue.unreadable) {
        (true, 0) => "the registry lists no extensions yet\n".to_owned(),
        (true, _) => "the registry lists nothing this build can read\n".to_owned(),
        (false, _) => {
            let rows: Vec<[String; 5]> = catalogue
                .extensions
                .iter()
                .map(|one| {
                    [
                        one.id.to_string(),
                        one.version.to_string(),
                        one.kind.as_str().to_owned(),
                        match one.installed {
                            true => "yes".to_owned(),
                            false => match &one.artifact {
                                ArtifactAvailability::OtherTargets { .. } => {
                                    "not for this machine".to_owned()
                                }
                                _ => "no".to_owned(),
                            },
                        },
                        one.description.clone(),
                    ]
                })
                .collect();

            table(["ID", "VERSION", "KIND", "INSTALLED", "DESCRIPTION"], &rows)
        }
    };

    if catalogue.stale {
        out.push_str(
            "\nthis is the last registry MixEngine could verify; the published one could not be \
             reached\n",
        );
    }

    // **Said rather than swallowed** — the T81 design's D4. An extension missing from a listing is
    // one somebody goes looking for in the wrong place.
    if catalogue.unreadable > 0 {
        out.push_str(&format!(
            "\n{} {} this build cannot read — update MixEngine to see {}\n",
            catalogue.unreadable,
            match catalogue.unreadable {
                1 => "entry",
                _ => "entries",
            },
            match catalogue.unreadable {
                1 => "it",
                _ => "them",
            }
        ));
    }

    out
}

/// `mix extension plan`, which is also the question `install` asks before it installs anything.
pub(crate) fn extension_plan(plan: &ExtensionPlan) -> String {
    let mut out = format!(
        "{} {} — {}\n  {}\n",
        plan.id,
        plan.version,
        plan.name,
        match plan.kind {
            ExtensionKind::Service => "a program MixEngine would supervise",
            ExtensionKind::WebApp => "source MixEngine would serve on an internal domain",
            ExtensionKind::DesktopApp =>
                "an application MixEngine would find and hand something to",
            ExtensionKind::Recipe => "configuration MixEngine would merge into what it generates",
        }
    );

    if !plan.description.is_empty() {
        out.push_str(&format!("  {}\n", plan.description));
    }

    out.push_str(&match plan.signed {
        true => "\nsigned       by the key this build trusts\n".to_owned(),
        false => {
            "\nUNSIGNED     nothing vouches for this: it was read from a directory\n".to_owned()
        }
    });

    out.push_str(&format!(
        "reaches      {}\n",
        match plan.permissions.network {
            NetworkReach::Loopback => "this machine only, on 127.0.0.1",
            NetworkReach::Lan =>
                "every interface, on 0.0.0.0 — reachable from other machines on this network",
        }
    ));

    if !plan.permissions.filesystem.is_empty() {
        let paths: Vec<&str> = plan
            .permissions
            .filesystem
            .iter()
            .map(|reach| match reach {
                FilesystemReach::OwnData => "its own installation and data directories",
                FilesystemReach::ProjectRootsRead => {
                    "reading project roots (declared; this build grants nothing for it)"
                }
            })
            .collect();
        out.push_str(&format!("paths        {}\n", paths.join(", ")));
    }

    if !plan.permissions.services.is_empty() {
        let calls: Vec<&str> = plan
            .permissions
            .services
            .iter()
            .map(|access| match access {
                ApiAccess::Read => "read",
                ApiAccess::Write => "change",
            })
            .collect();
        out.push_str(&format!(
            "api          says it would {} what MixEngine knows about services — a declaration \
             shown to you, not a permission MixEngine enforces\n",
            calls.join(" and ")
        ));
    }

    if !plan.ports.is_empty() {
        out.push_str(&format!(
            "ports        {}\n",
            plan.ports
                .iter()
                .map(|port| format!("{} (wants {})", port.name, port.wanted))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    if let Some(site) = &plan.site {
        out.push_str(&format!(
            "site         https://{}, on {}\n",
            site.domain, site.pool
        ));

        // **Which server this would open onto, before anybody agrees to it** — roadmap task **T82**.
        // An administrative interface onto a database is a thing to be shown the database.
        if let Some(database) = &site.database {
            out.push_str(&format!("database     {database}\n"));
        }

        // **And what it is really being granted** — roadmap task **T82a**, its design's D2. Handing
        // an application a database superuser's password is the most consequential thing an
        // extension can be given, so it is said in full: which account, where the password comes
        // from, and that nothing writes it down.
        if let Some(user) = &site.signs_in {
            out.push_str(&format!(
                "signs in     as {user}, in a php-fpm pool of its own — that pool reads the \
                 password from this machine's keyring when it starts, and nothing writes it to \
                 disk\n"
            ));
        }
    }

    out.push_str(&format!(
        "install dir  {}\ndata dir     {}\n",
        plan.install_dir, plan.data_dir
    ));

    out
}

/// `mix extension uninstall`.
pub(crate) fn extension_removal(removal: &ExtensionRemoval) -> String {
    let mut out = format!("{} was uninstalled\n", removal.id);

    if let Some(service) = &removal.service {
        out.push_str(&format!("  its service {service} went with it\n"));
    }

    // The pool a `web-app` was served on — roadmap task **T82a**. Named for the same reason the
    // service above is: a process that went is a thing to say, not a thing to leave out.
    if let Some(pool) = &removal.pool {
        out.push_str(&format!("  its pool {pool} went with it\n"));
    }

    if let Some(site) = &removal.site {
        out.push_str(&format!("  released {site}\n"));
    }

    match &removal.data_dir_kept {
        Some(path) => out.push_str(&format!(
            "  its data was kept at {path}\n  `mix extension uninstall {} --delete-data` removes \
             that too\n",
            removal.id
        )),
        None => out.push_str("  its data directory was deleted\n"),
    }

    out
}

#[cfg(test)]
mod tests {
    use mixengine_proto::{
        MetricsMinute, MetricsSample, MetricsSubject, PortWish, RuntimeKind, ServiceState,
        StepOutcome, Timestamp, VersionConstraint,
    };

    use super::*;

    /// **T81b.** A listing says who owns each site, and an extension's site says so in words a
    /// person can act on.
    #[test]
    fn a_site_listing_names_the_owner() {
        let list = SiteList {
            sites: vec![
                mixengine_proto::SiteSummary {
                    domain: "blog.test".to_owned(),
                    owner: SiteOwner::Project {
                        name: "blog".to_owned(),
                    },
                    kind: SiteKind::Static,
                    doc_root: String::new(),
                    https: true,
                    state: mixengine_proto::SiteState::Enabled,
                    sharing: None,
                },
                mixengine_proto::SiteSummary {
                    domain: "phpmyadmin.mixengine.test".to_owned(),
                    owner: SiteOwner::Extension {
                        id: mixengine_proto::ExtensionId::parse("phpmyadmin").expect("an id"),
                    },
                    kind: SiteKind::PhpFpm { pool: None },
                    doc_root: "app".to_owned(),
                    https: true,
                    state: mixengine_proto::SiteState::Enabled,
                    sharing: None,
                },
            ],
        };

        let rendered = site_list(&list);

        assert!(rendered.contains("OWNER"), "{rendered}");
        assert!(rendered.contains("  blog\n"), "{rendered}");
        assert!(rendered.contains("extension phpmyadmin"), "{rendered}");
    }

    /// **T81b.** A plan for a web-app says which name it takes and which pool it runs on — and
    /// since **T82a**, which account it would sign itself in as, because a database superuser's
    /// password is the one thing on this screen somebody must not agree to by accident.
    #[test]
    fn an_extension_plan_names_the_site_it_would_take() {
        let plan = ExtensionPlan {
            id: mixengine_proto::ExtensionId::parse("phpmyadmin").expect("an id"),
            name: "phpMyAdmin".to_owned(),
            version: PackageVersion::parse("5.2.1").expect("a version"),
            kind: ExtensionKind::WebApp,
            description: String::new(),
            signed: true,
            permissions: mixengine_proto::ExtensionPermissions::default(),
            ports: Vec::new(),
            install_dir: "/x".to_owned(),
            data_dir: "/y".to_owned(),
            site: Some(mixengine_proto::PlannedSite {
                domain: "phpmyadmin.mixengine.test".to_owned(),
                pool: ServiceId::parse("php-fpm@phpmyadmin").expect("an id"),
                database: Some(ServiceId::parse("mariadb@main").expect("an id")),
                signs_in: Some("root".to_owned()),
            }),
        };

        let rendered = extension_plan(&plan);

        assert!(
            rendered
                .contains("site         https://phpmyadmin.mixengine.test, on php-fpm@phpmyadmin"),
            "{rendered}"
        );
        assert!(
            rendered.contains("database     mariadb@main"),
            "which server it would open onto is shown before anybody agrees: {rendered}"
        );
        assert!(
            rendered.contains("signs in     as root"),
            "the account, and the sentence about where its password comes from: {rendered}"
        );
        assert!(
            rendered.contains("keyring"),
            "a person agreeing to this is told the password is never written down: {rendered}"
        );
    }

    /// An inspection of the Mailpit fixture, as the daemon would answer it.
    fn mailpit_inspection() -> ExtensionInspection {
        let spec = mixengine_proto::ServiceSpec::builder(
            ServiceId::parse("mailpit").expect("an id"),
            if cfg!(windows) {
                r"C:\home\.mixengine\extensions\mailpit\mailpit"
            } else {
                "/home/dev/.mixengine/extensions/mailpit/mailpit"
            },
        )
        .cwd(if cfg!(windows) {
            r"C:\home\.mixengine\extensions\mailpit\data"
        } else {
            "/home/dev/.mixengine/extensions/mailpit/data"
        })
        .args(["--listen", "127.0.0.1:8025", "--smtp", "127.0.0.1:1025"])
        .ready(mixengine_proto::ReadyCheck::Tcp {
            addr: "127.0.0.1:8025".parse().expect("an address"),
            timeout: mixengine_proto::Millis::from_secs(10),
        })
        .build()
        .expect("a spec");

        ExtensionInspection {
            id: mixengine_proto::ExtensionId::parse("mailpit").expect("an id"),
            name: "Mailpit".to_owned(),
            version: PackageVersion::parse("1.20.0").expect("a version"),
            kind: ExtensionKind::Service,
            description: "Local SMTP capture and web UI".to_owned(),
            homepage: Some("https://mailpit.axllent.org".to_owned()),
            permissions: mixengine_proto::ExtensionPermissions {
                services: std::collections::BTreeSet::new(),
                network: NetworkReach::Loopback,
                filesystem: [FilesystemReach::OwnData].into_iter().collect(),
            },
            artifact: ArtifactAvailability::Published {
                url: "https://example.invalid/mailpit.zip".to_owned(),
                sha256: "0".repeat(64),
            },
            ports: vec![
                PortWish {
                    name: "ui_port".to_owned(),
                    wanted: 8025,
                },
                PortWish {
                    name: "smtp_port".to_owned(),
                    wanted: 1025,
                },
            ],
            install_dir: "/home/dev/.mixengine/extensions/mailpit".to_owned(),
            data_dir: "/home/dev/.mixengine/extensions/mailpit/data".to_owned(),
            runs: Some(spec),
            serves: None,
            opens: None,
            extends: vec![RecipeAddition::PhpIni {
                key: "sendmail_path".to_owned(),
                value: "/home/dev/.mixengine/extensions/mailpit/mailpit sendmail".to_owned(),
            }],
        }
    }

    /// A person reads three things off this: what it is, what it would run, and what it asked
    /// for. The last is the one a line could mislead about.
    #[test]
    fn an_inspection_says_what_would_run_and_what_was_only_asked_for() {
        let rendered = extension_inspection(&mailpit_inspection());

        assert!(rendered.contains("mailpit"));
        assert!(rendered.contains("127.0.0.1:8025"));
        assert!(rendered.contains("asked for"));
        assert!(!rendered.contains("reserved"));
    }

    /// **`services` is a disclosure**, and the line says so rather than reading as a grant.
    #[test]
    fn the_permission_lines_do_not_read_as_grants() {
        let mut inspection = mailpit_inspection();
        inspection.permissions.services.insert(ApiAccess::Read);

        let rendered = extension_inspection(&inspection);

        assert!(rendered.contains("says it would"));
        assert!(rendered.contains("not a permission MixEngine enforces"));
    }

    /// `lan` renders `0.0.0.0`, which reads as alarming without the sentence beside it.
    #[test]
    fn every_interface_is_explained() {
        let mut inspection = mailpit_inspection();
        inspection.permissions.network = NetworkReach::Lan;

        let rendered = extension_inspection(&inspection);

        assert!(rendered.contains("reachable from other machines"));
    }

    /// A catalogue with nothing in it, which is what a freshly published registry answers.
    fn an_empty_catalogue(unreadable: usize) -> ExtensionCatalogue {
        ExtensionCatalogue {
            extensions: Vec::new(),
            unreadable,
            stale: false,
        }
    }

    /// Empty and old are different answers. The registry published for **T81a** lists nothing at
    /// all, and telling that person their build is too old sends them to update something that
    /// would not change the listing.
    #[test]
    fn an_empty_registry_does_not_read_as_a_build_too_old() {
        let rendered = extension_catalogue(&an_empty_catalogue(0));

        assert!(rendered.contains("no extensions"));
        assert!(!rendered.contains("this build"));
        assert!(!rendered.contains("update MixEngine"));
    }

    /// The other empty: every entry there is one this build cannot read, and that person *is* the
    /// one who should update.
    #[test]
    fn a_listing_this_build_cannot_read_still_says_to_update() {
        let rendered = extension_catalogue(&an_empty_catalogue(2));

        assert!(rendered.contains("this build can read"));
        assert!(rendered.contains("2 entries this build cannot read"));
        assert!(rendered.contains("update MixEngine"));
    }

    /// A plan with one of everything the renderer has a branch for.
    fn a_plan() -> BlueprintPlan {
        BlueprintPlan {
            blueprint: "laravel-php82".to_owned(),
            project: "shop".to_owned(),
            root: "/home/dev/shop".to_owned(),
            steps: vec![
                PlanStep {
                    action: PlanAction::InstallRuntime {
                        kind: mixengine_proto::RuntimeKind::Php,
                        wanted: mixengine_proto::VersionConstraint::parse("8.2.23")
                            .expect("a constraint"),
                    },
                    disposition: Disposition::Satisfied,
                    elevates: false,
                },
                PlanStep {
                    action: PlanAction::SetPhpExtension {
                        runtime: PackageVersion::parse("8.2.23").expect("a version"),
                        name: "xdebug".to_owned(),
                    },
                    disposition: Disposition::Create,
                    elevates: false,
                },
                PlanStep {
                    action: PlanAction::AddDomain {
                        domain: "shop.test".to_owned(),
                        primary: true,
                    },
                    disposition: Disposition::Blocked {
                        reason: "shop.test is already answered by blog.test".to_owned(),
                    },
                    elevates: true,
                },
                PlanStep {
                    action: PlanAction::IssueCertificate {
                        domains: vec!["shop.test".to_owned()],
                    },
                    disposition: Disposition::Create,
                    elevates: true,
                },
            ],
            source: mixengine_proto::BlueprintSource::Captured,
            trusted: true,
            signature: None,
        }
    }

    /// **Every step, and the one that did not run said in full** — roadmap task T78. A scaffold
    /// command nobody ran is the one line a person has to act on themselves, so it is not folded
    /// into a count.
    #[test]
    fn an_apply_prints_every_step_and_spells_out_what_did_not_run() {
        let applied = BlueprintApplied {
            blueprint: "blog-stack".to_owned(),
            project: "shop".to_owned(),
            root: "/tmp/shop".to_owned(),
            steps: vec![
                StepOutcome {
                    action: PlanAction::RegisterProject {
                        name: "shop".to_owned(),
                        root: "/tmp/shop".to_owned(),
                        pins: std::collections::BTreeMap::new(),
                    },
                    result: StepResult::Done,
                },
                StepOutcome {
                    action: PlanAction::InstallRuntime {
                        kind: RuntimeKind::Php,
                        wanted: VersionConstraint::parse("8.2.23").expect("a constraint"),
                    },
                    result: StepResult::AlreadyTrue,
                },
                StepOutcome {
                    action: PlanAction::RunScaffold {
                        command: "composer install".to_owned(),
                    },
                    result: StepResult::NotRun {
                        why: "`composer install` was not run: nobody agreed to it".to_owned(),
                    },
                },
            ],
        };

        let rendered = super::blueprint_applied(&applied);

        assert!(rendered.contains("done"), "{rendered}");
        assert!(rendered.contains("already"), "{rendered}");
        assert!(rendered.contains("composer install"), "{rendered}");
        assert!(rendered.contains("nobody agreed to it"), "{rendered}");
    }

    /// **A step that ran and failed reads as that, not as one that was skipped** — roadmap task
    /// **T78a**, its design's D7. What a person does next differs between the two, and the exit
    /// status differs with it.
    #[test]
    fn a_failed_step_prints_its_exit_rather_than_a_skip() {
        let applied = BlueprintApplied {
            blueprint: "borrowed".to_owned(),
            project: "shop".to_owned(),
            root: "/tmp/shop".to_owned(),
            steps: vec![StepOutcome {
                action: PlanAction::RunScaffold {
                    command: "composer install".to_owned(),
                },
                result: StepResult::Failed {
                    why: "`composer install` exited with 1".to_owned(),
                },
            }],
        };

        let rendered = super::blueprint_applied(&applied);

        assert!(rendered.contains("failed"), "{rendered}");
        assert!(!rendered.contains("not run"), "{rendered}");
        assert!(rendered.contains("exited with 1"), "{rendered}");
        assert!(super::blueprint_had_a_failed_step(&applied));
    }

    /// An import says which of the two things a person now has, because it is the only moment they
    /// are told: the trust is decided there and never again.
    #[test]
    fn an_import_says_whether_anything_vouched_for_it() {
        let untrusted = super::blueprint_imported(&BlueprintSummary {
            slug: "borrowed".to_owned(),
            name: "borrowed".to_owned(),
            description: String::new(),
            created_at: "2026-09-01T00:00:00Z".to_owned(),
            source: mixengine_proto::BlueprintSource::Imported,
            trusted: false,
            signature: Some(mixengine_proto::SignatureCheck::Missing),
            file: "/home/dev/.mixengine/blueprints/borrowed.toml".to_owned(),
        });

        assert!(untrusted.contains("untrusted"), "{untrusted}");
        assert!(untrusted.contains("nothing will"), "{untrusted}");
    }

    /// **Which kind of untrusted** — roadmap task **T79b**. A file nobody signed and a file whose
    /// signature did not verify are both untrusted and are not the same event, and the second is
    /// the one worth reading twice.
    #[test]
    fn an_import_says_which_kind_of_untrusted_it_is() {
        let summary = |signature| BlueprintSummary {
            slug: "borrowed".to_owned(),
            name: "borrowed".to_owned(),
            description: String::new(),
            created_at: "2026-09-01T00:00:00Z".to_owned(),
            source: mixengine_proto::BlueprintSource::Imported,
            trusted: false,
            signature,
            file: "/home/dev/.mixengine/blueprints/borrowed.toml".to_owned(),
        };

        let missing = super::blueprint_imported(&summary(Some(SignatureCheck::Missing)));
        assert!(missing.contains("nothing came with it"), "{missing}");

        // True of all three things the verifier folds together — edited after signing, signed by
        // another key, and not a signature at all. "The bytes changed" would accuse the last two of
        // the first.
        let rejected = super::blueprint_imported(&summary(Some(SignatureCheck::Rejected)));
        assert!(rejected.contains("not the gallery's"), "{rejected}");
        assert!(!rejected.contains("nothing came with it"), "{rejected}");

        // A row older than this task kept no reason, and keeps the sentence it has always had.
        let older = super::blueprint_imported(&summary(None));
        assert!(older.contains("nothing vouches for it"), "{older}");
    }

    /// The listing says it in one word, because a table is where somebody scans six blueprints at
    /// once — and `--json` carries the same three facts for anything that is not a person.
    #[test]
    fn the_listing_says_which_kind_of_untrusted_each_one_is() {
        let row = |slug: &str, trusted, signature| BlueprintSummary {
            slug: slug.to_owned(),
            name: slug.to_owned(),
            description: String::new(),
            created_at: "2026-09-01T00:00:00Z".to_owned(),
            source: mixengine_proto::BlueprintSource::Imported,
            trusted,
            signature,
            file: format!("/home/dev/.mixengine/blueprints/{slug}.toml"),
        };

        let listed = super::blueprint_list(&BlueprintList {
            blueprints: vec![
                row("good", true, Some(SignatureCheck::Verified)),
                row("bare", false, Some(SignatureCheck::Missing)),
                row("stale", false, Some(SignatureCheck::Rejected)),
                row("older", false, None),
            ],
        });

        let line = |slug: &str| {
            listed
                .lines()
                .find(|line| line.starts_with(slug))
                .unwrap_or_else(|| panic!("no row for {slug}: {listed}"))
                .to_owned()
        };

        assert!(line("good").contains("signed"), "{listed}");
        assert!(line("bare").contains("unsigned"), "{listed}");
        assert!(line("stale").contains("mismatched"), "{listed}");
        assert!(line("older").contains("untrusted"), "{listed}");
    }

    /// Words, not glyphs — and the reason a step is blocked travels with the step, because a person
    /// reading "blocked" without it has to go looking.
    #[test]
    fn a_plan_prints_one_line_per_step_and_says_who_holds_a_taken_domain() {
        let rendered = super::blueprint_plan(&a_plan());

        assert!(rendered.contains("installed"), "{rendered}");
        assert!(rendered.contains("blocked"), "{rendered}");
        assert!(
            rendered.contains("already answered by blog.test"),
            "{rendered}"
        );
        assert!(
            !rendered.contains('\u{2713}') && !rendered.contains('\u{2717}'),
            "a status glyph crept into a file that has never had one:\n{rendered}"
        );
    }

    /// **D11**: said once, at the end, so a person knows before they start rather than four lines
    /// in.
    #[test]
    fn a_plan_that_would_ask_for_a_password_says_so_once() {
        let rendered = super::blueprint_plan(&a_plan());

        assert_eq!(rendered.matches("elevation").count(), 1, "{rendered}");
    }

    /// Enabling an extension changes the PHP every project on this machine runs, and that belongs
    /// on the line somebody is deciding from.
    #[test]
    fn enabling_an_extension_says_that_it_reaches_past_this_project() {
        let rendered = super::blueprint_plan(&a_plan());

        assert!(rendered.contains("every project here"), "{rendered}");
    }

    /// An empty home says what to type next, the way every other empty listing here does.
    #[test]
    fn a_home_with_no_blueprints_says_how_to_make_one() {
        let rendered = super::blueprint_list(&BlueprintList {
            blueprints: Vec::new(),
        });

        assert!(rendered.contains("mix blueprint capture"), "{rendered}");
    }

    /// A share as the daemon answers one, advertised or not — roadmap tasks **T74** and **T75**.
    fn a_share(advertised: bool) -> SiteSharing {
        SiteSharing {
            interface: "Wi-Fi".to_owned(),
            address: "192.168.1.10".to_owned(),
            url: "http://192.168.1.10".to_owned(),
            name: Some("blog-mixengine.local".to_owned()),
            advertised,
            ca_url: "http://192.168.1.10/__mixengine/ca.crt".to_owned(),
            since: Timestamp(1_700_000_000_000),
            until: None,
        }
    }

    /// What `mix site share` prints once the phone has a name and somewhere to get the authority —
    /// roadmap task **T75**.
    ///
    /// **The QR stays on the address.** Android's resolver does not answer `.local` for a browser,
    /// so a code carrying the name would be a broken URL for a large share of phones — the T75
    /// design, D11.
    #[test]
    fn a_shared_site_prints_its_name_and_where_to_get_the_authority() {
        let rendered = super::site_shared(&a_share(true));

        assert!(rendered.contains("blog-mixengine.local"), "{rendered}");
        assert!(rendered.contains("/__mixengine/ca.crt"), "{rendered}");
        assert!(
            rendered.contains("Certificate Trust Settings"),
            "{rendered}"
        );
    }

    /// **A deadline is printed as a wait and not as a timestamp** — roadmap task **T76**. Somebody
    /// who has just typed `--for 2h` wants to know it took, and "in 2h" is the answer to that; an
    /// instant in milliseconds is a number they would have to convert.
    #[test]
    fn a_share_with_a_deadline_says_when_it_ends() {
        let Timestamp(millis) = Timestamp::from_system_time(SystemTime::now());

        let sharing = SiteSharing {
            until: Some(Timestamp(millis + 7_200_000)),
            ..a_share(true)
        };

        let rendered = super::site_shared(&sharing);

        // **The wait and not the figure.** `site_shared` reads its own clock, so the minutes have
        // already moved by the time this line runs; asserting `in 2h 0m` would be asserting that
        // no time passed between two statements, which is the shape of a test that fails once a
        // month on a loaded machine and is then marked flaky rather than read.
        assert!(rendered.contains("  ends       in "), "{rendered}");
    }

    /// The figure itself, where the clock is an argument and not a reading — the same rounding
    /// `ago` and `uptime` use, so all three say `1h 59m` about the same span.
    #[test]
    fn a_wait_is_rounded_the_way_an_age_is() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000);

        assert_eq!(in_time(Timestamp(1_000_000 + 7_200_000), now), "in 2h 0m");
        assert_eq!(in_time(Timestamp(1_000_000 + 90_000), now), "in 1m 30s");
    }

    /// **A share with no deadline prints no line about one.** Every line in this block is one a
    /// reader has to take in, and "ends never" is one they would have to learn to skip.
    #[test]
    fn a_share_with_no_deadline_says_nothing_about_one() {
        assert!(!super::site_shared(&a_share(true)).contains("ends"));
    }

    /// A deadline the loop has not caught up with yet is a wait, not a negative number: the sweep
    /// runs on a period, so an instant a few seconds past is the ordinary state.
    #[test]
    fn a_deadline_already_gone_reads_as_a_wait() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10);

        assert_eq!(in_time(Timestamp(1_000), now), "any moment now");
    }

    /// **A name nothing is answering for is said, not hidden.** The site still works by address,
    /// and a name printed as though it resolved is the slowest kind of wrong.
    #[test]
    fn a_name_that_is_not_advertised_says_so() {
        let rendered = super::site_shared(&a_share(false));

        assert!(rendered.contains("not being advertised"), "{rendered}");
    }

    /// What `mix site share` prints — roadmap task **T74**.
    ///
    /// **The URL is above the code, and both are there.** A QR is unreadable to anyone reading a
    /// transcript, piping the output, or working over a connection that mangles block characters,
    /// so the string a person could type by hand is never replaced by a picture of it.
    #[test]
    fn a_shared_site_prints_its_url_the_interface_and_a_code() {
        let rendered = super::site_shared(&a_share(true));

        assert!(rendered.contains("http://192.168.1.10"), "{rendered}");
        assert!(rendered.contains("Wi-Fi"), "{rendered}");

        // The code itself: half-height blocks, and enough of them to be a QR rather than a stray
        // character in a sentence.
        assert!(
            rendered.matches(['█', '▀', '▄']).count() > 100,
            "{rendered}"
        );

        // And why it is http, which is the question the URL raises for a site declaring HTTPS.
        assert!(rendered.contains("certificate authority"), "{rendered}");
    }

    /// The line names both numbers and the ending, so nobody has to infer either — task **T71a**.
    ///
    /// **The `false` case is the one worth a test.** A database over its ceiling is warned about and
    /// deliberately left alone, and a rendering that said only "watched" would read exactly like one
    /// that was about to rescue it.
    #[test]
    fn a_watchdog_line_says_what_happens_at_the_end_of_it() {
        let restarted = super::watchdog_line(Some(MemoryWatchdog {
            after_minutes: 3,
            restarts: true,
        }));

        assert!(restarted.contains('3'), "{restarted}");
        assert!(restarted.contains("restarted after"), "{restarted}");

        let warned = super::watchdog_line(Some(MemoryWatchdog {
            after_minutes: 3,
            restarts: false,
        }));

        assert!(
            warned.contains("not restarted automatically"),
            "a service that is only warned about must not read as one that is rescued: {warned}"
        );

        assert!(
            super::watchdog_line(None).is_empty(),
            "nothing watching is no line at all, not an empty one"
        );
    }

    fn sample(subject: MetricsSubject, cpu: Option<f32>, rss: u64) -> MetricsSample {
        MetricsSample {
            subject,
            cpu_percent: cpu,
            rss_bytes: rss,
            processes: 1,
        }
    }

    #[test]
    fn a_subject_with_no_cpu_figure_renders_a_dash_and_never_a_zero() {
        let rendered = metrics_frame(&MetricsFrame {
            at: Timestamp(60_000),
            samples: vec![sample(MetricsSubject::Daemon, None, 41_943_040)],
        });

        assert!(rendered.contains("40.0 MiB"), "{rendered}");
        assert!(rendered.contains(MISSING), "{rendered}");
        assert!(
            !rendered.contains("0.0%"),
            "a figure that could not be taken is not a service using no CPU: {rendered}"
        );
    }

    #[test]
    fn a_frame_that_measured_nothing_says_so_rather_than_printing_an_empty_table() {
        let rendered = metrics_frame(&MetricsFrame {
            at: Timestamp(60_000),
            samples: Vec::new(),
        });

        assert_eq!(
            rendered,
            "nothing could be measured
"
        );
    }

    #[test]
    fn a_history_row_shows_how_many_readings_it_is_made_of() {
        let now = SystemTime::now();
        let Timestamp(millis) = Timestamp::from_system_time(now);

        let rendered = metrics_history(
            &MetricsHistory {
                minutes: vec![MetricsMinute {
                    subject: MetricsSubject::Daemon,
                    minute: Timestamp(millis - 120_000),
                    cpu_avg: Some(1.5),
                    cpu_peak: Some(9.5),
                    rss_avg: 41_943_040,
                    rss_peak: 62_914_560,
                    samples: 60,
                }],
                retention_hours: 24,
            },
            now,
        );

        assert!(rendered.contains("SAMPLES"), "{rendered}");
        assert!(rendered.contains("60"), "{rendered}");
        assert!(
            rendered.contains("1.5%") && rendered.contains("9.5%"),
            "{rendered}"
        );
        assert!(
            rendered.contains("ago"),
            "a minute is printed as an age: {rendered}"
        );
    }

    #[test]
    fn an_empty_history_says_how_long_this_home_keeps_one() {
        let rendered = metrics_history(
            &MetricsHistory {
                minutes: Vec::new(),
                retention_hours: 24,
            },
            SystemTime::now(),
        );

        assert!(rendered.contains("24 hours"), "{rendered}");
    }

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
            elevation: mixengine_proto::ElevationSummary {
                elevated: false,
                can_prompt: true,
                pending: 0,
            },
            dns: mixengine_proto::DnsStatus {
                mode: DnsMode::HostsOnly,
                listening: None,
                wildcards: Vec::new(),
                because: Some("[dns] enabled = false in config.toml".to_owned()),
            },
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

    fn a_pending_probe(id: i64) -> mixengine_proto::PendingOp {
        let op = mixengine_proto::privileged::PrivilegedOp::Probe {};

        mixengine_proto::PendingOp {
            id: mixengine_proto::PendingOpId(id),
            description: op.describe(),
            op,
            requested_at: mixengine_proto::Timestamp(1_760_000_000_000),
        }
    }

    /// The screen T64 will build on: every operation, and what each will literally change, before
    /// anybody raises a prompt.
    #[test]
    fn a_pending_list_says_what_each_operation_will_change() {
        let rendered = elevation_status(&ElevationStatus {
            elevated: false,
            can_prompt: true,
            reason: None,
            helper: Some("/opt/mixengine/mixengine-elevate".to_owned()),
            pending: vec![a_pending_probe(1), a_pending_probe(2)],
            last: None,
        });

        assert!(rendered.contains("2 operations are waiting"), "{rendered}");
        assert!(rendered.contains("mix elevation grant"), "{rendered}");
        assert!(
            rendered.contains(&mixengine_proto::privileged::PrivilegedOp::Probe {}.describe()),
            "{rendered}"
        );
    }

    /// T64's screen: the same list, and one sentence about the prompt that is about to be raised.
    ///
    /// The assertion that matters is the negative one. `mix elevation status` ends by telling a
    /// person to run `mix elevation grant`; this is printed *by* that command, so repeating the
    /// advice would be telling somebody to run what they are already running.
    #[test]
    fn the_screen_before_a_prompt_is_the_list_and_what_the_prompt_will_be() {
        let rendered = elevation_prompt(&ElevationStatus {
            elevated: false,
            can_prompt: true,
            reason: None,
            helper: Some("/opt/mixengine/mixengine-elevate".to_owned()),
            pending: vec![a_pending_probe(1), a_pending_probe(2)],
            last: None,
        });

        assert!(rendered.contains("2 operations are waiting"), "{rendered}");
        assert!(
            rendered.contains(&mixengine_proto::privileged::PrivilegedOp::Probe {}.describe()),
            "{rendered}"
        );
        assert!(rendered.contains("once"), "{rendered}");
        assert!(!rendered.contains("mix elevation grant"), "{rendered}");
    }

    /// A machine that cannot prompt has to print the reason, because on Linux the reason is the
    /// command a person is meant to type.
    #[test]
    fn a_machine_that_cannot_prompt_prints_what_to_do_instead() {
        let rendered = elevation_status(&ElevationStatus {
            elevated: false,
            can_prompt: false,
            reason: Some(
                "no polkit agent; run: pkexec /opt/mixengine/mixengine-elevate /…".to_owned(),
            ),
            helper: Some("/opt/mixengine/mixengine-elevate".to_owned()),
            pending: vec![a_pending_probe(1)],
            last: None,
        });

        assert!(rendered.contains("pkexec"), "{rendered}");
        assert!(
            !rendered.contains("mix elevation grant"),
            "offering a command that cannot work: {rendered}"
        );
    }

    /// A decline is a normal outcome, so it reads as one — and the list stays, which is the whole of
    /// the degraded mode a person sees.
    #[test]
    fn a_declined_grant_reads_as_a_choice_rather_than_a_failure() {
        let rendered = elevation_status(&ElevationStatus {
            elevated: false,
            can_prompt: true,
            reason: None,
            helper: Some("/opt/mixengine/mixengine-elevate".to_owned()),
            pending: vec![a_pending_probe(1)],
            last: Some(mixengine_proto::GrantOutcome {
                job: mixengine_proto::JobId(4),
                at: mixengine_proto::Timestamp(1_760_000_000_000),
                outcome: mixengine_proto::privileged::ElevationOutcome::Declined,
                applied: 0,
                still_pending: 1,
            }),
        });

        assert!(rendered.contains("declined"), "{rendered}");
        assert!(!rendered.to_lowercase().contains("error"), "{rendered}");
    }

    /// Whichever mechanism is running, `mix status` says which — and a home on the hosts file is
    /// told what it is missing rather than left to discover it on the first subdomain.
    #[test]
    fn the_status_line_names_the_mechanism_this_home_resolves_through() {
        let hosts_only = status(&example());
        assert!(hosts_only.contains("names     hosts file"), "{hosts_only}");
        assert!(hosts_only.contains("no wildcards"), "{hosts_only}");
        assert!(hosts_only.contains("[dns] enabled"), "{hosts_only}");

        let on_dns = DaemonStatus {
            dns: mixengine_proto::DnsStatus {
                mode: DnsMode::Dns,
                listening: Some("127.0.0.1:53535".to_owned()),
                wildcards: vec!["test".to_owned(), "localhost".to_owned()],
                because: None,
            },
            ..example()
        };

        let rendered = status(&on_dns);
        assert!(
            rendered.contains("names     DNS on 127.0.0.1:53535"),
            "{rendered}"
        );
        assert!(
            rendered.contains("wildcards for *.test, *.localhost"),
            "{rendered}"
        );
    }

    /// `mix status` says it in one line, without a second round trip and without deciding for
    /// itself what degraded means.
    #[test]
    fn the_status_line_says_how_many_are_waiting_and_whether_the_daemon_is_elevated() {
        let waiting = DaemonStatus {
            elevation: mixengine_proto::ElevationSummary {
                elevated: true,
                can_prompt: true,
                pending: 3,
            },
            ..example()
        };

        let rendered = status(&waiting);

        assert!(rendered.contains("3 operations are waiting"), "{rendered}");
        assert!(rendered.contains("administrative token"), "{rendered}");
    }
    /// A report for `fakeservice@main` with `source` and nothing else varied.
    fn idle_report(source: mixengine_proto::IdleSource) -> IdleReport {
        IdleReport {
            service: mixengine_proto::ServiceId::parse("fakeservice@main").expect("an id"),
            policy: None,
            source,
            exempt: Vec::new(),
        }
    }

    /// The three ways a service is never idle-stopped read as three different sentences.
    ///
    /// **The one that matters is `Unmeasurable`**, which was found by a failing test rather than
    /// designed: a php-fpm pool on a Unix socket has no port to count, so a person who has just
    /// typed `--after 30m` would otherwise be told only "never" — the outcome without the reason,
    /// which is an invitation to type it again.
    #[test]
    fn the_three_ways_of_never_idling_are_three_different_sentences() {
        let unset = service_idle(&idle_report(mixengine_proto::IdleSource::Unset));
        let never = service_idle(&idle_report(mixengine_proto::IdleSource::Never));
        let unmeasurable = service_idle(&idle_report(mixengine_proto::IdleSource::Unmeasurable));

        for rendered in [&unset, &never, &unmeasurable] {
            assert!(rendered.contains("never"), "{rendered}");
        }

        assert!(unset.contains("no default yet"), "{unset}");
        assert!(never.contains("switched off"), "{never}");
        assert!(
            unmeasurable.contains("nothing to measure"),
            "asked for and unmeasurable is not the same answer as nobody asking: {unmeasurable}"
        );
    }

    /// An exemption names the thing a person would have to go and change.
    #[test]
    fn an_exemption_names_what_is_holding_the_service_open() {
        let report = IdleReport {
            exempt: vec![
                IdleExemption::DependentRunning {
                    service: mixengine_proto::ServiceId::parse("php-fpm@8.3").expect("an id"),
                },
                IdleExemption::ProjectKeptWarm {
                    project: "shop".to_owned(),
                },
            ],
            ..idle_report(mixengine_proto::IdleSource::Row)
        };

        let rendered = service_idle(&report);

        assert!(rendered.contains("php-fpm@8.3"), "{rendered}");
        assert!(rendered.contains("shop"), "{rendered}");
    }

    /// It says where the credential is and never what it is — the T77a design, D11. The test is the
    /// guard: a renderer that grew a password would put one in a terminal's scrollback.
    #[test]
    fn a_created_database_says_where_the_password_lives() {
        let rendered = database_created(&DatabaseAccount {
            service: ServiceId::parse("mariadb@main").expect("an id"),
            database: "blog".to_owned(),
            user: "blog".to_owned(),
            secret: "mariadb@main/blog".to_owned(),
            made: mixengine_proto::Provisioned {
                database: Made::Created,
                user: Made::Existing,
            },
        });

        assert!(
            rendered.contains("database blog created on mariadb@main"),
            "{rendered}"
        );
        assert!(
            rendered.contains("account  blog already existed"),
            "{rendered}"
        );
        assert!(rendered.contains("mariadb@main/blog"), "{rendered}");
        assert!(
            !rendered.to_ascii_lowercase().contains("password is"),
            "{rendered}"
        );
    }
}
