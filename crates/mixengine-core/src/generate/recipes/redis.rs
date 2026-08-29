//! Redis — roadmap task **T35**, with [`memcached`](super::memcached) the other half of it.
//!
//! The simplest recipe in the catalogue, and it is worth saying what makes it simple: there is
//! nothing to create before it runs. No data directory a second program has to build, no credential
//! that must exist in the OS keyring before anything touches the disk — a rendered file, a command
//! line, and a server. What T33 and T34 needed [`Recipe::ritual`] for, this one does not need
//! at all.
//!
//! # The one constraint, and it is upstream's
//!
//! `getAbsolutePath()` in Redis's `server.c` decides a path is absolute with
//! `if (relpath[0] == '/')` and otherwise joins it to `getcwd()`. **No Windows spelling of the
//! configuration path survives being passed as an argument** — both `C:\…` and `C:/…` arrive glued
//! onto the working directory. So the file is named *relatively* beside a working directory of its
//! own, which is one rule holding on all five published cells rather than a Windows arm here. That
//! was measured in `mixengine-packages` against every one of those cells and handed over as a
//! constraint on this side; a `/cygdrive/c/…` path would also have worked and is refused, because it
//! would put an emulation layer's private spelling into a command line MixEngine builds.
//!
//! # A cache, and it says so in every direction
//!
//! `save ""` and `appendonly no` mean this instance never writes its dataset anywhere, and the stop
//! is `SHUTDOWN NOSAVE` so that it does not start writing one on the way out either. That is a
//! decision rather than an omission: `.claude/features/services.md` says appendonly off by default
//! for development, and a cache that half-persists is the arrangement where somebody comes to trust
//! data that will disappear the next time the process is recycled.
//!
//! # What this recipe deliberately does not do
//!
//! **It offers no reload.** `CONFIG SET` changes a running server without touching the file, which
//! is the opposite of what a reload has to mean where the file is rendered from the database — the
//! row would be behind the server within one command. A changed configuration is a restart, which
//! is what [`ReloadBehaviour`](mixengine_proto::ReloadBehaviour)'s own documentation already says
//! about this service.
//!
//! **It listens on no Unix socket.** One address is one thing for T38 to diagnose and one thing for
//! a project's `.env` to name, and the socket would buy a local round trip that is loopback already.

use mixengine_proto::{
    HealthCheck, HealthProbe, Millis, ReadyCheck, ServiceSpec, ServiceSpecBuilder, StopBehaviour,
};

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::generate::document::Validator;
use crate::generate::recipe::{Context, Instancing, Recipe, TemplateFile, Upstream};
use crate::generate::settings::{Preset, Setting};
use crate::install::SmokeTest;
use crate::{Error, Result};

/// The `packages.name` this recipe is for.
const PACKAGE: &str = "redis";

/// The server, as the package publishes it — `bin/redis-server`, `bin/redis-server.exe`.
const SERVER: &str = "redis-server";

/// The client, which is this recipe's readiness check, health probe and shutdown all three.
const CLIENT: &str = "redis-cli";

/// The rendered configuration, under `etc/<service-id>/`.
///
/// **Named relatively wherever it is passed to the server** — see the module note.
const CONFIG_FILE: &str = "redis.conf";

/// How much this instance may hold before its policy starts evicting.
const MAXMEMORY: &str = "maxmemory";

/// What it evicts when it gets there, in Redis's own spelling.
const MAXMEMORY_POLICY: &str = "maxmemory_policy";

/// How many numbered databases the instance offers.
const DATABASES: &str = "databases";

/// What the server logs at, in Redis's own spelling: `debug`, `verbose`, `notice`, `warning`.
const LOGLEVEL: &str = "loglevel";

/// How long the server is given to answer a ping before the start is a failure, in milliseconds.
const READY_TIMEOUT: &str = "ready_timeout_ms";

/// How long the shutdown is given before the process group is killed, in milliseconds.
const STOP_GRACE: &str = "stop_grace_ms";

/// How often the running server is asked whether it is still there.
const HEALTH_INTERVAL: Millis = Millis(10_000);

/// How long one of those may take. Well inside the interval, which [`ServiceSpec::validate`]
/// insists on: two probes that could overlap are two probes that can queue.
const HEALTH_TIMEOUT: Millis = Millis(2_000);

/// Redis, as MixEngine runs it.
#[derive(Debug)]
pub struct Redis;

impl Recipe for Redis {
    fn package(&self) -> &'static str {
        PACKAGE
    }

    /// `redis@main` beside `redis@sessions`: two rows, two ports, two datasets.
    fn instancing(&self) -> Instancing {
        Instancing::Named
    }

    /// 6379, which a developer's own Redis routinely holds — and losing it is why the allocation
    /// says who took it rather than renumbering in silence.
    fn preferred_port(&self) -> Option<u16> {
        Some(6379)
    }

    fn smoke_test(&self) -> Option<SmokeTest> {
        Some(SmokeTest {
            executable: SERVER.to_owned(),
            args: vec!["--version".to_owned()],
        })
    }

    fn settings(&self) -> &'static [Setting] {
        &[
            Setting {
                key: DATABASES,
                default: Preset::Number(16),
            },
            Setting {
                key: LOGLEVEL,
                default: Preset::Text("notice"),
            },
            Setting {
                // Redis's own units, because this value is copied into its file verbatim: `256mb`,
                // `1gb`. A plain number of bytes works too, and is what Redis writes back.
                key: MAXMEMORY,
                default: Preset::Text("256mb"),
            },
            Setting {
                // Evict rather than refuse. `noeviction` is Redis's own default and is the right one
                // for a store; for a cache it turns a full instance into write errors at the site —
                // the one component whose failure should be a miss.
                key: MAXMEMORY_POLICY,
                default: Preset::Text("allkeys-lru"),
            },
            Setting {
                key: READY_TIMEOUT,
                default: Preset::Number(30_000),
            },
            Setting {
                key: STOP_GRACE,
                default: Preset::Number(10_000),
            },
        ]
    }

    fn files(&self) -> &'static [TemplateFile] {
        &[TemplateFile {
            path: CONFIG_FILE,
            source: include_str!("redis/redis.conf"),
        }]
    }

    /// There is none, and that is upstream's shape rather than a gap here.
    ///
    /// Redis has no `--test-config`, and no mode that reads a configuration without becoming a
    /// server: the only way to find out whether a `redis.conf` is acceptable is to start one. What
    /// T30's staging buys every other recipe, this one gets from the start itself — a configuration
    /// Redis refuses is a service that fails to start, with the server's own complaint on the stream
    /// `mix service logs` reads.
    fn validator(&self, _context: &Context) -> Option<Validator> {
        None
    }

    /// Connected clients, which for Redis is nearly the whole of what it is doing.
    ///
    /// `INFO` would give `total_commands_processed` and is RESP rather than HTTP, so it is not a
    /// probe this vocabulary has — and a Redis nobody is connected to is one nobody is about to
    /// command.
    fn idle_probe(&self, context: &Context) -> Option<mixengine_proto::IdleProbe> {
        context
            .port()
            .map(|port| mixengine_proto::IdleProbe::Connections { port })
    }

    fn spec(&self, context: &Context) -> Result<ServiceSpecBuilder> {
        let settings = context.settings();
        let server = context.provided(SERVER)?;
        let client = context.provided(CLIENT)?;
        let port = port(context)?;

        Ok(ServiceSpec::builder(context.service().clone(), server)
            // **Relative, and the working directory below is what makes it resolve.** See the module
            // note: any other spelling is joined to `getcwd()` by the server itself.
            .args([CONFIG_FILE.to_owned()])
            .cwd(context.etc())
            // What a failed start is diagnosed against (T38).
            .ports([port])
            .ready(ReadyCheck::Command {
                program: client.clone(),
                args: ping(context, port),
                timeout: millis(settings.number(READY_TIMEOUT)),
            })
            .health(HealthCheck {
                probe: HealthProbe::Command {
                    program: client.clone(),
                    args: ping(context, port),
                },
                interval: HEALTH_INTERVAL,
                timeout: HEALTH_TIMEOUT,
                // Three rather than one: a single-threaded server working through a large request
                // from a site is busy, not sick.
                failures_before_degraded: 3,
                successes_before_running: 1,
            })
            // **`NOSAVE`, and it is the same decision as `save ""` in the file.** A plain `SHUTDOWN`
            // writes a dump if any save point is configured, so the word is what keeps a later edit
            // to the template from quietly turning every stop into a write to disk.
            .stop(StopBehaviour::Command {
                program: client,
                args: {
                    let mut args = connection(context, port);
                    args.push("shutdown".to_owned());
                    args.push("nosave".to_owned());
                    args
                },
                grace: millis(settings.number(STOP_GRACE)),
            }))
    }

    /// Its port, and nothing else — T70a's D4. It listens on no Unix socket; the module doc above
    /// says why, and an address the server never binds is one the daemon must not bind either.
    fn held_while_stopped(&self, context: &Context) -> Result<Vec<Upstream>> {
        Ok(vec![Upstream::Tcp(address(context)?)])
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
}

/// Where this instance is, as the client's own arguments.
///
/// The address is written out rather than left to the client's defaults, because a developer's
/// machine routinely has a Redis of its own on 6379: a ping with no `-p` would report this service
/// ready against somebody else's server, and a shutdown with no `-p` would stop that one.
fn connection(context: &Context, port: u16) -> Vec<String> {
    vec![
        "-h".to_owned(),
        context.bind().to_owned(),
        "-p".to_owned(),
        port.to_string(),
    ]
}

/// The question both the readiness check and the health probe ask.
fn ping(context: &Context, port: u16) -> Vec<String> {
    let mut args = connection(context, port);
    args.push("ping".to_owned());
    args
}

/// Where this instance listens, as one value — T70a.
///
/// The client's arguments are built from [`connection`] rather than from this, because a client
/// takes a host and a port as two words; what needs them as one address is the daemon, which binds
/// this while nothing is serving it.
fn address(context: &Context) -> Result<SocketAddr> {
    Ok(SocketAddr::new(
        context
            .bind()
            .parse::<IpAddr>()
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        port(context)?,
    ))
}

/// The port this row was allocated, or the refusal that names the row.
fn port(context: &Context) -> Result<u16> {
    context.port().ok_or_else(|| Error::SettingValue {
        service: context.service().as_str().to_owned(),
        key: "port",
        value: "none".to_owned(),
        reason: "a cache listens on a TCP port and this service's row carries none; \
                 `service.create` allocates one",
    })
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
    use crate::generate::Upstream;
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

    /// What the package publishes, which is where both programs this recipe runs come from.
    fn provides() -> BTreeMap<String, String> {
        [
            (SERVER.to_owned(), format!("bin/{SERVER}")),
            (CLIENT.to_owned(), format!("bin/{CLIENT}")),
        ]
        .into_iter()
        .collect()
    }

    /// A `redis@main` on 6379 in a home at `root`, with `overrides` applied.
    fn context(overrides: &str) -> Context {
        with_provides(provides(), overrides)
    }

    /// The same, for an install that publishes something else — or nothing.
    fn with_provides(provides: BTreeMap<String, String>, overrides: &str) -> Context {
        let service = ServiceId::parse("redis@main").expect("an id");
        let settings =
            Settings::merge(Redis.settings(), overrides, &service).expect("usable overrides");

        Context::for_test(
            service,
            PACKAGE,
            Path::new(root()),
            provides,
            Some(6379),
            settings,
        )
    }

    /// The spec for `overrides`, built.
    fn spec(overrides: &str) -> ServiceSpec {
        Redis
            .spec(&context(overrides))
            .expect("a spec")
            .build()
            .expect("a valid spec")
    }

    /// The rendered `redis.conf` for `overrides`.
    fn rendered(overrides: &str) -> String {
        let documents = recipe::render(&Redis, &context(overrides)).expect("a rendering");

        assert_eq!(documents.len(), 1, "Redis renders one file");
        assert_eq!(documents[0].relative(), Path::new(CONFIG_FILE));

        documents[0].contents().to_owned()
    }

    /// Two instances of one cache are two rows, two ports and two datasets.
    /// **Redis is woken at its port and nowhere else**, which is not an omission: it listens on
    /// no Unix socket, and an address the server never binds is one the daemon must not bind
    /// either — a client reaching it would be answered by something that is not Redis.
    #[test]
    fn a_stopped_server_is_woken_at_its_port_alone() {
        assert_eq!(
            Redis
                .held_while_stopped(&context("{}"))
                .expect("the addresses it is woken at"),
            vec![Upstream::Tcp(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                6379
            ))]
        );
    }

    #[test]
    fn redis_exists_by_name() {
        assert_eq!(Redis.instancing(), Instancing::Named);
    }

    /// An artifact that unpacks and will not run is one the user meets against their own site,
    /// which is T20a's finding and the reason `Installer::install` takes a smoke test at all.
    #[test]
    fn redis_proves_itself_by_running() {
        let smoke = Redis.smoke_test().expect("a server proves that it runs");

        assert_eq!(smoke.executable, SERVER);
        assert_eq!(smoke.args, ["--version"]);
    }

    /// What a failed start is diagnosed against — roadmap task **T38**.
    #[test]
    fn the_spec_declares_the_port_it_will_bind() {
        assert_eq!(spec("{}").ports(), [6379]);
    }

    /// **The one constraint this recipe exists to obey, and it is upstream's.**
    ///
    /// `getAbsolutePath()` joins anything not starting with `/` to `getcwd()`, so no Windows
    /// spelling of an absolute path survives being passed as an argument. Naming the file relatively
    /// beside a working directory of its own is one rule that holds on all five published cells.
    #[test]
    fn the_configuration_is_named_relatively_beside_a_working_directory() {
        let spec = spec("{}");

        assert_eq!(spec.args(), [CONFIG_FILE]);
        assert_eq!(spec.cwd(), context("{}").etc());
    }

    /// **Every path this file writes has to survive being a Windows path**, and Redis decides that
    /// with its quoting rather than with its path handling.
    ///
    /// `sdssplitargs` processes escapes inside **double** quotes — `\n`, `\r`, `\xNN`, and any other
    /// `\<char>` as the character alone — so `dir "C:\Users\runner\…"` arrives with every backslash
    /// eaten and the server refuses the whole file: *FATAL CONFIG FILE ERROR … No such file or
    /// directory*, naming a directory that is right there. Inside **single** quotes only `\'` is an
    /// escape, so a Windows path arrives as it was written. This is Caddy's backtick finding in
    /// another program's spelling, and it cost a red CI leg to learn in this one too: Linux and
    /// macOS cannot see it, because their paths have no backslashes in them.
    #[test]
    fn the_data_directory_is_quoted_the_way_a_windows_path_survives() {
        let rendered = rendered("{}");
        let data = context("{}").data().display().to_string();

        assert!(rendered.contains(&format!("dir '{data}'")), "{rendered}");
        assert!(
            !rendered.contains("dir \""),
            "a double-quoted path is one Redis unescapes: {rendered}"
        );
    }

    /// Readiness is the client's own question, asked at the port *this* instance listens on.
    ///
    /// The port matters more than it looks: a developer's machine routinely has a Redis of its own
    /// on 6379, and a ping with no `-p` would report a second instance ready against somebody
    /// else's server.
    #[test]
    fn readiness_is_a_ping_at_this_instances_own_port() {
        let ready = spec("{}").ready().clone();

        let ReadyCheck::Command { program, args, .. } = ready else {
            panic!("a cache with a client of its own is asked rather than accepted: {ready:?}");
        };

        assert!(program.ends_with(CLIENT), "{program:?}");
        assert!(
            args.windows(2).any(|pair| pair == ["-p", "6379"]),
            "{args:?}"
        );
        assert!(args.contains(&"ping".to_owned()), "{args:?}");
    }

    /// **Nothing this cache holds is ever written to disk**, which is the whole of what "cache" is
    /// deciding here: no snapshot on a timer, no append-only file, and a stop that does not write
    /// one on the way out either.
    #[test]
    fn a_cache_keeps_nothing_across_a_restart() {
        let rendered = rendered("{}");

        assert!(rendered.contains("save \"\""), "{rendered}");
        assert!(rendered.contains("appendonly no"), "{rendered}");

        let stop = spec("{}").stop().clone();
        let StopBehaviour::Command { program, args, .. } = stop else {
            panic!("a cache is asked to shut down through its own client: {stop:?}");
        };

        assert!(program.ends_with(CLIENT), "{program:?}");
        assert_eq!(args.last().map(String::as_str), Some("nosave"), "{args:?}");
    }

    /// A server that forks is a server the supervisor loses, and one that writes its own log file is
    /// output nothing captures.
    #[test]
    fn the_rendering_keeps_the_server_in_the_foreground_and_its_log_on_the_stream() {
        let rendered = rendered("{}");

        assert!(rendered.contains("daemonize no"), "{rendered}");
        assert!(rendered.contains("logfile \"\""), "{rendered}");
    }

    /// The row decides where it listens; the defaults decide the rest.
    #[test]
    fn the_rendering_says_what_the_row_and_the_defaults_say() {
        let rendered = rendered("{}");

        assert!(rendered.contains("port 6379"), "{rendered}");
        assert!(rendered.contains("bind 127.0.0.1"), "{rendered}");
        assert!(rendered.contains("maxmemory 256mb"), "{rendered}");
        assert!(
            rendered.contains("maxmemory-policy allkeys-lru"),
            "{rendered}"
        );
        assert!(rendered.contains("databases 16"), "{rendered}");
    }

    /// An override reaches the file it belongs in.
    #[test]
    fn an_override_is_what_the_file_says() {
        let rendered = rendered(r#"{"maxmemory": "1gb", "databases": 4}"#);

        assert!(rendered.contains("maxmemory 1gb"), "{rendered}");
        assert!(rendered.contains("databases 4"), "{rendered}");
    }

    /// Redis re-reads its configuration only when it starts, so a changed rendering is a restart —
    /// and the spec says that by offering no reload rather than by offering one that lies.
    #[test]
    fn a_changed_configuration_is_a_restart_and_says_so_by_offering_no_reload() {
        assert!(spec("{}").reload().is_none());
    }

    /// A row with no port cannot become a server, and the refusal names the row rather than arriving
    /// later as a service that never came up.
    #[test]
    fn a_row_with_no_port_is_refused_by_name() {
        let service = ServiceId::parse("redis@main").expect("an id");
        let settings = Settings::merge(Redis.settings(), "{}", &service).expect("usable overrides");
        let context = Context::for_test(
            service,
            PACKAGE,
            Path::new(root()),
            provides(),
            None,
            settings,
        );

        let refused = Redis.spec(&context).expect_err("a port is not optional");

        assert!(format!("{refused}").contains("port"), "{refused}");
    }

    /// An install that publishes no client is one this recipe cannot check, probe or stop, and it
    /// says which program it wanted rather than failing at the first ping.
    #[test]
    fn an_install_without_the_client_names_the_program_it_wanted() {
        let context = with_provides(
            [(SERVER.to_owned(), format!("bin/{SERVER}"))]
                .into_iter()
                .collect(),
            "{}",
        );

        let refused = Redis.spec(&context).expect_err("no client, no readiness");

        assert!(format!("{refused}").contains(CLIENT), "{refused}");
    }
}
