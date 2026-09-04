//! What `autostart.*` answers: whether this machine starts a daemon for this home at login.
//!
//! **One answer type for all three methods**, on [`PathReport`](crate::PathReport)'s precedent:
//! `autostart.status`, `autostart.enable` and `autostart.disable` are the same sentence about the
//! same entry, and a client renders them with one function. What differs between them is
//! [`AutostartReport::changed`], which is how "already done" is told apart from "done just now".
//!
//! **None of the three takes parameters.** There is exactly one entry this can be about and one home
//! it can name — the daemon's own — and an argument would be an API for registering arbitrary
//! programs to run at somebody's login.

/// How this machine starts something at login, if it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutostartMechanism {
    /// Windows: a Task Scheduler logon task, run under this account's own token.
    LogonTask,

    /// macOS: a LaunchAgent in this user's `~/Library/LaunchAgents`.
    LaunchAgent,

    /// Linux: a systemd **user** unit, wanted by `default.target`.
    SystemdUser,

    /// This machine offers no way to start something at login.
    ///
    /// **A valid answer, not an error**, on the precedent `mixengine_platform`'s `ResolverMethod`
    /// already sets for a machine that cannot scope DNS: a Linux box with no systemd user manager —
    /// a container, a stripped image, a CI runner — has no mechanism, and what a person can act on
    /// there is a sentence rather than a failure. Windows and macOS never answer it, because Task
    /// Scheduler and launchd are part of the operating system.
    None,
}

/// What every `autostart.*` method answers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AutostartReport {
    /// Which mechanism this machine has.
    pub mechanism: AutostartMechanism,

    /// Where a person would go and look: the task name, the plist path, the unit path.
    ///
    /// A string and not a `PathBuf` for [`DaemonStatus`](crate::DaemonStatus)' reason, and here it
    /// is not always a path at all — on Windows it is a name in the Task Scheduler library, and on
    /// a machine with no mechanism it is what was looked for and did not answer.
    pub location: String,

    /// Whether the entry is registered as things stand.
    pub enabled: bool,

    /// Whether *this call* is what registered it or removed it.
    ///
    /// Always `false` from `autostart.status`. It is what lets a client say "already set up" rather
    /// than claiming a write it did not perform — the two are indistinguishable to somebody reading
    /// the output, and only one of them is true.
    pub changed: bool,

    /// What the registered entry will run, as its own words.
    ///
    /// Empty when nothing is registered. Read back off the machine rather than composed from what
    /// would be written, which is the whole point of carrying it: an entry naming a `mixengined`
    /// that has moved, or a home that is not this one, is the failure this field exists to be able
    /// to report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,

    /// Whether that command names *this* daemon's home.
    ///
    /// `false` when nothing is registered, and `false` when the entry belongs to another home. There
    /// is one entry per user, so enabling from a second home replaces it, and "registered, but not
    /// for you" is the half-state this exists to name. Composed by the daemon: a client folding it
    /// itself would be business logic in a client, and a place for two clients to disagree about
    /// whose entry it is.
    pub for_this_home: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> AutostartReport {
        AutostartReport {
            mechanism: AutostartMechanism::SystemdUser,
            location: "/home/me/.config/systemd/user/mixengined.service".to_owned(),
            enabled: true,
            changed: true,
            command: vec![
                "/usr/bin/mixengined".to_owned(),
                "--home".to_owned(),
                "/home/me/.local/share/mixengine".to_owned(),
            ],
            for_this_home: true,
        }
    }

    #[test]
    fn a_report_round_trips_through_the_wire() {
        let encoded = serde_json::to_value(report()).unwrap();

        assert_eq!(encoded["mechanism"], "systemd_user");
        assert_eq!(encoded["enabled"], true);
        assert_eq!(encoded["command"][1], "--home");

        assert_eq!(
            serde_json::from_value::<AutostartReport>(encoded).unwrap(),
            report()
        );
    }

    /// The machine that has no mechanism at all answers, rather than failing.
    #[test]
    fn a_machine_with_nowhere_to_register_says_so_and_carries_no_command() {
        let nothing = AutostartReport {
            mechanism: AutostartMechanism::None,
            location: "no systemd user manager on this machine".to_owned(),
            enabled: false,
            changed: false,
            command: Vec::new(),
            for_this_home: false,
        };

        let encoded = serde_json::to_value(&nothing).unwrap();

        assert_eq!(encoded["mechanism"], "none");
        assert!(
            encoded.get("command").is_none(),
            "the ordinary answer carries no empty list: {encoded}"
        );
        assert_eq!(
            serde_json::from_value::<AutostartReport>(encoded).unwrap(),
            nothing
        );
    }

    /// Every mechanism a client may have to render, spelled the way the wire spells it.
    #[test]
    fn every_mechanism_has_a_name_on_the_wire() {
        for (mechanism, spelled) in [
            (AutostartMechanism::LogonTask, "logon_task"),
            (AutostartMechanism::LaunchAgent, "launch_agent"),
            (AutostartMechanism::SystemdUser, "systemd_user"),
            (AutostartMechanism::None, "none"),
        ] {
            assert_eq!(serde_json::to_value(mechanism).unwrap(), spelled);
            assert_eq!(
                serde_json::from_value::<AutostartMechanism>(serde_json::json!(spelled)).unwrap(),
                mechanism
            );
        }
    }
}
