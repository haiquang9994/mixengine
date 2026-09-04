//! A LaunchAgent in this user's `~/Library/LaunchAgents`.
//!
//! **Writing the file is the registration, and `launchctl` is called for nothing.** `loginwindow`
//! bootstraps this user's LaunchAgents domain from that directory at every login, so a plist there
//! *is* an agent that starts at login. There is nothing to `bootstrap`, because `enable`
//! deliberately does not start the daemon; and there must be nothing to `bootout`, because that
//! terminates the running job — a person who turned off "start at login" must not lose the daemon
//! they are using. It also means this leg needs no session of any kind, which an SSH-only Mac and
//! some CI runners do not have.
//!
//! `KeepAlive` is `{ SuccessfulExit: false }` and **not** `true`: `mix daemon stop` must not be
//! undone half a second later by the thing that starts the daemon at login. `Restart=on-failure` on
//! Linux and `<RestartOnFailure>` on Windows say the same in their own vocabulary.
//!
//! **No `StandardOutPath` or `StandardErrorPath`.** The daemon says everything it has to say in
//! `<root>/logs/daemon.log`, and a second file nothing reads would be one more thing to rotate.

use std::path::{Path, PathBuf};

use crate::{AutostartMechanism, AutostartPlan, AutostartState, Error, Result, ServiceInstaller};

/// The agent's label, which is also the plist's file name.
///
/// Named in `.claude/architecture/daemon-and-ipc.md`, and reversed-domain as launchd expects.
const LABEL: &str = "dev.mixengine.daemon";

/// This user's LaunchAgents directory, and the agent inside it.
#[derive(Debug)]
pub(crate) struct Agent {
    /// `~/Library/LaunchAgents`, or nothing when the OS will not say where this account's home is.
    ///
    /// Held as an `Option` rather than resolved on every call, and as an absent answer rather than a
    /// panic: a daemon running as an account with no home has nowhere to put an agent, and the
    /// answer to "start at login" there is a sentence — `unix/path.rs` holds its `home` the same
    /// way and for the same reason.
    directory: Option<PathBuf>,
}

impl Agent {
    /// The LaunchAgents directory of the user this process runs as.
    pub(crate) fn of_this_user() -> Self {
        Self {
            directory: directories::BaseDirs::new()
                .map(|base| base.home_dir().join("Library").join("LaunchAgents")),
        }
    }

    /// An agent in a named directory — for a test that owns one.
    #[cfg(test)]
    pub(crate) fn in_directory(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: Some(directory.into()),
        }
    }

    /// Where the plist goes, or why there is nowhere for it.
    fn plist(&self) -> Result<PathBuf> {
        match &self.directory {
            Some(directory) => Ok(directory.join(format!("{LABEL}.plist"))),
            None => Err(Error::UnsupportedPlatform {
                capability: "ServiceInstaller",
                reason:
                    "this account has no home directory, so there is no ~/Library/LaunchAgents \
                         to put an agent in"
                        .to_owned(),
            }),
        }
    }

    /// What the registered agent will run, or nothing if there is no agent.
    fn registered(&self) -> Result<Vec<String>> {
        let Ok(plist) = self.plist() else {
            return Ok(Vec::new());
        };

        match std::fs::read_to_string(&plist) {
            Ok(document) => Ok(program_arguments(&document)),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(source) => Err(Error::Io {
                action: "read the launch agent at",
                path: plist,
                source,
            }),
        }
    }

    /// The state to answer with, given what is registered and whether this call wrote it.
    fn reading(&self, command: Vec<String>, changed: bool) -> AutostartState {
        AutostartState {
            mechanism: AutostartMechanism::LaunchAgent,
            location: match self.plist() {
                Ok(plist) => plist.display().to_string(),
                Err(error) => error.to_string(),
            },
            enabled: !command.is_empty(),
            changed,
            command,
        }
    }
}

impl ServiceInstaller for Agent {
    fn enable(&self, plan: &AutostartPlan) -> Result<AutostartState> {
        let plist = self.plist()?;
        let wanted = document(plan);

        if std::fs::read_to_string(&plist).is_ok_and(|already| already == wanted) {
            return Ok(self.reading(program_arguments(&wanted), false));
        }

        let directory = plist.parent().unwrap_or(Path::new("."));

        std::fs::create_dir_all(directory).map_err(|source| Error::Io {
            action: "create the launch agents directory",
            path: directory.to_path_buf(),
            source,
        })?;

        write_atomically(&plist, &wanted)?;

        Ok(self.reading(program_arguments(&wanted), true))
    }

    fn disable(&self) -> Result<AutostartState> {
        let plist = self.plist()?;

        match std::fs::remove_file(&plist) {
            Ok(()) => Ok(self.reading(Vec::new(), true)),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                Ok(self.reading(Vec::new(), false))
            }
            Err(source) => Err(Error::Io {
                action: "remove the launch agent at",
                path: plist,
                source,
            }),
        }
    }

    fn state(&self) -> Result<AutostartState> {
        Ok(self.reading(self.registered()?, false))
    }
}

/// The agent, as launchd's own schema spells it.
fn document(plan: &AutostartPlan) -> String {
    let arguments: String = command(plan)
        .iter()
        .map(|word| format!("      <string>{}</string>\n", escape(word)))
        .collect();

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
{arguments}    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
      <key>SuccessfulExit</key>
      <false/>
    </dict>
  </dict>
</plist>
"#
    )
}

/// The words an agent written from `plan` will run.
///
/// The one place the command is composed, so the comparison in `enable` and the answer a client
/// renders cannot drift apart.
fn command(plan: &AutostartPlan) -> Vec<String> {
    vec![
        plan.program.display().to_string(),
        "--home".to_owned(),
        plan.home.display().to_string(),
    ]
}

/// The `ProgramArguments` of a plist, in order.
///
/// A document with no such array reads as no command rather than as a failure: that is what an
/// unrelated plist and a truncated one both are, and neither is worth a panic in a status.
fn program_arguments(document: &str) -> Vec<String> {
    let Some(key) = document.find("<key>ProgramArguments</key>") else {
        return Vec::new();
    };

    let after = &document[key..];

    let Some(opened) = after.find("<array>") else {
        return Vec::new();
    };
    let Some(closed) = after.find("</array>") else {
        return Vec::new();
    };
    if closed < opened {
        return Vec::new();
    }

    strings(&after[opened..closed])
}

/// Every `<string>…</string>` in a fragment, unescaped.
fn strings(fragment: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut rest = fragment;

    while let Some(opened) = rest.find("<string>") {
        rest = &rest[opened + "<string>".len()..];

        let Some(closed) = rest.find("</string>") else {
            break;
        };

        words.push(unescape(&rest[..closed]));
        rest = &rest[closed..];
    }

    words
}

/// The five entities an XML document may not carry raw. `&` first, or it would escape its own work.
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

/// A temporary in the same directory, then a rename.
///
/// `.claude/architecture/platform-abstraction.md`'s second rule. A plist half written is one launchd
/// refuses at the next login, and the machine that would produce it is the one that lost power in
/// the middle of `mix autostart enable`.
fn write_atomically(path: &Path, contents: &str) -> Result<()> {
    let temporary = path.with_extension(format!("plist.{}.tmp", std::process::id()));

    std::fs::write(&temporary, contents).map_err(|source| Error::Io {
        action: "write the launch agent to",
        path: temporary.clone(),
        source,
    })?;

    std::fs::rename(&temporary, path).map_err(|source| {
        drop(std::fs::remove_file(&temporary));

        Error::Io {
            action: "put the launch agent at",
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
            program: PathBuf::from("/usr/local/bin/mixengined"),
            home: PathBuf::from("/Users/me/Library/Application Support/MixEngine"),
        }
    }

    #[test]
    fn the_document_reads_back_as_the_command_it_was_written_from() {
        assert_eq!(program_arguments(&document(&plan())), command(&plan()));
    }

    #[test]
    fn a_path_with_an_ampersand_and_an_angle_bracket_survives_both_ways() {
        let awkward = AutostartPlan {
            program: PathBuf::from("/Users/me/Fish & Chips/mixengined"),
            home: PathBuf::from("/Users/me/<home>"),
        };

        let plist = document(&awkward);

        assert!(plist.contains("Fish &amp; Chips"), "{plist}");
        assert!(!plist.contains("Fish & Chips"), "{plist}");
        assert_eq!(program_arguments(&plist), command(&awkward));
    }

    /// The setting that keeps `mix daemon stop` stopped.
    #[test]
    fn keep_alive_is_the_dictionary_and_never_a_bare_true() {
        let plist = document(&plan());

        assert!(plist.contains("<key>SuccessfulExit</key>"), "{plist}");
        assert!(
            !plist.contains("<key>KeepAlive</key>\n    <true/>"),
            "an unconditional KeepAlive would undo a deliberate stop: {plist}"
        );
    }

    #[test]
    fn an_unrelated_plist_reads_as_no_command_rather_than_panicking() {
        assert!(program_arguments("<plist><dict/></plist>").is_empty());
        assert!(program_arguments("").is_empty());
        assert!(
            program_arguments("<key>ProgramArguments</key><array>").is_empty(),
            "an unterminated array"
        );
    }

    /// The whole cycle, against a directory the test owns rather than this user's own.
    #[test]
    fn an_agent_registers_reads_back_and_disappears() {
        let directory = tempfile::TempDir::new().expect("a temporary directory");
        let agent = Agent::in_directory(directory.path().join("LaunchAgents"));

        assert!(
            !agent
                .state()
                .expect("a status on an empty directory")
                .enabled
        );

        let enabled = agent.enable(&plan()).expect("register");
        assert!(enabled.enabled && enabled.changed);
        assert_eq!(enabled.command, command(&plan()));

        let again = agent.enable(&plan()).expect("register again");
        assert!(
            !again.changed,
            "the second enable rewrote the file: {again:?}"
        );

        let disabled = agent.disable().expect("remove");
        assert!(!disabled.enabled && disabled.changed);
        assert!(!agent.state().expect("read").enabled);
        assert!(!agent.disable().expect("remove again").changed);
    }

    /// `.claude/standards/testing.md` rule 4: nothing outside the entry is touched.
    #[test]
    fn a_neighbouring_agent_is_left_byte_for_byte_alone() {
        let directory = tempfile::TempDir::new().expect("a temporary directory");
        let agents = directory.path().join("LaunchAgents");
        std::fs::create_dir_all(&agents).expect("the directory");

        let neighbour = agents.join("com.example.someone-else.plist");
        std::fs::write(&neighbour, "someone else's agent\n").expect("the neighbour");

        let agent = Agent::in_directory(&agents);
        agent.enable(&plan()).expect("register");
        agent.disable().expect("remove");

        assert_eq!(
            std::fs::read_to_string(&neighbour).expect("the neighbour survived"),
            "someone else's agent\n"
        );
    }
}
