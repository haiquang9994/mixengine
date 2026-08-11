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

use mixengine_proto::{DaemonStatus, DaemonVersion, PROTOCOL_VERSION, Uptime};

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
    use mixengine_proto::Timestamp;

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
