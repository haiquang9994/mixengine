//! Where a library error becomes the one a client sees.
//!
//! Every crate below keeps its own `thiserror` enum, shaped for the code that raises it and knowing
//! nothing about codes, hints or the wire. The translation happens here, once, at the boundary —
//! `.claude/standards/rust.md`. Three things happen in it, and none of them belong anywhere else:
//!
//! - **The chain is flattened.** A client is handed one string and has no `source()` to walk, so
//!   every cause is folded into the message before it leaves.
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
        use mixengine_core::Error as Core;

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
/// The library messages are written for this: none of them repeats its own `#[source]`, so the
/// result reads as one sentence with its causes appended — `cannot create C:\…: Access is denied.`
///
/// Every piece is trimmed on the way in, because not every cause is ours: `toml::de::Error` ends
/// its (deliberately multi-line) complaint with a newline, and an unnoticed one puts a blank line
/// between the message and the hint in a terminal and a stray `\n` at the end of a JSON string.
fn chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string().trim_end().to_owned();
    let mut cause = error.source();

    while let Some(error) = cause {
        let text = error.to_string().trim_end().to_owned();

        // `#[error(transparent)]` gives a variant its inner error's message *and* keeps that error
        // as the source, so a naive walk prints the same sentence twice. The variants that do this
        // are delegated above rather than flattened, but the guard costs nothing and the next one
        // added will not have to remember.
        if !message.ends_with(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }

        cause = error.source();
    }

    message
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

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
