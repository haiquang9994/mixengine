//! php-fpm: the FastCGI pool behind every PHP site — roadmap task **T32**.
//!
//! **The first recipe whose binary does not come from a package.** A PHP is installed with
//! `runtime.install` into `runtime_installs`, and the process that serves its sites lives inside
//! that directory — so this recipe's service row points there, `service.create` refuses to write one
//! by hand, and `runtime.uninstall` is the thing that takes it away.
//!
//! # Two mechanisms, one vocabulary
//!
//! There is **no php-fpm on Windows** and this is upstream's shape rather than an omission of ours:
//! every PHP in `mixengine-packages`' index from 7.0 to 8.5 publishes `php` and `php-fpm` on Linux
//! and macOS, and `php` and `php-cgi` on Windows. What was not obvious, and was measured against the
//! artifact this project publishes rather than read about, is that this costs almost nothing:
//! `php-cgi.exe` given `PHP_FCGI_CHILDREN` **is** a process manager — a master, N children, a child
//! respawned within a second of being killed, recycling at `PHP_FCGI_MAX_REQUESTS`, and every child
//! going with the master when it is terminated. That is php-fpm with `pm = static`, configured
//! through the environment instead of through a file.
//!
//! So the two systems differ only in the mechanism, and a user meets one vocabulary:
//!
//! | | Unix | Windows |
//! | --- | --- | --- |
//! | program | `provides["php-fpm"]` | `provides["php-cgi"]` |
//! | workers | `pm.max_children` in the pool file | `PHP_FCGI_CHILDREN` |
//! | recycling | `pm.max_requests` | `PHP_FCGI_MAX_REQUESTS` |
//! | listen | `run/php-fpm-<version>.sock` | `127.0.0.1:<services.port>` |
//! | reload | `SIGUSR2` | none |
//!
//! Which binary it is comes out of the artifact's own `provides` map rather than being written down
//! here, which is what keeps a `#[cfg]` out of this file: the index says where the executable is,
//! and the recipe asks for it by the name we gave it.
//!
//! # What this recipe deliberately does not do
//!
//! **No `pm = dynamic` and no `pm = ondemand`.** Windows can express neither, and an override that
//! works on two systems out of three is exactly the divide this task exists to avoid.
//!
//! **No `request_terminate_timeout` on Windows.** A hung script holds a worker there for as long as
//! it hangs, and with five of them that is a dead PHP. The fix needs no process manager — the master
//! respawns a killed child, so the daemon would only have to kill a worker that has run too long —
//! but doing it right needs its own measurement of how a hung script behaves on that system, and
//! that is a task of its own.
//!
//! **No `php.ini` and no `conf.d` of its own.** What a *pool* renders and what a *runtime's* ini set
//! contains are different files with different owners, and this recipe owns the first. What it does
//! do is name the second: `PHP_INI_SCAN_DIR` is set on both arms, so the pool and the `php` on
//! somebody's terminal load one set — see [`crate::runtimes::extensions`], roadmap task T28.
//!
//! **No site, and no `pool.d/` either.** Phase 4 renders the first per-site file and brings both the
//! directory and the `include` that finds it. Naming them here ahead of time was tried and reverted:
//! php-fpm treats a glob whose directory is missing as a hard error rather than as a pattern that
//! matched nothing, and the directory cannot be there for the first `--test` — `include` names the
//! *installed* path while validation runs over the *staged* one, before anything is installed. The
//! file says so where the line used to be.
//!
//! **No `pm.status_path` and no slowlog.** Neither exists on Windows, and nothing reads them yet.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use mixengine_proto::{
    HealthCheck, HealthProbe, Millis, ReadyCheck, ReloadBehaviour, ReloadSignal, RuntimeKind,
    ServiceSpec, ServiceSpecBuilder, StopBehaviour,
};

use crate::generate::document::{CONFIG, Validator};
use crate::generate::recipe::{Context, Instancing, Recipe, Source, TemplateFile};
use crate::generate::settings::{Preset, Setting};
use crate::{Error, Result};

/// The `packages.name` this recipe is found under, which for a pool is the id's own half: a service
/// is `php-fpm@8.3.33` and the row beneath it names a `php`.
pub const PACKAGE: &str = "php-fpm";

/// The executable that serves a pool on a system that has php-fpm, as the index names it.
const FPM: &str = "php-fpm";

/// And on Windows, where it does not. See the module note.
const CGI: &str = "php-cgi";

/// The rendered pool configuration, under `etc/<service-id>/`.
const POOL_FILE: &str = "php-fpm.conf";

/// How many workers the pool holds. Five is php-fpm's own `pm.max_children` for a `www` pool, and
/// is a number a laptop can serve a development site with while running everything else.
const MAX_CHILDREN: &str = "max_children";

/// How many requests a worker serves before it is retired and replaced. Bounds what a leaking
/// extension costs; zero turns it off.
const MAX_REQUESTS: &str = "max_requests";

/// How long one request may run before its worker is killed, in seconds. **Unix only** — see the
/// module note. `0` is php-fpm's own "no limit".
const REQUEST_TIMEOUT: &str = "request_timeout";

/// How long the pool is given to be listening before the start is a failure, in milliseconds.
const READY_TIMEOUT: &str = "ready_timeout_ms";

/// How long a stop is given before the process group is killed, in milliseconds.
const STOP_GRACE: &str = "stop_grace_ms";

/// How often the socket is asked whether the master is still accepting.
const HEALTH_INTERVAL: Millis = Millis(10_000);

/// How long one of those may take. Well inside the interval, which [`ServiceSpec::validate`]
/// insists on: two probes that could overlap are two probes that can queue.
const HEALTH_TIMEOUT: Millis = Millis(2_000);

/// How long a `SIGUSR2` is treated as in progress.
///
/// What it covers is a graceful pool restart: every worker finishes the request it is serving before
/// its replacement takes over, so the wait is really the longest request a site has in flight.
/// Nothing is killed when it expires.
const RELOAD_PATIENCE: Millis = Millis(10_000);

/// php-fpm, as MixEngine runs it.
#[derive(Debug)]
pub struct PhpFpm;

impl Recipe for PhpFpm {
    fn package(&self) -> &'static str {
        PACKAGE
    }

    /// One pool per installed PHP, named by the version it runs.
    ///
    /// The **full** version — `php-fpm@8.3.33` — because `runtime_installs` is
    /// `UNIQUE (kind, version)` over the full version, so 8.3.33 and 8.3.34 can both be installed
    /// and `php-fpm@8.3` would then name neither.
    fn instancing(&self) -> Instancing {
        Instancing::Named
    }

    fn source(&self) -> Source {
        Source::Runtime(RuntimeKind::Php)
    }

    /// One set of overrides on every system, rendered into a file or an environment as the platform
    /// requires. See the module note for what is deliberately absent from it.
    fn settings(&self) -> &'static [Setting] {
        &[
            Setting {
                key: MAX_CHILDREN,
                default: Preset::Number(5),
            },
            Setting {
                key: MAX_REQUESTS,
                default: Preset::Number(500),
            },
            Setting {
                key: REQUEST_TIMEOUT,
                default: Preset::Number(120),
            },
            Setting {
                // Fifteen seconds. A pool is up in tens of milliseconds; what this is really waiting
                // for is a first run on Windows, where Defender reads the whole of a PHP before the
                // process starts.
                key: READY_TIMEOUT,
                default: Preset::Number(15_000),
            },
            Setting {
                key: STOP_GRACE,
                default: Preset::Number(10_000),
            },
        ]
    }

    /// One file, rendered on every system — and read by php-fpm on the two that have one.
    ///
    /// **Windows renders it and runs none of it**, which is deliberate and is the cheaper of the two
    /// mistakes available. A `#[cfg]` here would break this crate's rule about platform conditionals
    /// for a file that costs a few hundred bytes; it would also make a home on one system
    /// structurally different from a home on another, so that a user comparing theirs with a
    /// colleague's finds a directory missing rather than a value differing.
    fn files(&self) -> &'static [TemplateFile] {
        &[TemplateFile {
            path: POOL_FILE,
            source: include_str!("php_fpm/php-fpm.conf"),
        }]
    }

    /// `php-fpm --test`, pointed at the staged file — and nothing on Windows, where there is no file
    /// to test and the SAPI has no such flag.
    ///
    /// [`None`] falls out of the lookup rather than being decided: a Windows PHP publishes no
    /// `php-fpm`, so [`Context::provided`] fails and there is nothing to run. That is the same
    /// answer a `#[cfg]` would give, arrived at from the index instead of from this file.
    fn validator(&self, context: &Context) -> Option<Validator> {
        let program = context.provided(FPM).ok()?;

        Some(Validator::new(program, POOL_FILE).args(["--test", "--fpm-config", CONFIG]))
    }

    /// The pool, in whichever of the two shapes this system runs it.
    ///
    /// [`cfg!`] is a *value* and not an attribute, so both arms compile everywhere — which is what
    /// keeps this file cross-platform and lets a test exercise the branch the machine it runs on is
    /// not.
    fn spec(&self, context: &Context) -> Result<ServiceSpecBuilder> {
        if cfg!(windows) {
            Self::windows(context)
        } else {
            Self::unix(context)
        }
    }
}

impl PhpFpm {
    /// The pool as a system with php-fpm runs it.
    fn unix(context: &Context) -> Result<ServiceSpecBuilder> {
        let settings = context.settings();
        let program = context.provided(FPM)?;
        let socket = socket_path(context)?;

        Ok(ServiceSpec::builder(context.service().clone(), &program)
            // `--nodaemonize`, so the process the supervisor holds is the master itself. Without it
            // php-fpm forks and the parent exits successfully, which looks from out here exactly
            // like a service that started and immediately stopped.
            .args([
                "--nodaemonize".to_owned(),
                "--fpm-config".to_owned(),
                context.config(POOL_FILE).to_string_lossy().into_owned(),
            ])
            .cwd(context.etc())
            // The runtime's own ini set, which is T28's and not this recipe's. Set identically on
            // both systems, which is why it is written twice rather than in one arm: `php-cgi.exe`
            // reads it exactly as php-fpm does, and a pool that did not would disagree with `php -m`.
            .env(
                crate::runtimes::extensions::SCAN_DIR_ENV,
                crate::runtimes::extensions::conf_d(
                    context.etc_root(),
                    RuntimeKind::Php,
                    context.version(),
                )
                .to_string_lossy()
                .into_owned(),
            )
            .ready(ReadyCheck::UnixSocket {
                path: socket.clone(),
                timeout: millis(settings.number(READY_TIMEOUT)),
            })
            .health(HealthCheck {
                probe: HealthProbe::UnixSocket { path: socket },
                interval: HEALTH_INTERVAL,
                timeout: HEALTH_TIMEOUT,
                // Three intervals rather than one: a reload cycles every worker, and a pool serving
                // a slow request can miss a probe doing it. That is a busy PHP, not a sick one.
                failures_before_degraded: 3,
                successes_before_running: 1,
            })
            // The master finishes what its workers are serving and replaces them with workers that
            // read the new file. This is the service the whole idea is for after Caddy: restarting
            // would drop every request in flight for a change to one site's settings.
            .reload(ReloadBehaviour::Signal {
                signal: ReloadSignal::Usr2,
                patience: RELOAD_PATIENCE,
            })
            // `SIGTERM` to the group, which php-fpm reads as an immediate shutdown; the workers are
            // in that group and go with it.
            .stop(StopBehaviour::Signal {
                grace: millis(settings.number(STOP_GRACE)),
            }))
    }

    /// The pool as Windows runs it: `php-cgi.exe` on a port, with the pool in the environment.
    fn windows(context: &Context) -> Result<ServiceSpecBuilder> {
        let settings = context.settings();
        let program = context.provided(CGI)?;
        let addr = address(context)?;

        Ok(ServiceSpec::builder(context.service().clone(), &program)
            .args(["-b".to_owned(), addr.to_string()])
            .cwd(context.etc())
            // The runtime's own ini set, which is T28's and not this recipe's. Set identically on
            // both systems, which is why it is written twice rather than in one arm: `php-cgi.exe`
            // reads it exactly as php-fpm does, and a pool that did not would disagree with `php -m`.
            .env(
                crate::runtimes::extensions::SCAN_DIR_ENV,
                crate::runtimes::extensions::conf_d(
                    context.etc_root(),
                    RuntimeKind::Php,
                    context.version(),
                )
                .to_string_lossy()
                .into_owned(),
            )
            // The two variables that make `php-cgi.exe` a process manager rather than a queue of
            // one. Measured, not assumed — see the module note. They are the same two numbers the
            // pool file carries on Unix, which is what makes the override set one set.
            .env(
                "PHP_FCGI_CHILDREN",
                settings.number(MAX_CHILDREN).to_string(),
            )
            .env(
                "PHP_FCGI_MAX_REQUESTS",
                settings.number(MAX_REQUESTS).to_string(),
            )
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
            // **No reload.** There is no signal to send here, so a changed override leaves the
            // running pool on its old configuration until somebody restarts it — and the daemon does
            // not restart a thing nobody asked it to restart. The supervisor says so once, in
            // `daemon.log`, and `mix doctor` (T47) owes the sentence.
            //
            // `StopBehaviour::Signal` degrades to a kill here (ADR 0008), which is safe for this
            // service and for a measured reason: terminating the master was observed to take every
            // child with it, so nothing is left holding the port.
            .stop(StopBehaviour::Signal {
                grace: millis(settings.number(STOP_GRACE)),
            }))
    }
}

/// Where this pool listens on a system with Unix sockets.
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
        .join(format!("php-fpm-{}.sock", context.version()));

    super::within_socket_limit(context.service().as_str(), "listen", &socket)?;

    Ok(socket)
}

/// Where this pool listens on Windows: the port its row was given, on loopback.
///
/// The port is the row's rather than a number derived here, because it is allocated once when the
/// pool is created and has to be the same on every start — see [`crate::services::pools`].
///
/// # Errors
///
/// [`Error::SettingValue`] when the row carries no port, which is a pool created on a system that
/// does not need one and then run on a system that does.
fn address(context: &Context) -> Result<SocketAddr> {
    let port = context.port().ok_or_else(|| Error::SettingValue {
        service: context.service().as_str().to_owned(),
        key: "port",
        value: "none".to_owned(),
        reason: "a pool on this system listens on a TCP port and its row carries none; \
                 `runtime.install` allocates one when it creates the pool",
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

    /// A pool for PHP 8.3.33 in a home at [`root`], with `overrides` applied.
    fn context(overrides: &str) -> Context {
        let service = ServiceId::parse("php-fpm@8.3.33").expect("an id");
        let settings =
            Settings::merge(PhpFpm.settings(), overrides, &service).expect("usable overrides");

        Context::for_test(
            service,
            PACKAGE,
            Path::new(root()),
            provides(),
            Some(9000),
            settings,
        )
    }

    /// **Both SAPIs are told the same thing.** A pool that reads the generated set while `php -m`
    /// does not is two answers to one question — and on Windows the terminal's answer is a PHP with
    /// no `curl`, no `mbstring` and no `intl`, because there those are shared modules that only an
    /// ini switches on.
    ///
    /// Both arms directly, for the reason the socket test gives: the claim is worth checking on the
    /// machine that does not take that branch.
    #[test]
    fn a_pool_reads_the_ini_set_its_runtime_carries() {
        let context = context("{}");

        for builder in [
            PhpFpm::unix(&context).expect("a spec"),
            PhpFpm::windows(&context).expect("a spec"),
        ] {
            let spec = builder.build().expect("a valid spec");
            let scan = match spec
                .env()
                .get(crate::runtimes::extensions::SCAN_DIR_ENV)
                .expect("a pool that is told where its ini set is")
            {
                mixengine_proto::EnvValue::Literal { value } => value.clone(),
                other => panic!("the ini set is not a secret: {other:?}"),
            };

            assert!(
                scan.contains("conf.d"),
                "the pool is pointed somewhere that is not a conf.d: {scan}"
            );
            assert!(
                scan.contains(context.version()),
                "the pool is reading another version's extensions: {scan}"
            );
        }
    }

    /// A PHP that publishes every executable this recipe might ask for, on either system.
    ///
    /// Both SAPIs at once, which no real artifact has: what these tests exercise is the branch this
    /// machine is *not*, so the map has to answer for both of them.
    fn provides() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("php".to_owned(), "bin/php".to_owned()),
            ("php-fpm".to_owned(), "sbin/php-fpm".to_owned()),
            ("php-cgi".to_owned(), "php-cgi.exe".to_owned()),
        ])
    }

    /// An absolute path on whichever system this is compiled for.
    const fn root() -> &'static str {
        if cfg!(windows) {
            r"C:\MixEngine"
        } else {
            "/opt/mixengine"
        }
    }

    /// One pool per installed PHP, named by the version it runs — so its id carries an `@`.
    #[test]
    fn a_pool_is_named_after_the_php_it_runs() {
        assert_eq!(PhpFpm.instancing(), Instancing::Named);
    }

    /// The recipe says where its binary comes from, and it is not the package table.
    ///
    /// This is what `service.create` refuses on and what the install hook keys off, so it is
    /// asserted rather than assumed: a recipe that answered `Package` here would be one a user could
    /// declare against a `packages` row that does not exist.
    #[test]
    fn a_pool_comes_out_of_an_installed_php() {
        assert_eq!(PhpFpm.source(), Source::Runtime(RuntimeKind::Php));
    }

    /// The rendered file carries the values the row and the overrides gave it.
    ///
    /// Rendered through [`recipe::render`] rather than through a generator, for `caddy.rs`' reason:
    /// what is being checked is the *template*, and running the real validator would need fifty
    /// megabytes of PHP to find out whether a variable name is misspelled.
    #[test]
    fn the_pool_file_says_what_the_overrides_said() {
        let context = context(r#"{"max_children": 12}"#);
        let documents = recipe::render(&PhpFpm, &context).expect("a rendering");

        assert_eq!(documents.len(), 1, "php-fpm renders one file");
        assert_eq!(documents[0].relative(), Path::new(POOL_FILE));

        let rendered = documents[0].contents();
        assert!(rendered.contains("pm.max_children = 12"), "{rendered}");
        assert!(rendered.contains("pm = static"), "{rendered}");
        assert!(
            rendered.contains(&format!("php-fpm-{}.sock", context.version())),
            "the socket is named after the PHP the pool runs\n{rendered}"
        );
    }

    /// **The template and the spec must name the same socket.**
    ///
    /// They are computed twice — once in Jinja for the file php-fpm reads, once in Rust for the
    /// readiness check the daemon makes — and the failure when they disagree is a service that
    /// starts perfectly and is reported as never having come up. Nothing else in this recipe is
    /// worth a test as much as this.
    ///
    /// [`PhpFpm::unix`] directly rather than through [`Recipe::spec`], which is the point of that
    /// branch being chosen by a `cfg!` value: the claim is about the Unix shape and is worth
    /// checking on the machine that does not run it.
    #[test]
    fn the_file_and_the_readiness_check_name_one_socket() {
        let context = context("{}");
        let rendered = recipe::render(&PhpFpm, &context).expect("a rendering")[0]
            .contents()
            .to_owned();

        let spec = PhpFpm::unix(&context)
            .expect("a spec")
            .build()
            .expect("a valid spec");

        let ReadyCheck::UnixSocket { path, .. } = spec.ready() else {
            panic!("a pool on this system is proved up by its socket");
        };

        // Both sides normalised to forward slashes, which is a no-op on the systems this branch
        // actually runs on. It matters only here, on the machine that takes the other branch: Jinja
        // joins `{{ paths.run }}/…` with a literal slash while `Path::join` uses the host's
        // separator, so a Windows run of this test would fail on a difference that cannot exist
        // where the code is used.
        let slashes = |text: &str| text.replace('\\', "/");

        assert!(
            slashes(&rendered).contains(&slashes(&path.display().to_string())),
            "the file says one socket and the readiness check waits on another\n{rendered}"
        );
    }

    /// A socket path `sockaddr_un` cannot hold is refused here, by name.
    ///
    /// T33a measured the cap at 103 characters against a real server, and what it costs to find out
    /// the hard way is the reason this is a check: php-fpm aborts *after* it has started, in a way
    /// that reads like a different failure entirely.
    ///
    /// Asked of [`PhpFpm::unix`] rather than of [`Recipe::spec`], for the reason the socket
    /// agreement above is: [`PhpFpm::windows`] computes no socket path at all, and the check is
    /// still worth running on a machine that would take that branch.
    #[test]
    fn a_socket_path_too_long_for_the_kernel_is_refused_by_name() {
        let deep = format!("/{}", "nested/".repeat(20));
        let service = ServiceId::parse("php-fpm@8.3.33").expect("an id");
        let settings = Settings::merge(PhpFpm.settings(), "{}", &service).expect("defaults");
        let context = Context::for_test(
            service,
            PACKAGE,
            Path::new(&deep),
            provides(),
            None,
            settings,
        );

        let error = PhpFpm::unix(&context).expect_err("a path no kernel accepts");

        assert!(
            error.to_string().contains("103"),
            "the measurement is what makes this message useful: {error}"
        );
    }

    /// A PHP packed without the SAPI this recipe needs is named as such.
    #[test]
    fn a_php_without_the_right_sapi_is_named() {
        let service = ServiceId::parse("php-fpm@8.3.33").expect("an id");
        let settings = Settings::merge(PhpFpm.settings(), "{}", &service).expect("defaults");
        let context = Context::for_test(
            service,
            PACKAGE,
            Path::new(root()),
            BTreeMap::from([("php".to_owned(), "bin/php".to_owned())]),
            Some(9000),
            settings,
        );

        let error = PhpFpm.spec(&context).expect_err("no SAPI to run");

        assert!(
            matches!(error, Error::ServiceProvidesNothing { .. }),
            "{error:?}"
        );
    }
}
