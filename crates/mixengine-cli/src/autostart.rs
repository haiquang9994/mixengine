//! Starting a daemon for a home that has none.
//!
//! `.claude/architecture/daemon-and-ipc.md`: *if a client cannot connect, it spawns
//! `mixengined --detach`, which returns only once the daemon answers on its endpoint.* That is the
//! whole of the mechanism, and the reason it is this short is that the waiting belongs on the other
//! side — the process that knows whether its child is still alive is the one that started it, so
//! there is no backoff loop here and no readiness probe. `--detach` exiting zero *is* the readiness
//! probe.
//!
//! Nothing here is asynchronous. `Command::output` blocks until the daemon is up, which on a
//! current-thread runtime with no other task pending is exactly the wait we want and is one fewer
//! moving part than a `tokio::process` child polled by a runtime that has nothing else to do.
//!
//! **Reading that child to end-of-file needs one thing arranged first, and it is not obvious.**
//! Roadmap task T9 found that a daemon started with `--detach` inherited the pipe its caller's
//! stdout was on, and fixed it inside `spawn_detached` — which is one copy too late for the caller
//! *this* module is. Inheritance on Windows is transitive: `mix`'s own stdout reaches
//! `mixengined --detach` the moment it is spawned, and that process passes it on to the daemon
//! before `spawn_detached` gets a say. Whatever is reading `mix` — a shell script, `cargo test`,
//! the GUI — then waits for an end-of-file that arrives when the daemon exits, days later. So the
//! spawn below is wrapped in [`process::hide_stdio_from_children`], which is `mix` declining to
//! hand on what it was handed. Found exactly the way T9's was: as a `mix status` that returned
//! promptly and a test that never did.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use mixengine_platform::process;
use mixengine_proto::{Error, ErrorCode, flatten};

/// The environment variable that names the daemon binary outright.
///
/// Exists for the two callers that cannot rely on the layout: a test running `mix` out of
/// `target/debug` against a daemon built elsewhere, and a packager placing the two binaries apart.
pub(crate) const BINARY: &str = "MIXENGINE_DAEMON_BIN";

/// The daemon this client will start if it finds none, and the home to start it for.
#[derive(Debug)]
pub(crate) struct Autostart {
    program: OsString,
    root: PathBuf,
}

impl Autostart {
    /// Prepare to start a daemon for `root`, without starting one.
    pub(crate) fn for_home(root: &Path) -> Self {
        Self {
            program: program(),
            root: root.to_path_buf(),
        }
    }

    /// Start it, and return once it is listening.
    ///
    /// The home is passed as the *resolved* root rather than left to the child's own environment:
    /// this process has already decided which home it is talking about, and a daemon that
    /// re-resolved `MIXENGINE_HOME` against a different working directory could end up owning
    /// another one entirely.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::DependencyMissing`] when there is no `mixengined` to run, and
    /// [`ErrorCode::ProcessFailed`] when there is one and it refused to come up — carrying whatever
    /// it wrote to stderr, which is the only diagnosis available before it has a log to write into.
    pub(crate) fn run(&self) -> Result<(), Error> {
        let mut command = Command::new(&self.program);
        command.arg("--detach").arg("--home").arg(&self.root);

        // Held across the spawn and no further — see the note at the top of this module for what
        // goes wrong without it, and `mixengine_platform::process` for why the window is this
        // narrow. `output()` both spawns and waits, so the guard covers the wait too; that costs
        // nothing, since nothing else in `mix` starts a child while it is blocked here.
        let hidden = process::hide_stdio_from_children();
        let output = command.output();
        drop(hidden);

        let output = output.map_err(|source| self.cannot_run(&source))?;

        if output.status.success() {
            return Ok(());
        }

        // Its stderr, verbatim. A `--detach` that fails has already been given the daemon's own
        // startup error through the same wire mapping the daemon uses for a client, hint and all,
        // and rewriting it here would replace a real diagnosis with a guess.
        let complaint = String::from_utf8_lossy(&output.stderr);
        let complaint = complaint.trim();

        Err(Error::new(
            ErrorCode::ProcessFailed,
            if complaint.is_empty() {
                format!(
                    "{} could not start a daemon for {} ({})",
                    self.program.to_string_lossy(),
                    self.root.display(),
                    output.status
                )
            } else {
                format!(
                    "{} could not start a daemon for {} ({}): {complaint}",
                    self.program.to_string_lossy(),
                    self.root.display(),
                    output.status
                )
            },
        ))
    }

    /// The daemon binary could not be run at all.
    fn cannot_run(&self, source: &std::io::Error) -> Error {
        let message = format!(
            "cannot start {}: {}",
            self.program.to_string_lossy(),
            flatten(source)
        );

        match source.kind() {
            // The interesting case, and the one worth spelling the search out for: a `mix` that was
            // copied somewhere on its own, or a development build run from a directory the daemon
            // has not been built into. `dependency_missing` rather than `process_failed` because
            // nothing ran and the way out is an install — which is what the GUI turns this code
            // into an offer to do.
            std::io::ErrorKind::NotFound => Error::new(ErrorCode::DependencyMissing, message)
                .with_hint(format!(
                    "`mix` looks for the daemon at {BINARY}, then next to itself, then on PATH — \
                     install MixEngine, or point {BINARY} at a mixengined binary"
                )),

            // Present and unrunnable: on Unix nearly always the executable bit, and never something
            // a retry fixes. Not a missing dependency, so not that code either.
            _ => Error::new(ErrorCode::ProcessFailed, message),
        }
    }
}

/// Which `mixengined` to run.
///
/// Three places, most explicit first:
///
/// 1. **`MIXENGINE_DAEMON_BIN`**, which is an instruction rather than a guess and therefore wins
///    outright.
/// 2. **Next to this binary**, which is how every installer lays MixEngine out and — more to the
///    point day to day — how `cargo build` does: a `target/debug/mix` that autostarted whatever
///    `mixengined` happened to be installed on the machine would be a confusing afternoon.
/// 3. **`PATH`**, by bare name. Handed to the OS rather than searched here: `Command::new` already
///    does it on all three systems, and doing it by hand would mean reimplementing `PATHEXT` and
///    getting it subtly wrong on Windows.
fn program() -> OsString {
    // Read here and passed down rather than looked up inside `choose`, per the rule in
    // `.claude/standards/rust.md` that configuration enters at the top: it is also what lets the
    // search be tested without `set_var`, which is `unsafe` in edition 2024 and process-global.
    let mix = std::env::current_exe().ok();

    choose(
        std::env::var_os(BINARY).filter(|value| !value.is_empty()),
        mix.as_deref(),
    )
}

/// The search itself, given what the environment said and where this binary is.
fn choose(named: Option<OsString>, mix: Option<&Path>) -> OsString {
    if let Some(named) = named {
        return named;
    }

    let name = format!("mixengined{}", std::env::consts::EXE_SUFFIX);

    if let Some(directory) = mix.and_then(Path::parent) {
        let sibling = directory.join(&name);

        // Checked rather than used unconditionally, so that a `mix` installed without its daemon
        // falls through to `PATH` instead of failing with a path nobody asked about.
        if sibling.is_file() {
            return sibling.into_os_string();
        }
    }

    name.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_daemon_that_is_not_there_names_the_three_places_that_were_looked_in() {
        let autostart = Autostart {
            program: OsString::from("mixengined-that-does-not-exist"),
            root: PathBuf::from("/srv/mixengine"),
        };

        let error = autostart
            .run()
            .expect_err("there is no binary by that name");

        // `dependency_missing` and not `process_failed`: nothing ran, and the way out is an
        // install rather than a repair. The GUI turns this code into an offer to install (T66).
        assert_eq!(error.code, ErrorCode::DependencyMissing);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains(BINARY)),
            "{:?}",
            error.hint
        );
    }

    /// A directory holding a `mix` and, if asked, the daemon next to it.
    fn layout(with_daemon: bool) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let mix = directory
            .path()
            .join(format!("mix{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&mix, b"").expect("a stand-in for this binary");

        if with_daemon {
            std::fs::write(
                directory
                    .path()
                    .join(format!("mixengined{}", std::env::consts::EXE_SUFFIX)),
                b"",
            )
            .expect("a stand-in for the daemon");
        }

        (directory, mix)
    }

    #[test]
    fn the_daemon_built_next_to_this_binary_beats_whatever_is_installed_on_the_machine() {
        let (directory, mix) = layout(true);

        // The case this ordering exists for: a `target/debug/mix` that autostarted the packaged
        // daemon already on the machine would be a confusing afternoon.
        assert_eq!(
            choose(None, Some(&mix)),
            directory
                .path()
                .join(format!("mixengined{}", std::env::consts::EXE_SUFFIX))
                .into_os_string()
        );
    }

    #[test]
    fn a_mix_installed_without_its_daemon_falls_through_to_the_bare_name() {
        let (_directory, mix) = layout(false);

        // Which is `PATH`, searched by the OS rather than here.
        assert_eq!(
            choose(None, Some(&mix)),
            OsString::from(format!("mixengined{}", std::env::consts::EXE_SUFFIX))
        );
    }

    #[test]
    fn the_environment_is_an_instruction_and_beats_the_binary_next_door() {
        let (_directory, mix) = layout(true);

        assert_eq!(
            choose(
                Some(OsString::from("/opt/mixengine/mixengined")),
                Some(&mix)
            ),
            OsString::from("/opt/mixengine/mixengined")
        );
    }
}
