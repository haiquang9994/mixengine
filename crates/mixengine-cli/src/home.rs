//! Which home this `mix` is talking about, and where that home's daemon listens.
//!
//! Both answers have to come out the same as the daemon's or the two would be talking past each
//! other, and the daemon reaches them through `mixengine_core::Paths`. This client does not: `core`
//! carries `sqlx`, and linking a bundled SQLite into `mix` to learn where a socket goes is not a
//! trade worth making — `mix` is the binary that has to start in milliseconds because a person is
//! waiting for it. So the two rules it needs are restated here, and
//! [`tests/status.rs`](../tests/status.rs) starts a real daemon and a real client against one
//! temporary home to prove they still agree.

use std::path::{Path, PathBuf};

use mixengine_platform::Host;
use mixengine_platform::ipc::Endpoint;
use mixengine_proto::{Error, ErrorCode};

use crate::error::to_wire;

/// The daemon's runtime scratch directory, relative to the root.
///
/// Safe to know for a reason rather than by luck: `run/` is the one directory `[paths]` cannot
/// move. `mixengine_core::paths::Paths::new` passes `None` for it deliberately, so that the
/// single-instance lock and the endpoint can never end up in two different places and let two
/// daemons both own one home. Everything an override *can* move — `data/`, `runtimes/`, `logs/` —
/// is something only the daemon ever opens, which is why a client can be this incurious about
/// `config.toml`.
const RUN: &str = "run";

/// Decide which directory is `MIXENGINE_HOME` for this invocation.
///
/// The same three steps `mixengine_core::paths::resolve_root` takes, and it has to stay that way:
/// an override wins outright, the platform decides when there is none, and the result is made
/// absolute and spelled in full rather than canonicalised — the daemon may already be running against a home that has
/// since been renamed, and `canonicalize` would both require the directory to exist and hand back a
/// `\\?\` path on Windows that no endpoint fingerprint would match.
///
/// **The spelling is part of the agreement and not a detail.**
/// [`mixengine_platform::paths::in_full`] resolves an 8.3 alias to the name behind it, and a
/// client that skipped it would derive its endpoint from one spelling of a home while the
/// daemon derived one from the other.
///
/// # Errors
///
/// [`ErrorCode::InvalidArgument`] when the override is empty, and whatever the platform layer says
/// when there is no override and the OS cannot name a data directory.
pub(crate) fn resolve_root(override_: Option<&Path>, host: &dyn Host) -> Result<PathBuf, Error> {
    let root = match override_ {
        // Unreachable from a command line — `clap` refuses `--home ""` with its own usage error,
        // which `tests/status.rs` pins because neither this function nor `core`'s equivalent can
        // see it. Kept anyway, and for the same reason `core` keeps its: the one thing that must
        // never happen is an empty override being read as "not given", which would silently point
        // a sandbox run at the real install, and this is the last place that can say so.
        Some(path) if path.as_os_str().is_empty() => {
            return Err(Error::new(
                ErrorCode::InvalidArgument,
                "the home directory is empty — unset --home and MIXENGINE_HOME to use this \
                 platform's default location",
            ));
        }
        Some(path) => path.to_path_buf(),
        None => host
            .home_dirs()
            .default_home()
            .map_err(|error| to_wire(&error))?,
    };

    let absolute = std::path::absolute(&root).map_err(|source| {
        Error::new(
            ErrorCode::Io,
            format!("cannot resolve {}: {source}", root.display()),
        )
    })?;

    Ok(mixengine_platform::paths::in_full(&absolute))
}

/// Where the daemon for `root` listens, whether or not one is running.
///
/// # Errors
///
/// [`ErrorCode::InvalidArgument`] when the OS will not accept an address derived from this home —
/// on Unix a root nested deeply enough that the socket path exceeds `sun_path`.
pub(crate) fn endpoint(root: &Path) -> Result<Endpoint, Error> {
    Endpoint::in_run_dir(&root.join(RUN)).map_err(|error| to_wire(&error))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A host whose default home is somewhere a test can name without touching the real one.
    fn host() -> mixengine_platform::mock::Host {
        mixengine_platform::mock::Host::with_home(if cfg!(windows) {
            r"C:\default-home"
        } else {
            "/default-home"
        })
    }

    #[test]
    fn an_override_wins_and_is_made_absolute_rather_than_canonical() {
        let root = resolve_root(Some(Path::new("mixengine-home")), &host()).expect("a root");

        // Absolute because the daemon it may go on to start outlives any working directory, and
        // relative-to-cwd would quietly follow that around.
        assert!(root.is_absolute(), "{root:?}");
        assert!(root.ends_with("mixengine-home"), "{root:?}");
        assert!(!root.starts_with("/default-home"), "{root:?}");
    }

    #[test]
    fn without_an_override_the_platform_decides() {
        let root = resolve_root(None, &host()).expect("a root");

        assert!(root.ends_with("default-home"), "{root:?}");
    }

    #[test]
    fn an_empty_override_is_refused_instead_of_meaning_the_default() {
        let error = resolve_root(Some(Path::new("")), &host())
            .expect_err("an empty home is not the same as no home");

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn a_machine_that_cannot_name_a_data_directory_says_so_rather_than_guessing() {
        let error = resolve_root(None, &mixengine_platform::mock::Host::without_home())
            .expect_err("there is no default home to fall back to");

        assert_eq!(error.code, ErrorCode::PreconditionFailed);
        assert!(
            error.message.contains("MIXENGINE_HOME"),
            "{}",
            error.message
        );
    }

    #[test]
    fn the_endpoint_is_the_one_the_daemon_computes_for_the_same_home() {
        // The claim `home.rs` rests on, stated where a change to either side has to face it: the
        // endpoint belongs to `<root>/run`, and `[paths]` cannot move that directory. The end-to-end
        // half of this — that the daemon agrees — is `tests/status.rs`.
        let root = PathBuf::from(if cfg!(windows) {
            r"C:\mixengine"
        } else {
            "/srv/mixengine"
        });

        assert_eq!(
            endpoint(&root).expect("an endpoint"),
            Endpoint::in_run_dir(&root.join("run")).expect("the same endpoint")
        );
    }
}
