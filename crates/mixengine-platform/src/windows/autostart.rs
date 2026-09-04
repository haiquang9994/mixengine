//! A Task Scheduler logon task, registered through `schtasks`.
//!
//! # Why a tool and not the API
//!
//! `.claude/architecture/platform-abstraction.md` rule 5 prefers a Windows API to a shell-out, and
//! the Task Scheduler's is COM — `ITaskService`, `ITaskDefinition`, `IRegisteredTask`. This
//! workspace depends on `windows-sys`, which is raw FFI with no COM support, so reaching it would
//! mean hand-written vtable calls and `IUnknown` reference counting for an operation that runs when
//! somebody types a command. `DirectoryAccess`'s `icacls` is the standing exception for exactly this
//! trade, and this is the second instance of it.
//!
//! **What that rule is really about is kept**: the task is described in a *file*, so no path ever
//! reaches a command line. `/SC ONLOGON /TR "<command line>"` would put a path containing a space
//! through two levels of quoting, which is the bug class this project refuses everywhere else — the
//! T40 design, D9. Inside the document the program and its arguments are separate elements, and
//! everything interpolated goes through [`escape`].
//!
//! # The settings that are not defaults, and what each costs if left alone
//!
//! - `DisallowStartIfOnBatteries` defaults to **true**: a laptop that logged in on battery would
//!   never start the daemon. This is the single most likely way this feature would be reported as
//!   "does not work".
//! - `ExecutionTimeLimit` defaults to **three days**: a long-running process under the default is
//!   one Task Scheduler eventually kills.
//! - `MultipleInstancesPolicy` is `IgnoreNew`, so the task never produces a second daemon for the
//!   single-instance lock to refuse.
//! - `RestartOnFailure` and **not** an unconditional restart, so `mix daemon stop` stays stopped.
//!
//! # `<Hidden>` is not the answer to the console window, and neither is this file
//!
//! Measured on Windows 11 Pro 26200, 2026-09-04: a console program run by Task Scheduler under
//! `InteractiveToken` gets a console window, `IsWindowVisible` says `true`, in the user's own
//! session — and the identical task with `<Hidden>true</Hidden>` reported exactly the same, because
//! that element hides the *task* in the Task Scheduler UI and says nothing about a process's
//! windows. What answers it is [`crate::process::release_unattended_console`], inside the daemon
//! this task starts.

use std::ffi::OsStr;
use std::path::PathBuf;

use super::{command, sid};
use crate::{AutostartMechanism, AutostartPlan, AutostartState, Error, Result, ServiceInstaller};

/// The task's name in the Task Scheduler library.
///
/// At the root and not inside a folder of its own: there is one entry per user, and `schtasks` has
/// no way to delete an empty folder — a folder would be a leftover `mix uninstall` could not remove.
const TASK: &str = "MixEngine";

/// The tool, resolved out of `System32` by [`command::system32`] rather than off the `PATH`.
const SCHTASKS: &str = "schtasks";

/// This user's logon task, and the name it is filed under.
#[derive(Debug)]
pub(crate) struct Logon {
    /// Which task to write and read.
    ///
    /// A field rather than the constant, so this crate's own system suite can drive the real tool
    /// against a name it creates and deletes instead of against the entry that decides whether the
    /// person running it has a daemon tomorrow. `windows/path.rs`'s `key` is the same arrangement
    /// for the same reason.
    task: String,
}

impl Logon {
    /// The logon task of the user this process runs as.
    pub(crate) fn of_this_user() -> Self {
        Self {
            task: TASK.to_owned(),
        }
    }

    /// A named task that is nobody's — for the suite that has to register a real one.
    #[cfg(test)]
    pub(crate) fn named(task: &str) -> Self {
        Self {
            task: task.to_owned(),
        }
    }

    /// What the registered task will run, or nothing if there is no such task.
    ///
    /// **A non-zero exit is the ordinary case and not a failure**: `schtasks /Query` on a task that
    /// is not there says "the system cannot find the file specified", which is the answer "nothing
    /// is registered". So this asks with [`command::output_of`] and reads whatever came back.
    ///
    /// `schtasks` writes its output in the console codepage rather than in UTF-8, so a home whose
    /// path is not ASCII may not round-trip. The consequence is bounded: the comparison in
    /// [`Logon::enable`] then rewrites a task that was already correct, which is a needless write
    /// and never a wrong answer.
    fn registered(&self) -> Result<Vec<String>> {
        let queried = command::output_of(
            SCHTASKS,
            [
                OsStr::new("/Query"),
                OsStr::new("/TN"),
                OsStr::new(&self.task),
                OsStr::new("/XML"),
                OsStr::new("ONE"),
            ],
        )?;

        Ok(words(&queried))
    }

    /// The state to answer with, given what is registered and whether this call wrote it.
    fn reading(&self, command: Vec<String>, changed: bool) -> AutostartState {
        AutostartState {
            mechanism: AutostartMechanism::LogonTask,
            location: format!(r"Task Scheduler library \{}", self.task),
            enabled: !command.is_empty(),
            changed,
            command,
        }
    }
}

impl ServiceInstaller for Logon {
    fn enable(&self, plan: &AutostartPlan) -> Result<AutostartState> {
        let wanted = expected(plan);

        if self.registered()? == wanted {
            return Ok(self.reading(wanted, false));
        }

        let user = sid::current_user()?;
        let file = write_document(&document(plan, &user))?;

        let registered = command::run(
            SCHTASKS,
            None,
            [
                OsStr::new("/Create"),
                OsStr::new("/TN"),
                OsStr::new(&self.task),
                OsStr::new("/XML"),
                file.as_os_str(),
                OsStr::new("/F"),
            ],
        );

        // Best effort, and after the tool has read it: the file holds a path and not a secret, and a
        // temporary left behind is worth less than an error that replaced the one above.
        drop(std::fs::remove_file(&file));

        registered?;

        Ok(self.reading(self.registered()?, true))
    }

    fn disable(&self) -> Result<AutostartState> {
        if self.registered()?.is_empty() {
            return Ok(self.reading(Vec::new(), false));
        }

        command::run(
            SCHTASKS,
            None,
            [
                OsStr::new("/Delete"),
                OsStr::new("/TN"),
                OsStr::new(&self.task),
                OsStr::new("/F"),
            ],
        )?;

        Ok(self.reading(Vec::new(), true))
    }

    fn state(&self) -> Result<AutostartState> {
        Ok(self.reading(self.registered()?, false))
    }
}

/// The words a task registered from `plan` will run.
///
/// The one place the command is composed, so that the comparison in `enable` and the answer a client
/// renders cannot drift apart.
fn expected(plan: &AutostartPlan) -> Vec<String> {
    vec![
        plan.program.display().to_string(),
        "--home".to_owned(),
        plan.home.display().to_string(),
    ]
}

/// The task, as Task Scheduler's own schema spells it.
///
/// The trigger and the principal name a **SID** rather than a display name: `sid::current_user` is
/// already how this crate identifies an account, a display name is localised and can be changed, and
/// two accounts on two machines can share one.
fn document(plan: &AutostartPlan, user: &str) -> String {
    let user = escape(user);

    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Starts the MixEngine daemon when this user logs in.</Description>
    <URI>\{task}</URI>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <Enabled>true</Enabled>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <AllowHardTerminate>true</AllowHardTerminate>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>3</Count>
    </RestartOnFailure>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{program}</Command>
      <Arguments>--home "{home}"</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        task = escape(TASK),
        program = escape(&plan.program.display().to_string()),
        home = escape(&plan.home.display().to_string()),
    )
}

/// The document on disk, as UTF-16LE with a byte-order mark.
///
/// **The encoding Microsoft documents for `/XML`.** A UTF-8 file was accepted by `schtasks` on the
/// machine this was measured on, and that is not relied on: a file whose declaration says
/// `encoding="UTF-16"` and whose bytes are not is a coincidence rather than a contract.
fn write_document(document: &str) -> Result<PathBuf> {
    let file = std::env::temp_dir().join(format!("mixengine-autostart-{}.xml", std::process::id()));

    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(
        document
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes().into_iter()),
    );

    std::fs::write(&file, &bytes).map_err(|source| Error::Io {
        action: "write the task document to",
        path: file.clone(),
        source,
    })?;

    Ok(file)
}

/// The five entities an XML document may not carry raw.
///
/// `&` first: replacing it after the others would escape the ampersands they just introduced.
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The same five, on the way back out.
fn unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The command a queried task will run: the program, then its arguments as separate words.
///
/// A document with no `<Exec>` reads as no command rather than as a failure — that is what a machine
/// with no such task returns, and it is the ordinary case.
fn words(queried: &str) -> Vec<String> {
    let Some(program) = element(queried, "Command") else {
        return Vec::new();
    };

    let mut command = vec![unescape(program.trim())];

    if let Some(arguments) = element(queried, "Arguments") {
        command.extend(split(&unescape(arguments.trim())));
    }

    command
}

/// The text of the first `<name>…</name>` in `document`.
fn element<'a>(document: &'a str, name: &str) -> Option<&'a str> {
    let opened = document.find(&format!("<{name}>"))? + name.len() + 2;
    let closed = document[opened..].find(&format!("</{name}>"))?;

    Some(&document[opened..opened + closed])
}

/// One argument string, as the words a command line of it would be taken apart into.
///
/// Only double quotes, which is all [`document`] ever writes and all Windows itself treats as
/// grouping. Nothing here has to be a general command-line parser: what it reads back is what this
/// module wrote.
fn split(arguments: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;

    for character in arguments.chars() {
        match character {
            '"' => quoted = !quoted,
            character if character.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            character => word.push(character),
        }
    }

    if !word.is_empty() {
        words.push(word);
    }

    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> AutostartPlan {
        AutostartPlan {
            program: PathBuf::from(r"C:\Users\me\mixengined.exe"),
            home: PathBuf::from(r"C:\Users\me\AppData\Local\MixEngine"),
        }
    }

    #[test]
    fn a_path_with_a_space_and_an_ampersand_survives_the_document() {
        let plan = AutostartPlan {
            program: PathBuf::from(r"C:\Program Files\Fish & Chips\mixengined.exe"),
            home: PathBuf::from(r"C:\Users\me\App<Data>\MixEngine"),
        };

        let xml = document(&plan, "S-1-5-21-1-2-3-1001");

        assert!(
            xml.contains(r"C:\Program Files\Fish &amp; Chips\mixengined.exe"),
            "{xml}"
        );
        assert!(xml.contains(r"App&lt;Data&gt;"), "{xml}");
        assert!(
            !xml.contains("Fish & Chips"),
            "the raw ampersand is still in it: {xml}"
        );
    }

    /// The four settings whose defaults would each stop a daemon in a different way.
    #[test]
    fn the_document_overrides_every_default_that_would_stop_a_daemon() {
        let xml = document(&plan(), "S-1-5-21-1-2-3-1001");

        assert!(xml.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(xml.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"));
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(xml.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"));
    }

    #[test]
    fn the_trigger_and_the_principal_name_the_sid_and_never_a_display_name() {
        let xml = document(&plan(), "S-1-5-21-1-2-3-1001");

        assert_eq!(xml.matches("S-1-5-21-1-2-3-1001").count(), 2, "{xml}");
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(xml.contains("<RunLevel>LeastPrivilege</RunLevel>"));
    }

    /// What `enable` compares against is what a query of its own document would return.
    #[test]
    fn the_document_reads_back_as_the_command_it_was_written_from() {
        let plan = AutostartPlan {
            program: PathBuf::from(r"C:\Users\me\mixengined.exe"),
            home: PathBuf::from(r"C:\Users\me\My Home"),
        };

        assert_eq!(
            words(&document(&plan, "S-1-5-21-1-2-3-1001")),
            expected(&plan)
        );
    }

    #[test]
    fn a_document_with_no_action_reads_as_no_command_rather_than_panicking() {
        assert!(words("<Task><Actions/></Task>").is_empty());
        assert!(words("").is_empty());
        assert!(words("<Command>").is_empty(), "an unterminated element");
    }

    #[test]
    fn an_argument_in_quotes_is_one_word_and_the_quotes_are_not_part_of_it() {
        assert_eq!(
            split(r#"--home "C:\Users\me\My Home""#),
            vec!["--home".to_owned(), r"C:\Users\me\My Home".to_owned()]
        );
        assert!(split("   ").is_empty());
    }

    /// A real task, created and deleted, under a name nobody's daemon depends on.
    ///
    /// `#[ignore]` **and** `MIXENGINE_SYSTEM_TESTS`, per `.claude/standards/testing.md` rule 1: this
    /// writes the Task Scheduler library of whoever runs it.
    #[test]
    #[ignore = "registers a real logon task on this machine"]
    fn a_real_task_registers_reads_back_and_disappears() {
        if std::env::var_os("MIXENGINE_SYSTEM_TESTS").is_none() {
            eprintln!("skipped: MIXENGINE_SYSTEM_TESTS is not set");
            return;
        }

        let logon = Logon::named("MixEngineSystemSuite");
        drop(logon.disable());

        let plan = plan();
        let enabled = logon.enable(&plan).expect("register");
        assert!(enabled.enabled && enabled.changed, "{enabled:?}");
        assert_eq!(enabled.command, expected(&plan), "{enabled:?}");

        let again = logon.enable(&plan).expect("register again");
        assert!(!again.changed, "the second enable wrote: {again:?}");

        let disabled = logon.disable().expect("remove");
        assert!(!disabled.enabled && disabled.changed, "{disabled:?}");
        assert!(
            !logon.state().expect("read").enabled,
            "the task outlived its disable"
        );
    }
}
