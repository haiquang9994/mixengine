//! The services this build knows how to run.
//!
//! One module per `packages.name`, each one a [`Recipe`](super::Recipe) and each one its own roadmap
//! task: Caddy is T31, php-fpm T32, MariaDB T33 and PostgreSQL T34, with Redis and Memcached T35
//! still to come. The machinery they are plugged into — the merge, the render, the diff, the
//! staging, the validation — is T30's and lives one directory up; what a module in here owns is a
//! template, the overrides worth having, and the [`ServiceSpec`] that runs the program.
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
pub mod php_fpm;
pub mod postgres;

pub use caddy::Caddy;
pub use mariadb::Mariadb;
pub use php_fpm::PhpFpm;
pub use postgres::Postgres;

use std::path::Path;

use crate::{Error, Result};

/// What `sockaddr_un` can hold, measured against a real server in T33a.
///
/// Here rather than in one recipe because two of them listen on a socket and the limit belongs to
/// the kernel, not to php-fpm: a path longer than this does not fail at `bind`, the server starts,
/// gets some way in, and aborts in a way that reads like a different failure entirely. MariaDB's is
/// the worse of the two — it aborts *after* InnoDB has started, which reads as a storage failure.
const SOCKET_PATH_LIMIT: usize = 103;

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
