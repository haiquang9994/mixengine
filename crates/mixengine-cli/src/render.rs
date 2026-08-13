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
    DaemonStatus, DaemonVersion, PROTOCOL_VERSION, ServiceId, ServiceList, ServiceState,
    ServiceSummary, ServiceWalk, StateReason, Timestamp, Uptime,
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

    let headings = ["SERVICE", "STATE", "SUPERVISED", "PID", "DEPENDS ON"];
    let widths = std::array::from_fn(|column| {
        rows.iter()
            .map(|row| row[column].chars().count())
            .chain(std::iter::once(headings[column].chars().count()))
            .max()
            .unwrap_or_default()
    });

    let mut rendered = String::new();
    for row in std::iter::once(headings.map(str::to_owned)).chain(rows) {
        rendered.push_str(line(&row, &widths).trim_end());
        rendered.push('\n');
    }

    rendered
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

/// One row of the listing, each cell padded to its column.
fn line(row: &[String; 5], widths: &[usize; 5]) -> String {
    row.iter()
        .zip(widths)
        .map(|(cell, width)| format!("{cell:width$}"))
        .collect::<Vec<_>>()
        .join("  ")
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
            last_started_at: running.then_some(Timestamp(1_723_000_000_000)),
            last_exit_code: None,
            depends_on: Vec::new(),
        }
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
