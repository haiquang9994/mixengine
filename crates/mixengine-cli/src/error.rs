//! Where a platform failure becomes the error `mix` prints.
//!
//! The daemon has a boundary like this one and it is deliberately not shared. The two map the same
//! enum for different readers: `mixengine-daemon` turns a failure into something a *client* is told
//! about, while this turns one into something the person who typed the command is told about — and
//! the advice differs at exactly the interesting points. A refused endpoint means "another daemon
//! is already running" to the daemon that could not bind it, and "the home you asked about belongs
//! to somebody else" to a client that could not dial it.
//!
//! What the two do share is [`mixengine_proto::flatten`], because the shape of the message is a
//! property of the wire error rather than of either binary.
//!
//! Only the failures a client can genuinely reach are classified here. `mix` binds nothing, runs no
//! external tool and asks the platform layer for exactly three things — the default home, the
//! endpoint address for a home, and a connection to it.

use mixengine_proto::{Error, ErrorCode, flatten};

/// Translate a platform failure into the error `mix` reports.
pub(crate) fn to_wire(error: &mixengine_platform::Error) -> Error {
    use mixengine_platform::Error as Platform;

    match error {
        // The environment is missing something the OS considers mandatory, and nothing was
        // attempted — so not `io`. The message already ends in "set MIXENGINE_HOME to choose one
        // explicitly", which is the entire advice available.
        Platform::NoHomeDirectory { .. } => {
            Error::new(ErrorCode::PreconditionFailed, flatten(error))
        }

        // The address is computed from the home, so this is the home being wrong rather than
        // anything having failed, and `reason` already names the constraint it broke.
        Platform::Address { .. } => Error::new(ErrorCode::InvalidArgument, flatten(error)),

        // Reading this account's SID, on Windows. `io` rather than `internal` for the reason the
        // daemon gives: the honest reading of a machine locked down that far is not "report a bug".
        Platform::Os { .. } => Error::new(ErrorCode::Io, flatten(error)),

        Platform::Io { source, .. } => {
            let failure = Error::new(ErrorCode::Io, flatten(error));

            match source.kind() {
                // On Unix the socket is mode 0600 inside a 0700 `run/`, and on Windows the pipe's
                // DACL names one account: if the OS refused this process, the home belongs to
                // somebody else. That is a `--home` or a `MIXENGINE_HOME` pointing somewhere
                // surprising far more often than it is a permission to repair.
                std::io::ErrorKind::PermissionDenied => failure.with_hint(
                    "a MixEngine home is readable only by the account that owns it — check which \
                     home --home or MIXENGINE_HOME is pointing at",
                ),

                _ => failure,
            }
        }

        // `UnsupportedPlatform`, `EndpointInUse` and `Command` are unreachable from a client: it
        // binds nothing and runs no external tool, and the one capability it asks for exists on all
        // three systems. The arm is required because the enum is `#[non_exhaustive]`, and
        // `internal` is the honest name for a failure nobody has classified for this side yet.
        _ => Error::new(ErrorCode::Internal, flatten(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_home_the_os_cannot_name_is_a_precondition_and_needs_no_hint_of_ours() {
        let error = to_wire(&mixengine_platform::Error::NoHomeDirectory {
            reason: "%LOCALAPPDATA% is not set",
        });

        assert_eq!(error.code, ErrorCode::PreconditionFailed);
        assert!(
            error.message.contains("MIXENGINE_HOME"),
            "{}",
            error.message
        );
        assert_eq!(error.hint, None, "the message already says what to do");
    }

    #[test]
    fn an_endpoint_this_account_may_not_open_says_whose_home_it_might_be() {
        let error = to_wire(&mixengine_platform::Error::Io {
            action: "connect to",
            path: std::path::PathBuf::from("/srv/other/run/mixengined.sock"),
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        });

        assert_eq!(error.code, ErrorCode::Io);
        // The cause is appended rather than dropped: "permission denied" alone names nothing, and
        // the path alone does not say what happened to it.
        assert!(
            error
                .message
                .starts_with("cannot connect to /srv/other/run/mixengined.sock: "),
            "{}",
            error.message
        );
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("MIXENGINE_HOME")),
            "{:?}",
            error.hint
        );
    }

    #[test]
    fn an_ordinary_io_failure_carries_no_advice_it_cannot_back_up() {
        let error = to_wire(&mixengine_platform::Error::Io {
            action: "connect to",
            path: std::path::PathBuf::from("/home/dev/.local/share/mixengine/run/mixengined.sock"),
            source: std::io::Error::from(std::io::ErrorKind::ConnectionAborted),
        });

        assert_eq!(error.code, ErrorCode::Io);
        assert_eq!(error.hint, None);
    }
}
