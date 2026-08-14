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

use std::path::Path;

use mixengine_proto::RuntimeKind;

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
