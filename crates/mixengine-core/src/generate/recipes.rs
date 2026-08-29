//! The services this build knows how to run.
//!
//! One module per `packages.name`, each one a [`Recipe`](super::Recipe) and each one its own roadmap
//! task: Caddy is T31, php-fpm T32, MariaDB T33, PostgreSQL T34, Redis and Memcached both T35,
//! MySQL T34c and Nginx T37. The machinery they are plugged into — the merge, the render, the diff,
//! the staging, the validation — is T30's and lives one directory up; what a module in here owns is
//! a template, the overrides worth having, and the [`ServiceSpec`] that runs the program.
//!
//! **Two of them are for the same job, and only one of them may be doing it.** [`caddy`] and
//! [`nginx`] are the two programs a site can be reached through, and
//! [`Role::FrontEnd`](super::Role) is how they say so — a rule about a *job*, which
//! [`Instancing`](super::Instancing) cannot express because it is about a package. Everything else
//! about the pair is a difference in how one server answers what the other answers differently, and
//! `crates/mixengine-cli/tests/harness/frontend.rs` is the sequence both have to walk.
//!
//! **One of them renders nothing at all.** Memcached has no configuration file format — every
//! setting is a command-line flag — so its overrides land in the spec's arguments and it is the one
//! service with no `etc/<service-id>/` directory. See [`memcached`].
//!
//! **Two of them are the same programs under different names, and are not the same product.**
//! [`mariadb`] and [`mysql`] both publish a `mysqld`-shaped server, a `my.cnf` and a client that
//! reads `MYSQL_PWD`, and everything a person maintains against one of them — the bootstrap, the
//! grant tables, which accounts exist afterwards — differs. They are two recipes and two packages,
//! and the one thing they share is the port they would both like: 3306, which is why an allocation
//! happens when a row is written rather than at start ([`crate::services::ports`]).
//!
//! **One of them has to create something before it can run.** A database is a rendered file, a
//! command line, *and* a data directory that a different program makes once, with a credential that
//! must exist nowhere on disk — see [`Recipe::ritual`](super::Recipe::ritual) and
//! [`first_run`](super::first_run). **Two of them do**: MariaDB is the first and PostgreSQL the
//! second, and everything either of them needs is on the trait rather than inside one module.
//!
//! **One of them does not come out of a package.** php-fpm's process lives inside a PHP that
//! `runtime.install` put in `runtime_installs`, which is what [`Recipe::source`](super::Source) is
//! for — see [`php_fpm`].
//!
//! **Compiled in, not published.** The reasoning is [`super::recipe`]'s and is worth repeating only
//! as the consequence it has for this directory: a template that changes with a MixEngine release
//! belongs in a MixEngine release. The package index describes a *download*.
//!
//! [`ServiceSpec`]: mixengine_proto::ServiceSpec

pub mod caddy;
pub mod mariadb;
pub mod memcached;
pub mod mysql;
pub mod nginx;
pub mod php_fpm;
pub mod postgres;
pub mod redis;

pub use caddy::Caddy;
pub use mariadb::Mariadb;
pub use memcached::Memcached;
pub use mysql::Mysql;
pub use nginx::Nginx;
pub use php_fpm::PhpFpm;
pub use postgres::Postgres;
pub use redis::Redis;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mixengine_proto::Millis;

use crate::generate::first_run::Step;
use crate::generate::recipe::Context;
use crate::{Error, Result};

/// What `sockaddr_un` can hold, measured against a real server in T33a.
///
/// Here rather than in one recipe because two of them listen on a socket and the limit belongs to
/// the kernel, not to php-fpm: a path longer than this does not fail at `bind`, the server starts,
/// gets some way in, and aborts in a way that reads like a different failure entirely. MariaDB's is
/// the worse of the two — it aborts *after* InnoDB has started, which reads as a storage failure.
const SOCKET_PATH_LIMIT: usize = 103;

/// Where the Unix bootstrap sees this install and this data directory from.
///
/// **Both databases, from one place.** Lifted out of [`mariadb`] by T34c, when MySQL 5.6 turned out
/// to need exactly it: upstream's `mysql_install_db` is the ancestor of `mariadb-install-db` and
/// leaves `$basedir` unquoted in the same places, so two copies of the workaround would be two
/// things to fix on the day that bug is.
///
/// **Upstream's script leaves both `$basedir` and `$datadir` unquoted**, so either containing a space
/// is split into two arguments and the script stops with `Could not find my_print_defaults` or
/// `Cannot change ownership of the database directories`. It is upstream's escaping, it has nothing
/// to do with relocation, and it fails identically for a user whose home has a space in it — which
/// on macOS and Linux is a real user rather than a hypothetical one.
///
/// `/tmp` rather than the home, because the home is where the space is: a MixEngine root under
/// `/Users/Nguyen Hai Quang/.mixengine` puts one in every path it owns, `run/` included. `/tmp` is
/// POSIX and has no space in it on any system this runs on.
///
/// **Always, rather than only when a path contains a space**, which is the cheaper of the two
/// mistakes available: one code path, exercised by every first run on every Unix machine, instead of
/// a branch that is only ever taken on the developer machines nobody tests on.
pub(super) fn space_free_view(context: &Context) -> PathBuf {
    PathBuf::from("/tmp").join(format!("mixengine-init-{}", context.service().as_str()))
}

/// Make that view: a fresh directory with a link to the install and a link to the data directory.
///
/// One `sh -c` rather than four steps, and **the quoting is ours rather than upstream's** — the
/// paths arrive as positional arguments and are used quoted, which is precisely what the script this
/// works around does not do.
///
/// It removes the view first, so a ritual that failed half-way leaves nothing the next one trips
/// over. The cleanup after a *successful* run is the last step; a failed run's leftovers are cleared
/// by the next attempt rather than by an unwinding path that would itself have to be right.
pub(super) fn link_a_space_free_view(context: &Context, view: &Path) -> Step {
    Step {
        label: "make a space-free view of the install, which upstream's script requires".to_owned(),
        program: PathBuf::from("/bin/sh"),
        args: vec![
            "-c".to_owned(),
            "rm -rf \"$1\" && mkdir -p \"$1\" && ln -s \"$2\" \"$1/basedir\" && \
             mkdir -p \"$3\" && ln -s \"$3\" \"$1/datadir\""
                .to_owned(),
            "sh".to_owned(),
            view.display().to_string(),
            context.install_path().display().to_string(),
            context.data().display().to_string(),
        ],
        stdin: None,
        secret_file: None,
        env: BTreeMap::new(),
        cwd: PathBuf::from("/tmp"),
        timeout: Millis(30_000),
    }
}

/// And take it away again once the bootstrap is done with it.
pub(super) fn remove_the_space_free_view(view: &Path) -> Step {
    Step {
        label: "remove the space-free view".to_owned(),
        program: PathBuf::from("/bin/sh"),
        args: vec![
            "-c".to_owned(),
            "rm -rf \"$1\"".to_owned(),
            "sh".to_owned(),
            view.display().to_string(),
        ],
        stdin: None,
        secret_file: None,
        env: BTreeMap::new(),
        cwd: PathBuf::from("/tmp"),
        timeout: Millis(30_000),
    }
}

/// `socket` unchanged, or the refusal that names the number.
///
/// Refusing here, by name and with the limit in the message, is the difference between a sentence
/// somebody can act on and an afternoon.
///
/// # Errors
///
/// [`Error::SettingValue`] — the variant that names the service and the reason — when the path this
/// home would need is longer than the kernel accepts.
fn within_socket_limit(service: &str, key: &'static str, socket: &Path) -> Result<()> {
    if socket.as_os_str().len() > SOCKET_PATH_LIMIT {
        return Err(Error::SettingValue {
            service: service.to_owned(),
            key,
            value: socket.display().to_string(),
            reason: "a Unix socket path is capped at 103 characters by `sockaddr_un`, and a server                      given a longer one aborts after it has started — move the MixEngine home                      somewhere shorter",
        });
    }

    Ok(())
}

/// Where the activator listens for the service whose own socket is `socket` — roadmap task **T70**.
///
/// `run/php-fpm-8.3.sock` becomes `run/php-fpm-8.3.activate.sock`: the same directory, the same
/// name, one component inserted before the extension. A site file names both, the pool first, so
/// that a request arriving while the pool is idle-stopped is retried against this one instead of
/// answered with a 502 — the design's D2 and D3.
///
/// **Derived rather than allocated, because a rendered site file must not move.** The address is the
/// same string whether the pool is up or down, so an idle stop rewrites nothing and reloads nothing.
/// That is free for a socket and is *not* available for a TCP pool, whose activator port is
/// allocated once onto its row instead — see [`super::super::services::ports`].
///
/// # Errors
///
/// [`Error::SettingValue`] when the derived path is longer than `sockaddr_un` accepts. Checked
/// against the derived path and never inherited from `socket`: `.activate` is nine characters, which
/// is enough to put a home that was just inside the limit outside it, and the failure that would
/// otherwise follow is a bind refused at run time and a site that 502s for no visible reason.
fn activator_socket(service: &str, key: &'static str, socket: &Path) -> Result<PathBuf> {
    let mut name = socket
        .file_stem()
        .unwrap_or(socket.as_os_str())
        .to_os_string();

    name.push(".activate");

    if let Some(extension) = socket.extension() {
        name.push(".");
        name.push(extension);
    }

    let activator = socket.with_file_name(name);

    within_socket_limit(service, key, &activator)?;

    Ok(activator)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pool's socket in a home short enough for both, and the activator's beside it.
    #[test]
    fn an_activator_listens_beside_the_pool_it_would_start() {
        let pool = Path::new("/home/x/.mixengine/run/php-fpm-8.3.sock");

        assert_eq!(
            activator_socket("php-fpm@8.3", "listen", pool).expect("a short enough home"),
            Path::new("/home/x/.mixengine/run/php-fpm-8.3.activate.sock")
        );
    }

    /// **The dots in `8.3` are not an extension**, which is the whole reason this is a function
    /// rather than a `format!`: splitting on the first dot would render `php-fpm-8.activate.3.sock`
    /// and the site file would name a socket nothing ever binds.
    #[test]
    fn a_version_with_a_dot_in_it_keeps_its_dot() {
        let pool = Path::new("/run/php-fpm-8.3.sock");

        assert_eq!(
            activator_socket("php-fpm@8.3", "listen", pool).expect("a short path"),
            Path::new("/run/php-fpm-8.3.activate.sock")
        );
    }

    /// **A home that fits the pool's socket and not the activator's is refused here.**
    ///
    /// Nine characters is enough to cross `sockaddr_un`'s line for a home that was just inside it,
    /// so the check is made against the derived path and not inherited from the original. Without
    /// this the pool starts, the site renders, and the activator fails to bind at run time — which
    /// reads as a site that 502s for no reason anybody can see.
    #[test]
    fn a_home_that_fits_the_pool_and_not_the_activator_is_refused() {
        // Built to land exactly on the limit rather than computed to — the arithmetic is easy to get
        // wrong by one, and a test that was accidentally over the line would pass for the wrong
        // reason. `/` is written into the string so the separator is not this OS's to choose.
        const NAME: &str = "php-fpm-8.3.sock";

        let pool = PathBuf::from(format!(
            "/{}/{NAME}",
            "d".repeat(SOCKET_PATH_LIMIT - NAME.len() - 2)
        ));

        assert_eq!(
            pool.as_os_str().len(),
            SOCKET_PATH_LIMIT,
            "the pool's own socket must sit exactly on the limit for this test to mean anything: {}",
            pool.display()
        );

        within_socket_limit("php-fpm@8.3", "listen", &pool).expect("the pool itself fits");

        let refused = activator_socket("php-fpm@8.3", "listen", &pool)
            .expect_err("the activator's is nine characters longer and does not");

        assert!(
            matches!(refused, Error::SettingValue { key: "listen", .. }),
            "{refused:?}"
        );
    }
}
