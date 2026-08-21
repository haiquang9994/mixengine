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

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use mixengine_platform::KEYRING_SERVICE;
use mixengine_proto::{
    HealthCheck, HealthProbe, Millis, ReadyCheck, ServiceSpec, ServiceSpecBuilder, StopBehaviour,
};

use crate::generate::first_run::{Ritual, SecretSpec, Step};
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

/// The one-shot that bootstraps a data directory — a shell script on Unix and a different C++
/// program of the same name on Windows. See the module note.
const INSTALL_DB: &str = "mariadb-install-db";

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

/// What this recipe's ritual needs the daemon to generate.
///
/// Thirty-two characters of `[A-Za-z0-9]` — 190 bits — and the alphabet is what makes the SQL
/// interpolation in [`bootstrap`] safe without an escaper. See
/// [`generate_secret`](mixengine_platform::generate_secret).
const SECRETS: &[SecretSpec] = &[SecretSpec {
    key: ROOT,
    length: 32,
}];

/// How long each half of the bootstrap is given.
///
/// Fifteen minutes: `mariadb-install-db` starts a server of its own and loads the whole system
/// schema, and on a cold Windows machine Defender reads every one of those files on the way past.
const BOOTSTRAP_PATIENCE: Millis = Millis(900_000);

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

    /// 3306, which MySQL names too — see [`Recipe::preferred_port`]. Whichever of the two is
    /// created first gets it.
    fn preferred_port(&self) -> Option<u16> {
        Some(3306)
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
            // What a failed start is diagnosed against (T38): the port this server will bind, and
            // not the socket beside it — a path cannot be held by a program with no `services` row.
            .ports([addr.port()])
            // **One option, and it has to be the first one.** `--defaults-file` means *read this
            // file and no other*, which is the difference between running this instance and running
            // whatever the machine already has: a user with a MariaDB of their own has an
            // `/etc/mysql/my.cnf` naming a datadir, a socket and a port, and silently inheriting any
            // of them would be writing into somebody else's database.
            //
            // **Not preceded by `--no-defaults`**, which is the shape this was first written in and
            // which does not work: MariaDB honours whichever of the two comes first, so the pair
            // means "read nothing at all" — and a server that read nothing looks for its data
            // directory beside its own binary. Measured rather than reasoned about: it started,
            // failed to `chdir`, and crash-looped six times before the supervisor gave up.
            .args([format!(
                "--defaults-file={}",
                context.config(CONFIG_FILE).display()
            )])
            .cwd(context.etc())
            // The credential the three client runs below need, named rather than carried: a spec is
            // data and cannot hold a password (ADR 0006). The supervisor reads it out of the OS
            // keyring at spawn time and keeps the resolved environment for the life of the process,
            // which is what lets a health probe and a shutdown authenticate too.
            .env_from_keyring(
                PASSWORD_VARIABLE,
                KEYRING_SERVICE,
                context.secret_address(ROOT),
            )
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

    /// The data directory, created once, with a root password that exists only in the OS keyring.
    fn ritual(&self) -> Option<Ritual> {
        Some(Ritual {
            secrets: SECRETS,
            steps,
        })
    }
}

/// The things that have to happen before this database is ever started.
///
/// # Errors
///
/// [`Error::ServiceProvidesNothing`] for an install missing one of the commands this needs, and
/// [`Error::SettingValue`] for a credential this recipe will not put in a SQL literal.
fn steps(context: &Context) -> Result<Vec<Step>> {
    let password = context.secret(ROOT);

    // **Refused rather than escaped.** The only producer of this value is
    // `mixengine_platform::generate_secret`, whose alphabet is chosen so that the interpolation in
    // `bootstrap` is safe; an escaper here would be a second thing to get right for a case that
    // cannot arise, and what it would hide is a credential in the wrong half of a SQL statement.
    if password.is_empty() || !password.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(Error::SettingValue {
            service: context.service().as_str().to_owned(),
            key: "root password",
            value: "<redacted>".to_owned(),
            reason: "a generated credential is alphanumeric so that it needs no escaping in the \
                     statement that sets it; this one is not, which is a bug in whatever made it",
        });
    }

    let mut steps = Vec::with_capacity(4);

    if cfg!(windows) {
        steps.push(windows_install_db(context)?);
        steps.push(bootstrap(context, password)?);
    } else {
        let view = super::space_free_view(context);

        steps.push(super::link_a_space_free_view(context, &view));
        steps.push(unix_install_db(context, &view)?);
        steps.push(bootstrap(context, password)?);
        steps.push(super::remove_the_space_free_view(&view));
    }

    Ok(steps)
}

/// `mariadb-install-db` as a system with a shell runs it.
///
/// Three things stated that nothing would think to state, each measured in T33a:
///
/// - **`--no-defaults`**, or the script and the server it starts read the machine's own
///   `/etc/mysql/my.cnf`. A user with their own MariaDB installed has one naming a datadir, a socket
///   and a port, and an instance that inherited any of them would be writing into somebody else's
///   database.
/// - **`--user`**, or it decides the data directory should belong to a `mysql` account nobody
///   created and stops when it cannot hand it over. `id -un` inside the same `sh` rather than a
///   platform call, because a shell is already what runs this step.
/// - **`/usr/sbin` and `/sbin` on the PATH.** `chown` is in `/usr/sbin` on macOS and `/usr/bin` on
///   Linux, and a cut-down path produced `chown: command not found` on one and not the other.
///
/// `--auth-root-authentication-method=normal` is Unix-only and is why this is not one command with a
/// flag: without it root authenticates through `unix_socket` against an OS account of the same name
/// and cannot be reached by whoever MixEngine runs as. The Windows program answers `unknown
/// variable` and exits 7. **`--service` is never passed**: a first-run job that registered a system
/// service would have installed something the daemon cannot see.
fn unix_install_db(context: &Context, view: &Path) -> Result<Step> {
    // Named from the map so a series that spells it `mysql_install_db` is found too — and taken
    // relative to the view rather than to the install, because the script resolves its own helpers
    // against `$basedir`.
    let relative = context
        .provided(INSTALL_DB)?
        .strip_prefix(context.install_path())
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| PathBuf::from(INSTALL_DB));
    let basedir = view.join("basedir");

    Ok(Step {
        label: "create the data directory".to_owned(),
        program: PathBuf::from("/bin/sh"),
        args: vec![
            "-c".to_owned(),
            "exec \"$1\" --no-defaults --basedir=\"$2\" --datadir=\"$3\" \
             --auth-root-authentication-method=normal --skip-test-db --user=\"$(id -un)\""
                .to_owned(),
            "sh".to_owned(),
            basedir.join(&relative).display().to_string(),
            basedir.display().to_string(),
            view.join("datadir").display().to_string(),
        ],
        stdin: None,
        secret_file: None,
        env: BTreeMap::from([(
            "PATH".to_owned(),
            format!(
                "{}:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
                basedir.join("bin").display()
            ),
        )]),
        cwd: PathBuf::from("/tmp"),
        timeout: BOOTSTRAP_PATIENCE,
    })
}

/// And as Windows runs it: a different program of the same name, sharing almost none of its options.
///
/// The data directory is **not** created first — this program writes it itself and refuses one that
/// is already there. Its parent is what has to exist, and the daemon's marker step made it.
fn windows_install_db(context: &Context) -> Result<Step> {
    Ok(Step {
        label: "create the data directory".to_owned(),
        program: context.provided(INSTALL_DB)?,
        args: vec![format!("--datadir={}", context.data().display())],
        stdin: None,
        secret_file: None,
        env: BTreeMap::new(),
        cwd: context.etc().to_path_buf(),
        timeout: BOOTSTRAP_PATIENCE,
    })
}

/// Set the root password and drop what should not survive, through a server listening on nothing.
///
/// **`--bootstrap` listens on no port and no socket**, so there is no moment where a password-less
/// root sits on `127.0.0.1:3306` waiting for whoever is quickest. The two rejected alternatives are
/// in the design document: a temporary server with `--skip-networking`, which is Unix vocabulary and
/// would need a second shape on Windows; and starting the real server and setting the password
/// afterwards, which is a window measured in seconds during which anyone on the machine is root.
///
/// Each statement is named rather than left to "secure defaults", and each is one line because that
/// is what bootstrap mode reads.
///
/// # Why the grant tables are written to directly
///
/// **`SET PASSWORD` does not work here, and neither does `ALTER USER`.** Measured against 11.4.12:
/// bootstrap mode implies `--skip-grant-tables`, and both statements are refused with `ERROR 1290 —
/// The MariaDB server is running with the --skip-grant-tables option so it cannot execute this
/// statement`. What does work is the row itself, which is what upstream's own `mariadb-install-db`
/// does in the same mode. `PASSWORD()` is a function rather than a statement and is available.
///
/// # And why every `root` row, not `root@localhost`
///
/// The configuration says `skip-name-resolve`, so a client connecting over TCP to 127.0.0.1 is
/// matched as `root@127.0.0.1` and never as `root@localhost` — and the readiness check, the health
/// check and the shutdown are all exactly that client. `mariadb-install-db` creates four root rows
/// (`localhost`, this machine's name, `127.0.0.1`, `::1`); the password goes on all of them, and the
/// one named after the machine is then removed, because a machine's name is reachable from off it.
fn bootstrap(context: &Context, password: &str) -> Result<Step> {
    let mut args = vec![
        "--no-defaults".to_owned(),
        "--bootstrap".to_owned(),
        format!("--basedir={}", context.install_path().display()),
        format!("--datadir={}", context.data().display()),
    ];

    // The one line the Windows server is not able to derive — see the module note and the template.
    if let Some(plugins) = context.plugins() {
        args.push(format!("--plugin-dir={}", plugins.display()));
    }

    Ok(Step {
        label: "set the root password and remove the accounts nobody should have".to_owned(),
        program: context.provided(SERVER)?,
        args,
        stdin: Some(format!(
            "USE mysql;\n\
             UPDATE global_priv SET priv = JSON_SET(priv, '$.plugin', 'mysql_native_password', '$.authentication_string', PASSWORD('{password}')) WHERE User = 'root';\n\
             DELETE FROM global_priv WHERE User = '';\n\
             DELETE FROM global_priv WHERE User = 'root' AND Host NOT IN ('localhost', '127.0.0.1', '::1');\n\
             DROP DATABASE IF EXISTS test;\n"
        )),
        secret_file: None,
        env: BTreeMap::new(),
        cwd: context.etc().to_path_buf(),
        timeout: BOOTSTRAP_PATIENCE,
    })
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
    use crate::generate::first_run::FirstRun;
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

    /// What a failed start is diagnosed against — roadmap task **T38**.
    ///
    /// The row's port and nothing else: the socket is a path and cannot be in conflict with a
    /// program that has no `services` row, which is what this declaration is read for.
    #[test]
    fn the_spec_declares_the_port_the_server_will_bind() {
        let context = context("{}");
        let spec = Mariadb
            .spec(&context)
            .expect("a spec")
            .build()
            .expect("a valid spec");

        assert_eq!(spec.ports(), [3306]);
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

    /// The server is pointed at its own file, and at nothing else.
    ///
    /// **`--no-defaults` must not be there.** MariaDB honours whichever of `--no-defaults` and
    /// `--defaults-file` comes first, so the pair means "read nothing" — and a server that read
    /// nothing looks for its data directory beside its own binary, fails to `chdir`, and
    /// crash-loops. Which is what this recipe did until it was run against a real server.
    #[test]
    fn the_server_reads_its_own_file_and_no_other() {
        let spec = Mariadb
            .spec(&context("{}"))
            .expect("a spec")
            .build()
            .expect("a valid spec");

        assert!(
            spec.args()
                .first()
                .is_some_and(|first| first.starts_with("--defaults-file=")),
            "the configuration file has to be the first option: {:?}",
            spec.args()
        );
        assert!(
            !spec.args().iter().any(|arg| arg == "--no-defaults"),
            "`--no-defaults` before `--defaults-file` means the file is never read: {:?}",
            spec.args()
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

    /// **A first run is measured before its credentials exist.**
    ///
    /// The daemon waits for the bootstrap job for as long as the steps ask plus a little, and it has
    /// to decide that before it generates the credential those steps interpolate. Measuring a plan
    /// by building its steps with no secrets does not work: this recipe refuses an empty password —
    /// rightly — so the measurement comes back empty and thirty declared minutes collapse to the
    /// slack alone. CI met it the first time two of these bootstrapped at once on Windows and the
    /// first one was killed at sixty seconds.
    #[test]
    fn a_first_run_is_measured_before_its_credentials_exist() {
        let plan = FirstRun::new(
            &context("{}"),
            Mariadb.ritual().expect("mariadb bootstraps"),
        );

        assert!(
            plan.budget() >= BOOTSTRAP_PATIENCE,
            "one bootstrap step alone asks for {BOOTSTRAP_PATIENCE:?}, and the plan measured {:?}",
            plan.budget()
        );
    }

    /// A context carrying the credential the daemon would have generated.
    fn initialised(secret: &str) -> Context {
        let mut context = context("{}");
        context.put_secret(ROOT, secret);

        context
    }

    /// The keyring entry the spec names and the one the ritual is stored under are one address.
    ///
    /// Composed twice — once for the spec's `EnvValue::Keyring`, once for the daemon writing the
    /// generated value — and the failure when they disagree is a server that starts and a client
    /// that cannot authenticate against it, reported as a service that never became ready.
    #[test]
    fn the_spec_and_the_ritual_name_one_credential() {
        let context = context("{}");
        let spec = Mariadb
            .spec(&context)
            .expect("a spec")
            .build()
            .expect("a valid spec");

        let named = spec
            .env()
            .get(PASSWORD_VARIABLE)
            .expect("the spec names a credential");

        assert!(
            matches!(named, mixengine_proto::EnvValue::Keyring { key, .. }
                if key == &context.secret_address(ROOT)),
            "{named:?}"
        );
        assert_eq!(context.secret_address(ROOT), "mariadb@main/root");
    }

    /// A recipe that declares a credential also declares what to do with it.
    #[test]
    fn mariadb_declares_the_one_credential_its_ritual_needs() {
        let ritual = Mariadb.ritual().expect("a database has a first run");

        assert_eq!(ritual.secrets.len(), 1, "{:?}", ritual.secrets);
        assert_eq!(ritual.secrets[0].key, ROOT);
        assert!(ritual.secrets[0].length >= 32, "{:?}", ritual.secrets);
    }

    /// The bootstrap sets the password it was given, and drops what should not survive.
    ///
    /// Each statement named rather than left to "secure defaults", because the whole value of this
    /// step is that nothing is left to be assumed.
    #[test]
    fn the_bootstrap_says_exactly_what_it_does_to_the_grant_tables() {
        let steps = steps(&initialised("hunter2")).expect("steps");
        let bootstrap = steps
            .iter()
            .find(|step| step.stdin.is_some())
            .expect("one step speaks SQL");
        let sql = bootstrap.stdin.as_deref().expect("the SQL is on stdin");

        // The row rather than `SET PASSWORD`, and every root rather than `root@localhost` — both
        // measured against 11.4.12, both explained at `bootstrap`.
        assert!(
            sql.contains("UPDATE global_priv SET priv = JSON_SET("),
            "{sql}"
        );
        assert!(sql.contains("WHERE User = 'root';"), "{sql}");
        assert!(sql.contains("PASSWORD('hunter2')"), "{sql}");
        assert!(
            sql.contains("DELETE FROM global_priv WHERE User = ''"),
            "{sql}"
        );
        assert!(sql.contains("DROP DATABASE IF EXISTS test"), "{sql}");
        assert!(
            !sql.contains("SET PASSWORD"),
            "bootstrap mode refuses that statement with error 1290: {sql}"
        );

        // Bootstrap mode reads one statement per line, so a statement wrapped over two would be two
        // statements and neither of them valid.
        for line in sql.lines().filter(|line| !line.trim().is_empty()) {
            assert!(
                line.trim_end().ends_with(';'),
                "a wrapped statement: {line}"
            );
        }

        assert!(
            bootstrap.args.iter().any(|arg| arg == "--bootstrap"),
            "the password is set by a server listening on nothing: {:?}",
            bootstrap.args
        );
        assert!(
            bootstrap.args.iter().any(|arg| arg == "--no-defaults"),
            "a machine own my.cnf must not reach this: {:?}",
            bootstrap.args
        );
    }

    /// A credential that would need escaping is refused rather than escaped.
    ///
    /// `mixengine_platform::generate_secret` restricts its alphabet for exactly the interpolation
    /// above; this is the other end of that arrangement, and it fails loudly rather than quietly
    /// producing a statement whose second half is somebody password.
    #[test]
    fn a_credential_that_would_need_escaping_is_refused() {
        let error = steps(&initialised("a'b")).expect_err("that cannot go in a SQL literal");

        assert!(
            matches!(&error, Error::SettingValue { value, .. } if !value.contains("a'b")),
            "the refusal quoted the credential: {error:?}"
        );
    }

    /// And so is no credential at all, which is a recipe that declared one and never got it.
    #[test]
    fn no_credential_at_all_is_refused_rather_than_setting_an_empty_password() {
        let error = steps(&context("{}")).expect_err("an empty password is not a password");

        assert!(matches!(error, Error::SettingValue { .. }), "{error:?}");
    }

    /// The Unix ritual is four steps and the Windows one is two, and the difference is upstream.
    #[test]
    fn the_ritual_has_the_shape_this_system_needs() {
        let steps = steps(&initialised("abc123")).expect("steps");
        let labels: Vec<&str> = steps.iter().map(|step| step.label.as_str()).collect();

        if cfg!(windows) {
            assert_eq!(labels.len(), 2, "{labels:?}");
        } else {
            assert_eq!(labels.len(), 4, "{labels:?}");
            assert!(labels[0].contains("space"), "{labels:?}");
            assert!(labels[3].contains("space"), "{labels:?}");
        }

        assert!(
            steps.iter().all(|step| step.program.is_absolute()),
            "a relative program is whatever the PATH says at the moment it runs: {steps:?}"
        );
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
