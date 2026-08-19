//! The services this build knows how to run.
//!
//! One module per `packages.name`, each one a [`Recipe`](super::Recipe) and each one its own roadmap
//! task: Caddy is T31 and php-fpm T32, with MariaDB T33, PostgreSQL T34, Redis and Memcached T35
//! still to come. The machinery they are plugged into — the merge, the render, the diff, the
//! staging, the validation — is T30's and lives one directory up; what a module in here owns is a
//! template, the overrides worth having, and the [`ServiceSpec`] that runs the program.
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
pub mod php_fpm;

pub use caddy::Caddy;
pub use php_fpm::PhpFpm;
