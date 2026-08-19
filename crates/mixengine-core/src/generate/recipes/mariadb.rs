//! MariaDB: the database MixEngine runs for a site — roadmap task **T33**.
//!
//! **The first recipe with something to do before it can ever start.** Caddy and php-fpm are a
//! rendered file and a command line; a database is a rendered file, a command line, *and* a data
//! directory that has to be created once by a different program, with a root password that must
//! exist nowhere on disk. The second half is [`Recipe::ritual`] and
//! [`first_run`](crate::generate::first_run); this module's own share of it is the steps.
//!
//! # What was measured rather than assumed
//!
//! Every platform difference below comes out of T33a, which ran a real server on all three systems
//! rather than reading about one:
//!
//! - **`mariadb-install-db` is two different programs.** A shell script on Unix, a C++ program of
//!   the same name on Windows, sharing almost no options —
//!   `--auth-root-authentication-method=normal` is Unix-only and the Windows build answers `unknown
//!   variable` and exits 7.
//! - **Upstream's script does not quote `$basedir`**, so a path with a space in it is split into two
//!   arguments and the script stops with "Could not find my_print_defaults". A user called
//!   `Nguyen Hai Quang` is not an edge case.
//! - **`sockaddr_un` caps a socket path at 103 characters** and the server aborts *after* InnoDB has
//!   started, which reads like a storage failure and is not one.
//! - **Windows `mariadbd` writes only to `<datadir>/<hostname>.err`** and sends nothing to stdout,
//!   so a supervisor reading the process's own output finds an empty file.
//! - **MariaDB's option parser treats `\` as an escape** and everything after an unquoted `#` as a
//!   comment, which is why every path in the rendered file is quoted and forward-slashed.
//!
//! # What this recipe deliberately does not do
//!
//! **No reload.** MariaDB reads its configuration once, at startup. A changed override takes effect
//! when somebody restarts the service, and the daemon does not restart what nobody asked it to.
//!
//! **No `unix_socket` authentication.** Root is created with a password, which is the one
//! arrangement that means the same thing on all three systems — and the credential lives in the OS
//! keyring, never in the rendered file (ADR 0006).
//!
//! **No `mariadb-upgrade`.** Running a data directory bootstrapped by one version against a later
//! one is a task of its own; what is recorded here is the version that performed the bootstrap, in
//! [`READY_MARKER`](crate::generate::first_run::READY_MARKER), so that task has something to read.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use mixengine_platform::KEYRING_SERVICE;
use mixengine_proto::{
    HealthCheck, HealthProbe, Millis, ReadyCheck, ServiceSpec, ServiceSpecBuilder, StopBehaviour,
};

use crate::generate::recipe::{Context, Endpoints, Instancing, Recipe, TemplateFile};
use crate::generate::settings::{Preset, Setting};
use crate::{Error, Result};

/// The `packages.name` this recipe is found under.
pub const PACKAGE: &str = "mariadb";

/// The server. Asked for by name rather than by path: the archive puts it at `bin/mariadbd`, and
/// upstream renamed it from `mysqld` between 10.4 and 10.6 — so the index's own `provides` map is
/// what knows.
const SERVER: &str = "mariadbd";

/// The client the readiness check, the health check and the shutdown all run.
const ADMIN: &str = "mariadb-admin";

/// The rendered configuration, under `etc/<service-id>/`.
const CONFIG_FILE: &str = "my.cnf";

/// How much memory InnoDB is given. **Dev-tuned rather than upstream's**: this is a laptop running a
/// development site beside an editor and a browser, not a server whose whole job is the database.
const BUFFER_POOL: &str = "innodb_buffer_pool_size";

/// How many connections it accepts at once. MariaDB's own default, which is ample for one machine.
const MAX_CONNECTIONS: &str = "max_connections";

/// The server's character set.
const CHARACTER_SET: &str = "character_set";

/// The collation that goes with it.
///
/// `utf8mb4_general_ci` and **not** 11.4's own `utf8mb4_uca1400_ai_ci`, deliberately: this recipe is
/// found by package name and runs whichever series a user installed, and the 1400 collations do not
/// exist before 11.4. A default that fails to start on 10.11 is a default nobody can use.
const COLLATION: &str = "collation";

/// How long the server is given to answer a ping before the start is a failure, in milliseconds.
///
/// Two minutes. Long for what it usually is — a warm start answers in a second or two — and it is
/// not the warm start this is sized for: a first InnoDB recovery after an unclean stop reads the
/// whole redo log before it accepts a single query.
const READY_TIMEOUT: &str = "ready_timeout_ms";

/// How long `mariadb-admin shutdown` is given before the process group is killed, in milliseconds.
///
/// A minute, and for the same reason: flushing a dirty buffer pool is what this waits for, and a
/// database killed part-way through it is a database that recovers on the next start.
const STOP_GRACE: &str = "stop_grace_ms";

/// How often the server is asked whether it is still answering.
const HEALTH_INTERVAL: Millis = Millis(10_000);

/// How long one of those may take. Well inside the interval, which [`ServiceSpec::validate`] insists
/// on: two probes that could overlap are two probes that can queue.
const HEALTH_TIMEOUT: Millis = Millis(5_000);

/// The environment variable MariaDB's own clients read a password from.
///
/// **The reason there is no password on any command line this recipe builds.** An argument list is
/// visible to every process on the machine through `ps` and Task Manager; an environment is not, and
/// this is the variable the client library was given for exactly that.
pub(super) const PASSWORD_VARIABLE: &str = "MYSQL_PWD";

/// The account the ritual creates and everything here authenticates as.
pub(super) const ROOT: &str = "root";

/// MariaDB, as MixEngine runs it.
#[derive(Debug)]
pub struct Mariadb;

impl Recipe for Mariadb {
    fn package(&self) -> &'static str {
        PACKAGE
    }

    /// As many as are named: `mariadb@main`, `mariadb@legacy`. A machine that serves two projects
    /// with incompatible schemas needs two databases, not one with two schemas in it.
    fn instancing(&self) -> Instancing {
        Instancing::Named
    }

    /// `mariadbd --version`, which is cheap and touches the server's own machinery.
    fn smoke_test(&self) -> Option<crate::install::SmokeTest> {
        Some(crate::install::SmokeTest {
            executable: SERVER.to_owned(),
            args: vec!["--version".to_owned()],
        })
    }

    fn settings(&self) -> &'static [Setting] {
        &[
            Setting {
                key: BUFFER_POOL,
                default: Preset::Text("128M"),
            },
            Setting {
                key: MAX_CONNECTIONS,
                default: Preset::Number(151),
            },
            Setting {
                key: CHARACTER_SET,
                default: Preset::Text("utf8mb4"),
            },
            Setting {
                key: COLLATION,
                default: Preset::Text("utf8mb4_general_ci"),
            },
            Setting {
                key: READY_TIMEOUT,
                default: Preset::Number(120_000),
            },
            Setting {
                key: STOP_GRACE,
                default: Preset::Number(60_000),
            },
        ]
    }

    fn files(&self) -> &'static [TemplateFile] {
        &[TemplateFile {
            path: CONFIG_FILE,
            source: include_str!("mariadb/my.cnf"),
        }]
    }

    /// The socket on a system that has them, and the plugin directory on the one that needs saying.
    ///
    /// [`cfg!`] is a *value* and not an attribute, so both arms compile everywhere and a test can
    /// exercise the branch the machine it runs on is not.
    fn endpoints(&self, context: &Context) -> Result<Endpoints> {
        if cfg!(windows) {
            return Ok(Endpoints {
                socket: None,
                plugins: Some(context.install_path().join("lib").join("plugin")),
            });
        }

        Ok(Endpoints {
            socket: Some(socket_path(context)?),
            plugins: None,
        })
    }

    /// The server, and the three things that are all one client run with one credential.
    ///
    /// **Readiness, health and shutdown all speak SQL over TCP**, and every one of them is the same
    /// decision: a TCP accept proves a listener, which stays true for the whole of InnoDB's crash
    /// recovery while the server refuses every query. `--protocol=TCP` even on Unix, where a socket
    /// exists, because the socket is the thing the *configuration* names and the port is the thing
    /// the row does — and a probe should ask the question a client would.
    fn spec(&self, context: &Context) -> Result<ServiceSpecBuilder> {
        let settings = context.settings();
        let server = context.provided(SERVER)?;
        let admin = context.provided(ADMIN)?;
        let addr = address(context)?;

        Ok(ServiceSpec::builder(context.service().clone(), &server)
            .args([
                // First, and it is the difference between running this instance and running whatever
                // the machine already has: without it the server reads `/etc/mysql/my.cnf`, and a
                // user with a MariaDB of their own has one naming a datadir, a socket and a port.
                // Silently inheriting any of them would be writing into somebody else's database.
                "--no-defaults".to_owned(),
                format!("--defaults-file={}", context.config(CONFIG_FILE).display()),
            ])
            .cwd(context.etc())
            // The credential the three client runs below need, named rather than carried: a spec is
            // data and cannot hold a password (ADR 0006). The supervisor reads it out of the OS
            // keyring at spawn time and keeps the resolved environment for the life of the process,
            // which is what lets a health probe and a shutdown authenticate too.
            .env_from_keyring(PASSWORD_VARIABLE, KEYRING_SERVICE, keyring_key(context))
            .ready(ReadyCheck::Command {
                program: admin.clone(),
                args: ping(addr),
                timeout: millis(settings.number(READY_TIMEOUT)),
            })
            .health(HealthCheck {
                probe: HealthProbe::Command {
                    program: admin.clone(),
                    args: ping(addr),
                },
                interval: HEALTH_INTERVAL,
                timeout: HEALTH_TIMEOUT,
                // Three rather than one: a probe can miss its window behind a checkpoint flush on a
                // busy database, and that is not a sick server.
                failures_before_degraded: 3,
                successes_before_running: 1,
            })
            // **Asked rather than signalled**, and this is the whole reason `StopBehaviour::Command`
            // exists: a terminated `mariadbd` leaves a dirty buffer pool, and the next start pays for
            // it with a crash recovery that reads the entire redo log. `shutdown` flushes first.
            .stop(StopBehaviour::Command {
                program: admin,
                args: {
                    let mut args = connection(addr);
                    args.push("shutdown".to_owned());
                    args
                },
                grace: millis(settings.number(STOP_GRACE)),
            }))
        // **No reload.** MariaDB reads its configuration once, at startup — there is no signal and
        // no command that would make it read this file again, so a changed override waits for a
        // restart somebody asked for.
    }
}

/// Where this instance's credential lives inside [`KEYRING_SERVICE`].
///
/// The service id and the account: `mariadb@main/root`. The id rather than the package name, because
/// two instances are two databases with two different passwords.
pub(super) fn keyring_key(context: &Context) -> String {
    format!("{}/{ROOT}", context.service().as_str())
}

/// How a client is told which server to ask and who to be.
///
/// One function rather than three copies, because the readiness check, the health check and the
/// shutdown must reach the *same* server: a probe that reached a different one would report a
/// service that is not this one.
fn connection(addr: SocketAddr) -> Vec<String> {
    vec![
        "--protocol=TCP".to_owned(),
        format!("--host={}", addr.ip()),
        format!("--port={}", addr.port()),
        format!("--user={ROOT}"),
    ]
}

/// [`connection`], asking the one question that proves the server answers queries.
fn ping(addr: SocketAddr) -> Vec<String> {
    let mut args = connection(addr);
    args.push("ping".to_owned());
    args
}

/// Where this instance listens on a system with Unix sockets.
///
/// `run/` and not the data directory, and short on purpose: the kernel's cap on a socket path is the
/// whole reason — see [`within_socket_limit`](super::within_socket_limit) — and `run/` is near the
/// top of the home while a data directory is two levels down inside one whose name the user chose.
///
/// # Errors
///
/// [`Error::SettingValue`] when the path this home would need is longer than the kernel accepts.
fn socket_path(context: &Context) -> Result<PathBuf> {
    let socket = context
        .run()
        .join(format!("{}.sock", context.service().as_str()));

    super::within_socket_limit(context.service().as_str(), "socket", &socket)?;

    Ok(socket)
}

/// Where this instance listens on TCP: the port its row was given, on the address it names.
///
/// # Errors
///
/// [`Error::SettingValue`] when the row carries no port. A database that listens on nothing a client
/// can be pointed at is not a database anybody can use, and the rendered file would say `port =
/// none` — so this is refused here rather than discovered by a server that will not start.
fn address(context: &Context) -> Result<SocketAddr> {
    let port = context.port().ok_or_else(|| Error::SettingValue {
        service: context.service().as_str().to_owned(),
        key: "port",
        value: "none".to_owned(),
        reason: "a database listens on a TCP port and this service's row carries none; \
                 `service.create` allocates one",
    })?;

    Ok(SocketAddr::new(
        context
            .bind()
            .parse::<IpAddr>()
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        port,
    ))
}

/// A setting as a length of time, with a negative one read as none at all.
fn millis(number: i64) -> Millis {
    Millis(u64::try_from(number).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use mixengine_proto::ServiceId;

    use super::*;
    use crate::generate::recipe;
    use crate::generate::settings::Settings;

    /// An absolute path on whichever system this is compiled for.
    const fn root() -> &'static str {
        if cfg!(windows) {
            r"C:\MixEngine"
        } else {
            "/opt/mixengine"
        }
    }

    /// What the artifact publishes, as `mixengine-packages` publishes it.
    fn provides() -> BTreeMap<String, String> {
        [
            ("mariadbd", "bin/mariadbd"),
            ("mariadb", "bin/mariadb"),
            ("mariadb-admin", "bin/mariadb-admin"),
            ("mariadb-install-db", "scripts/mariadb-install-db"),
        ]
        .into_iter()
        .map(|(name, path)| {
            (
                name.to_owned(),
                format!("{path}{}", std::env::consts::EXE_SUFFIX),
            )
        })
        .collect()
    }

    /// A `mariadb@main` in a home at [`root`], with `overrides` applied.
    fn context(overrides: &str) -> Context {
        with_provides(provides(), overrides)
    }

    /// The same, for an install that publishes something else — or nothing.
    fn with_provides(provides: BTreeMap<String, String>, overrides: &str) -> Context {
        let service = ServiceId::parse("mariadb@main").expect("an id");
        let settings =
            Settings::merge(Mariadb.settings(), overrides, &service).expect("usable overrides");

        let context = Context::for_test(
            service,
            PACKAGE,
            Path::new(root()),
            provides,
            Some(3306),
            settings,
        );
        let endpoints = Mariadb
            .endpoints(&context)
            .expect("a home this short has a usable socket path");

        context.with_endpoints(endpoints)
    }

    /// The rendered `my.cnf` for `overrides`.
    fn rendered(overrides: &str) -> String {
        recipe::render(&Mariadb, &context(overrides))
            .expect("a rendering")
            .first()
            .expect("one file")
            .contents()
            .to_owned()
    }

    /// Two instances of one server, so its id carries an `@`.
    #[test]
    fn mariadb_is_named_because_a_home_may_have_two() {
        assert_eq!(Mariadb.instancing(), Instancing::Named);
    }

    /// Every path in the rendered file is quoted and forward-slashed.
    ///
    /// **Both halves are one measurement, made in T33a against a real server**: MariaDB's option
    /// parser treats `\` as an escape and everything after an unquoted `#` as a comment, so
    /// `C:\Users\Nguyen Hai Quang` breaks a naive rendering in two different ways at once — and a
    /// user whose home has a space in it is a real user on all three systems.
    #[test]
    fn every_path_in_the_configuration_is_quoted_and_forward_slashed() {
        let rendered = rendered("{}");
        let named = ["basedir", "datadir", "log_error", "socket", "plugin-dir"];

        let mut seen = 0;
        for line in rendered
            .lines()
            .filter(|line| named.iter().any(|key| line.starts_with(key)))
        {
            let value = line.split_once('=').expect("a setting").1.trim();

            assert!(
                value.starts_with('"') && value.ends_with('"'),
                "an unquoted path ends at the first `#`: {line}"
            );
            assert!(
                !value.contains('\\'),
                "a backslash is an escape to this parser: {line}"
            );
            seen += 1;
        }

        assert!(seen >= 3, "no paths were checked at all:\n{rendered}");
    }

    /// The lines that are stated rather than defaulted, each for something measured.
    #[test]
    fn the_configuration_states_what_a_supervisor_cannot_guess() {
        let rendered = rendered("{}");

        assert!(rendered.contains("log_error"), "{rendered}");
        assert!(rendered.contains("skip-name-resolve"), "{rendered}");
        assert!(rendered.contains("bind-address = 127.0.0.1"), "{rendered}");
        assert!(rendered.contains("port = 3306"), "{rendered}");
    }

    /// An override reaches the file rather than being ignored.
    #[test]
    fn the_configuration_says_what_the_overrides_said() {
        let rendered = rendered(r#"{"innodb_buffer_pool_size": "512M"}"#);

        assert!(
            rendered.contains("innodb_buffer_pool_size = 512M"),
            "{rendered}"
        );
    }

    /// **The file and the readiness check must name one server.**
    ///
    /// They are computed twice — once in Jinja for the file the server reads, once in Rust for the
    /// check the daemon makes — and the failure when they disagree is a database that starts
    /// perfectly and is reported as never having come up.
    #[test]
    fn the_file_and_the_readiness_check_name_one_port() {
        let context = context("{}");
        let rendered = rendered("{}");
        let spec = Mariadb
            .spec(&context)
            .expect("a spec")
            .build()
            .expect("a valid spec");

        let ReadyCheck::Command { args, .. } = spec.ready() else {
            panic!(
                "a database is proved up by a query, not by an accept: {:?}",
                spec.ready()
            );
        };

        assert!(args.iter().any(|arg| arg == "--port=3306"), "{args:?}");
        assert!(rendered.contains("port = 3306"), "{rendered}");
    }

    /// No password reaches an argument list, which every process on the machine can read.
    #[test]
    fn the_credential_is_named_rather_than_carried() {
        let spec = Mariadb
            .spec(&context("{}"))
            .expect("a spec")
            .build()
            .expect("a valid spec");

        assert!(
            matches!(
                spec.env().get(PASSWORD_VARIABLE),
                Some(mixengine_proto::EnvValue::Keyring { service, key })
                    if service == KEYRING_SERVICE && key == "mariadb@main/root"
            ),
            "{:?}",
            spec.env()
        );
    }

    /// A stop that is a command, because a signal leaves an unclean InnoDB.
    #[test]
    fn mariadb_is_stopped_by_asking_it_to_shut_down() {
        let spec = Mariadb
            .spec(&context("{}"))
            .expect("a spec")
            .build()
            .expect("a valid spec");

        assert!(
            matches!(spec.stop(), StopBehaviour::Command { program, args, .. }
                if program.ends_with(format!("mariadb-admin{}", std::env::consts::EXE_SUFFIX))
                    && args.iter().any(|arg| arg == "shutdown")),
            "{:?}",
            spec.stop()
        );
    }

    /// And no reload at all: MariaDB reads its configuration once.
    #[test]
    fn mariadb_has_no_reload_because_it_cannot() {
        let spec = Mariadb
            .spec(&context("{}"))
            .expect("a spec")
            .build()
            .expect("a valid spec");

        assert!(spec.reload().is_none(), "{:?}", spec.reload());
    }

    /// A package that publishes none of what this recipe needs is named as such.
    #[test]
    fn a_mariadb_without_its_own_binaries_is_named() {
        let context = with_provides(BTreeMap::new(), "{}");

        let error = Mariadb.spec(&context).expect_err("nothing to run");

        assert!(
            matches!(error, Error::ServiceProvidesNothing { .. }),
            "{error:?}"
        );
    }
}
