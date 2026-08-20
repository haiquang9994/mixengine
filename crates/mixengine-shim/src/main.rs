//! `php`, `node`, `npm`, `python`, `ruby` — one binary, copied into `<root>/bin` under each of those
//! names, that works out which version this directory means and then gets out of its way.
//!
//! This is the program that makes `cd project-a && php -v` and `cd project-b && php -v` disagree
//! with no shell hook installed, which is Phase 2's milestone. Four steps, and the interesting thing
//! about each of them is what it does *not* do:
//!
//! ```text
//! 1  which command is this?        argv[0], against `core::shims::COMMANDS`
//! 2  which version does it mean?   `core::resolve`, in this process — no daemon, no IPC
//! 3  which file is that?           the `provides` map recorded when it was installed
//! 4  become it                     exec on Unix, a Job Object child on Windows — carrying `PATH`
//!                                  and the generated ini set (T28), and nothing else
//! ```
//!
//! # It has no arguments of its own, and cannot have
//!
//! Every argument belongs to the program being fronted: a `--home` flag here would be one `php`
//! could never be given, and `php --version` has to reach PHP rather than print ours. So the only
//! input beside `argv[0]` is the environment — `MIXENGINE_HOME` for which install this is, and
//! `MIXENGINE_PHP` (per kind, [`RuntimeKind::override_env`]) for a version chosen for one command.
//! **That is also why there is no `--json`, no logging and no `--explain`**: anything this printed
//! on its own account would be a line in the middle of somebody's `php -r` output.
//!
//! # Why it links `mixengine-core` when `mix` deliberately does not
//!
//! `mix` avoids that edge because it can ask a daemon, and a bundled SQLite is a poor trade for
//! finding out where a socket lives. Here there is nothing to ask: the whole promise is that a
//! version resolves **without a daemon** — with one stopped, still starting, or never installed —
//! and in a budget (15 ms, T29) that a connection, a request and a response would spend before the
//! query even started. So the resolution is the same `core::resolve` the daemon and the GUI call,
//! run in this process against the database opened read-only.
//!
//! # What it is not allowed to do to the home
//!
//! Read it. [`Store::open_read_only`] does not create the file and does not migrate it, and SQLite
//! is what enforces that rather than our remembering: a schema upgrade decided by whichever `php -v`
//! ran first, possibly several at once from a build script, is the one thing `mixengine.db` cannot
//! afford. A home that has never had a daemon in it is an error here, not a home this creates.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use mixengine_core::config::PathOverrides;
use mixengine_core::{Paths, Store, paths, resolve, runtimes, shims};
use mixengine_platform::process;
use mixengine_proto::{PackageVersion, RuntimeKind, VersionConstraint};

/// What this exits with when it cannot become the program it was asked to be.
///
/// A shell's own word for it: 127 is "command not found", which is what every failure here amounts
/// to from the outside — the version is not installed, the home is not there, the artifact publishes
/// no such executable. Deliberately one code rather than a taxonomy: a script branching on *why* a
/// `php` did not run would be branching on MixEngine's internals, and the sentence on stderr is
/// where the reason belongs.
const NOT_RUNNABLE: i32 = 127;

fn main() {
    // `argv[0]` rather than `current_exe`, and the difference is the whole dispatch: a shim is this
    // binary under another name, so what has to be read is the name it was *invoked* by. On Unix
    // `current_exe` follows a symlink back to `mixengine-shim`, which would make every command in
    // `bin/` the same unknown one.
    let invoked = std::env::args_os().next().unwrap_or_default();
    let invoked = PathBuf::from(invoked);

    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();

    match run(&invoked, &arguments) {
        Ok(code) => std::process::exit(code),

        Err(refusal) => {
            // Named after the command the user typed rather than after MixEngine, the way every
            // other program on their PATH complains. `mix` is named in the hint instead, which is
            // where it is something to type rather than a brand.
            let called = called(&invoked);
            eprintln!("{called}: {}", refusal.said);

            if let Some(hint) = refusal.hint {
                eprintln!("{called}: {hint}");
            }

            std::process::exit(NOT_RUNNABLE);
        }
    }
}

/// Resolve, look up, and hand over.
///
/// Answers a status only on Windows, where the shim outlives the program it started; on Unix the
/// hand-over is an `exec` and the only way back here is the [`Refusal`].
fn run(invoked: &Path, arguments: &[OsString]) -> Result<i32, Refusal> {
    let command = shims::dispatch(invoked).ok_or_else(unknown_command)?;

    let Resolution {
        program,
        root,
        version,
    } = resolved(command)?;

    let environment = surroundings(command, &program, &root, &version);

    process::hand_over(&program, arguments, &environment).map_err(|error| Refusal {
        said: explain(&error),
        hint: None,
    })
}

/// What step three answered: the file to run, and the two things step four needs to describe it.
struct Resolution {
    /// The executable inside the runtime's own directory.
    program: PathBuf,

    /// `MIXENGINE_HOME`, so the generated ini set can be found without resolving the root twice.
    root: PathBuf,

    /// Which version this directory meant, which is what names the ini set.
    version: PackageVersion,
}

/// Everything the fronted program is given beside its own arguments.
///
/// Two variables and no more. `PATH` is what makes a runtime's own tools reach each other, and
/// `PHP_INI_SCAN_DIR` is the generated ini set the pool also reads — the whole point of it being
/// here is that `php -m` in a terminal and `phpinfo()` in a browser answer the same thing.
///
/// **Keyed off the directory existing rather than off the command being `php`**:
/// [`runtimes::extensions`] renders nothing for a runtime whose artifact declares no extension
/// directory, and a variable pointing at a directory nothing writes is worse than no variable.
fn surroundings(
    command: &shims::Command,
    program: &Path,
    root: &Path,
    version: &PackageVersion,
) -> BTreeMap<String, OsString> {
    let mut environment = BTreeMap::new();

    // The directory the program lives in, ahead of everything already on the path. It is what makes
    // a runtime's own tools reach each other: `php-config` invoked by an extension build, `node`
    // invoked by `npm`, `gem` invoked by `bundle`. Prepended rather than replacing, because the rest
    // of the PATH is the user's session and a shim is standing in the middle of it.
    if let Some(directory) = program.parent() {
        environment.insert("PATH".to_owned(), ahead_of_the_path(directory));
    }

    // `PathOverrides::default()` for `resolved`'s reason: a shim does not read `config.toml`.
    let paths = Paths::new(root.to_path_buf(), &PathOverrides::default());
    let conf_d = runtimes::extensions::conf_d(paths.etc(), command.kind, version.as_str());

    if conf_d.is_dir() {
        environment.insert(
            runtimes::extensions::SCAN_DIR_ENV.to_owned(),
            conf_d.into_os_string(),
        );
    }

    environment
}

/// Steps two and three: which version this directory means, and which file that is.
///
/// A `tokio` runtime of its own rather than `#[tokio::main]`, and dropped before the hand-over: what
/// follows is an `exec` on one system and a wait on the other, and neither wants a reactor thread
/// still standing behind it.
fn resolved(command: &shims::Command) -> Result<Resolution, Refusal> {
    let home = home_override().map(PathBuf::from);
    let root = paths::resolve_root(home.as_deref(), mixengine_platform::host().as_ref()).map_err(
        |error| Refusal {
            said: explain(&error),
            hint: None,
        },
    )?;

    // `[paths]` cannot move the database — `Paths` passes `None` for it deliberately — so the
    // defaults are enough to name the one file this reads, and `config.toml` is not opened at all.
    // A shim that parsed the user's configuration would be a shim that fails when it has a typo in
    // it, on every command they run.
    let database = Paths::new(root.clone(), &PathOverrides::default())
        .database_file()
        .to_path_buf();

    let asked = override_version(command.kind)?;

    // A directory that has been deleted out from under this process is `None` rather than a refusal:
    // there is nothing to walk, which is exactly what the default is for. It is also the one shape
    // `resolve` already has a meaning for.
    let cwd = std::env::current_dir().ok();

    let runtime = tokio::runtime::Builder::new_current_thread()
        // Time and not `enable_all`, and the timer is not optional: `sqlx` panics outright without
        // one, because the busy timeout and the pool's own acquire deadline are both timers. What
        // is left out is the I/O driver, which would be an epoll or a completion port registered
        // for a database that is a file.
        .enable_time()
        .build()
        .map_err(|source| Refusal {
            said: format!("cannot start: {source}"),
            hint: None,
        })?;

    runtime.block_on(async move {
        let store = Store::open_read_only(&database)
            .await
            .map_err(|error| Refusal {
                said: explain(&error),
                hint: Some(format!(
                    "{} is where this shim looks — set MIXENGINE_HOME if that is not the install \
                     it belongs to",
                    database.display()
                )),
            })?;

        let resolved = resolve::runtime(
            &store,
            &resolve::Question {
                kind: command.kind,
                cwd: cwd.as_deref(),
                explicit: asked.as_ref(),
            },
        )
        .await
        .map_err(|error| Refusal {
            hint: hint_for(&error),
            said: explain(&error),
        })?;

        let program = runtimes::program(
            &store,
            command.kind,
            &resolved.runtime.version,
            command.executable,
        )
        .await
        .map_err(|error| Refusal {
            said: explain(&error),
            hint: None,
        })?;

        Ok(Resolution {
            program,
            root,
            version: resolved.runtime.version,
        })
    })
}

/// `MIXENGINE_HOME`, if it says anything.
///
/// Read here rather than deeper in, per the standards' rule that configuration enters at `main` — and
/// an empty value is passed on rather than treated as absent, because `paths::resolve_root` is what
/// refuses it. A variable somebody meant to point somewhere must not silently become the default.
fn home_override() -> Option<OsString> {
    std::env::var_os("MIXENGINE_HOME")
}

/// The version this one command was told to use, from the kind's own environment variable.
///
/// `MIXENGINE_PHP=8.1 php -v` is step one of the resolution order, and it is read *here* for the
/// reason [`RuntimeKind::override_env`] gives: the process that reads it has to be the one the user
/// invoked. An empty value is "not set"; anything else that is not a constraint is refused rather
/// than skipped past, because a variable that quietly does nothing is the exact confusion this is
/// meant to end.
fn override_version(kind: RuntimeKind) -> Result<Option<VersionConstraint>, Refusal> {
    let name = kind.override_env();

    let Some(value) = std::env::var_os(name) else {
        return Ok(None);
    };

    let Some(value) = value.to_str().map(str::trim) else {
        return Err(Refusal {
            said: format!("{name} is not text this can read as a version"),
            hint: None,
        });
    };

    if value.is_empty() {
        return Ok(None);
    }

    VersionConstraint::parse(value)
        .map(Some)
        .map_err(|error| Refusal {
            said: format!("{name} is set to something that is not a version: {error}"),
            hint: Some(format!(
                "a version ({}=8.3.33), a series ({name}=8.3) or a range ({name}=^8.3)",
                name
            )),
        })
}

/// `directory`, then everything that was already on `PATH`.
///
/// `join_paths` rather than a separator of our own: the character differs by platform, and a
/// `#[cfg]` for it in a client is the one thing `.claude/standards/rust.md` will not have. A `PATH`
/// that cannot be rebuilt — an entry containing the separator itself, which Windows allows inside
/// quotes — leaves the directory on its own rather than failing the command: the program still runs
/// and still finds its siblings, which is what the entry was for.
fn ahead_of_the_path(directory: &Path) -> OsString {
    let existing = std::env::var_os("PATH").unwrap_or_default();

    let entries = std::iter::once(directory.as_os_str().to_owned())
        .chain(std::env::split_paths(&existing).map(PathBuf::into_os_string));

    std::env::join_paths(entries).unwrap_or_else(|_| directory.as_os_str().to_owned())
}

/// What to call this program in a message.
///
/// The name it was invoked by, which is the one the user typed — `php`, not `mixengine-shim`, and
/// not the path it was found at.
fn called(invoked: &Path) -> String {
    invoked
        .file_stem()
        .unwrap_or(OsStr::new("mixengine-shim"))
        .to_string_lossy()
        .into_owned()
}

/// A reason this could not become the program it was asked to be, and what to do about it.
///
/// Two strings rather than an error enum: nothing branches on these — the exit code is
/// [`NOT_RUNNABLE`] either way — and what a shim owes its user is one sentence and, where there is
/// one, the command that would fix it.
struct Refusal {
    /// What went wrong, as a sentence.
    said: String,

    /// What to type about it.
    hint: Option<String>,
}

/// Being invoked under a name this build does not front.
///
/// Reached two ways: the binary run directly, before it has been copied into `bin/` under a name
/// that means something, and a leftover copy in a `bin/` from a build that fronted more commands
/// than this one does.
fn unknown_command() -> Refusal {
    let names: Vec<&str> = shims::COMMANDS.iter().map(|command| command.name).collect();

    Refusal {
        said: "this is a MixEngine shim and is not meant to be run under this name".to_owned(),
        hint: Some(format!("it answers to: {}", names.join(", "))),
    }
}

/// One line out of an error and everything that caused it.
///
/// The chain is walked because these types keep the interesting half in the `#[source]` — "cannot
/// open the database at …" is the sentence, and "no such file" is the reason — and a shim has one
/// line to say both in.
fn explain(error: &dyn std::error::Error) -> String {
    let mut said = error.to_string();
    let mut cause = error.source();

    while let Some(next) = cause {
        said.push_str(": ");
        said.push_str(&next.to_string());
        cause = next.source();
    }

    said
}

/// The command that would make a failed resolution succeed, where there is one.
///
/// The same sentence the daemon puts in the `dependency_missing` hint, from the same function, so
/// that a version missing in a terminal and a version missing in the GUI tell the user to type the
/// same thing.
fn hint_for(error: &mixengine_core::Error) -> Option<String> {
    match error {
        mixengine_core::Error::RuntimeUnresolved {
            kind, constraint, ..
        } => Some(resolve::install_command(*kind, constraint)),

        mixengine_core::Error::NoDefaultRuntime { kind } => Some(format!(
            "`mix runtime list --kind {kind}` shows what is installed, and \
             `mix runtime default {kind} <version>` chooses which one is used here"
        )),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T25 left this note: "No `PHPRC`, no `GEM_HOME` — the rest are files T28's `conf.d` model
    /// generates, and a variable pointing at a file nothing writes is worse than no variable."
    /// Something writes them now.
    #[test]
    fn a_php_shim_is_told_where_its_ini_set_is() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let root = home.path();
        let version = PackageVersion::parse("8.3.33").expect("a version");

        let command = shims::COMMANDS
            .iter()
            .find(|command| command.name == "php")
            .expect("this build fronts php");

        let conf_d = mixengine_core::runtimes::extensions::conf_d(
            &root.join("etc"),
            command.kind,
            version.as_str(),
        );
        std::fs::create_dir_all(&conf_d).expect("a generated set");

        let environment = surroundings(
            command,
            &root.join("runtimes/php/8.3.33/bin/php"),
            root,
            &version,
        );

        assert!(
            environment.contains_key("PATH"),
            "the runtime's own tools still reach each other"
        );
        let scan = environment
            .get(mixengine_core::runtimes::extensions::SCAN_DIR_ENV)
            .expect("a php that is told where its extensions are");
        assert!(
            scan.to_string_lossy().contains("8.3.33"),
            "the shim is pointing at another version's set: {scan:?}"
        );
    }

    /// A runtime with no generated set gets no variable, rather than one pointing at nothing.
    #[test]
    fn a_runtime_with_no_generated_set_is_told_nothing() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let version = PackageVersion::parse("20.11.0").expect("a version");

        let command = shims::COMMANDS
            .iter()
            .find(|command| command.name == "node")
            .expect("this build fronts node");

        let environment = surroundings(
            command,
            &home.path().join("runtimes/node/20.11.0/bin/node"),
            home.path(),
            &version,
        );

        assert!(!environment.contains_key(mixengine_core::runtimes::extensions::SCAN_DIR_ENV));
    }
}
