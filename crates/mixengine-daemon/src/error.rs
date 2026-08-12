//! Where a library error becomes the one a client sees.
//!
//! Every crate below keeps its own `thiserror` enum, shaped for the code that raises it and knowing
//! nothing about codes, hints or the wire. The translation happens here, once, at the boundary —
//! `.claude/standards/rust.md`. Three things happen in it, and none of them belong anywhere else:
//!
//! - **The chain is flattened.** A client is handed one string and has no `source()` to walk, so
//!   every cause is folded into the message before it leaves. That part is
//!   [`mixengine_proto::flatten`], because `mix` maps the handful of failures it can meet without a
//!   daemon and has to produce the same shape of message.
//! - **A code is chosen.** That is the part a program branches on, so the choice is made by
//!   somebody who can see both sides — the library variant and the published vocabulary.
//! - **A hint is written**, where the daemon knows something the library did not. The library knows
//!   that `create` returned `EACCES`; only this layer knows that MixEngine never runs as an
//!   administrator and that `[paths]` exists. Where the library message already names the way out,
//!   the hint stays `None` rather than saying it a second time in the GUI.

use std::io;
use std::path::Path;

use mixengine_proto::{Error, ErrorCode};

/// A library error, rendered for the wire.
///
/// A trait rather than `From`, because both types are foreign here and the orphan rule says so —
/// which is no loss: `to_wire` at a call site names what is happening better than `.into()` would.
pub(crate) trait ToWire {
    /// Translate into the error a client sees.
    fn to_wire(&self) -> Error;
}

impl ToWire for mixengine_core::Error {
    fn to_wire(&self) -> Error {
        use mixengine_core::{Error as Core, services::GraphError};

        match self {
            // `kind` is the RPC namespace the entity belongs to (`site`, `runtime`, …), which is
            // also the noun `mix` uses, so the hint can name the command that would have listed it.
            Core::NotFound { kind, .. } => Error::new(ErrorCode::NotFound, chain(self))
                .with_hint(format!("`mix {kind} list` shows what does exist")),

            Core::Io { path, source, .. } => io_failure(chain(self), path, source),

            Core::Config { path, .. } => Error::new(ErrorCode::InvalidArgument, chain(self))
                .with_hint(format!(
                    "fix {}, or delete it and MixEngine will write a fresh one with the defaults",
                    path.display()
                )),

            // Every way this fails is a file that could not be used as a database, so `io` is what
            // a client branches on — and the reason is nearly always the directory rather than the
            // file, which is what the hint says.
            Core::Database { path, .. } => {
                Error::new(ErrorCode::Io, chain(self)).with_hint(format!(
                    "{} is opened by `mixengined` and by nothing else — a home directory that has \
                     been moved, emptied or copied from another account is the usual reason it \
                     cannot be",
                    path.display()
                ))
            }

            // The hint has to say what did *not* happen, because the database being untouched is
            // the whole point of stopping here.
            Core::Backup { path, .. } => Error::new(ErrorCode::Io, chain(self)).with_hint(format!(
                "the upgrade stopped rather than migrate a database it could not copy first — \
                 make room for {} and start MixEngine again",
                path.display()
            )),

            // Our SQL, not the user's file: `internal` is the honest name for it. The hint is
            // worth writing anyway, and it is true because each migration runs in a transaction —
            // the one that failed rolled back, so the previous release still opens this database.
            Core::Migration { .. } => Error::new(ErrorCode::Internal, chain(self)).with_hint(
                "this is a bug in this release of MixEngine — the database was left as it was, \
                 and the version you upgraded from still runs against it",
            ),

            // Not a bug and not a dead end, which is why it is not `internal`: the file is from
            // another build, and the copy taken before the upgrade is the way back.
            Core::IncompatibleDatabase { path, .. } => {
                Error::new(ErrorCode::PreconditionFailed, chain(self)).with_hint(format!(
                    "MixEngine copies the database aside before it migrates one — the \
                     `{}.bak-…` next to it is from before the upgrade that did this",
                    path.display()
                ))
            }

            // Split rather than given one code, because two different things arrive here: a
            // question about a service that is not there, and a set of specs that does not describe
            // a runnable system. Neither is `internal` — the second is a fault in whatever
            // *declared* those services and not in this machine, and calling it a bug of ours would
            // send the user looking anywhere except the one file they can fix.
            Core::Graph(error) => match error {
                GraphError::NoSuchService { .. } => Error::new(ErrorCode::NotFound, chain(self))
                    .with_hint("`mix service list` shows what does exist"),

                // The message names the services involved, and for a loop it writes the loop out;
                // what it cannot know is where they were written down, which is the whole of the
                // advice available.
                _ => Error::new(ErrorCode::InvalidArgument, chain(self)).with_hint(
                    "services are declared by MixEngine's own packages and by any `extension.toml` \
                     you have added — the edge to change is in whichever of them declares the \
                     services named above",
                ),
            },

            // The message already ends in "unset it to use this platform's default location",
            // which is the entire advice available; a hint here would be the same sentence twice.
            Core::EmptyHome => Error::new(ErrorCode::InvalidArgument, chain(self)),

            // Delegated rather than flattened: the OS failure keeps its own code, and going
            // through `chain` here would print a `#[error(transparent)]` message twice.
            Core::Platform(error) => error.to_wire(),

            // `mixengine_core::Error` is `#[non_exhaustive]`, so this arm is mandatory rather than
            // chosen. A variant that lands in it is one nobody has classified yet, and `internal`
            // is the honest name for that.
            _ => Error::new(ErrorCode::Internal, chain(self)),
        }
    }
}

impl ToWire for mixengine_platform::Error {
    fn to_wire(&self) -> Error {
        use mixengine_platform::Error as Platform;

        match self {
            // `reason` is required to describe the manual workaround where there is one
            // (`.claude/architecture/platform-abstraction.md`, rule 4), and it is already in the
            // message.
            Platform::UnsupportedPlatform { .. } => {
                Error::new(ErrorCode::UnsupportedPlatform, chain(self))
            }

            // Not `io`: nothing was touched. The environment is missing something the OS considers
            // mandatory, and the user can fix that before anything else will work.
            Platform::NoHomeDirectory { .. } => {
                Error::new(ErrorCode::PreconditionFailed, chain(self))
            }

            Platform::Io { path, source, .. } => io_failure(chain(self), path, source),

            // `io` rather than `internal`, even though most ways this can happen are a bug: the
            // rest are a machine locked down past the point where a token can be read, and greeting
            // that with "report a bug" would send somebody to the wrong place. The code says the OS
            // refused something, which is true either way.
            Platform::Os { .. } => Error::new(ErrorCode::Io, chain(self)),

            // The address is computed from `MIXENGINE_HOME`, so this is the home being wrong rather
            // than anything having failed — and `reason` already ends in what to do about it.
            Platform::Address { .. } => Error::new(ErrorCode::InvalidArgument, chain(self)),

            // Not a failure of this daemon so much as a fact about the machine, which is why the
            // hint points at the daemon that *is* running instead of at something to repair.
            Platform::EndpointInUse { .. } => Error::new(ErrorCode::Conflict, chain(self))
                .with_hint(
                    "a MixEngine daemon is already running for this home — `mix status` talks to \
                     it, and `mix daemon stop` ends it",
                ),

            // The tool's own complaint is in the message, and it is a better hint than anything
            // that could be written here.
            Platform::Command { .. } => Error::new(ErrorCode::ProcessFailed, chain(self)),

            _ => Error::new(ErrorCode::Internal, chain(self)),
        }
    }
}

impl ToWire for mixengine_supervisor::Error {
    fn to_wire(&self) -> Error {
        use mixengine_supervisor::Error as Supervisor;

        match self {
            Supervisor::Spawn { program, source } => match source.kind() {
                // A program that is not there is not a failed process — it is a missing
                // dependency, which is the code that tells a client to offer an install.
                io::ErrorKind::NotFound => Error::new(ErrorCode::DependencyMissing, chain(self))
                    .with_hint(format!(
                        "`{program}` is neither on PATH nor where the service expects it — install \
                         the runtime that provides it, or correct its path"
                    )),

                // The file is there and the OS refused to run it, which on Unix is nearly always
                // the executable bit and never something a restart fixes.
                io::ErrorKind::PermissionDenied => {
                    Error::new(ErrorCode::ProcessFailed, chain(self))
                        .with_hint(format!("`{program}` exists but is not executable"))
                }

                _ => Error::new(ErrorCode::ProcessFailed, chain(self)),
            },

            _ => Error::new(ErrorCode::Internal, chain(self)),
        }
    }
}

/// `io`, plus whatever the OS error kind implies about the way out.
///
/// Shared by `core` and `platform`, whose `Io` variants are deliberately the same shape: the path
/// belongs in the message, the OS error stays the cause, and the advice depends on which of the two
/// or three things that actually go wrong here went wrong.
fn io_failure(message: String, path: &Path, source: &io::Error) -> Error {
    let error = Error::new(ErrorCode::Io, message);

    match source.kind() {
        // The one thing a user cannot guess: MixEngine is not going to elevate its way out of this
        // one. Everything privileged is a one-shot `mixengine-elevate` call for a listed operation
        // (ADR 0005), and touching an arbitrary directory is not on the list.
        io::ErrorKind::PermissionDenied => error.with_hint(format!(
            "MixEngine runs as your own user account and never as an administrator — give \
             yourself access to {}, or move it with [paths] in config.toml",
            path.display()
        )),

        // Carefully worded: which component is missing depends on the action. `create` goes
        // through `create_dir_all`, so a failure there is the drive or the mount point rather than
        // the parent directory, while a `read` is usually the file itself. The one thing worth
        // saying is the one only this layer knows, and it holds either way.
        io::ErrorKind::NotFound => error.with_hint(
            "check the [paths] section of config.toml — a relocation onto a disk that is not \
             mounted is the usual reason a path is simply not there",
        ),

        _ => error,
    }
}

/// Flatten an error and its causes into the single string a client is given.
///
/// A local name for [`mixengine_proto::flatten`], which is where it lives because `mix` needs the
/// same one — see the note at the top of this module. Kept as a function rather than inlined at
/// every arm so that the mapping above reads the way it did when this was written here.
fn chain(error: &dyn std::error::Error) -> String {
    mixengine_proto::flatten(error)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use mixengine_core::services::GraphError;
    use mixengine_proto::ServiceId;

    use super::*;

    #[test]
    fn an_io_failure_keeps_the_path_and_says_who_is_not_going_to_fix_it() {
        let error = mixengine_core::Error::Io {
            action: "create",
            path: PathBuf::from("/srv/mixengine"),
            source: io::Error::from(io::ErrorKind::PermissionDenied),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::Io);
        assert!(
            error.message.starts_with("cannot create /srv/mixengine: "),
            "the cause is appended, not dropped: {}",
            error.message
        );
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("/srv/mixengine")),
            "the hint names the path the user has to act on: {:?}",
            error.hint
        );
    }

    #[test]
    fn a_path_that_is_not_there_points_at_the_relocation_and_not_at_a_parent() {
        let error = mixengine_core::Error::Io {
            action: "create directory",
            path: PathBuf::from("Z:/mixengine/data"),
            source: io::Error::from(io::ErrorKind::NotFound),
        }
        .to_wire();

        let hint = error.hint.expect("a missing path has advice attached");
        assert!(hint.contains("[paths]"), "{hint}");
        // `create` goes through `create_dir_all`, which makes the parents itself, and the same
        // variant covers `read`, where the leaf is what is missing. Neither claim is safe to make.
        assert!(!hint.contains("directory above"), "{hint}");
    }

    #[test]
    fn a_missing_entity_is_answered_with_the_command_that_lists_them() {
        let error = mixengine_core::Error::NotFound {
            kind: "site",
            id: "blog.test".to_owned(),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::NotFound);
        assert_eq!(error.message, "no such site: blog.test");
        assert_eq!(
            error.hint.as_deref(),
            Some("`mix site list` shows what does exist")
        );
    }

    /// The one a user can actually fix, and the one `internal` would have hidden: the loop has to
    /// survive into the message, and the code has to say whose fault it is.
    #[test]
    fn a_dependency_loop_is_the_declaration_s_fault_and_says_so() {
        let id = |value: &str| ServiceId::parse(value).expect("a valid service id");
        let error = mixengine_core::Error::Graph(GraphError::Cycle {
            path: vec![id("caddy"), id("php-fpm@8.3"), id("mariadb@main")],
        })
        .to_wire();

        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(
            error
                .message
                .ends_with("caddy → php-fpm@8.3 → mariadb@main → caddy"),
            "the loop is what says which edge to delete: {}",
            error.message
        );
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("extension.toml")),
            "the message names the services; the hint says where they were written: {:?}",
            error.hint
        );
    }

    /// The other half of the same variant, which is not a declaration failure at all.
    #[test]
    fn asking_about_a_service_that_is_not_there_is_a_plain_not_found() {
        let error = mixengine_core::Error::Graph(GraphError::NoSuchService {
            id: ServiceId::parse("mailpit").expect("a valid service id"),
        })
        .to_wire();

        assert_eq!(error.code, ErrorCode::NotFound);
        assert_eq!(
            error.hint.as_deref(),
            Some("`mix service list` shows what does exist")
        );
    }

    #[test]
    fn an_io_failure_with_nothing_to_suggest_carries_no_hint() {
        let error = mixengine_core::Error::Io {
            action: "read",
            path: PathBuf::from("/srv/mixengine/config.toml"),
            source: io::Error::from(io::ErrorKind::InvalidData),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::Io);
        assert_eq!(error.hint, None);
    }

    #[test]
    fn a_platform_failure_keeps_its_own_code_through_core() {
        let error =
            mixengine_core::Error::Platform(mixengine_platform::Error::UnsupportedPlatform {
                capability: "PortAccess",
                reason: "no pf on this system".to_owned(),
            })
            .to_wire();

        assert_eq!(error.code, ErrorCode::UnsupportedPlatform);
        assert_eq!(
            error.message,
            "PortAccess is not available on this platform: no pf on this system"
        );
    }

    #[test]
    fn a_transparent_variant_is_not_printed_twice() {
        // What `chain` would do to `Core::Platform` if the mapping flattened it instead of
        // delegating: thiserror keeps the inner error as the source *and* borrows its message.
        let error = mixengine_core::Error::Platform(mixengine_platform::Error::NoHomeDirectory {
            reason: "%LOCALAPPDATA% is not set",
        });

        let message = chain(&error);
        assert_eq!(message, error.to_string());
        assert_eq!(message.matches("%LOCALAPPDATA%").count(), 1);
    }

    #[test]
    fn a_missing_home_directory_is_a_precondition_not_an_io_failure() {
        let error = mixengine_platform::Error::NoHomeDirectory {
            reason: "$HOME is not set",
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::PreconditionFailed);
        // The message already ends in "set MIXENGINE_HOME to choose one explicitly".
        assert_eq!(error.hint, None);
    }

    #[test]
    fn an_empty_home_is_an_invalid_argument() {
        let error = mixengine_core::Error::EmptyHome.to_wire();

        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert_eq!(error.hint, None);
    }

    #[test]
    fn a_configuration_that_does_not_parse_names_the_file_in_its_hint() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let file = home.path().join(mixengine_core::config::FILE_NAME);
        std::fs::write(&file, "[log]\nlevel = \"chatty\"\n").expect("write the broken config");

        let error = mixengine_core::config::load(&file)
            .expect_err("`chatty` is not a level")
            .to_wire();

        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("config.toml")),
            "the hint names the file to fix: {:?}",
            error.hint
        );
        // `toml` ends its complaint with a newline, which would print a blank line above the hint.
        assert_eq!(error.message.trim_end(), error.message);
        assert!(
            error.message.contains("unknown variant `chatty`"),
            "the parse failure is what makes this actionable: {}",
            error.message
        );
    }

    #[test]
    fn a_database_that_cannot_be_opened_names_the_file_and_not_the_query() {
        let error = mixengine_core::Error::Database {
            action: "open",
            path: PathBuf::from("Z:/mixengine/mixengine.db"),
            source: sqlx::Error::Io(io::Error::from(io::ErrorKind::NotFound)),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::Io);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("Z:/mixengine/mixengine.db")),
            "{:?}",
            error.hint
        );
    }

    #[test]
    fn a_database_from_another_build_is_something_the_user_can_fix() {
        let error = mixengine_core::Error::IncompatibleDatabase {
            path: PathBuf::from("/home/dev/.local/share/mixengine/mixengine.db"),
            source: sqlx::migrate::MigrateError::VersionNotPresent(2),
        }
        .to_wire();

        // Not `internal`: nothing is broken, the file is simply newer than this binary.
        assert_eq!(error.code, ErrorCode::PreconditionFailed);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains(".bak")),
            "the way back is the copy taken before the upgrade: {:?}",
            error.hint
        );
    }

    #[test]
    fn a_migration_that_fails_is_ours_and_says_the_database_is_untouched() {
        let error = mixengine_core::Error::Migration {
            path: PathBuf::from("/home/dev/mixengine.db"),
            source: sqlx::migrate::MigrateError::VersionMissing(1),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::Internal);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("left as it was")),
            "{:?}",
            error.hint
        );
    }

    #[test]
    fn a_backup_that_cannot_be_written_stops_the_upgrade_and_says_so() {
        let error = mixengine_core::Error::Backup {
            path: PathBuf::from("/home/dev/mixengine.db.bak-0.1.0"),
            source: sqlx::Error::Io(io::Error::from(io::ErrorKind::StorageFull)),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::Io);
        let hint = error.hint.expect("a failed backup has advice attached");
        assert!(hint.contains("mixengine.db.bak-0.1.0"), "{hint}");
        // The database is untouched, and the user needs to know that before they panic.
        assert!(hint.contains("stopped rather than migrate"), "{hint}");
    }

    #[test]
    fn a_program_that_is_not_installed_is_a_missing_dependency() {
        let error = mixengine_supervisor::Error::Spawn {
            program: "php-fpm".to_owned(),
            source: io::Error::from(io::ErrorKind::NotFound),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::DependencyMissing);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("php-fpm")),
            "the hint names the program: {:?}",
            error.hint
        );
    }

    #[test]
    fn a_program_that_will_not_run_is_a_failed_process() {
        let error = mixengine_supervisor::Error::Spawn {
            program: "php-fpm".to_owned(),
            source: io::Error::from(io::ErrorKind::PermissionDenied),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::ProcessFailed);
    }
}
