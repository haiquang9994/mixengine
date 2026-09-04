//! Whether this machine starts a daemon for this home at login — the only door onto it. Roadmap
//! task **T85b**.
//!
//! **Written only when somebody asks**, which is [`crate::shims`]' rule and the same one: a logon
//! task, a LaunchAgent and a systemd user unit are all outside `MIXENGINE_HOME`, and
//! `.claude/architecture/daemon-and-ipc.md` has no method that writes there on the daemon's own
//! initiative. `path.*` is the first of them and `autostart.*` is the second. So nothing here is
//! called at start-up, and this type holds no state of its own to refresh.
//!
//! **`enable` registers and does not start; `disable` removes and does not stop.** There is a daemon
//! running by the time either can be called — it is the one answering the call — and a person who
//! turned off "start at login" must not lose it.
//!
//! **`for_this_home` is composed here and not by a client.** There is one entry per user, so a
//! second home enabling replaces it; a client folding "is this entry mine" for itself would be
//! business logic in a client and a place for two clients to disagree.

use std::path::PathBuf;
use std::sync::Arc;

use mixengine_platform::{AutostartPlan, AutostartState, Host};
use mixengine_proto::{AutostartMechanism, AutostartReport, Error};

use crate::error::ToWire as _;

/// The daemon this machine would start at login, and the machine that would start it.
#[derive(Debug)]
pub(crate) struct Autostart {
    /// The binary an entry names: this process's own image.
    ///
    /// The same value [`crate::shims::Shims`] is given, and taken once in `main` rather than read
    /// again here — a daemon that answered with a different path from the one it registered would
    /// be reporting about a program it is not.
    program: PathBuf,

    /// The home an entry names, as `--home`.
    root: PathBuf,

    /// The OS.
    host: Arc<dyn Host>,
}

impl Autostart {
    pub(crate) fn new(program: PathBuf, root: PathBuf, host: Arc<dyn Host>) -> Self {
        Self {
            program,
            root,
            host,
        }
    }

    /// `autostart.status` — what this machine would do at the next login.
    pub(crate) fn status(&self) -> Result<AutostartReport, Error> {
        let state = self
            .host
            .service_installer()
            .state()
            .map_err(|error| error.to_wire())?;

        Ok(self.report(state))
    }

    /// `autostart.enable` — register the entry, replacing whatever was there.
    pub(crate) fn enable(&self) -> Result<AutostartReport, Error> {
        let plan = AutostartPlan {
            program: self.program.clone(),
            home: self.root.clone(),
        };

        let state = self
            .host
            .service_installer()
            .enable(&plan)
            .map_err(|error| error.to_wire())?;

        Ok(self.report(state))
    }

    /// `autostart.disable` — take it away, and leave the running daemon alone.
    pub(crate) fn disable(&self) -> Result<AutostartReport, Error> {
        let state = self
            .host
            .service_installer()
            .disable()
            .map_err(|error| error.to_wire())?;

        Ok(self.report(state))
    }

    /// The wire answer, with the one thing a client may not work out for itself.
    ///
    /// **Compared on the `--home` the entry carries and never on the program.** An update moves
    /// `mixengined` and leaves the home exactly where it was, so a comparison on the binary's path
    /// would call an entry somebody else's the day after an update — which is the opposite of what
    /// this field is for.
    fn report(&self, state: AutostartState) -> AutostartReport {
        let named = state
            .command
            .iter()
            .skip_while(|word| *word != "--home")
            .nth(1)
            .map(PathBuf::from);

        AutostartReport {
            mechanism: mechanism(state.mechanism),
            location: state.location,
            enabled: state.enabled,
            changed: state.changed,
            command: state.command,
            for_this_home: named.is_some_and(|home| home == self.root),
        }
    }
}

/// The platform's answer, in the wire's vocabulary.
///
/// Two enums rather than one, because `mixengine-platform` may not depend on the wire types and the
/// wire may not depend on the platform — the arrangement every other capability here has.
fn mechanism(mechanism: mixengine_platform::AutostartMechanism) -> AutostartMechanism {
    match mechanism {
        mixengine_platform::AutostartMechanism::LogonTask => AutostartMechanism::LogonTask,
        mixengine_platform::AutostartMechanism::LaunchAgent => AutostartMechanism::LaunchAgent,
        mixengine_platform::AutostartMechanism::SystemdUser => AutostartMechanism::SystemdUser,
        mixengine_platform::AutostartMechanism::None => AutostartMechanism::None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use mixengine_platform::mock;

    use super::*;

    fn autostart(root: &Path) -> Autostart {
        Autostart::new(
            PathBuf::from("/usr/bin/mixengined"),
            root.to_path_buf(),
            Arc::new(mock::Host::with_home(root)),
        )
    }

    #[test]
    fn an_entry_this_daemon_registered_is_this_home_s() {
        let autostart = autostart(Path::new("/tmp/mine"));

        let enabled = autostart.enable().expect("register");

        assert!(enabled.enabled);
        assert!(enabled.changed);
        assert!(enabled.for_this_home);
    }

    /// The half-state the field exists to name: an entry that is there and is somebody else's.
    #[test]
    fn an_entry_naming_another_home_is_registered_and_not_this_home_s() {
        let host = Arc::new(mock::Host::with_home("/tmp/mine"));

        // Registered from the other home, through the same machine.
        let elsewhere = Autostart::new(
            PathBuf::from("/usr/bin/mixengined"),
            PathBuf::from("/tmp/theirs"),
            Arc::clone(&host) as Arc<dyn Host>,
        );
        elsewhere.enable().expect("register");

        let mine = Autostart::new(
            PathBuf::from("/usr/bin/mixengined"),
            PathBuf::from("/tmp/mine"),
            host as Arc<dyn Host>,
        );

        let status = mine.status().expect("read");

        assert!(status.enabled);
        assert!(!status.for_this_home);
        assert!(!status.changed, "a status never claims a write");
    }

    #[test]
    fn nothing_registered_is_not_this_home_s_either() {
        let status = autostart(Path::new("/tmp/mine")).status().expect("read");

        assert!(!status.enabled);
        assert!(!status.for_this_home);
        assert!(status.command.is_empty());
    }

    #[test]
    fn a_machine_with_no_mechanism_answers_a_status_and_refuses_an_enable() {
        let root = PathBuf::from("/tmp/mine");
        let autostart = Autostart::new(
            PathBuf::from("/usr/bin/mixengined"),
            root.clone(),
            Arc::new(mock::Host::without_an_autostart_mechanism(root)),
        );

        assert_eq!(
            autostart.status().expect("a status reports").mechanism,
            AutostartMechanism::None
        );

        let refused = autostart.enable().expect_err("an enable refuses");
        assert_eq!(
            refused.code,
            mixengine_proto::ErrorCode::UnsupportedPlatform
        );
    }
}
