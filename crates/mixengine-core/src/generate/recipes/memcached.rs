//! Memcached — roadmap task **T35**, with [`redis`](super::redis) the other half of it.
//!
//! # The first recipe that renders nothing
//!
//! `.claude/features/services.md` opens with "no hand-written config files", and every recipe before
//! this one answered that by rendering one. **Memcached has no configuration file format at all** —
//! not one it declines to use, one that does not exist: every setting is a command-line flag, and
//! what distributions call `/etc/memcached.conf` is a list of flags their init script pastes onto
//! the command line. So this recipe's [`files`](Recipe::files) is empty, `etc/memcached@main/` is
//! never created, and the typed overrides land in [`ServiceSpec::args`] instead of in a template.
//!
//! That is the honest shape rather than a shortcut. Rendering a file nothing reads, so that the
//! catalogue looks uniform, would put a document in front of the user that changes nothing when they
//! edit it — which is the exact failure the "users edit overrides, never the generated file" rule
//! exists to prevent.
//!
//! # An accept really is readiness here
//!
//! T33 argued at length that a TCP accept is a dishonest readiness check for a database, because it
//! stays true for the whole of InnoDB's crash recovery while the server refuses every query. None of
//! that applies to this program: memcached allocates its slabs and enters its event loop, and there
//! is no recovery phase in which it is listening and unable to answer. The accept is the readiness.
//!
//! It is also all there is. **The package is one file** — `bin/memcached`, and nothing else — so
//! there is no client to ask a question with, the way `redis-cli` and `mariadb-admin` are asked. The
//! end-to-end suite speaks the text protocol over a socket itself, which is the same thing
//! `mixengine-packages` does to smoke-test the artifact.
//!
//! # Stopped by being killed, on purpose
//!
//! [`ADR 0008`] names Memcached as the service where stopping without a signal costs nothing, and
//! the artifact agrees from the other end: it is built **without** `--enable-shutdown`, so the
//! `shutdown` verb does not exist on the wire. That was a packaging decision worth keeping here in
//! words, because the alternative reads like an omission — enabling it would put an unauthenticated
//! `shutdown` on a loopback port that anything served by this machine can reach, to save flushing a
//! cache that has nothing to flush.
//!
//! [`ADR 0008`]: https://github.com/haiquang9994/MixEngine/blob/master/.claude/decisions/0008-no-signal-stop-on-windows.md
//! [`ServiceSpec::args`]: mixengine_proto::ServiceSpec::args

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use mixengine_proto::{
    HealthCheck, HealthProbe, Millis, ReadyCheck, ServiceSpec, ServiceSpecBuilder, StopBehaviour,
};

use crate::generate::recipe::{Context, Instancing, Recipe};
use crate::generate::settings::{Preset, Setting};
use crate::install::SmokeTest;
use crate::{Error, Result};

/// The `packages.name` this recipe is for, and the one program the package publishes.
const PACKAGE: &str = "memcached";

/// How many megabytes of items this instance may hold.
const MEMORY_MB: &str = "memory_mb";

/// How many clients may be connected at once.
const MAX_CONNECTIONS: &str = "max_connections";

/// How many worker threads it runs.
const THREADS: &str = "threads";

/// How long the server is given to accept a connection before the start is a failure.
const READY_TIMEOUT: &str = "ready_timeout_ms";

/// How often the running server is asked whether it is still there.
const HEALTH_INTERVAL: Millis = Millis(10_000);

/// How long one of those may take. Well inside the interval, which [`ServiceSpec::validate`]
/// insists on.
const HEALTH_TIMEOUT: Millis = Millis(2_000);

/// Memcached, as MixEngine runs it.
#[derive(Debug)]
pub struct Memcached;

impl Recipe for Memcached {
    fn package(&self) -> &'static str {
        PACKAGE
    }

    /// `memcached@main` beside `memcached@sessions`: two rows, two ports.
    fn instancing(&self) -> Instancing {
        Instancing::Named
    }

    /// 11211.
    fn preferred_port(&self) -> Option<u16> {
        Some(11211)
    }

    fn smoke_test(&self) -> Option<SmokeTest> {
        Some(SmokeTest {
            executable: PACKAGE.to_owned(),
            // `-V` rather than `--version`: the short flag is in every line this index publishes,
            // and an install is not the place to find out which release grew the long one.
            args: vec!["-V".to_owned()],
        })
    }

    fn settings(&self) -> &'static [Setting] {
        &[
            Setting {
                key: MAX_CONNECTIONS,
                default: Preset::Number(1024),
            },
            Setting {
                // Memcached's own default, and the number `.claude/features/services.md` names.
                key: MEMORY_MB,
                default: Preset::Number(64),
            },
            Setting {
                key: READY_TIMEOUT,
                default: Preset::Number(15_000),
            },
            Setting {
                key: THREADS,
                default: Preset::Number(4),
            },
        ]
    }

    /// **None, and see the module note**: memcached has no configuration file format to render into.
    fn files(&self) -> &'static [crate::generate::recipe::TemplateFile] {
        &[]
    }

    fn spec(&self, context: &Context) -> Result<ServiceSpecBuilder> {
        let settings = context.settings();
        let program = context.provided(PACKAGE)?;
        let addr = address(context)?;

        Ok(ServiceSpec::builder(context.service().clone(), program)
            .args([
                // Loopback, from the row's own bind. Memcached has no authentication in its default
                // build, so what stops the machine's cache being a stranger's cache is this flag.
                "-l".to_owned(),
                addr.ip().to_string(),
                "-p".to_owned(),
                addr.port().to_string(),
                // **UDP off, written out rather than left to the default.** It has been off by
                // default since 1.5.6, and it is off because a UDP memcached is the reflector in one
                // of the largest amplification attacks ever recorded. A default that had to change
                // once is a default worth spelling.
                "-U".to_owned(),
                "0".to_owned(),
                "-m".to_owned(),
                settings.number(MEMORY_MB).to_string(),
                "-c".to_owned(),
                settings.number(MAX_CONNECTIONS).to_string(),
                "-t".to_owned(),
                settings.number(THREADS).to_string(),
            ])
            // Nothing is read or written relative to anywhere, so this is only where a crash dump
            // would land. The data directory rather than `etc/`, which this service never has.
            .cwd(context.data())
            // What a failed start is diagnosed against (T38).
            .ports([addr.port()])
            // **An accept, and here that is the honest answer** — see the module note. It is also
            // the only one available: the package is one binary, with no client to ask.
            .ready(ReadyCheck::Tcp {
                addr,
                timeout: millis(settings.number(READY_TIMEOUT)),
            })
            .health(HealthCheck {
                probe: HealthProbe::Tcp { addr },
                interval: HEALTH_INTERVAL,
                timeout: HEALTH_TIMEOUT,
                failures_before_degraded: 3,
                successes_before_running: 1,
            })
            // **Killed, and it costs nothing** — see the module note. A grace period here would be
            // time paid on every stop for a flush that cannot happen: there is nothing on disk to
            // write to, and the artifact ships without the `shutdown` verb on purpose.
            .stop(StopBehaviour::Kill))
    }
}

/// Where this instance listens, or the refusal that names the row.
fn address(context: &Context) -> Result<SocketAddr> {
    let port = context.port().ok_or_else(|| Error::SettingValue {
        service: context.service().as_str().to_owned(),
        key: "port",
        value: "none".to_owned(),
        reason: "a cache listens on a TCP port and this service's row carries none; \
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

    /// A `memcached@main` on 11211 in a home at `root`, with `overrides` applied.
    fn context(overrides: &str) -> Context {
        let service = ServiceId::parse("memcached@main").expect("an id");
        let settings =
            Settings::merge(Memcached.settings(), overrides, &service).expect("usable overrides");

        Context::for_test(
            service,
            PACKAGE,
            Path::new(root()),
            [(PACKAGE.to_owned(), format!("bin/{PACKAGE}"))]
                .into_iter()
                .collect(),
            Some(11211),
            settings,
        )
    }

    /// The spec for `overrides`, built.
    fn spec(overrides: &str) -> ServiceSpec {
        Memcached
            .spec(&context(overrides))
            .expect("a spec")
            .build()
            .expect("a valid spec")
    }

    /// `args` as one string, for assertions that are about a pair rather than a position.
    fn flag(spec: &ServiceSpec, flag: &str) -> String {
        let args = spec.args();
        let at = args
            .iter()
            .position(|arg| arg == flag)
            .unwrap_or_else(|| panic!("`{flag}` is on the command line: {args:?}"));

        args.get(at + 1)
            .unwrap_or_else(|| panic!("`{flag}` has a value: {args:?}"))
            .clone()
    }

    /// Two instances of one cache are two rows and two ports.
    #[test]
    fn memcached_exists_by_name() {
        assert_eq!(Memcached.instancing(), Instancing::Named);
    }

    /// **The first recipe in the catalogue that renders nothing**, because memcached has no
    /// configuration file format at all — see the module note. `etc/memcached@main/` is never made.
    #[test]
    fn memcached_renders_no_file_because_it_reads_none() {
        assert!(Memcached.files().is_empty());

        let documents = recipe::render(&Memcached, &context("{}")).expect("a rendering");

        assert!(documents.is_empty(), "{documents:?}");
    }

    /// So the configuration is the command line, and the overrides are what is on it.
    #[test]
    fn the_configuration_this_service_has_is_its_command_line() {
        let spec = spec("{}");

        assert_eq!(flag(&spec, "-l"), "127.0.0.1");
        assert_eq!(flag(&spec, "-p"), "11211");
        assert_eq!(flag(&spec, "-m"), "64");
        assert_eq!(flag(&spec, "-c"), "1024");
        assert_eq!(flag(&spec, "-t"), "4");
    }

    /// An override reaches the flag it belongs to.
    #[test]
    fn an_override_is_what_the_command_line_says() {
        let spec = spec(r#"{"memory_mb": 512, "max_connections": 64}"#);

        assert_eq!(flag(&spec, "-m"), "512");
        assert_eq!(flag(&spec, "-c"), "64");
    }

    /// **UDP off, and written out rather than inherited.**
    ///
    /// It has been off by default since 1.5.6 — a default that changed once, after a UDP memcached
    /// turned out to be the reflector in some of the largest amplification attacks recorded. The
    /// flag is what keeps this instance's answer from depending on which line a user installed.
    #[test]
    fn the_command_line_turns_udp_off_itself() {
        assert_eq!(flag(&spec("{}"), "-U"), "0");
    }

    /// What a failed start is diagnosed against — roadmap task **T38**.
    #[test]
    fn the_spec_declares_the_port_it_will_bind() {
        assert_eq!(spec("{}").ports(), [11211]);
    }

    /// **An accept is the readiness, and here that is honest** — there is no recovery phase in which
    /// this program is listening and unable to answer, and the package ships no client to ask with.
    #[test]
    fn readiness_is_an_accept_because_the_package_ships_no_client() {
        let ready = spec("{}").ready().clone();

        let ReadyCheck::Tcp { addr, .. } = ready else {
            panic!("one binary is the whole package, so there is nothing to ask: {ready:?}");
        };

        assert_eq!(addr.port(), 11211);
        assert!(addr.ip().is_loopback(), "{addr}");
    }

    /// A cache with nothing on disk is stopped by being killed, which is what ADR 0008 says about
    /// this service and what the artifact was built to expect.
    #[test]
    fn a_cache_with_nothing_to_flush_is_stopped_by_being_killed() {
        assert_eq!(spec("{}").stop(), &StopBehaviour::Kill);
    }

    /// An artifact that unpacks and will not run is one the user meets against their own site.
    #[test]
    fn memcached_proves_itself_by_running() {
        let smoke = Memcached
            .smoke_test()
            .expect("a server proves that it runs");

        assert_eq!(smoke.executable, PACKAGE);
        assert_eq!(smoke.args, ["-V"]);
    }

    /// A row with no port cannot become a server, and the refusal names the row.
    #[test]
    fn a_row_with_no_port_is_refused_by_name() {
        let service = ServiceId::parse("memcached@main").expect("an id");
        let settings =
            Settings::merge(Memcached.settings(), "{}", &service).expect("usable overrides");
        let context = Context::for_test(
            service,
            PACKAGE,
            Path::new(root()),
            [(PACKAGE.to_owned(), format!("bin/{PACKAGE}"))]
                .into_iter()
                .collect(),
            None,
            settings,
        );

        let refused = Memcached
            .spec(&context)
            .expect_err("a port is not optional");

        assert!(format!("{refused}").contains("port"), "{refused}");
    }
}
