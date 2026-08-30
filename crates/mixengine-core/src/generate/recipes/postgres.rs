//! PostgreSQL: the other database MixEngine runs — roadmap task **T34**.
//!
//! **The second and last recipe with something to do before it can start**, after MariaDB: a data
//! directory `initdb` creates once, and a superuser password that exists only in the OS keyring.
//! [`Recipe::ritual`] and [`first_run`](crate::generate::first_run) are the machinery; this
//! module's share is the two steps.
//!
//! # What was measured rather than assumed
//!
//! Every platform difference below was measured against a real server — `mixengine-packages`'
//! `tools/postgres_smoke.py` runs this sequence on every runner, and `docs/packages/postgres.md`
//! records what it cost:
//!
//! - **`postgres` refuses an elevated Windows token.** `check_root()` asks `pgwin32_is_admin()` and
//!   exits. That is not a database problem and is not solved here: every child MixEngine starts to
//!   run a user's software is created from a restricted token — roadmap task **T34a**, and
//!   `.claude/decisions/0010-supervised-child-never-inherits-administrators.md`.
//! - **`initdb` inherits the machine's locale when it is not told one**, reports *could not find
//!   suitable text search configuration* on a machine whose locale it does not recognise, sets the
//!   default to `simple`, and **exits zero**. Two developers, two databases that answer
//!   differently. Hence the `locale` setting, stated rather than defaulted.
//! - **`initdb` refuses `--auth-*=scram-sha-256` unless it is also given a password**, which is the
//!   `--pwfile` this design exists to avoid. It is asked for `reject` instead — see
//!   the ritual's first step.
//! - **A socket path is capped at 103 characters** and the failure arrives after the server has
//!   started, which reads like a storage problem. `recipes::within_socket_limit`
//!   is what refuses by name instead.
//!
//! # Three generated files, and none of them the ones in the cluster
//!
//! `initdb` writes a `postgresql.conf` and a `pg_hba.conf` **inside the data directory**, and
//! nothing here ever reads, edits or regenerates them: the server is started with `--config-file`
//! pointing at `etc/`, which names the other two and names the data directory. That is the whole
//! reason generated configuration and the data directory can keep opposite policies — the same
//! separation `basedir`/`datadir` gives MariaDB, reached through a different door.
//!
//! # This is the first service with a real reload on all three systems
//!
//! MariaDB has none and cannot have one; php-fpm's is a signal, which Windows answers `unsupported`.
//! `pg_ctl reload` is one shape everywhere, and a running server re-reads both `postgresql.conf` and
//! `pg_hba.conf`. What it does **not** re-read is `shared_buffers`, `port` and `listen_addresses`:
//! those wait for a restart somebody asked for.
//!
//! # What this recipe deliberately does not do
//!
//! `pg_upgrade` — a cluster bootstrapped by one major cannot be read by the next, and
//! [`READY_MARKER`](crate::generate::first_run::READY_MARKER) already records which one did it. A
//! second instance beside the first (T36). An application role that is not the superuser.
//! Extensions: the artifact ships 46 and `CREATE EXTENSION` is the user's to run;
//! `shared_preload_libraries` stays empty. Backup and restore.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use mixengine_platform::KEYRING_SERVICE;
use mixengine_proto::{
    HealthCheck, HealthProbe, Millis, ReadyCheck, ReloadBehaviour, ServiceSpec, ServiceSpecBuilder,
    StopBehaviour,
};

use crate::generate::first_run::{Ritual, SecretSpec, Step};
use crate::generate::recipe::{Context, Endpoints, Instancing, Recipe, TemplateFile, Upstream};
use crate::generate::settings::{Preset, Setting};
use crate::{Error, Result};

/// The `packages.name` this recipe is found under.
///
/// **`postgres` and not `postgresql`.** The index publishes `"kind": "postgres"` and a recipe is
/// found by that name, so the ids are `postgres@main` and the directory is `etc/postgres@main/`.
/// The publisher is the authority on its own name.
pub const PACKAGE: &str = "postgres";

/// The server. One of the five commands an artifact must publish to be published at all.
const SERVER: &str = "postgres";

/// The one-shot that creates a cluster, and the only thing that ever reads [`LOCALE`].
const INITDB: &str = "initdb";

/// What stops and reloads a running server, through `postmaster.pid` in the data directory.
const CTL: &str = "pg_ctl";

/// The client the readiness check runs one query with.
const PSQL: &str = "psql";

/// The probe the health check repeats. See [`Postgres::spec`] for why these are two different
/// programs asking two different questions.
const ISREADY: &str = "pg_isready";

/// The file the server is started against, under `etc/<service-id>/`.
const CONFIG_FILE: &str = "postgresql.conf";

/// Who may connect, from where, and how they prove it.
const HBA_FILE: &str = "pg_hba.conf";

/// Empty, and it has to exist. See the template.
const IDENT_FILE: &str = "pg_ident.conf";

/// How much memory the server keeps pages in. Dev-tuned: this is a laptop running a development
/// site beside an editor and a browser. **Not re-read by a reload.**
const SHARED_BUFFERS: &str = "shared_buffers";

/// How many connections it accepts at once. PostgreSQL's own default. Not re-read by a reload.
const MAX_CONNECTIONS: &str = "max_connections";

/// How long the server is given to answer an authenticated query before the start is a failure.
///
/// Two minutes, for MariaDB's reason: a warm start answers in a second, and the start this is sized
/// for is a recovery after an unclean stop, which replays the write-ahead log first.
const READY_TIMEOUT: &str = "ready_timeout_ms";

/// How long `pg_ctl stop --mode fast` is given before the group is killed.
const STOP_GRACE: &str = "stop_grace_ms";

/// The locale the cluster is created with — **read by `initdb`, once, ever**.
///
/// `C` is not a preference, it is the only locale name that means the same thing on all three
/// systems: Windows spells one `English_United States.1252` and Unix spells it `en_US.UTF-8`, and
/// no single string is right on both. What it costs is stated plainly: `ORDER BY` on text is byte
/// order, so `Z` sorts before `a`. A user who wants their production collation says so and owns the
/// per-OS spelling when they do.
///
/// **`C.UTF-8` was the tempting middle and is rejected**: it is a glibc locale, macOS and Windows do
/// not have it, and the `builtin` provider that knows it only exists from PostgreSQL 17 — a recipe
/// running anything from 14 to 18 would have two behaviours decided by the machine, which is the
/// disease this setting exists to cure. ICU is out one version further back: `--locale-provider=icu`
/// is 15 and later.
///
/// **Changing this afterwards does nothing at all.** It is baked into the cluster by `initdb`.
/// Changing it for real means deleting the data directory.
const LOCALE: &str = "locale";

/// The encoding the cluster is created with — read by `initdb`, once, ever. See [`LOCALE`].
const ENCODING: &str = "encoding";

/// How often the server is asked whether it is still answering.
const HEALTH_INTERVAL: Millis = Millis(10_000);

/// How long one of those may take. Well inside the interval, which [`ServiceSpec`]'s validation
/// insists on: two probes that could overlap are two probes that can queue.
const HEALTH_TIMEOUT: Millis = Millis(5_000);

/// How long a reload is waited for. Nothing is killed when it expires — the server goes on running
/// the configuration it already had.
const RELOAD_PATIENCE: Millis = Millis(10_000);

/// The environment variable PostgreSQL's own clients read a password from.
///
/// **The reason no password reaches a command line this recipe builds.** An argument list is visible
/// to every process on the machine through `ps` and Task Manager; an environment is not.
const PASSWORD_VARIABLE: &str = "PGPASSWORD";

/// The account the ritual creates and everything here authenticates as.
const SUPERUSER: &str = "postgres";

/// What this recipe's ritual needs the daemon to generate.
///
/// Thirty-two characters of `[A-Za-z0-9]` — 190 bits — and the alphabet is what makes the SQL
/// interpolation in [`set_the_password`] safe without an escaper.
const SECRETS: &[SecretSpec] = &[SecretSpec {
    key: SUPERUSER,
    length: 32,
}];

/// How long each half of the bootstrap is given.
///
/// Fifteen minutes, MariaDB's number and for the same reason: `initdb` writes and fsyncs a whole
/// cluster, and on a cold Windows machine Defender reads every one of those files on the way past.
const BOOTSTRAP_PATIENCE: Millis = Millis(900_000);

/// PostgreSQL, as MixEngine runs it.
#[derive(Debug)]
pub struct Postgres;

impl Recipe for Postgres {
    fn package(&self) -> &'static str {
        PACKAGE
    }

    /// As many as are named: `postgres@main`, `postgres@legacy`. A machine serving two projects
    /// with incompatible schemas needs two clusters, not one with two schemas in it.
    fn instancing(&self) -> Instancing {
        Instancing::Named
    }

    /// 5432, and every `psql` and connection string in the world knows it.
    fn preferred_port(&self) -> Option<u16> {
        Some(5432)
    }

    /// `postgres --version`, which is cheap and touches the server's own machinery.
    fn smoke_test(&self) -> Option<crate::install::SmokeTest> {
        Some(crate::install::SmokeTest {
            executable: SERVER.to_owned(),
            args: vec!["--version".to_owned()],
        })
    }

    fn settings(&self) -> &'static [Setting] {
        &[
            Setting {
                key: SHARED_BUFFERS,
                default: Preset::Text("128MB"),
            },
            Setting {
                key: MAX_CONNECTIONS,
                default: Preset::Number(100),
            },
            Setting {
                key: READY_TIMEOUT,
                default: Preset::Number(120_000),
            },
            Setting {
                key: STOP_GRACE,
                default: Preset::Number(60_000),
            },
            Setting {
                key: LOCALE,
                default: Preset::Text("C"),
            },
            Setting {
                key: ENCODING,
                default: Preset::Text("UTF8"),
            },
        ]
    }

    /// The directory the socket goes in, on a system that has sockets.
    ///
    /// [`cfg!`] is a *value* and not an attribute, so both arms compile everywhere and a test can
    /// exercise the branch the machine it runs on is not.
    fn endpoints(&self, context: &Context) -> Result<Endpoints> {
        if cfg!(windows) {
            return Ok(Endpoints::default());
        }

        Ok(Endpoints {
            socket: Some(socket_directory(context)?),
            plugins: None,
            ..Endpoints::default()
        })
    }

    /// Its port, and the socket file inside the directory only this recipe may name — T70a's D4.
    ///
    /// `unix_socket_directories` takes a *directory* and the server creates `.s.PGSQL.<port>`
    /// inside it. That convention belongs to this recipe and its own template — [`Endpoints`] says
    /// in as many words that nothing outside the pair may read its `socket` either way — so the
    /// file name is derived here and nowhere above.
    fn held_while_stopped(&self, context: &Context) -> Result<Vec<Upstream>> {
        let listening = address(context)?;
        let mut held = vec![Upstream::Tcp(listening)];

        if !cfg!(windows) {
            held.push(Upstream::Socket(
                socket_directory(context)?.join(format!(".s.PGSQL.{}", listening.port())),
            ));
        }

        Ok(held)
    }

    /// An hour — T70a, design D9, and the number `resource-isolation.md` already publishes.
    ///
    /// **Longer than php-fpm's half hour on purpose.** A pool starts in tens of milliseconds; a
    /// server replays its log first, so a developer coming back to a project after fifty minutes
    /// would pay for the stop rather than benefit from it. What the extra half hour costs is one
    /// idle server's memory, which is the thing being traded and is worth naming.
    ///
    /// **Answerable only now.** Until T70a the daemon could stop this and nothing could start it
    /// again, and a default that idled it would have been a default that broke a home which changed
    /// nothing — which is why the number arrives in the last commit of that task rather than the
    /// first.
    fn idle_default(&self) -> Option<mixengine_proto::Millis> {
        Some(mixengine_proto::Millis::from_secs(60 * 60))
    }

    /// The server, and the four things around it that are three different programs.
    ///
    /// **Readiness is a query and health is a probe.** `pg_isready` is a good liveness check and a
    /// weak readiness one: it sends a startup packet and can tell *accepting* from *rejecting*,
    /// which a TCP accept cannot — but it never authenticates, so a cluster whose superuser password
    /// never got set passes it every time. Stop and reload are both `pg_ctl`, which reaches the
    /// postmaster through `postmaster.pid` and so does not need to have started it.
    ///
    /// # Errors
    ///
    /// [`Error::ServiceProvidesNothing`] for an install publishing none of the four commands this
    /// names, and [`Error::SettingValue`] for a row with no port.
    /// A backend per connection, so the count is the thing this server is actually spending.
    ///
    /// `pg_stat_database` holds the transaction counters and is a table — reachable only by
    /// connecting as a role, which is the same refusal MariaDB's note here records.
    fn idle_probe(&self, context: &Context) -> Option<mixengine_proto::IdleProbe> {
        context
            .port()
            .map(|port| mixengine_proto::IdleProbe::Connections { port })
    }

    fn spec(&self, context: &Context) -> Result<ServiceSpecBuilder> {
        let settings = context.settings();
        let server = context.provided(SERVER)?;
        let psql = context.provided(PSQL)?;
        let isready = context.provided(ISREADY)?;
        let ctl = context.provided(CTL)?;
        let addr = address(context)?;

        Ok(ServiceSpec::builder(context.service().clone(), &server)
            // As MariaDB's: the port a failed start is diagnosed against (T38).
            .ports([addr.port()])
            // **One option, and the cluster is not on it.** `data_directory` is stated inside this
            // file; naming it here as well would be two places for one path to drift.
            .args([format!(
                "--config-file={}",
                context.config(CONFIG_FILE).display()
            )])
            .cwd(context.etc())
            // Named rather than carried: a spec is data and cannot hold a password (ADR 0006). The
            // supervisor reads it out of the OS keyring at spawn and keeps the resolved environment
            // for the life of the process — which is what lets the readiness check authenticate.
            // The *server* never reads this variable; `psql` does.
            .env_from_keyring(
                PASSWORD_VARIABLE,
                KEYRING_SERVICE,
                context.secret_address(SUPERUSER),
            )
            .ready(ReadyCheck::Command {
                program: psql,
                args: query(addr),
                timeout: millis(settings.number(READY_TIMEOUT)),
            })
            .health(HealthCheck {
                probe: HealthProbe::Command {
                    program: isready,
                    args: connection(addr),
                },
                interval: HEALTH_INTERVAL,
                timeout: HEALTH_TIMEOUT,
                // Three rather than one: a probe can miss its window behind a checkpoint on a busy
                // database, and that is not a sick server.
                failures_before_degraded: 3,
                successes_before_running: 1,
            })
            .stop(StopBehaviour::Command {
                program: ctl.clone(),
                args: {
                    let mut args = vec!["stop".to_owned()];
                    args.extend(cluster(context));
                    args.push("--mode=fast".to_owned());

                    args
                },
                grace: millis(settings.number(STOP_GRACE)),
            })
            // **A running server really honours this**: `SIGHUP` re-reads `postgresql.conf` *and*
            // `pg_hba.conf`. What it does not re-read is `shared_buffers`, `port` and
            // `listen_addresses` — those wait for a restart somebody asked for.
            .reload(ReloadBehaviour::Command {
                program: ctl,
                args: {
                    let mut args = vec!["reload".to_owned()];
                    args.extend(cluster(context));

                    args
                },
                patience: RELOAD_PATIENCE,
            }))
    }

    /// The cluster, created once, with a superuser password that exists only in the OS keyring.
    fn ritual(&self) -> Option<Ritual> {
        Some(Ritual {
            secrets: SECRETS,
            steps,
        })
    }

    fn files(&self) -> &'static [TemplateFile] {
        &[
            TemplateFile {
                path: CONFIG_FILE,
                source: include_str!("postgres/postgresql.conf"),
            },
            TemplateFile {
                path: HBA_FILE,
                source: include_str!("postgres/pg_hba.conf"),
            },
            TemplateFile {
                path: IDENT_FILE,
                source: include_str!("postgres/pg_ident.conf"),
            },
        ]
    }
}

/// Where this instance's socket file goes on a system with Unix sockets.
///
/// **A directory, not a file**, because `unix_socket_directories` takes a directory — and the file
/// PostgreSQL then creates in it is `.s.PGSQL.<port>`. The limit is checked against *that*, because
/// what `sockaddr_un` caps is the whole path, and a recipe measuring the directory would be
/// seventeen characters optimistic about a failure that arrives after the server has started.
///
/// **`run/` itself, and not a directory named after the service — measured.** The first version of
/// this returned `run/<service-id>/`, which reads better and does not exist: nothing creates a
/// directory under `run/` for a service, and the server answers
/// *could not create lock file … No such file or directory* and then crash-loops. `run/` is there
/// because the daemon's own socket is in it. Nothing is lost by sharing it — the file name carries
/// the port, so two clusters cannot collide any more than two of them could share a port — and the
/// path is shorter, which is the one budget this function is spending.
///
/// # Errors
///
/// [`Error::SettingValue`] when the path this home would need is longer than the kernel accepts.
fn socket_directory(context: &Context) -> Result<PathBuf> {
    let directory = context.run().to_path_buf();
    let port = context.port().unwrap_or(u16::MAX);
    let file = directory.join(format!(".s.PGSQL.{port}"));

    super::within_socket_limit(context.service().as_str(), "socket", &file)?;

    Ok(directory)
}

/// Where this instance listens on TCP: the port its row was given, on the address it names.
///
/// # Errors
///
/// [`Error::SettingValue`] when the row carries no port. A database nothing can be pointed at is not
/// a database anybody can use, and the rendered file would say `port = none` — so this is refused
/// here rather than discovered by a server that will not start.
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

/// How a client is told which server to ask and who to be.
///
/// One function rather than two copies, because the readiness check and the health check must reach
/// the *same* server: a probe that reached a different one would report a service that is not this
/// one. `--host` with an address rather than a socket, because the port is what the row carries and
/// the socket is what the file names — and a probe should ask the question a client would.
fn connection(addr: SocketAddr) -> Vec<String> {
    vec![
        format!("--host={}", addr.ip()),
        format!("--port={}", addr.port()),
        format!("--username={SUPERUSER}"),
    ]
}

/// [`connection`], asking the one question that proves the server answers queries as the superuser.
///
/// `-t` and `-A` so the answer is one word with no decoration, and `--no-password` so a client that
/// cannot authenticate **fails** instead of stopping at a prompt nobody is there to answer — which
/// is the difference between a readiness check that reports a failure and one that times out.
/// Measured: without it, `psql` waits at `Password:` for ever.
fn query(addr: SocketAddr) -> Vec<String> {
    let mut args = connection(addr);

    args.push("--dbname=postgres".to_owned());
    args.push("--no-password".to_owned());
    args.push("-tAc".to_owned());
    args.push("SELECT 1".to_owned());

    args
}

/// Which cluster `pg_ctl` is being asked about.
///
/// The one place the data directory is named on a command line, and it is `pg_ctl`'s because
/// `pg_ctl` has no `--config-file`: it reads `postmaster.pid` out of the directory to find the
/// process to signal.
fn cluster(context: &Context) -> Vec<String> {
    vec![format!("--pgdata={}", context.data().display())]
}

/// The two things that have to happen before this cluster is ever started.
///
/// **The same two on every system**, which is where this differs from MariaDB: upstream ships one
/// `initdb` rather than two programs of one name, and PostgreSQL is told nothing about where it
/// lives, so there is no unquoted `$basedir` to work around and no space-free view to build.
///
/// # Errors
///
/// [`Error::ServiceProvidesNothing`] for an install missing `initdb` or `postgres`, and
/// [`Error::SettingValue`] for a credential this recipe will not put in a SQL literal.
fn steps(context: &Context) -> Result<Vec<Step>> {
    let password = context.secret(SUPERUSER);

    // **Refused rather than escaped.** The only producer of this value is
    // `mixengine_platform::generate_secret`, whose alphabet is chosen so that the interpolation in
    // `set_the_password` is safe; an escaper here would be a second thing to get right for a case
    // that cannot arise, and what it would hide is a credential in the wrong half of a statement.
    if password.is_empty() || !password.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(Error::SettingValue {
            service: context.service().as_str().to_owned(),
            key: "superuser password",
            value: "<redacted>".to_owned(),
            reason: "a generated credential is alphanumeric so that it needs no escaping in the \
                     statement that sets it; this one is not, which is a bug in whatever made it",
        });
    }

    Ok(vec![
        create_the_cluster(context)?,
        set_the_password(context, password)?,
    ])
}

/// `initdb`, told the two things it would otherwise read off the machine.
///
/// **The superuser is created with no password at all**, which is safe for exactly as long as this
/// step and the next one are the only things that can reach the cluster: `initdb` opens no port and
/// no socket, and the next step opens neither either. See [`set_the_password`] for why the
/// alternative — `--pwfile` — is not taken.
///
/// **`--auth-*=reject`, and it was measured.** Naming an authentication method that needs a verifier
/// obliges `initdb` to be given one: with `scram-sha-256` it refuses outright — *must specify a
/// password for the superuser to enable password authentication* — which is the `--pwfile` this
/// ritual exists to avoid. `reject` needs none, and it is stricter than the `trust` `initdb` would
/// otherwise default to: the `pg_hba.conf` it writes **inside the cluster** then permits nothing at
/// all. The server never reads that file — `hba_file` names the generated one — so this is the belt
/// rather than the braces.
///
/// The data directory is **not** created first: `initdb` refuses a directory that already has
/// anything in it, and the daemon's marker step writes beside the directory rather than inside it
/// for exactly this reason — see [`STARTED_MARKER`](crate::generate::first_run::STARTED_MARKER).
///
/// # Errors
///
/// [`Error::ServiceProvidesNothing`] for an install that publishes no `initdb`.
fn create_the_cluster(context: &Context) -> Result<Step> {
    let settings = context.settings();

    Ok(Step {
        label: "create the data directory".to_owned(),
        program: context.provided(INITDB)?,
        args: vec![
            format!("--pgdata={}", context.data().display()),
            format!("--username={SUPERUSER}"),
            "--auth-local=reject".to_owned(),
            "--auth-host=reject".to_owned(),
            format!("--encoding={}", settings.text(ENCODING)),
            format!("--locale={}", settings.text(LOCALE)),
        ],
        stdin: None,
        secret_file: None,
        env: BTreeMap::new(),
        cwd: context.etc().to_path_buf(),
        timeout: BOOTSTRAP_PATIENCE,
    })
}

/// Set the superuser's password, through a server that listens on nothing.
///
/// **`--single` opens no port and no socket**, so there is no instant at which a password-less
/// superuser sits on `127.0.0.1:5432` waiting for whoever is quickest.
///
/// # Why not `initdb --pwfile`
///
/// Because `initdb` will only take a password from a *file*, and that file is a plaintext superuser
/// credential on disk for the whole of a bootstrap that can take minutes — and one that a ritual
/// which failed half way through leaves behind. [`Step::stdin`] exists for exactly this shape, and
/// MariaDB's bootstrap is its only other caller.
///
/// `password_encryption` defaults to `scram-sha-256` from PostgreSQL 14, which is the floor the
/// index publishes, so the stored verifier is SCRAM without this recipe stating anything.
///
/// **Nothing may read this step's exit code as proof.** Measured: single-user mode prints
/// `ERROR:  syntax error` and still exits 0. What proves the password was set is the readiness
/// check, which is an authenticated query — see [`Postgres::spec`].
///
/// # Errors
///
/// [`Error::ServiceProvidesNothing`] for an install that publishes no `postgres`.
fn set_the_password(context: &Context, password: &str) -> Result<Step> {
    Ok(Step {
        label: "set the superuser's password".to_owned(),
        program: context.provided(SERVER)?,
        args: vec![
            "--single".to_owned(),
            "-D".to_owned(),
            context.data().display().to_string(),
            // The database the single-user backend opens. `postgres` is one of the three `initdb`
            // creates and is the one everything else here connects to.
            "postgres".to_owned(),
        ],
        // One statement, one line: single-user mode reads a statement per line, so a wrapped one is
        // two statements and neither of them valid.
        stdin: Some(format!("ALTER ROLE {SUPERUSER} PASSWORD '{password}';\n")),
        secret_file: None,
        env: BTreeMap::new(),
        cwd: context.etc().to_path_buf(),
        timeout: BOOTSTRAP_PATIENCE,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use mixengine_proto::ServiceId;

    use super::*;
    use crate::generate::Upstream;
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
    ///
    /// One spelling per command on every route — the Debian cell buys that with a symlink — so this
    /// is a layout rather than a search. The five required ones only: the artifact publishes
    /// fifteen, and a recipe may rely on these.
    fn provides() -> BTreeMap<String, String> {
        ["postgres", "initdb", "pg_ctl", "psql", "pg_isready"]
            .into_iter()
            .map(|name| {
                (
                    name.to_owned(),
                    format!("bin/{name}{}", std::env::consts::EXE_SUFFIX),
                )
            })
            .collect()
    }

    /// What a failed start is diagnosed against — roadmap task **T38**.
    #[test]
    fn the_spec_declares_the_port_the_cluster_will_bind() {
        let context = context("{}");
        let spec = Postgres
            .spec(&context)
            .expect("a spec")
            .build()
            .expect("a valid spec");

        assert_eq!(spec.ports(), [5432]);
    }

    /// A `postgres@main` in a home at [`root`], with `overrides` applied.
    fn context(overrides: &str) -> Context {
        with_provides(provides(), overrides)
    }

    /// The same, for an install that publishes something else — or nothing.
    fn with_provides(provides: BTreeMap<String, String>, overrides: &str) -> Context {
        let service = ServiceId::parse("postgres@main").expect("an id");
        let settings =
            Settings::merge(Postgres.settings(), overrides, &service).expect("usable overrides");

        let context = Context::for_test(
            service,
            PACKAGE,
            Path::new(root()),
            provides,
            Some(5432),
            settings,
        );
        let endpoints = Postgres
            .endpoints(&context)
            .expect("a home this short has a usable socket path");

        context.with_endpoints(endpoints)
    }

    /// The rendered `name` for `overrides`.
    fn rendered(name: &str, overrides: &str) -> String {
        recipe::render(&Postgres, &context(overrides))
            .expect("a rendering")
            .into_iter()
            .find(|document| document.relative() == Path::new(name))
            .unwrap_or_else(|| panic!("this recipe renders no {name}"))
            .contents()
            .to_owned()
    }

    /// The built spec for `overrides`.
    fn built(overrides: &str) -> ServiceSpec {
        Postgres
            .spec(&context(overrides))
            .expect("a spec")
            .build()
            .expect("a valid spec")
    }

    /// Two clusters in one home, so the id carries an `@`.
    /// **A database is woken at the addresses it listens on itself** — T70a's D4.
    ///
    /// On a system with Unix sockets that is *two* addresses and not one, and the difference is a
    /// client's habit rather than a configuration: a generated `.env` names `127.0.0.1`, and the
    /// client typed with no host at all names the socket. Waking on only one of them leaves the
    /// other hanging against an address nothing holds.
    #[test]
    fn a_stopped_server_is_woken_at_its_port_and_at_its_socket() {
        let held = Postgres
            .held_while_stopped(&context("{}"))
            .expect("the addresses it is woken at");

        assert!(
            held.contains(&Upstream::Tcp(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                5432
            ))),
            "a client dialling 127.0.0.1:5432 would not wake it: {held:?}"
        );

        assert_eq!(
            held.len(),
            if cfg!(windows) { 1 } else { 2 },
            "the socket is an address on every system that has one: {held:?}"
        );
    }

    /// **The socket file PostgreSQL would have created, named by the recipe that knows the
    /// convention** — T70a's D4.
    ///
    /// `unix_socket_directories` takes a directory and the server creates `.s.PGSQL.<port>` inside
    /// it, so [`Endpoints::socket`] holds a directory here and a *file* for MariaDB — which is why
    /// its own doc says nothing outside each pair may read it either way, and why this name is
    /// derived in this file and nowhere above.
    #[test]
    #[cfg(not(windows))]
    fn a_stopped_server_is_woken_at_the_socket_file_the_server_would_have_created() {
        let held = Postgres
            .held_while_stopped(&context("{}"))
            .expect("the addresses it is woken at");

        assert!(
            held.iter().any(|address| matches!(
                address,
                Upstream::Socket(path) if path.file_name() == Some(".s.PGSQL.5432".as_ref())
            )),
            "nothing here is the file a client finds in unix_socket_directories: {held:?}"
        );
    }

    #[test]
    fn postgres_is_named_because_a_home_may_have_two() {
        assert_eq!(Postgres.instancing(), Instancing::Named);
    }

    /// Every path in the rendered configuration is quoted and forward-slashed.
    ///
    /// PostgreSQL's parser reads `\` inside a quoted string as an escape, so a home under
    /// `C:\Users\Nguyen Hai Quang` breaks an unrendered path — the same measurement `my.cnf` was
    /// written from, made against a different parser.
    #[test]
    fn every_path_in_the_configuration_is_quoted_and_forward_slashed() {
        let rendered = rendered(CONFIG_FILE, "{}");
        let named = [
            "data_directory",
            "hba_file",
            "ident_file",
            "unix_socket_directories",
        ];

        let mut seen = 0;
        for line in rendered
            .lines()
            .filter(|line| named.iter().any(|key| line.starts_with(key)))
        {
            let value = line.split_once('=').expect("a setting").1.trim();

            assert!(
                value.starts_with('\'') && value.ends_with('\''),
                "an unquoted path is not a path to this parser: {line}"
            );
            assert!(
                !value.contains('\\'),
                "a backslash is an escape inside a quoted string: {line}"
            );
            seen += 1;
        }

        assert!(seen >= 3, "no paths were checked at all:\n{rendered}");
    }

    /// **The wall between this data and everybody else on the machine.**
    ///
    /// Three lines, and not one of them `trust`. `initdb`'s own default would have written `trust`
    /// for local connections; this is the file the server actually reads, and it is generated
    /// rather than inherited precisely so that default never applies.
    #[test]
    fn nothing_in_pg_hba_trusts_anybody() {
        let rendered = rendered(HBA_FILE, "{}");

        assert!(
            !rendered
                .lines()
                .filter(|line| !line.trim_start().starts_with('#'))
                .any(|line| line.split_whitespace().any(|word| word == "trust")),
            "a `trust` line lets anybody on this machine in as the superuser:\n{rendered}"
        );

        for expected in [
            "local   all   all                scram-sha-256",
            "host    all   all   127.0.0.1/32 scram-sha-256",
            "host    all   all   ::1/128      scram-sha-256",
        ] {
            assert!(
                rendered.contains(expected),
                "{expected} is missing:\n{rendered}"
            );
        }
    }

    /// `ident_file` has to name a file that exists, and it may not name one inside the cluster.
    ///
    /// `etc/` reaching into the data directory would be generated configuration touching the one
    /// place that must never be regenerated — so the file is generated, empty, beside the other two.
    #[test]
    fn an_empty_pg_ident_is_generated_because_it_has_to_exist() {
        let rendered = rendered(IDENT_FILE, "{}");

        assert!(
            rendered
                .lines()
                .all(|line| line.trim().is_empty() || line.trim_start().starts_with('#')),
            "pg_ident maps an OS account to a database role and this recipe maps none:\n{rendered}"
        );
    }

    /// An override reaches the file rather than being ignored.
    #[test]
    fn the_configuration_says_what_the_overrides_said() {
        let rendered = rendered(CONFIG_FILE, r#"{"shared_buffers": "512MB"}"#);

        assert!(rendered.contains("shared_buffers = 512MB"), "{rendered}");
    }

    /// **A development machine's durability, which is not the server's** — roadmap task **T73**.
    ///
    /// The commit barrier and nothing else: `synchronous_commit = off` costs the last second of
    /// committed transactions and cannot leave a cluster that will not open. `fsync = off` would,
    /// which is why the second half of this test is the more important one.
    #[test]
    fn the_commit_barrier_is_relaxed_and_the_recovery_barriers_are_not() {
        let rendered = rendered(CONFIG_FILE, "{}");

        assert!(rendered.contains("synchronous_commit = off"), "{rendered}");

        for line in rendered.lines().filter(|line| !line.starts_with('#')) {
            assert!(!line.contains("fsync"), "{line}\n{rendered}");
            assert!(!line.contains("full_page_writes"), "{line}\n{rendered}");
        }
    }

    /// The escape hatch is real: `extra` renders after every directive above it, and PostgreSQL
    /// reads a file in order, so a later line wins.
    #[test]
    fn a_user_can_put_the_servers_own_durability_back() {
        let rendered = rendered(CONFIG_FILE, r#"{"extra": "synchronous_commit = on"}"#);

        let relaxed = rendered
            .find("synchronous_commit = off")
            .expect("the recipe states its own value");
        let restored = rendered
            .rfind("synchronous_commit = on")
            .expect("the override reaches the file");

        assert!(restored > relaxed, "{rendered}");
    }

    /// The server's own output is the supervisor's, which is a line worth stating rather than
    /// defaulting: `logging_collector = on` starts a background process that writes into `log/`
    /// *inside the data directory*, and a supervisor reading the process's streams would find an
    /// empty file and report that the server said nothing.
    #[test]
    fn the_server_writes_to_the_stream_the_supervisor_is_reading() {
        let rendered = rendered(CONFIG_FILE, "{}");

        assert!(rendered.contains("logging_collector = off"), "{rendered}");
        assert!(
            rendered.contains("log_destination = 'stderr'"),
            "{rendered}"
        );
    }

    /// The socket is a *directory* here, and the limit is checked against the file inside it.
    ///
    /// `unix_socket_directories` takes a directory and PostgreSQL puts `.s.PGSQL.<port>` in it — so
    /// a recipe that measured the directory against 103 characters would be measuring the wrong
    /// string by seventeen, and the failure it let through arrives after the server has started.
    ///
    /// **And the directory is `run/` itself, which is the half that was measured the hard way.**
    /// A directory named after the service reads better and does not exist: nothing creates one,
    /// and the server answers *could not create lock file … No such file or directory* and
    /// crash-loops. `run/` is there because the daemon's own socket is in it.
    #[cfg(unix)]
    #[test]
    fn the_socket_endpoint_is_a_directory_that_exists() {
        let context = context("{}");
        let endpoints = Postgres.endpoints(&context).expect("a short home");
        let directory = endpoints.socket.expect("this system has sockets");

        assert_eq!(
            directory,
            context.run(),
            "a socket directory nothing creates is a server that will not start"
        );
        assert!(
            !directory.to_string_lossy().contains(".s.PGSQL"),
            "the endpoint names the file rather than the directory: {}",
            directory.display()
        );
    }

    /// And a home too deep for one is refused by name, rather than by a server that has started.
    #[cfg(unix)]
    #[test]
    fn a_home_that_cannot_hold_a_socket_is_refused_by_name() {
        let deep = format!("/{}", "d".repeat(120));
        let service = ServiceId::parse("postgres@main").expect("an id");
        let settings =
            Settings::merge(Postgres.settings(), "{}", &service).expect("usable overrides");
        let context = Context::for_test(
            service,
            PACKAGE,
            Path::new(&deep),
            provides(),
            Some(5432),
            settings,
        );

        let error = Postgres
            .endpoints(&context)
            .expect_err("103 characters is 103 characters");

        assert!(
            matches!(&error, Error::SettingValue { key, .. } if *key == "socket"),
            "{error:?}"
        );
    }

    /// There are no Unix sockets on Windows, and the template states that rather than leaving it
    /// out.
    #[cfg(windows)]
    #[test]
    fn windows_has_no_socket_and_the_file_says_so() {
        let endpoints = Postgres.endpoints(&context("{}")).expect("endpoints");

        assert!(endpoints.socket.is_none(), "{endpoints:?}");

        let rendered = rendered(CONFIG_FILE, "{}");
        assert!(
            rendered.contains("unix_socket_directories = ''"),
            "{rendered}"
        );
    }

    /// A row with no port is refused here rather than by a server that will not start.
    #[test]
    fn a_service_row_with_no_port_is_refused() {
        let service = ServiceId::parse("postgres@main").expect("an id");
        let settings =
            Settings::merge(Postgres.settings(), "{}", &service).expect("usable overrides");
        let context = Context::for_test(
            service,
            PACKAGE,
            Path::new(root()),
            provides(),
            None,
            settings,
        );

        let error = address(&context).expect_err("a database listens on a port");

        assert!(
            matches!(&error, Error::SettingValue { key, .. } if *key == "port"),
            "{error:?}"
        );
    }

    /// The server is pointed at its own file, and at nothing else — and at the cluster only once.
    ///
    /// **`--config-file` alone, with no `-D`.** `data_directory` is stated *in* that file, and
    /// naming the cluster twice is two places for it to drift.
    #[test]
    fn the_server_reads_one_file_and_names_the_cluster_once() {
        let spec = built("{}");

        assert!(
            spec.args()
                .iter()
                .any(|arg| arg.starts_with("--config-file=") && arg.ends_with(CONFIG_FILE)),
            "{:?}",
            spec.args()
        );
        assert!(
            !spec
                .args()
                .iter()
                .any(|arg| arg == "-D" || arg.starts_with("--pgdata")),
            "the cluster is named in the file, not on the command line: {:?}",
            spec.args()
        );
    }

    /// **Ready and health are two different questions**, and this is where the difference lives.
    ///
    /// `pg_isready` is a good liveness probe and a weak readiness one: it sends a startup packet and
    /// distinguishes *accepting* from *rejecting*, which a TCP accept cannot — and it never
    /// authenticates. A cluster whose superuser password did not get set would pass it every time.
    /// So readiness is a query run as the superuser with the generated password, and health is the
    /// cheap probe that needs no credential.
    #[test]
    fn ready_authenticates_and_health_does_not() {
        let spec = built("{}");

        let ReadyCheck::Command { program, args, .. } = spec.ready() else {
            panic!("a database is proved up by a query: {:?}", spec.ready());
        };
        assert!(
            program.ends_with(format!("psql{}", std::env::consts::EXE_SUFFIX)),
            "{program:?}"
        );
        assert!(args.iter().any(|arg| arg == "SELECT 1"), "{args:?}");

        let HealthProbe::Command { program, .. } = &spec.health().expect("a health check").probe
        else {
            panic!("a database is watched by asking it, not by connecting to it");
        };
        assert!(
            program.ends_with(format!("pg_isready{}", std::env::consts::EXE_SUFFIX)),
            "{program:?}"
        );
    }

    /// **The file and the readiness check must name one server.**
    ///
    /// Computed twice — once in Jinja for the file, once in Rust for the check — and the failure
    /// when they disagree is a database that starts perfectly and is reported as never having come
    /// up.
    #[test]
    fn the_file_and_the_readiness_check_name_one_port() {
        let spec = built("{}");
        let rendered = rendered(CONFIG_FILE, "{}");

        let ReadyCheck::Command { args, .. } = spec.ready() else {
            panic!("{:?}", spec.ready());
        };

        assert!(args.iter().any(|arg| arg == "--port=5432"), "{args:?}");
        assert!(rendered.contains("port = 5432"), "{rendered}");
    }

    /// No password reaches an argument list, which every process on the machine can read.
    #[test]
    fn the_credential_is_named_rather_than_carried() {
        let spec = built("{}");

        assert!(
            matches!(
                spec.env().get(PASSWORD_VARIABLE),
                Some(mixengine_proto::EnvValue::Keyring { service, key })
                    if service == KEYRING_SERVICE && key == "postgres@main/postgres"
            ),
            "{:?}",
            spec.env()
        );
    }

    /// **Asked to shut down, not terminated.** A killed postmaster leaves an unclean shutdown and
    /// the next start pays for it by replaying the write-ahead log; `--mode fast` disconnects
    /// clients and shuts down cleanly. `pg_ctl` finds the postmaster through `postmaster.pid`, so it
    /// does not need to have started it.
    #[test]
    fn postgres_is_stopped_by_asking_it_to_shut_down() {
        let spec = built("{}");

        let StopBehaviour::Command { program, args, .. } = spec.stop() else {
            panic!("a signal leaves a cluster to recover: {:?}", spec.stop());
        };

        assert!(
            program.ends_with(format!("pg_ctl{}", std::env::consts::EXE_SUFFIX)),
            "{program:?}"
        );
        assert!(args.iter().any(|arg| arg == "stop"), "{args:?}");
        assert!(args.iter().any(|arg| arg == "--mode=fast"), "{args:?}");
    }

    /// **The first service in this catalogue with a real reload on all three systems.**
    ///
    /// MariaDB has none and cannot have one; php-fpm's is a signal, which Windows answers
    /// `unsupported`. `pg_ctl reload` is one shape everywhere.
    #[test]
    fn postgres_reloads_the_same_way_on_every_system() {
        let spec = built("{}");

        let Some(ReloadBehaviour::Command { program, args, .. }) = spec.reload() else {
            panic!("this is the recipe that has one: {:?}", spec.reload());
        };

        assert!(
            program.ends_with(format!("pg_ctl{}", std::env::consts::EXE_SUFFIX)),
            "{program:?}"
        );
        assert!(args.iter().any(|arg| arg == "reload"), "{args:?}");
    }

    /// A package that publishes none of what this recipe needs is named as such.
    #[test]
    fn a_postgres_without_its_own_binaries_is_named() {
        let context = with_provides(BTreeMap::new(), "{}");

        let error = Postgres.spec(&context).expect_err("nothing to run");

        assert!(
            matches!(error, Error::ServiceProvidesNothing { .. }),
            "{error:?}"
        );
    }

    /// A context carrying the credential the daemon would have generated.
    fn initialised(secret: &str) -> Context {
        let mut context = context("{}");
        context.put_secret(SUPERUSER, secret);

        context
    }

    /// A recipe that declares a credential also declares what to do with it.
    #[test]
    fn postgres_declares_the_one_credential_its_ritual_needs() {
        let ritual = Postgres.ritual().expect("a database has a first run");

        assert_eq!(ritual.secrets.len(), 1, "{:?}", ritual.secrets);
        assert_eq!(ritual.secrets[0].key, SUPERUSER);
        assert!(ritual.secrets[0].length >= 32, "{:?}", ritual.secrets);
    }

    /// The keyring entry the spec names and the one the ritual is stored under are one address.
    ///
    /// Composed twice, and the failure when they disagree is a server that starts and a client that
    /// cannot authenticate against it — reported as a service that never became ready.
    #[test]
    fn the_spec_and_the_ritual_name_one_credential() {
        let context = context("{}");
        let spec = built("{}");

        let named = spec
            .env()
            .get(PASSWORD_VARIABLE)
            .expect("the spec names a credential");

        assert!(
            matches!(named, mixengine_proto::EnvValue::Keyring { key, .. }
                if key == &context.secret_address(SUPERUSER)),
            "{named:?}"
        );
        assert_eq!(context.secret_address(SUPERUSER), "postgres@main/postgres");
    }

    /// **`initdb` is told its locale and its encoding, because otherwise it reads the machine's.**
    ///
    /// Measured in `mixengine-packages`: on a machine whose system locale it does not recognise it
    /// reports *could not find suitable text search configuration*, sets the default to `simple`,
    /// and **exits zero**. Two developers, two databases that answer differently.
    ///
    /// And `reject` rather than `scram-sha-256`, which `initdb` refuses without a password to go
    /// with it — see [`create_the_cluster`].
    #[test]
    fn the_cluster_is_created_with_a_locale_this_recipe_chose() {
        let steps = steps(&initialised("hunter2")).expect("steps");
        let initdb = &steps[0];

        assert!(
            initdb.args.iter().any(|arg| arg == "--locale=C"),
            "{:?}",
            initdb.args
        );
        assert!(
            initdb.args.iter().any(|arg| arg == "--encoding=UTF8"),
            "{:?}",
            initdb.args
        );
        assert!(
            initdb.args.iter().any(|arg| arg == "--auth-local=reject"),
            "{:?}",
            initdb.args
        );
        assert!(
            initdb.args.iter().any(|arg| arg == "--auth-host=reject"),
            "{:?}",
            initdb.args
        );
    }

    /// And an override of either reaches it, because a user who wants their production collation
    /// says so — and owns the per-OS spelling when they do.
    #[test]
    fn a_stated_locale_reaches_initdb() {
        let mut context = context(r#"{"locale": "en_US.UTF-8"}"#);
        context.put_secret(SUPERUSER, "hunter2");

        let steps = steps(&context).expect("steps");

        assert!(
            steps[0]
                .args
                .iter()
                .any(|arg| arg == "--locale=en_US.UTF-8"),
            "{:?}",
            steps[0].args
        );
    }

    /// **No `--pwfile`, and the reason is the whole shape of this ritual.**
    ///
    /// `initdb` will only take a password from a file, and a file is a plaintext superuser
    /// credential on disk for the whole of a bootstrap that can take minutes — and one a half-failed
    /// ritual leaves behind. So the role is created with **no** password and the password is set
    /// through single-user mode, which opens no port and no socket: there is no instant at which a
    /// password-less superuser is reachable by anybody.
    #[test]
    fn the_password_is_set_through_a_server_listening_on_nothing() {
        let steps = steps(&initialised("hunter2")).expect("steps");

        assert!(
            !steps
                .iter()
                .any(|step| step.args.iter().any(|arg| arg.starts_with("--pwfile"))),
            "a plaintext credential on disk: {steps:?}"
        );

        let single = steps
            .iter()
            .find(|step| step.stdin.is_some())
            .expect("one step speaks SQL");

        assert!(
            single.args.iter().any(|arg| arg == "--single"),
            "{:?}",
            single.args
        );
        assert!(
            single
                .program
                .ends_with(format!("postgres{}", std::env::consts::EXE_SUFFIX)),
            "{:?}",
            single.program
        );

        let sql = single.stdin.as_deref().expect("the SQL is on stdin");
        assert!(
            sql.contains("ALTER ROLE postgres PASSWORD 'hunter2';"),
            "{sql}"
        );

        // Single-user mode reads one statement per line, so a statement wrapped over two would be
        // two statements and neither of them valid.
        for line in sql.lines().filter(|line| !line.trim().is_empty()) {
            assert!(
                line.trim_end().ends_with(';'),
                "a wrapped statement: {line}"
            );
        }
    }

    /// A credential that would need escaping is refused rather than escaped.
    ///
    /// `mixengine_platform::generate_secret` restricts its alphabet for exactly the interpolation
    /// above; this is the other end of that arrangement.
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

    /// Two steps, the same two, on every system — which is the difference from MariaDB, whose
    /// bootstrap is two different programs and four steps on Unix.
    #[test]
    fn the_ritual_is_two_steps_everywhere() {
        let steps = steps(&initialised("abc123")).expect("steps");

        assert_eq!(
            steps.len(),
            2,
            "{:?}",
            steps.iter().map(|step| &step.label).collect::<Vec<_>>()
        );
        assert!(
            steps.iter().all(|step| step.program.is_absolute()),
            "a relative program is whatever the PATH says at the moment it runs: {steps:?}"
        );
    }

    /// An install missing `initdb` is named, rather than failing at the moment of the first run.
    #[test]
    fn a_postgres_that_cannot_bootstrap_is_named() {
        let mut without = provides();
        without.remove("initdb");

        let mut context = with_provides(without, "{}");
        context.put_secret(SUPERUSER, "abc123");

        let error = steps(&context).expect_err("there is nothing to create a cluster with");

        assert!(
            matches!(error, Error::ServiceProvidesNothing { .. }),
            "{error:?}"
        );
    }
    /// **A first run is measured before its credentials exist** — the same guarantee, for this
    /// recipe's own validation of a stand-in credential. See the MariaDB test of this name for the
    /// failure it exists against.
    #[test]
    fn a_first_run_is_measured_before_its_credentials_exist() {
        let plan = FirstRun::new(
            &context("{}"),
            Postgres.ritual().expect("postgres bootstraps"),
        );

        assert!(
            plan.budget() >= BOOTSTRAP_PATIENCE,
            "one bootstrap step alone asks for {BOOTSTRAP_PATIENCE:?}, and the plan measured {:?}",
            plan.budget()
        );
    }
}
