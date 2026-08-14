//! Which commands `<root>/bin` fronts, and what each one runs.
//!
//! A shim is one binary, copied once per name it answers to, that reads its own file name to find
//! out which command was invoked ([runtime-versions.md](../../../.claude/features/runtime-versions.md)).
//! This module is the table that name is looked up in, and it is here rather than in the shim binary
//! for the reason every table like it is in `core`: the process that *fills* `<root>/bin` needs the
//! same list, and two lists would be a `bin/` holding a name nothing dispatches — a program that
//! exists, runs, and refuses to be anything.
//!
//! # A command and an executable are two different names
//!
//! [`Command::name`] is what the user types and what the file in `bin/` is called. `executable` is
//! the key of the artifact's `provides` map, which is **ours rather than the publisher's** — the
//! path inside the archive belongs to whoever packed it, the name it is published under is a
//! convention this project sets, and the index is written to match. That is what lets `python3` and
//! `python` be one program, and `bundler` and `bundle` be one program, without the shim caring which
//! of them a given archive happened to call its file.
//!
//! # What is deliberately not in the table
//!
//! **`composer`**, and every other tool that is not inside a language's archive. The feature spec
//! lists it among the commands `bin/` will eventually hold, and it is a `.phar` fetched separately —
//! so a row here would be a shim that resolves a PHP correctly and then fails to find a file no
//! artifact was ever going to contain. It arrives with the task that installs it.
//!
//! **Only PHP has artifacts today** (T20a), so the other three rows are unexercised until T27
//! publishes theirs. They are written now because the table is what a shim dispatches on: a row
//! missing when the artifact lands is a `node` in `bin/` that says it is nobody's, and the failure a
//! wrong row produces is one sentence naming what the runtime *does* publish, which is the same
//! sentence a missing row would need anyway.
//!
//! # Filling `bin/` — roadmap task T26
//!
//! [`refresh`] is the other half of the table's reason for being here: one copy of the shim binary
//! per row, under the row's name. It is what turns T25's binary into commands a person can type.
//!
//! **`bin/` is entirely MixEngine's**, which is what lets a refresh remove what it does not
//! recognise. `.claude/architecture/overview.md` describes the directory as "version-resolving
//! shims" and nothing else, so a file in there answering to no command is a command that was
//! renamed or dropped between releases — a program that exists, runs, and refuses to be anything.
//! Somebody who wants a script of their own on the PATH has every other directory on the machine to
//! put it in.
//!
//! **It does not depend on what is installed.** [`COMMANDS`] is a constant, so `bin/` holds `node`
//! on a machine with no Node.js: the shim there resolves nothing and says which command to type,
//! which is a better answer than `node: command not found` for a tool whose whole job is managing
//! versions of Node. That is also why nothing calls this after an install —
//! [runtime-versions.md](../../../.claude/features/runtime-versions.md) lists "refresh shims" as the
//! last step of one, and there is nothing to refresh.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use mixengine_proto::RuntimeKind;

use crate::{Error, Result};

/// One command `<root>/bin` answers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Command {
    /// What the user types, and what the shim file in `bin/` is named.
    pub name: &'static str,

    /// Which language's version resolution decides what this runs.
    pub kind: RuntimeKind,

    /// Which of the artifact's executables to run, by the name the index publishes it under.
    pub executable: &'static str,
}

/// Every command a shim answers to, grouped by language and in the order `bin/` is listed in.
///
/// The set per language is the tools that ship *inside* that language's archive and that a person
/// runs directly. `php-fpm` is absent although PHP ships one: it is a service the daemon supervises
/// with a generated pool config (T28), not a command anybody types in a project directory, and a
/// shim in front of it would be a second way to start one nothing was supervising.
pub const COMMANDS: &[Command] = &[
    // PHP. `pecl` and `pear` are scripts the Unix builds ship and the Windows ones do not, which is
    // not a special case here: an artifact that publishes neither answers the lookup with the list
    // of what it does publish, which is the honest message on a machine where they were never
    // packed.
    Command {
        name: "php",
        kind: RuntimeKind::Php,
        executable: "php",
    },
    Command {
        name: "php-config",
        kind: RuntimeKind::Php,
        executable: "php-config",
    },
    Command {
        name: "phpize",
        kind: RuntimeKind::Php,
        executable: "phpize",
    },
    Command {
        name: "pecl",
        kind: RuntimeKind::Php,
        executable: "pecl",
    },
    Command {
        name: "pear",
        kind: RuntimeKind::Php,
        executable: "pear",
    },
    // Node.
    Command {
        name: "node",
        kind: RuntimeKind::Node,
        executable: "node",
    },
    Command {
        name: "npm",
        kind: RuntimeKind::Node,
        executable: "npm",
    },
    Command {
        name: "npx",
        kind: RuntimeKind::Node,
        executable: "npx",
    },
    Command {
        name: "corepack",
        kind: RuntimeKind::Node,
        executable: "corepack",
    },
    // Python. `python3` and `pip3` are the same programs under the names most projects' scripts
    // actually call, which is the whole reason a command and an executable are separate fields.
    Command {
        name: "python",
        kind: RuntimeKind::Python,
        executable: "python",
    },
    Command {
        name: "python3",
        kind: RuntimeKind::Python,
        executable: "python",
    },
    Command {
        name: "pip",
        kind: RuntimeKind::Python,
        executable: "pip",
    },
    Command {
        name: "pip3",
        kind: RuntimeKind::Python,
        executable: "pip",
    },
    // Ruby.
    Command {
        name: "ruby",
        kind: RuntimeKind::Ruby,
        executable: "ruby",
    },
    Command {
        name: "gem",
        kind: RuntimeKind::Ruby,
        executable: "gem",
    },
    Command {
        name: "bundle",
        kind: RuntimeKind::Ruby,
        executable: "bundle",
    },
    Command {
        name: "bundler",
        kind: RuntimeKind::Ruby,
        executable: "bundle",
    },
    Command {
        name: "rake",
        kind: RuntimeKind::Ruby,
        executable: "rake",
    },
    Command {
        name: "irb",
        kind: RuntimeKind::Ruby,
        executable: "irb",
    },
];

/// Which command a program invoked at this path is being asked to be.
///
/// `argv[0]` is the whole input, because a shim has no arguments of its own — every one of them
/// belongs to the program it fronts, and a `--home` flag here would be a flag `php` could never
/// receive. What is read off it is the file name with any executable suffix removed.
///
/// **The comparison is case-insensitive on Windows and not on Unix**, which is the filesystem's own
/// rule rather than a courtesy: `PHP.EXE` and `php.exe` are one file there and two files here, so
/// folding case on Unix would let a program genuinely called `PHP` be dispatched as `php`.
///
/// [`None`] for a name the table does not hold — including `mixengine-shim` itself, which is what
/// the binary is called before it is copied into `bin/` under a name that means something.
#[must_use]
pub fn dispatch(invoked_as: &Path) -> Option<&'static Command> {
    let stem = invoked_as.file_stem()?.to_str()?;

    COMMANDS.iter().find(|command| {
        if cfg!(windows) {
            command.name.eq_ignore_ascii_case(stem)
        } else {
            command.name == stem
        }
    })
}

/// The file name a shim answering to `command` has in `bin/` on this operating system.
///
/// The suffix is the loader's rule and not a convention: Windows resolves a bare `php` typed at a
/// prompt by appending `.exe`, so a copy without one is a file nothing can run.
#[must_use]
pub fn file_name(command: &Command) -> String {
    format!("{}{}", command.name, std::env::consts::EXE_SUFFIX)
}

/// The name a copy that could not be replaced is moved aside under.
///
/// Windows will not overwrite a running executable, and a `php -S` somebody left in another
/// terminal is exactly that. It *will* let the file be renamed out of the way while it runs, which
/// is the only way to put a new one in its place — and the moved copy keeps working for the process
/// that is holding it until that process exits.
const MOVED_ASIDE: &str = ".mixengine-replaced";

/// What one [`refresh`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Refreshed {
    /// Every command `bin/` now answers to, in [`COMMANDS`]' order.
    pub commands: Vec<String>,

    /// The ones this call put there, because they were missing or were a different build.
    ///
    /// Empty on the ordinary start, which is the point of comparing before writing: a daemon
    /// restarted twenty times a day must not rewrite nineteen megabytes each time. What the *first*
    /// start of a home costs is `place`'s question rather than this one's.
    pub written: Vec<String>,

    /// What was in `bin/` that no command answers to, and was removed.
    pub removed: Vec<String>,

    /// What was in `bin/` that no command answers to, and could **not** be removed.
    ///
    /// Reported rather than swallowed: a leftover here is a name on the user's PATH that runs
    /// something MixEngine no longer understands, and a refresh that said nothing about it would be
    /// claiming a `bin/` it had not achieved.
    pub refused: Vec<String>,
}

/// Where the shim binary is, given the program that is asking.
///
/// It sits beside whatever is running — `mixengined` in an install, and the same `target/debug` in
/// a development tree — because a release ships the two next to each other and there is nothing
/// else to look at: a `PATH` search would find the *copy in `bin/`* on a machine where the PATH is
/// already set up, and copying a shim from `bin/` into `bin/` would make an upgrade a no-op.
///
/// # Errors
///
/// [`Error::ShimMissing`] when there is no such file, which is a broken installation rather than
/// anything a user did.
pub fn source(program: &Path) -> Result<PathBuf> {
    let beside = program.parent().unwrap_or_else(|| Path::new("."));
    let shim = beside.join(format!("mixengine-shim{}", std::env::consts::EXE_SUFFIX));

    match shim.is_file() {
        true => Ok(shim),
        false => Err(Error::ShimMissing { path: shim }),
    }
}

/// Put one copy of `shim` in `bin` for every command in [`COMMANDS`], and remove what is not one.
///
/// Idempotent, and cheap when there is nothing to do: a copy whose length matches the shim's and
/// whose modification time is not older is left alone, so the common case is a stat per command and
/// no bytes moved. An upgrade replaces the shim binary with one that is a different length or newer
/// than the copies, and every copy is rewritten.
///
/// **The pass that is *not* idempotent — the first one, on a home that has never had a daemon — is
/// the expensive one, and `place` is where that cost is paid or avoided.** Nineteen names is a hard
/// link apiece where the filesystem gives one file a second name, and nineteen times the shim binary
/// where it does not — which on Windows is always, for the reason stated there.
///
/// **Not a transaction, and it cannot be one**: nineteen files cannot be renamed into place at
/// once. What that costs is bounded by the fact that every copy is the *same program* — a `bin/`
/// half written by an upgrade holds some new shims and some old ones, and both dispatch on their own
/// file name and resolve against the same database.
///
/// # Errors
///
/// [`Error::Io`] naming the file that could not be written. Failing to *remove* a stranger is not
/// one — it lands in [`Refreshed::refused`] — because a directory that has what it should have is
/// working, and refusing to start over a file nobody can delete would be worse than saying so.
pub fn refresh(bin: &Path, shim: &Path) -> Result<Refreshed> {
    crate::paths::create_dir(bin)?;

    let mut refreshed = Refreshed::default();

    let expected: HashSet<String> = COMMANDS
        .iter()
        .map(|command| fold(&file_name(command)))
        .collect();

    // Swept **before** the copies rather than after, so that a name which moved from one command to
    // another — a row renamed between releases — is removed and then written afresh rather than
    // removed a moment after being put there.
    sweep(bin, &expected, &mut refreshed);

    for command in COMMANDS {
        let name = file_name(command);
        let target = bin.join(&name);

        if place(shim, &target)? {
            refreshed.written.push(name.clone());
        }

        refreshed.commands.push(name);
    }

    Ok(refreshed)
}

/// Take `bin` back to nothing, which is what an uninstall of the whole home does first.
///
/// Its own function rather than `refresh` against an empty table, because the two disagree about
/// what a failure means: a copy that cannot be removed here is reported and the rest still go, so
/// that one file held open by a shell somebody forgot about does not leave eighteen others behind.
///
/// # Errors
///
/// [`Error::Io`] only when `bin` itself cannot be listed. Everything else is in
/// [`Refreshed::refused`].
pub fn clear(bin: &Path) -> Result<Refreshed> {
    let mut refreshed = Refreshed::default();

    if bin.is_dir() {
        sweep(bin, &HashSet::new(), &mut refreshed);
    }

    Ok(refreshed)
}

/// Remove everything in `bin` that is not one of `expected`.
///
/// Best effort by design — see [`refresh`]'s own note — and silent about the copies it moved aside
/// itself: one of those is a shim a running process is still holding, and reporting it as a
/// stranger every start would make an ordinary Windows situation look like a fault.
fn sweep(bin: &Path, expected: &HashSet<String>, refreshed: &mut Refreshed) {
    let Ok(entries) = std::fs::read_dir(bin) else {
        return;
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();

        if expected.contains(&fold(&name)) {
            continue;
        }

        let removed = std::fs::remove_file(entry.path()).is_ok();

        if name.ends_with(MOVED_ASIDE) {
            continue;
        }

        match removed {
            true => refreshed.removed.push(name),
            false => refreshed.refused.push(name),
        }
    }
}

/// Put `shim` at `target` unless what is there is already this build. `true` when it wrote.
///
/// **A second name for the shim wherever the filesystem will give it one, and a second set of bytes
/// only where it will not.** The bytes are the whole cost of filling `bin/`, and paying them
/// nineteen times per home is not a cost this has to meet: a hard link is one directory entry, and
/// every property `refresh` relies on survives it — [`is_current`] compares length and modification
/// time, which a link shares with the file it names, so the next start still writes nothing; a build
/// that *replaces* the shim binary leaves the links on the file they were made from, which is older
/// or a different length, and every one of them is replaced.
///
/// It is also what stops a debug build from being pathological. A `mixengine-shim` with its debug
/// info in it is tens of megabytes, so a start used to move most of a gigabyte before it bound its
/// endpoint — and a test suite that gives every test a home of its own paid that per test, which is
/// what made four daemons on one CI runner take thirty seconds each to answer.
fn place(shim: &Path, target: &Path) -> Result<bool> {
    if is_current(shim, target) {
        return Ok(false);
    }

    if link(shim, target) {
        return Ok(true);
    }

    let io = |source| Error::Io {
        action: "install the shim at",
        path: target.to_path_buf(),
        source,
    };

    match std::fs::copy(shim, target) {
        Ok(_) => Ok(true),

        // The Windows case: something is running this copy. Renaming it away is allowed while a
        // process holds it, and the process keeps running the file it opened — so the moved copy is
        // rubbish the next sweep collects rather than something anybody still needs.
        Err(_) if target.exists() => {
            let aside = target.with_file_name(format!(
                "{}{MOVED_ASIDE}",
                target.file_name().unwrap_or_default().to_string_lossy()
            ));

            // A copy moved aside by an earlier attempt and never collected. Removed first, because
            // a rename onto an existing file fails on Windows.
            let _ = std::fs::remove_file(&aside);
            std::fs::rename(target, &aside).map_err(io)?;

            match std::fs::copy(shim, target) {
                Ok(_) => Ok(true),

                // Put back what was there. The command keeps working as the build it was, which is
                // a great deal better than a name on the PATH with no file behind it.
                Err(source) => {
                    let _ = std::fs::rename(&aside, target);
                    Err(io(source))
                }
            }
        }

        Err(source) => Err(io(source)),
    }
}

/// Give the shim a second name at `target`, and say whether the system allowed it.
///
/// Never an error: every way of failing has the same answer, which is to copy the bytes instead —
/// a `bin/` on a different filesystem from the install, a filesystem with no links in it, a
/// permission a link needs and a write does not. The caller's next line is the copy, so a `false`
/// here costs one failed syscall and nothing else.
///
/// # Not on Windows, and it is the shim's own behaviour that decides it
///
/// A link is the same file under two names, so whatever holds one holds both — and a Windows shim
/// **outlives the program it starts**: it stays as the parent of a Job Object child (see
/// `mixengine-shim`) rather than `exec`ing away as it does on Unix. So a `php -S` somebody left
/// running would hold `mixengine-shim.exe` itself open for hours, and the next upgrade — or the next
/// `cargo build` in this tree — would meet a sharing violation on a file it has every right to
/// replace. [`place`]'s existing dance moves a *copy* aside for exactly that case and cannot move
/// aside a file that is the source. On Unix the same shim has `exec`ed into PHP microseconds after
/// it started, and the file it came from is nobody's any more.
///
/// The `cfg!` is the same one [`dispatch`] and [`fold`] use: a constant this module reads, not a
/// call into the operating system — `.claude/architecture/platform-abstraction.md` draws that line
/// at behaviour a trait can be written for, and "does a running program hold its own file" is not
/// something either side of `bin/` can be asked.
fn link(shim: &Path, target: &Path) -> bool {
    if cfg!(windows) {
        return false;
    }

    if std::fs::hard_link(shim, target).is_ok() {
        return true;
    }

    // A link refuses an existing name, where a copy overwrites one — so the file that is there is
    // unlinked and the link tried once more. It is `place`'s own case: something is at `target` and
    // [`is_current`] has just said it is not this build. Unlinking it on Unix takes the name and
    // leaves the file to whatever process is still running it, which is the property this whole
    // function rests on.
    if !target.exists() || std::fs::remove_file(target).is_err() {
        return false;
    }

    std::fs::hard_link(shim, target).is_ok()
}

/// Is the copy at `target` the same build as `shim`?
///
/// Length and modification time rather than the bytes: this runs once per command on every daemon
/// start, and hashing nineteen copies of a multi-megabyte binary to discover that nothing changed
/// would be the most expensive thing a start does. `>=` and not `==` because a copy is stamped when
/// it is made, which is after the source it came from — and because a link ([`link`]) is the *same*
/// file, so the two times it compares are one time and the answer has to be yes.
fn is_current(shim: &Path, target: &Path) -> bool {
    let (Ok(source), Ok(copy)) = (shim.metadata(), target.metadata()) else {
        return false;
    };

    if source.len() != copy.len() {
        return false;
    }

    match (source.modified(), copy.modified()) {
        (Ok(source), Ok(copy)) => copy >= source,
        // A filesystem that will not say. Copying is the safe answer: the cost is one write per
        // start, and the alternative is a shim that is never upgraded.
        _ => false,
    }
}

/// A file name as this filesystem compares one — [`dispatch`]'s rule, applied to `bin/` itself.
fn fold(name: &str) -> String {
    match cfg!(windows) {
        true => name.to_ascii_lowercase(),
        false => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shim_knows_which_command_it_was_copied_to_be() {
        let bin = Path::new(if cfg!(windows) {
            r"C:\Users\someone\AppData\Local\MixEngine\bin"
        } else {
            "/home/someone/.local/share/mixengine/bin"
        });

        let php = dispatch(&bin.join(format!("php{}", std::env::consts::EXE_SUFFIX)))
            .expect("php is a command");
        assert_eq!(php.kind, RuntimeKind::Php);
        assert_eq!(php.executable, "php");

        // A name of the user's world that is a different name in the artifact's, which is the pair
        // of fields' whole reason.
        let python3 = dispatch(&bin.join("python3")).expect("python3 is a command");
        assert_eq!(python3.executable, "python");
        assert_eq!(python3.kind, RuntimeKind::Python);

        // The binary before it is copied into `bin/` under a name that means something.
        assert_eq!(dispatch(Path::new("mixengine-shim")), None);
        assert_eq!(dispatch(Path::new("composer")), None, "not in an artifact");
    }

    /// The filesystem's rule, not a courtesy — see [`dispatch`].
    #[test]
    fn case_is_folded_exactly_where_the_filesystem_folds_it() {
        assert_eq!(dispatch(Path::new("PHP")).is_some(), cfg!(windows));
    }

    /// Two rows with one name would make `bin/` a directory whose entries are decided by the order
    /// of this table, which is not a thing anybody should have to know.
    #[test]
    fn no_two_commands_answer_to_the_same_name() {
        let mut names: Vec<&str> = COMMANDS.iter().map(|command| command.name).collect();
        names.sort_unstable();

        let mut unique = names.clone();
        unique.dedup();

        assert_eq!(names, unique, "a name is listed twice");
    }

    /// Every name has to be a filename on all three systems, since `bin/` is where it lands.
    #[test]
    fn every_command_is_a_name_a_file_can_have() {
        for command in COMMANDS {
            assert!(
                !command.name.is_empty()
                    && command
                        .name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{} is not a name to put in bin/",
                command.name
            );
        }
    }
}
