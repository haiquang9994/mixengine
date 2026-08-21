//! MySQL: the other database with these programs' names — roadmap task **T34c**.
//!
//! **Not a version of [`mariadb`](super::mariadb)**, and anybody maintaining an application against
//! one of them can say which. It is its own package, its own recipe and its own rows — and the two
//! of them name the same port, which is the whole reason a port is allocated when a row is written
//! rather than taken from a recipe at start ([`crate::services::ports`]).
//!
//! # What was measured rather than assumed
//!
//! Every route below was run against a real server — 8.4.10 and 5.6.51 while this was written, and
//! every published cell by `mixengine-packages`' own `mysql_smoke.py` before the artifact was
//! allowed out:
//!
//! - **There is no `--bootstrap` after 5.7.6.** MariaDB sets its root password through a server that
//!   reads SQL on standard input; MySQL removed that mode, so the statement goes into a file the
//!   daemon writes and removes around one step — see
//!   [`SecretFile`].
//! - **`--initialize-insecure` creates exactly one account, `root@localhost`.** MariaDB's installer
//!   creates `root@127.0.0.1` as well, which is what makes its `skip-name-resolve` safe; copying
//!   that line into this template would leave every client refused by a server whose own log says it
//!   is ready for connections. So this configuration does not set it, and the template says why.
//! - **A modern MySQL opens a second listener nobody asked for**: the X Protocol, on 33060, which no
//!   allocation ever handed out and no `services` row records. `loose-mysqlx = OFF` switches it off
//!   on 8.0 and newer and is a warning rather than a refusal on 5.6 and 5.7, where the option does
//!   not exist — one line instead of a version branch inside a template.
//! - **Bootstrapping is a table of three routes and not a version test** — see `Route`. 5.6
//!   answers differently on Windows than on Unix, and its Unix installer is *Perl* in a tree
//!   compiled from source, so what runs it is read off its own first line.
//!
//! # What this recipe deliberately does not do
//!
//! **No reload**, for MariaDB's reason: the configuration is read once, at startup.
//!
//! **No authentication-plugin default.** 8.0 wants `default_authentication_plugin`, 8.4 removed that
//! variable and disabled `mysql_native_password` altogether, and 5.6 knows neither. A line that
//! makes an old client work on one release refuses to start on another, and this recipe is found by
//! package name and runs whichever of five lines a user installed. The day a client needs it, it is
//! an override on one instance rather than a default for all of them.
//!
//! **No `mysql_upgrade`.** Running a data directory bootstrapped by one version against a later one
//! is a task of its own; what is recorded is the version that performed the bootstrap, in
//! [`READY_MARKER`](crate::generate::first_run::READY_MARKER).

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use mixengine_platform::KEYRING_SERVICE;
use mixengine_proto::{
    HealthCheck, HealthProbe, Millis, ReadyCheck, ServiceSpec, ServiceSpecBuilder, StopBehaviour,
};

use crate::generate::first_run::{Ritual, SecretFile, SecretSpec, Step};
use crate::generate::recipe::{Context, Endpoints, Instancing, Recipe, TemplateFile};
use crate::generate::settings::{Preset, Setting};
use crate::{Error, Result};

/// The `packages.name` this recipe is found under.
pub const PACKAGE: &str = "mysql";

/// The server.
const SERVER: &str = "mysqld";

/// The client the readiness check, the health check and the shutdown all run.
const ADMIN: &str = "mysqladmin";

/// The installer 5.6 publishes on Unix and nowhere else — `scripts/mysql_install_db`.
const INSTALL_DB: &str = "mysql_install_db";

/// The rendered configuration, under `etc/<service-id>/`.
const CONFIG_FILE: &str = "my.cnf";

/// What the file holding the statement that sets the root password is called.
///
/// In `run/`, which [`Paths::bootstrap`](crate::paths::Paths::bootstrap) restricts to this account
/// on every start, and named after the service so two instances bootstrapping at once cannot meet.
const INIT_FILE_SUFFIX: &str = "-init.sql";

/// How much memory InnoDB is given. Dev-tuned, for [`mariadb`](super::mariadb)'s reason.
const BUFFER_POOL: &str = "innodb_buffer_pool_size";

/// How many connections it accepts at once. MySQL's own default.
const MAX_CONNECTIONS: &str = "max_connections";

/// The server's character set.
const CHARACTER_SET: &str = "character_set";

/// The collation that goes with it.
///
/// `utf8mb4_general_ci` and **not** 8.0's own `utf8mb4_0900_ai_ci`, deliberately: this recipe runs
/// whichever of five lines a user installed, and the 0900 collations do not exist before 8.0. A
/// default that refuses to start on 5.7 is a default nobody can use.
const COLLATION: &str = "collation";

/// How long the server is given to answer a ping before the start is a failure, in milliseconds.
const READY_TIMEOUT: &str = "ready_timeout_ms";

/// How long `mysqladmin shutdown` is given before the process group is killed, in milliseconds.
const STOP_GRACE: &str = "stop_grace_ms";

/// How often the server is asked whether it is still answering.
const HEALTH_INTERVAL: Millis = Millis(10_000);

/// How long one of those may take, well inside the interval.
const HEALTH_TIMEOUT: Millis = Millis(5_000);

/// The environment variable MySQL's own clients read a password from.
///
/// The reason no command line this recipe builds carries one.
pub(super) const PASSWORD_VARIABLE: &str = "MYSQL_PWD";

/// The account the ritual gives a password, and everything here authenticates as.
pub(super) const ROOT: &str = "root";

/// What this recipe's ritual needs the daemon to generate.
const SECRETS: &[SecretSpec] = &[SecretSpec {
    key: ROOT,
    length: 32,
}];

/// How long each half of the bootstrap is given.
const BOOTSTRAP_PATIENCE: Millis = Millis(900_000);

/// Which program can make a data directory for this artifact.
///
/// **A table rather than a version test**, because the split is not only between lines: 5.6 answers
/// differently on Windows than it does on Unix, and what decides is what upstream put in the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Route {
    /// 5.7 and newer: `mysqld --initialize-insecure`, which writes into an empty directory and
    /// leaves root without a password — what a development machine wants, and what `--initialize`
    /// (a random password buried in the error log) does not give.
    Initialize,

    /// 5.6 on Unix: `scripts/mysql_install_db`, reached through a space-free view because upstream's
    /// script does not quote `$basedir`, and run by the interpreter its own first line names.
    Script,

    /// 5.6 on Windows: neither of those exists, and upstream's zip ships a `data/` directory whose
    /// system tables are already built. The documented first run is to copy it.
    ShippedData,
}

/// MySQL, as MixEngine runs it.
#[derive(Debug)]
pub struct Mysql;

impl Recipe for Mysql {
    fn package(&self) -> &'static str {
        PACKAGE
    }

    fn instancing(&self) -> Instancing {
        Instancing::Named
    }

    /// 3306, which MariaDB names too. Whichever of the two is created first is given it.
    fn preferred_port(&self) -> Option<u16> {
        Some(3306)
    }

    /// `mysqld --version`, which is cheap and touches the server's own machinery.
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
            source: include_str!("mysql/my.cnf"),
        }]
    }

    /// The socket on a system that has them, and nothing else.
    ///
    /// No plugin directory: MySQL derives one from `basedir` on every cell `mixengine-packages`
    /// publishes, which is where this differs from MariaDB on Windows and was measured rather than
    /// assumed.
    fn endpoints(&self, context: &Context) -> Result<Endpoints> {
        if cfg!(windows) {
            return Ok(Endpoints::default());
        }

        Ok(Endpoints {
            socket: Some(socket_path(context)?),
            plugins: None,
            ..Endpoints::default()
        })
    }

    /// The server, and the three things that are one client run with one credential.
    fn spec(&self, context: &Context) -> Result<ServiceSpecBuilder> {
        let settings = context.settings();
        let server = context.provided(SERVER)?;
        let admin = context.provided(ADMIN)?;
        let addr = address(context)?;

        Ok(ServiceSpec::builder(context.service().clone(), &server)
            // What a failed start is diagnosed against (T38).
            .ports([addr.port()])
            // **First, and on its own**: `--defaults-file` means *read this file and no other*,
            // which is what keeps a machine's own `/etc/my.cnf` — naming a datadir, a socket and a
            // port — from being inherited into somebody else's database.
            .args([format!(
                "--defaults-file={}",
                context.config(CONFIG_FILE).display()
            )])
            .cwd(context.etc())
            // Named rather than carried: a spec is data and cannot hold a password (ADR 0006).
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
                failures_before_degraded: 3,
                successes_before_running: 1,
            })
            // Asked rather than signalled: a terminated `mysqld` leaves a dirty buffer pool, and the
            // next start pays for it with a recovery that reads the whole redo log.
            .stop(StopBehaviour::Command {
                program: admin,
                args: {
                    let mut args = connection(addr);
                    args.push("shutdown".to_owned());
                    args
                },
                grace: millis(settings.number(STOP_GRACE)),
            }))
        // **No reload.** MySQL reads its configuration once, at startup.
    }

    /// The data directory, created once, with a root password that exists only in the OS keyring.
    fn ritual(&self) -> Option<Ritual> {
        Some(Ritual {
            secrets: SECRETS,
            steps,
        })
    }
}

/// Which of the three bootstraps this artifact has.
///
/// `windows` is an argument rather than a [`cfg!`] so that the whole table is exercised wherever the
/// tests run: two of its three answers would otherwise be unreachable on any one machine.
pub(super) fn route(version: &str, windows: bool) -> Route {
    let mut parts = version
        .split(['.', '-'])
        .filter_map(|part| part.parse::<u32>().ok());

    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);

    // A version this cannot read is treated as a modern one: every line published since 5.7 has
    // `--initialize-insecure`, and the two that do not are the two whose numbers are unmistakable.
    if (major, minor) > (5, 6) {
        return Route::Initialize;
    }

    if windows {
        Route::ShippedData
    } else {
        Route::Script
    }
}

/// The things that have to happen before this database is ever started.
///
/// # Errors
///
/// [`Error::ServiceProvidesNothing`] for an install missing one of the commands this needs, and
/// [`Error::SettingValue`] for a credential this recipe will not put in a SQL literal.
fn steps(context: &Context) -> Result<Vec<Step>> {
    steps_for(context, route(context.version(), cfg!(windows)))
}

/// The steps for one route, which is what a test can ask for on any system.
///
/// # Errors
///
/// As [`steps`].
pub(super) fn steps_for(context: &Context, route: Route) -> Result<Vec<Step>> {
    let password = context.secret(ROOT);

    // **Refused rather than escaped**, for MariaDB's reason: the only producer of this value is
    // `mixengine_platform::generate_secret`, whose alphabet is what makes the interpolation below
    // safe without an escaper nobody could prove right.
    if password.is_empty() || !password.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(Error::SettingValue {
            service: context.service().as_str().to_owned(),
            key: "root password",
            value: "<redacted>".to_owned(),
            reason: "a generated credential is alphanumeric so that it needs no escaping in the \
                     statement that sets it; this one is not, which is a bug in whatever made it",
        });
    }

    match route {
        Route::Initialize => Ok(vec![
            initialize(context)?,
            set_the_password(context, password)?,
        ]),

        Route::Script => {
            let view = super::space_free_view(context);

            Ok(vec![
                super::link_a_space_free_view(context, &view),
                install_db(context, &view)?,
                bootstrap(context, password)?,
                super::remove_the_space_free_view(&view),
            ])
        }

        Route::ShippedData => Ok(vec![
            copy_the_shipped_data(context)?,
            bootstrap(context, password)?,
        ]),
    }
}

/// `mysqld --initialize-insecure`: the server builds its own data directory.
///
/// `--no-defaults` first, or the server reads whatever `my.cnf` this machine already has and
/// initialises somewhere else entirely. The error log is named because a failure here has nowhere
/// else to be: the process writes almost nothing to its own output.
fn initialize(context: &Context) -> Result<Step> {
    Ok(Step {
        label: "create the data directory".to_owned(),
        program: context.provided(SERVER)?,
        args: vec![
            "--no-defaults".to_owned(),
            format!("--basedir={}", context.install_path().display()),
            format!("--datadir={}", context.data().display()),
            format!("--log-error={}", bootstrap_log(context).display()),
            "--initialize-insecure".to_owned(),
        ],
        stdin: None,
        secret_file: None,
        env: BTreeMap::new(),
        cwd: context.etc().to_path_buf(),
        timeout: BOOTSTRAP_PATIENCE,
    })
}

/// Give `root` its password, through a server nothing can connect to.
///
/// **`--skip-networking` is the whole point.** `--initialize-insecure` leaves an account with no
/// password, and every other way of setting one has a window in which that account is reachable: on
/// 3306, on a temporary port, or through a socket. This server binds nothing at all, runs the two
/// statements it was given and stops itself with the second of them.
///
/// The statement is in a file rather than on the command line because an argument list is readable
/// by every process on this machine. The daemon writes that file inside `run/`, which is owner-only,
/// and removes it whatever the step does — see [`SecretFile`].
fn set_the_password(context: &Context, password: &str) -> Result<Step> {
    let init = context
        .run()
        .join(format!("{}{INIT_FILE_SUFFIX}", context.service().as_str()));

    Ok(Step {
        label: "set the root password".to_owned(),
        program: context.provided(SERVER)?,
        args: vec![
            "--no-defaults".to_owned(),
            format!("--basedir={}", context.install_path().display()),
            format!("--datadir={}", context.data().display()),
            format!("--log-error={}", bootstrap_log(context).display()),
            "--skip-networking".to_owned(),
            format!("--init-file={}", init.display()),
        ],
        stdin: None,
        secret_file: Some(SecretFile {
            path: init,
            // `--initialize-insecure` creates `root@localhost` and nothing else: no anonymous
            // accounts and no `test` database, so there is nothing here to clean up after it.
            content: format!(
                "ALTER USER '{ROOT}'@'localhost' IDENTIFIED BY '{password}';\nSHUTDOWN;\n"
            ),
        }),
        env: BTreeMap::new(),
        cwd: context.etc().to_path_buf(),
        timeout: BOOTSTRAP_PATIENCE,
    })
}

/// `mysql_install_db` as 5.6 on Unix publishes it: a Perl program under a name that says nothing.
///
/// Three things stated that nothing would think to state, each of them measured:
///
/// - **The interpreter comes off the script's own first line.** A tree compiled by
///   `mixengine-packages` configures `mysql_install_db.pl.in` on every platform, so handing this to
///   `/bin/sh` answers `use: command not found` and reads like a corrupt archive.
/// - **`--no-defaults`**, or the script and the server it starts read this machine's own `my.cnf`.
/// - **`/usr/sbin` and `/sbin` on the PATH**: `chown` is in `/usr/sbin` on macOS and `/usr/bin` on
///   Linux, and a cut-down path fails on exactly one of the two.
fn install_db(context: &Context, view: &Path) -> Result<Step> {
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
            // The shebang is a path off the machine that built the tree, so it is used when it is
            // still there and looked up by name when it is not — and `sh` is the last resort, for a
            // script that has no first line to read.
            "shebang=$(sed -n '1s|^#! *||p' \"$1\" | cut -d' ' -f1); \
             if [ ! -x \"$shebang\" ]; then \
               shebang=$(command -v \"$(basename \"${shebang:-sh}\")\" 2>/dev/null || echo /bin/sh); \
             fi; \
             exec \"$shebang\" \"$1\" --no-defaults --basedir=\"$2\" --datadir=\"$3\" --user=\"$(id -un)\""
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

/// 5.6 on Windows: copy the `data/` directory upstream's zip ships with its system tables built.
///
/// `xcopy` rather than a shell, and directly rather than through `cmd.exe`: a copy needs no command
/// interpreter, and one handed a program's standard input reads it as commands. `/I` says the
/// destination is a directory, `/E` includes the empty ones, `/Q` keeps a hundred and thirty-six
/// file names out of the job log, and `/Y` overwrites without asking a question nobody is there to
/// answer.
fn copy_the_shipped_data(context: &Context) -> Result<Step> {
    Ok(Step {
        label: "create the data directory".to_owned(),
        program: PathBuf::from(r"C:\Windows\System32\xcopy.exe"),
        args: vec![
            context.install_path().join("data").display().to_string(),
            context.data().display().to_string(),
            "/E".to_owned(),
            "/I".to_owned(),
            "/Q".to_owned(),
            "/Y".to_owned(),
        ],
        stdin: None,
        secret_file: None,
        env: BTreeMap::new(),
        cwd: context.etc().to_path_buf(),
        timeout: BOOTSTRAP_PATIENCE,
    })
}

/// Set the root password on 5.6, through a server in bootstrap mode listening on nothing.
///
/// **The one line that still has `--bootstrap`**, which is why this route reads its SQL on standard
/// input where the modern one cannot: the mode was removed at 5.7.6.
///
/// The grant tables are written to directly because bootstrap mode implies `--skip-grant-tables`,
/// which refuses `SET PASSWORD` — the same refusal MariaDB's own bootstrap meets. `PASSWORD()` is a
/// function rather than a statement and is available there.
///
/// Every `root` row rather than `root@localhost`, and the anonymous accounts and the `test` database
/// go with them: what this route starts from is a directory an installer or upstream's own zip
/// built, and both of those leave all three.
fn bootstrap(context: &Context, password: &str) -> Result<Step> {
    Ok(Step {
        label: "set the root password and remove the accounts nobody should have".to_owned(),
        program: context.provided(SERVER)?,
        args: vec![
            "--no-defaults".to_owned(),
            "--bootstrap".to_owned(),
            format!("--basedir={}", context.install_path().display()),
            format!("--datadir={}", context.data().display()),
        ],
        stdin: Some(format!(
            "USE mysql;\n\
             UPDATE user SET Password = PASSWORD('{password}'), plugin = 'mysql_native_password' WHERE User = '{ROOT}';\n\
             DELETE FROM user WHERE User = '';\n\
             DELETE FROM user WHERE User = '{ROOT}' AND Host NOT IN ('localhost', '127.0.0.1', '::1');\n\
             DROP DATABASE IF EXISTS test;\n\
             DELETE FROM db WHERE Db LIKE 'test%';\n\
             FLUSH PRIVILEGES;\n"
        )),
        secret_file: None,
        env: BTreeMap::new(),
        cwd: context.etc().to_path_buf(),
        timeout: BOOTSTRAP_PATIENCE,
    })
}

/// Where a bootstrap says what went wrong, which is not the process's own output.
fn bootstrap_log(context: &Context) -> PathBuf {
    context.logs().join("mysql-bootstrap.err")
}

/// How a client is told which server to ask and who to be.
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
/// [`Error::SettingValue`] when the row carries no port.
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

    use mixengine_proto::{ReadyCheck, ServiceId, StopBehaviour};

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

    /// What an 8.x artifact publishes, as `mixengine-packages` publishes it.
    fn provides() -> BTreeMap<String, String> {
        [
            ("mysqld", "bin/mysqld"),
            ("mysql", "bin/mysql"),
            ("mysqladmin", "bin/mysqladmin"),
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

    /// The same for a 5.6 tree compiled here, which publishes the installer script as well.
    fn provides_5_6() -> BTreeMap<String, String> {
        let mut provides = provides();
        provides.insert(INSTALL_DB.to_owned(), "scripts/mysql_install_db".to_owned());
        provides
    }

    /// A `mysql@main` of `version` in a home at [`root`], with `overrides` applied.
    fn context_of(version: &str, provides: BTreeMap<String, String>, overrides: &str) -> Context {
        let service = ServiceId::parse("mysql@main").expect("an id");
        let settings =
            Settings::merge(Mysql.settings(), overrides, &service).expect("usable overrides");

        let context = Context::for_test(
            service,
            PACKAGE,
            Path::new(root()),
            provides,
            Some(3306),
            settings,
        )
        .with_version(version);

        let endpoints = Mysql
            .endpoints(&context)
            .expect("a home this short has a usable socket path");

        context.with_endpoints(endpoints)
    }

    /// The version this recipe is written against first: the line `services.md` names.
    fn context(overrides: &str) -> Context {
        context_of("8.4.10", provides(), overrides)
    }

    /// The rendered `my.cnf` for `overrides`.
    fn rendered(overrides: &str) -> String {
        recipe::render(&Mysql, &context(overrides))
            .expect("a rendering")
            .first()
            .expect("one file")
            .contents()
            .to_owned()
    }

    /// The same context with the credential a ritual would have been handed.
    fn initialised(version: &str, provides: BTreeMap<String, String>) -> Context {
        let mut context = context_of(version, provides, "{}");
        context.put_secret(ROOT, "abcd1234abcd1234abcd1234abcd1234");
        context
    }

    /// Two instances of one server, so its id carries an `@`.
    #[test]
    fn mysql_is_named_because_a_home_may_have_two() {
        assert_eq!(Mysql.instancing(), Instancing::Named);
    }

    /// Every path in the rendered file is quoted and forward-slashed.
    ///
    /// MySQL reads its option file with the same parser MariaDB does: `\` escapes and an unquoted
    /// `#` starts a comment, so a home under `C:\Users\Nguyen Hai Quang` breaks an unrendered path
    /// in two ways at once.
    #[test]
    fn every_path_in_the_configuration_is_quoted_and_forward_slashed() {
        let rendered = rendered("{}");

        for line in rendered.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };

            if !key.trim_end().ends_with("dir") && !key.trim_end().contains("log_error") {
                continue;
            }

            let value = value.trim();

            assert!(value.starts_with('"'), "{line}");
            assert!(!value.contains('\\'), "{line}");
        }
    }

    /// **No `skip-name-resolve`, and it is a measurement rather than a preference.**
    ///
    /// MariaDB's installer creates `root@127.0.0.1`; MySQL's `--initialize-insecure` creates only
    /// `root@localhost`. With name resolution off, a client connecting over TCP to 127.0.0.1 is
    /// matched as a string and finds no account — so every client, this recipe's own readiness
    /// check included, is refused by a server whose log says it is ready.
    #[test]
    fn the_configuration_does_not_switch_off_the_lookup_that_makes_root_reachable() {
        let rendered = rendered("{}");

        // The comment explaining the absence names it, which is the point of the comment — what
        // must not exist is a line the server reads.
        for line in rendered.lines().filter(|line| !line.starts_with('#')) {
            assert!(!line.contains("skip-name-resolve"), "{rendered}");
        }
    }

    /// The X Protocol is switched off, in the one spelling every line here can read.
    ///
    /// A second listener on 33060 is a port the allocator never handed out and cannot see, so two
    /// instances of MySQL 8 would collide on it however carefully their 3306s were chosen. The
    /// `loose-` prefix is what makes one line safe on 5.6 and 5.7, where the option does not exist
    /// and an unprefixed one is a server that refuses to start.
    #[test]
    fn the_second_listener_a_modern_mysql_would_open_is_switched_off() {
        let rendered = rendered("{}");

        assert!(rendered.contains("loose-mysqlx"), "{rendered}");
    }

    /// The file and the readiness check name one port, whatever the row says.
    #[test]
    fn the_file_and_the_readiness_check_name_one_port() {
        let rendered = rendered("{}");
        let spec = Mysql
            .spec(&context("{}"))
            .expect("a spec")
            .build()
            .expect("a valid spec");

        assert!(rendered.contains("port = 3306"), "{rendered}");
        assert_eq!(spec.ports(), [3306]);

        let ReadyCheck::Command { args, .. } = spec.ready() else {
            panic!("readiness is a client run: {:?}", spec.ready());
        };

        assert!(args.iter().any(|arg| arg == "--port=3306"), "{args:?}");
    }

    /// The password is named for the supervisor to resolve, and never carried.
    #[test]
    fn the_credential_is_named_rather_than_carried() {
        let spec = Mysql
            .spec(&context("{}"))
            .expect("a spec")
            .build()
            .expect("a valid spec");

        let ReadyCheck::Command { args, .. } = spec.ready() else {
            panic!("readiness is a client run");
        };

        assert!(
            !args.iter().any(|arg| arg.contains("password")),
            "a password on a command line is readable by every process here: {args:?}"
        );
        assert!(
            spec.env()
                .iter()
                .any(|(key, _)| key == super::PASSWORD_VARIABLE),
            "the client reads its password out of the environment: {:?}",
            spec.env()
        );
    }

    /// The server reads the file this recipe rendered, and no other.
    #[test]
    fn the_server_reads_its_own_file_and_no_other() {
        let spec = Mysql
            .spec(&context("{}"))
            .expect("a spec")
            .build()
            .expect("a valid spec");

        let first = spec.args().first().expect("an argument");

        assert!(first.starts_with("--defaults-file="), "{first}");
        assert!(first.ends_with(CONFIG_FILE), "{first}");
    }

    /// Stopped by asking it to, so the next start is not a crash recovery.
    #[test]
    fn mysql_is_stopped_by_asking_it_to_shut_down() {
        let spec = Mysql
            .spec(&context("{}"))
            .expect("a spec")
            .build()
            .expect("a valid spec");

        let StopBehaviour::Command { args, .. } = spec.stop() else {
            panic!("a database is asked to stop: {:?}", spec.stop());
        };

        assert!(args.iter().any(|arg| arg == "shutdown"), "{args:?}");
    }

    /// And has no reload, because MySQL reads its configuration once.
    #[test]
    fn mysql_has_no_reload_because_it_cannot() {
        let spec = Mysql
            .spec(&context("{}"))
            .expect("a spec")
            .build()
            .expect("a valid spec");

        assert!(spec.reload().is_none(), "{:?}", spec.reload());
    }

    /// **The route is a table, and it is asked on every system rather than only on this one.**
    ///
    /// Which program makes a data directory is not one question with a version answer: 5.6 answers
    /// differently on Windows than on Unix, and a `cfg!` inside the step builder would leave two of
    /// the three routes unexercised wherever the tests happen to run.
    #[test]
    fn the_bootstrap_route_is_a_table_of_three_and_not_a_version_test() {
        assert_eq!(route("8.4.10", true), Route::Initialize);
        assert_eq!(route("8.4.10", false), Route::Initialize);
        assert_eq!(route("5.7.44", false), Route::Initialize);
        assert_eq!(
            route("5.6.51", false),
            Route::Script,
            "5.6 predates --initialize-insecure and has an installer script"
        );
        assert_eq!(
            route("5.6.51", true),
            Route::ShippedData,
            "upstream's 5.6 zip has no installer and ships a built data/ instead"
        );
    }

    /// A modern line is bootstrapped by the server itself, and then told its password.
    ///
    /// The second step is a server that listens on nothing — `--skip-networking` — which is what
    /// keeps a password-less root off 3306 during the one window it would otherwise exist in. It
    /// stops itself: the statement after the password is `SHUTDOWN`.
    #[test]
    fn a_modern_line_initialises_itself_and_sets_the_password_on_a_server_nobody_can_reach() {
        let steps =
            steps_for(&initialised("8.4.10", provides()), Route::Initialize).expect("two steps");

        assert_eq!(steps.len(), 2, "{steps:?}");
        assert!(
            steps[0]
                .args
                .iter()
                .any(|arg| arg == "--initialize-insecure"),
            "{:?}",
            steps[0]
        );

        let setting = &steps[1];
        let file = setting
            .secret_file
            .as_ref()
            .expect("the statement goes into a file this step is given");

        assert!(
            setting.args.iter().any(|arg| arg == "--skip-networking"),
            "{setting:?}"
        );
        assert!(
            setting
                .args
                .iter()
                .any(|arg| arg == &format!("--init-file={}", file.path.display())),
            "{setting:?}"
        );
        assert!(
            file.content.contains("ALTER USER 'root'@'localhost'"),
            "the account `--initialize-insecure` creates is the only one there is"
        );
        assert!(
            file.content.contains("SHUTDOWN"),
            "nothing else can stop a server listening on nothing"
        );
    }

    /// 5.6 on Unix runs its installer through the interpreter the script's own first line names.
    ///
    /// In a tree compiled by `mixengine-packages` that script is Perl rather than shell, because
    /// 5.6's `scripts/CMakeLists.txt` configures `mysql_install_db.pl.in` on every platform and
    /// only appends `.pl` to the name on Windows. Handed to `/bin/sh` it answers `use: command not
    /// found` and reads like a corrupt archive.
    #[test]
    fn five_six_on_unix_runs_its_installer_through_the_interpreter_the_script_names() {
        let steps =
            steps_for(&initialised("5.6.51", provides_5_6()), Route::Script).expect("the steps");

        let installing = steps
            .iter()
            .find(|step| step.label.contains("data directory"))
            .expect("a step that creates the data directory");

        let script = installing.args.join(" ");

        assert!(script.contains("mysql_install_db"), "{installing:?}");
        assert!(
            script.contains("#!"),
            "the interpreter is read off the script's own first line: {installing:?}"
        );
    }

    /// 5.6 on Windows has no installer at all, and copies the directory upstream ships built.
    #[test]
    fn five_six_on_windows_copies_the_data_directory_upstream_ships() {
        let steps =
            steps_for(&initialised("5.6.51", provides()), Route::ShippedData).expect("the steps");

        let copying = steps
            .iter()
            .find(|step| step.label.contains("data directory"))
            .expect("a step that creates the data directory");

        assert!(
            copying.args.iter().any(|arg| arg.ends_with("data")),
            "{copying:?}"
        );
    }

    /// Wherever the password goes, it is never an argument.
    ///
    /// An argument list is readable by every process on the machine through `ps` and Task Manager.
    /// The two places it may be are a file this step is given and removed with, and the standard
    /// input of a server in bootstrap mode.
    #[test]
    fn the_password_is_never_in_an_argument_list() {
        const PASSWORD: &str = "abcd1234abcd1234abcd1234abcd1234";

        for (version, provides, route) in [
            ("8.4.10", provides(), Route::Initialize),
            ("5.6.51", provides_5_6(), Route::Script),
            ("5.6.51", provides(), Route::ShippedData),
        ] {
            let steps = steps_for(&initialised(version, provides), route).expect("the steps");

            for step in &steps {
                assert!(
                    !step.args.iter().any(|arg| arg.contains(PASSWORD)),
                    "{version} {route:?}: {step:?}"
                );
            }

            assert!(
                steps.iter().any(|step| {
                    step.stdin
                        .as_ref()
                        .is_some_and(|input| input.contains(PASSWORD))
                        || step
                            .secret_file
                            .as_ref()
                            .is_some_and(|file| file.content.contains(PASSWORD))
                }),
                "{version} {route:?} never sets the password at all"
            );
        }
    }

    /// A credential that would need escaping in a SQL literal is refused rather than escaped.
    #[test]
    fn a_credential_that_would_need_escaping_is_refused() {
        let mut context = context_of("8.4.10", provides(), "{}");
        context.put_secret(ROOT, "hunter2'; DROP DATABASE mysql; --");

        steps(&context).expect_err("a password that is not alphanumeric is a bug upstream of here");
    }

    /// The ritual declares the one credential it needs, and its steps are the ones above.
    #[test]
    fn mysql_declares_the_one_credential_its_ritual_needs() {
        let ritual = Mysql.ritual().expect("a database has a first run");

        assert_eq!(ritual.secrets.len(), 1);
        assert_eq!(ritual.secrets[0].key, ROOT);
    }
    /// **A first run is measured before its credentials exist** — the same guarantee, for this
    /// recipe's own validation of a stand-in credential. See the MariaDB test of this name for the
    /// failure it exists against.
    #[test]
    fn a_first_run_is_measured_before_its_credentials_exist() {
        let plan = FirstRun::new(&context("{}"), Mysql.ritual().expect("mysql bootstraps"));

        assert!(
            plan.budget() >= BOOTSTRAP_PATIENCE,
            "one bootstrap step alone asks for {BOOTSTRAP_PATIENCE:?}, and the plan measured {:?}",
            plan.budget()
        );
    }
}
