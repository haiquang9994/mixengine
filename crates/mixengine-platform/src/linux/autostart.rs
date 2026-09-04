//! A systemd **user** unit, wanted by `default.target`.
//!
//! **The unit file is not the registration; the symlink is.** A unit in `~/.config/systemd/user/`
//! starts nothing until something wants it, and what creates the `default.target.wants` link is
//! `systemctl --user enable`. That link could be written by hand — `WantedBy=default.target` fully
//! determines its path — but `systemctl` is the authority on what an `[Install]` section means, and
//! re-deriving that here would be a second implementation of it. So this leg runs a tool where
//! `macos/autostart.rs` runs none: on that system writing the file *is* the registration, and here
//! it is not.
//!
//! **A machine with no user manager is an answer and not a failure**, on
//! [`ResolverMethod::None`](crate::ResolverMethod)'s precedent: a container, a stripped image and a
//! CI runner all have one, and what a person can act on there is a sentence naming the command to
//! run by hand.
//!
//! **`loginctl enable-linger` is deliberately not called.** Without it a systemd user manager stops
//! at logout, which is exactly the lifetime `.claude/architecture/overview.md` states for the
//! daemon: *login → logout*.
//!
//! `Restart=on-failure` and **not** `Restart=always`: `mix daemon stop` must stay stopped. macOS
//! says the same with `KeepAlive: { SuccessfulExit: false }` and Windows with `<RestartOnFailure>`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{AutostartMechanism, AutostartPlan, AutostartState, Error, Result, ServiceInstaller};

/// The unit's file name, named in `.claude/architecture/daemon-and-ipc.md`.
const UNIT: &str = "mixengined.service";

/// The tool. Off the `PATH`, because systemd is not always in the same place and there is no
/// `System32` here to prefer.
const SYSTEMCTL: &str = "systemctl";

/// What a machine with no user manager is told to do instead.
const BY_HAND: &str = "this machine has no systemd user manager, so there is nothing to register an autostart entry \
     with — start the daemon from whatever your session does use, with: mixengined --home";

/// This user's systemd unit directory, and the unit inside it.
#[derive(Debug)]
pub(crate) struct Unit {
    /// `$XDG_CONFIG_HOME/systemd/user`, or nothing when the OS will not say where the config of
    /// this account belongs.
    ///
    /// An `Option` rather than a resolved path, and an absent answer rather than a panic:
    /// `unix/path.rs` holds its `home` the same way and for the same reason.
    directory: Option<PathBuf>,

    /// Which unit to write, enable and read.
    ///
    /// A field rather than the constant, so this crate's own system suite can drive a real
    /// `systemctl` against a unit it creates and removes instead of against the one that decides
    /// whether the person running it has a daemon tomorrow. `windows/autostart.rs` holds its task
    /// name the same way — and here it has to be the *name* rather than only the directory, because
    /// `systemctl --user enable` looks a bare name up in its own search path and would never find a
    /// unit in a directory of a test's own.
    name: String,
}

impl Unit {
    /// The unit directory of the user this process runs as.
    pub(crate) fn of_this_user() -> Self {
        Self {
            directory: directories::BaseDirs::new()
                .map(|base| base.config_dir().join("systemd").join("user")),
            name: UNIT.to_owned(),
        }
    }

    /// A unit in a named directory — for a test that owns one, and drives no `systemctl`.
    #[cfg(test)]
    pub(crate) fn in_directory(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: Some(directory.into()),
            name: UNIT.to_owned(),
        }
    }

    /// A named unit in this user's own directory — for the system suite, which needs a real
    /// `systemctl` to be able to find it.
    #[cfg(test)]
    pub(crate) fn named(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ..Self::of_this_user()
        }
    }

    /// Where the unit goes, or why there is nowhere for it.
    fn unit(&self) -> Result<PathBuf> {
        match &self.directory {
            Some(directory) => Ok(directory.join(&self.name)),
            None => Err(Error::UnsupportedPlatform {
                capability: "ServiceInstaller",
                reason:
                    "this account has no configuration directory, so there is nowhere to put a \
                         systemd user unit"
                        .to_owned(),
            }),
        }
    }

    /// What the registered unit will run, or nothing if there is no unit.
    fn registered(&self) -> Result<Vec<String>> {
        let Ok(unit) = self.unit() else {
            return Ok(Vec::new());
        };

        match std::fs::read_to_string(&unit) {
            Ok(document) => Ok(exec_start(&document)),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(source) => Err(Error::Io {
                action: "read the systemd unit at",
                path: unit,
                source,
            }),
        }
    }

    /// The state to answer with, given what is registered and whether this call wrote it.
    fn reading(
        &self,
        mechanism: AutostartMechanism,
        command: Vec<String>,
        changed: bool,
    ) -> AutostartState {
        AutostartState {
            mechanism,
            location: match self.unit() {
                Ok(unit) => unit.display().to_string(),
                Err(error) => error.to_string(),
            },
            enabled: !command.is_empty(),
            changed,
            command,
        }
    }
}

impl ServiceInstaller for Unit {
    fn enable(&self, plan: &AutostartPlan) -> Result<AutostartState> {
        if probe() == AutostartMechanism::None {
            return Err(Error::UnsupportedPlatform {
                capability: "ServiceInstaller",
                reason: format!("{BY_HAND} {}", plan.home.display()),
            });
        }

        let unit = self.unit()?;
        let wanted = document(plan);
        let already = std::fs::read_to_string(&unit).is_ok_and(|already| already == wanted);

        if !already {
            let directory = unit.parent().unwrap_or(Path::new("."));

            std::fs::create_dir_all(directory).map_err(|source| Error::Io {
                action: "create the systemd user unit directory",
                path: directory.to_path_buf(),
                source,
            })?;

            write_atomically(&unit, &wanted)?;
        }

        // **Both, and in this order, on every call rather than only when the file changed.** A unit
        // that is on disk and not linked starts nothing, which is the state a half-finished earlier
        // call leaves — so an `enable` that found the file already right still makes sure the link
        // is there. `daemon-reload` first, or `enable` acts on a unit this manager has not read.
        run(&["--user", "daemon-reload"])?;
        run(&["--user", "enable", &self.name])?;

        Ok(self.reading(
            AutostartMechanism::SystemdUser,
            exec_start(&wanted),
            !already,
        ))
    }

    fn disable(&self) -> Result<AutostartState> {
        let unit = self.unit()?;
        let mechanism = probe();

        if mechanism != AutostartMechanism::None {
            // Best effort: a unit that was never enabled is not an error, and `systemctl` says so
            // with a non-zero exit that carries nothing this caller can act on.
            drop(run(&["--user", "disable", &self.name]));
        }

        let changed = match std::fs::remove_file(&unit) {
            Ok(()) => true,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => false,
            Err(source) => {
                return Err(Error::Io {
                    action: "remove the systemd unit at",
                    path: unit,
                    source,
                });
            }
        };

        if mechanism != AutostartMechanism::None {
            // So systemd forgets the unit rather than reporting it as `not-found` for the rest of
            // this session.
            drop(run(&["--user", "daemon-reload"]));
        }

        Ok(self.reading(mechanism, Vec::new(), changed))
    }

    fn state(&self) -> Result<AutostartState> {
        Ok(self.reading(probe(), self.registered()?, false))
    }
}

/// Whether this machine has a systemd user manager to register anything with.
///
/// **Any exit status means yes.** `systemctl --user is-system-running` answers `degraded` and exits
/// non-zero on a perfectly ordinary machine where one unit failed, and that machine can still start
/// a daemon at login. What says no is the tool not being there at all, or a manager that cannot be
/// reached — which systemd reports by name, on a bus it could not connect to.
fn probe() -> AutostartMechanism {
    let Ok(output) = Command::new(SYSTEMCTL)
        .args(["--user", "is-system-running"])
        .output()
    else {
        return AutostartMechanism::None;
    };

    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    match said.contains("Failed to connect to bus") || said.contains("offline") {
        true => AutostartMechanism::None,
        false => AutostartMechanism::SystemdUser,
    }
}

/// Run `systemctl` with an argument vector and nothing interpolated.
fn run(args: &[&str]) -> Result<()> {
    let output = Command::new(SYSTEMCTL)
        .args(args)
        .output()
        .map_err(|source| Error::Command {
            command: SYSTEMCTL,
            path: None,
            status: "could not be started".to_owned(),
            output: source.to_string(),
        })?;

    if !output.status.success() {
        return Err(Error::Command {
            command: SYSTEMCTL,
            path: None,
            status: output.status.to_string(),
            output: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    Ok(())
}

/// The unit, as systemd's own syntax spells it.
fn document(plan: &AutostartPlan) -> String {
    let exec: Vec<String> = command(plan).iter().map(|word| quote(word)).collect();

    format!(
        "[Unit]\n\
         Description=MixEngine daemon\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        exec.join(" ")
    )
}

/// The words a unit written from `plan` will run.
fn command(plan: &AutostartPlan) -> Vec<String> {
    vec![
        plan.program.display().to_string(),
        "--home".to_owned(),
        plan.home.display().to_string(),
    ]
}

/// One word, as systemd reads one.
///
/// Quoted only when it has to be, because a unit a person opens should look like the command they
/// would have typed. Inside quotes systemd takes `\` and `"` as escapes, so both are escaped here.
fn quote(word: &str) -> String {
    if !word.is_empty()
        && !word
            .chars()
            .any(|character| character.is_whitespace() || character == '"' || character == '\\')
    {
        return word.to_owned();
    }

    format!("\"{}\"", word.replace('\\', r"\\").replace('"', "\\\""))
}

/// The `ExecStart=` of a unit, as the words it will run.
///
/// A document with no such line reads as no command rather than as a failure: that is what an
/// unrelated file and a truncated one both are, and neither is worth a panic in a status.
fn exec_start(document: &str) -> Vec<String> {
    document
        .lines()
        .find_map(|line| line.strip_prefix("ExecStart="))
        .map(split)
        .unwrap_or_default()
}

/// One `ExecStart=` value, as the words [`quote`] made it out of.
fn split(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut started = false;

    for character in value.chars() {
        match character {
            _ if escaped => {
                word.push(character);
                escaped = false;
            }
            '\\' if quoted => escaped = true,
            '"' => {
                quoted = !quoted;
                started = true;
            }
            character if character.is_whitespace() && !quoted => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            character => {
                word.push(character);
                started = true;
            }
        }
    }

    if started || !word.is_empty() {
        words.push(word);
    }

    words
}

/// A temporary in the same directory, then a rename.
///
/// `.claude/architecture/platform-abstraction.md`'s second rule. A unit half written is one systemd
/// refuses at the next login, and the machine that would produce it is the one that lost power in
/// the middle of `mix autostart enable`.
fn write_atomically(path: &Path, contents: &str) -> Result<()> {
    let temporary = path.with_extension(format!("service.{}.tmp", std::process::id()));

    std::fs::write(&temporary, contents).map_err(|source| Error::Io {
        action: "write the systemd unit to",
        path: temporary.clone(),
        source,
    })?;

    std::fs::rename(&temporary, path).map_err(|source| {
        drop(std::fs::remove_file(&temporary));

        Error::Io {
            action: "put the systemd unit at",
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> AutostartPlan {
        AutostartPlan {
            program: PathBuf::from("/usr/bin/mixengined"),
            home: PathBuf::from("/home/me/.local/share/mixengine"),
        }
    }

    #[test]
    fn the_document_reads_back_as_the_command_it_was_written_from() {
        assert_eq!(exec_start(&document(&plan())), command(&plan()));
    }

    #[test]
    fn a_home_with_a_space_in_it_is_one_word_both_ways() {
        let awkward = AutostartPlan {
            home: PathBuf::from("/home/me/My Home"),
            ..plan()
        };

        let unit = document(&awkward);

        assert!(
            unit.contains(r#"ExecStart=/usr/bin/mixengined --home "/home/me/My Home""#),
            "{unit}"
        );
        assert_eq!(exec_start(&unit), command(&awkward));
    }

    #[test]
    fn a_word_that_needs_no_quotes_does_not_get_them() {
        assert_eq!(quote("--home"), "--home");
        assert_eq!(quote("/usr/bin/mixengined"), "/usr/bin/mixengined");
        assert_eq!(quote("a b"), r#""a b""#);
        assert_eq!(quote(r#"a"b"#), r#""a\"b""#);
        assert_eq!(quote(r"a\b"), r#""a\\b""#);
        assert_eq!(quote(""), r#""""#);
    }

    #[test]
    fn a_quoted_word_round_trips_through_the_split() {
        for word in [
            "plain",
            "with a space",
            r#"with a " quote"#,
            r"with a \ backslash",
        ] {
            assert_eq!(split(&quote(word)), vec![word.to_owned()], "{word}");
        }
    }

    /// The setting that keeps `mix daemon stop` stopped.
    #[test]
    fn the_unit_restarts_on_failure_and_never_unconditionally() {
        let unit = document(&plan());

        assert!(unit.contains("Restart=on-failure"), "{unit}");
        assert!(!unit.contains("Restart=always"), "{unit}");
        assert!(unit.contains("WantedBy=default.target"), "{unit}");
    }

    #[test]
    fn an_unrelated_file_reads_as_no_command_rather_than_panicking() {
        assert!(exec_start("[Unit]\nDescription=something else\n").is_empty());
        assert!(exec_start("").is_empty());
    }

    /// The file half of the cycle, against a directory the test owns.
    ///
    /// **No `systemctl` here**: `enable` runs one, so this exercises what it writes rather than what
    /// it registers. The whole cycle against a real manager is the system suite's, gated on
    /// `MIXENGINE_SYSTEM_TESTS`.
    #[test]
    fn a_unit_that_is_written_reads_back_and_is_removed_again() {
        let directory = tempfile::TempDir::new().expect("a temporary directory");
        let unit = Unit::in_directory(directory.path().join("systemd/user"));

        assert!(
            !unit
                .state()
                .expect("a status on an empty directory")
                .enabled
        );

        std::fs::create_dir_all(directory.path().join("systemd/user")).expect("the directory");
        write_atomically(&unit.unit().unwrap(), &document(&plan())).expect("write");

        let state = unit.state().expect("read");
        assert!(state.enabled);
        assert_eq!(state.command, command(&plan()));

        std::fs::remove_file(unit.unit().unwrap()).expect("remove");
        assert!(!unit.state().expect("read again").enabled);
    }

    /// What this machine actually answers, read-only.
    ///
    /// **No `#[ignore]`, because nothing is written**: reading a unit that is not there touches
    /// nothing, and what is asserted is the shape rather than whether this account has an entry —
    /// that is a fact about the account and not about the code.
    #[test]
    fn this_user_s_unit_is_named_under_their_own_configuration_directory() {
        let unit = Unit::of_this_user();
        let state = unit
            .state()
            .expect("a status never fails for want of a mechanism");

        assert!(
            matches!(
                state.mechanism,
                AutostartMechanism::SystemdUser | AutostartMechanism::None
            ),
            "{state:?}"
        );
        assert!(state.location.ends_with(UNIT), "{state:?}");
        assert!(!state.changed, "a status never claims a write");
    }

    /// The whole cycle against a **real** `systemctl`, under a unit name nobody's daemon depends on.
    ///
    /// `#[ignore]` **and** `MIXENGINE_SYSTEM_TESTS`, per `.claude/standards/testing.md` rule 1: this
    /// writes the systemd configuration of whoever runs it.
    ///
    /// **Two branches, and each asserts something different.** A machine with a user manager has to
    /// register, read back, and disappear; a machine without one has to refuse with a reason
    /// somebody can act on. A test that passed by doing nothing on the second kind would be worse
    /// than none, which is why the machine it took is printed.
    #[test]
    #[ignore = "writes a real systemd user unit on this machine"]
    fn a_real_unit_registers_reads_back_and_disappears() {
        if std::env::var_os("MIXENGINE_SYSTEM_TESTS").is_none() {
            eprintln!("skipped: MIXENGINE_SYSTEM_TESTS is not set");
            return;
        }

        let unit = Unit::named("mixengine-system-suite.service");
        drop(unit.disable());

        let plan = plan();

        if probe() == AutostartMechanism::None {
            eprintln!("this machine has no systemd user manager; asserting the refusal instead");

            let refused = unit
                .enable(&plan)
                .expect_err("a machine with no mechanism refuses");
            assert!(
                refused.to_string().contains("mixengined --home"),
                "the refusal has to name the command to run by hand: {refused}"
            );
            assert_eq!(
                unit.state().expect("a status still answers").mechanism,
                AutostartMechanism::None
            );
            return;
        }

        let enabled = unit.enable(&plan).expect("register");
        assert!(enabled.enabled && enabled.changed, "{enabled:?}");
        assert_eq!(enabled.command, command(&plan), "{enabled:?}");

        let again = unit.enable(&plan).expect("register again");
        assert!(
            !again.changed,
            "the second enable rewrote the unit: {again:?}"
        );

        let disabled = unit.disable().expect("remove");
        assert!(!disabled.enabled && disabled.changed, "{disabled:?}");
        assert!(
            !unit.state().expect("read").enabled,
            "the unit outlived its disable"
        );
    }

    /// `.claude/standards/testing.md` rule 4: nothing outside the entry is touched.
    #[test]
    fn a_neighbouring_unit_is_left_byte_for_byte_alone() {
        let directory = tempfile::TempDir::new().expect("a temporary directory");
        let units = directory.path().join("systemd/user");
        std::fs::create_dir_all(&units).expect("the directory");

        let neighbour = units.join("someone-else.service");
        std::fs::write(&neighbour, "someone else's unit\n").expect("the neighbour");

        let unit = Unit::in_directory(&units);
        write_atomically(&unit.unit().unwrap(), &document(&plan())).expect("write");
        drop(unit.disable());

        assert_eq!(
            std::fs::read_to_string(&neighbour).expect("the neighbour survived"),
            "someone else's unit\n"
        );
    }
}
