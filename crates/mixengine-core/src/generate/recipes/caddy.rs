//! Caddy: the default front end — roadmap task **T31**.
//!
//! The first real [`Recipe`], and the shape the four after it are meant to follow. What it renders
//! is one file, and what makes that file interesting is the three mechanisms hanging off it:
//!
//! - **`caddy validate` judges it before it is installed.** A [`Validator`] is all that takes, and
//!   the staging directory T30 built is what makes it honest — the whole rendering is checked where
//!   it is staged, `import sites/*.caddy` and all, so a configuration that is refused is one nothing
//!   was installed from and the server goes on reading the last one that worked.
//! - **The admin endpoint is both the readiness check and the health check.** `GET /config/` on it
//!   answers `200` with the running configuration, which is a stronger statement than a TCP accept:
//!   a Caddy whose listener is up and whose config failed to apply is not a Caddy anything should be
//!   routed to.
//! - **A changed rendering is reloaded rather than restarted**, through
//!   [`ReloadBehaviour::Command`] and that same endpoint. This is the service the whole idea is for:
//!   every site on the machine is reached through this process, and dropping every connection
//!   because one of them was edited is a cost nobody asked for.
//!
//! # Judged against the real server
//!
//! Every one of those was measured against Caddy 2.11.4 rather than read about, which is what
//! `.claude/roadmap/phase-3-services.md` means by a recipe being judged against the real server.
//! Two of the findings are in the template beside the lines they explain — backtick-quoted paths,
//! `persist_config off`. The third is here because it is about the *spec* and not the file:
//! **`caddy run`, not `caddy start`.** `start` spawns a child, hands it the parent's stdout and
//! returns, so anything capturing that output waits for the server rather than for the launcher.
//! `run` is the process that serves, which is the only kind of process a supervisor can supervise.
//!
//! # What this recipe deliberately does not do
//!
//! **It renders no site.** `sites/*.caddy` is imported and matches nothing, because a site is a
//! `sites` row and there are none until Phase 4 — T39 and T43 in
//! `.claude/roadmap/phase-4-sites-and-elevation.md`. The import is here rather than there because
//! of where it has to point: the glob resolves
//! against the directory holding the file it is written in, so a site file rendered anywhere but
//! into this recipe's own set would be invisible to `caddy validate` and present at run time —
//! which is the one arrangement that cannot be checked. Whoever renders the first site renders it
//! through here.
//!
//! **It issues no certificate.** `auto_https` is `off`, which is what a machine with no CA and no
//! sites should say: on, Caddy would try to obtain a public certificate for a name it will never be
//! reachable at. Phase 5 owns the answer — MixEngine's own CA in the OS trust store — and it is a
//! setting rather than a constant so that the day it changes is a default moving, not a template
//! being edited.
//!
//! [`Recipe`]: crate::generate::Recipe

use mixengine_proto::{
    HealthCheck, HealthProbe, Millis, ReadyCheck, ReloadBehaviour, ServiceSpec, ServiceSpecBuilder,
    StopBehaviour,
};

use crate::generate::document::{CONFIG, Validator};
use crate::generate::recipe::{Context, Instancing, Recipe, TemplateFile};
use crate::generate::settings::{Preset, Setting};
use crate::install::SmokeTest;
use crate::{Error, Result};

/// The `packages.name` this recipe is for, which is also the name of the binary inside the package.
///
/// One value and not two: `mixengine-packages` publishes Caddy as a single executable at the root of
/// the archive, and [`Context::program`] is what spells it `caddy.exe` on Windows.
const PACKAGE: &str = "caddy";

/// The rendered configuration, under `etc/<service-id>/`.
const CADDYFILE: &str = "Caddyfile";

/// Where the admin endpoint listens. Loopback always — see the template.
const ADMIN_HOST: &str = "127.0.0.1";

/// The port the admin endpoint listens on. Caddy's own default, so a `caddy` command typed by hand
/// with no `--address` reaches the server MixEngine is running.
const ADMIN_PORT: &str = "admin_port";

/// The port a site written without one is served on over TLS. The row's own `port` is the other
/// half of the pair.
const HTTPS_PORT: &str = "https_port";

/// Caddy's `auto_https` global option, verbatim: `off`, `disable_redirects`, `disable_certs`,
/// `ignore_loaded_certs`.
///
/// Free text rather than a flag, and it costs nothing to be wrong about: an override Caddy does not
/// recognise is refused by `caddy validate` in that program's own words, before anything is
/// installed. A closed list here would be a second copy of Caddy's vocabulary to keep in step with
/// it across releases.
const AUTO_HTTPS: &str = "auto_https";

/// The level Caddy logs at, in its own spelling: `DEBUG`, `INFO`, `WARN`, `ERROR`.
const LOG_LEVEL: &str = "log_level";

/// How long the admin endpoint is given to answer before the start is a failure, in milliseconds.
const READY_TIMEOUT: &str = "ready_timeout_ms";

/// How long `caddy stop` is given before the process group is killed, in milliseconds.
const STOP_GRACE: &str = "stop_grace_ms";

/// How often the admin endpoint is asked whether the server is still there.
const HEALTH_INTERVAL: Millis = Millis(10_000);

/// How long one of those may take. Well inside the interval, which [`ServiceSpec::validate`]
/// insists on: two probes that could overlap are two probes that can queue.
const HEALTH_TIMEOUT: Millis = Millis(2_000);

/// How long a reload is waited for.
///
/// Generous because of what it covers: `caddy reload` adapts the whole Caddyfile, sends it to the
/// running server and waits for the new configuration to be *provisioned*, which on a machine with
/// forty sites is real work. Nothing is killed when it expires — see [`ReloadBehaviour::Command`].
const RELOAD_PATIENCE: Millis = Millis(30_000);

/// Caddy, as MixEngine runs it.
#[derive(Debug)]
pub struct Caddy;

impl Recipe for Caddy {
    fn package(&self) -> &'static str {
        PACKAGE
    }

    /// There is one Caddy.
    ///
    /// `caddy@main` would be a distinction without a difference, and a second one is two processes
    /// contending for port 80 — which is not a configuration anybody meant to ask for. What
    /// `.claude/features/services.md` calls "exactly one active front end" is this, spelled where a
    /// creation can be refused by it.
    fn instancing(&self) -> Instancing {
        Instancing::Single
    }

    fn smoke_test(&self) -> Option<SmokeTest> {
        Some(SmokeTest {
            executable: PACKAGE.to_owned(),
            // A subcommand and not a flag: `caddy --version` exits non-zero, which would fail the
            // install of an archive that is perfectly good.
            args: vec!["version".to_owned()],
        })
    }

    fn settings(&self) -> &'static [Setting] {
        &[
            Setting {
                key: ADMIN_PORT,
                default: Preset::Number(2019),
            },
            Setting {
                key: AUTO_HTTPS,
                default: Preset::Text("off"),
            },
            Setting {
                key: HTTPS_PORT,
                default: Preset::Number(443),
            },
            Setting {
                key: LOG_LEVEL,
                default: Preset::Text("INFO"),
            },
            Setting {
                // Thirty seconds. Caddy itself is up in tens of milliseconds; what this is really
                // waiting for is a first run on Windows, where the binary is fifty megabytes and
                // Defender reads all of it before the process starts.
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
            path: CADDYFILE,
            source: include_str!("caddy/Caddyfile"),
        }]
    }

    /// `caddy validate`, pointed at the staged `Caddyfile`.
    ///
    /// The binary inside the package this instance was installed from, and not whichever `caddy` is
    /// on the `PATH`: a configuration is judged by the version that will read it, or the check is
    /// about a different program.
    fn validator(&self, context: &Context) -> Option<Validator> {
        Some(Validator::new(context.program(PACKAGE), CADDYFILE).args([
            "validate",
            "--adapter",
            "caddyfile",
            "--config",
            CONFIG,
        ]))
    }

    fn spec(&self, context: &Context) -> Result<ServiceSpecBuilder> {
        let settings = context.settings();

        let caddy = context.program(PACKAGE);
        let config = context.config(CADDYFILE).to_string_lossy().into_owned();
        let admin_port = port(context, ADMIN_PORT)?;
        let address = format!("{ADMIN_HOST}:{admin_port}");

        // Every one of these is what a person would type, which is the point of the admin endpoint
        // being on Caddy's own default port: the commands in this file are the commands in Caddy's
        // documentation, with `--address` added so that two instances cannot answer for each other.
        let admin = format!("http://{address}/config/");

        Ok(ServiceSpec::builder(context.service().clone(), &caddy)
            .args(["run", "--config", &config, "--adapter", "caddyfile"])
            // What a failed start is diagnosed against (T38), and it is the admin endpoint alone:
            // `http_port` and `https_port` are in the global block, but Caddy binds neither until a
            // site asks it to — and sites arrive with T43.
            .ports([admin_port])
            // The configuration directory, and not the data directory: `import sites/*.caddy`
            // resolves against the file rather than the process, but a relative path inside a site
            // — a document root somebody wrote by hand — resolves against this.
            .cwd(context.etc())
            .ready(ReadyCheck::Http {
                url: admin.clone(),
                expect_status: 200,
                timeout: millis(settings.number(READY_TIMEOUT)),
            })
            .health(HealthCheck {
                probe: HealthProbe::Http {
                    url: admin,
                    expect_status: 200,
                },
                interval: HEALTH_INTERVAL,
                timeout: HEALTH_TIMEOUT,
                // Three intervals rather than one: a reload provisions the new configuration before
                // it swaps it in, and a machine with many sites can miss a probe doing it. That is a
                // busy web server, not a sick one.
                failures_before_degraded: 3,
                successes_before_running: 1,
            })
            .reload(ReloadBehaviour::Command {
                program: caddy.clone(),
                args: vec![
                    "reload".to_owned(),
                    "--config".to_owned(),
                    config,
                    "--adapter".to_owned(),
                    "caddyfile".to_owned(),
                    "--address".to_owned(),
                    address.clone(),
                ],
                patience: RELOAD_PATIENCE,
            })
            // Through the admin endpoint rather than by signal, and the same on all three systems.
            // A signal would work on Unix and be a console control event on Windows; this is one
            // mechanism, it is the one Caddy documents, and it is the one T30a proved from a moved
            // directory on all six targets.
            .stop(StopBehaviour::Command {
                program: caddy,
                args: vec!["stop".to_owned(), "--address".to_owned(), address],
                grace: millis(settings.number(STOP_GRACE)),
            }))
    }
}

/// One of this recipe's port settings, as a port.
///
/// A whole number is what the merge guarantees, and a *port* is what this recipe needs — so 70000
/// is refused here, by name, rather than reaching the supervisor as a URL that cannot be parsed and
/// being reported as a service that never came up.
fn port(context: &Context, key: &'static str) -> Result<u16> {
    let number = context.settings().number(key);

    u16::try_from(number)
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| Error::SettingValue {
            service: context.service().as_str().to_owned(),
            key,
            value: number.to_string(),
            reason: "a port is a number from 1 to 65535",
        })
}

/// A setting as a length of time, with a negative one read as none at all.
///
/// Zero and below are refused by [`ServiceSpec::validate`] rather than here: a timeout of zero is a
/// statement about a spec, it is checked in one place for every recipe, and the message it produces
/// names the field.
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

    /// A Caddy on port 80 in a home at `root`, with `overrides` applied.
    ///
    /// The root is a plain string rather than a temporary directory: nothing here writes a file, and
    /// what the assertions are about is the *text* a path becomes. On Windows that text contains
    /// backslashes, which is the whole subject of one of these tests.
    fn context(overrides: &str) -> Context {
        let service = ServiceId::parse("caddy").expect("an id");
        let settings =
            Settings::merge(Caddy.settings(), overrides, &service).expect("usable overrides");

        Context::for_test(
            service,
            PACKAGE,
            Path::new(root()),
            BTreeMap::new(),
            Some(80),
            settings,
        )
    }

    /// What a failed start is diagnosed against — roadmap task **T38**.
    ///
    /// **The admin endpoint alone, and that is not an omission.** `http_port` and `https_port` are
    /// written into the global block, but Caddy binds neither until a site tells it to — and until
    /// sites exist (roadmap task T43) a Caddy that failed to start never wanted 80. Declaring one
    /// anyway would put another program's IIS into the reason for a failure that was not about it.
    #[test]
    fn the_spec_declares_the_admin_endpoint_it_will_bind() {
        let context = context("{}");
        let spec = Caddy
            .spec(&context)
            .expect("a spec")
            .build()
            .expect("a valid spec");

        assert_eq!(spec.ports(), [2019]);
    }

    /// An absolute path on whichever system this is compiled for.
    const fn root() -> &'static str {
        if cfg!(windows) {
            r"C:\MixEngine"
        } else {
            "/opt/mixengine"
        }
    }

    /// There is one Caddy, which is what stops `service.create` being asked for a second front end.
    #[test]
    fn caddy_exists_once() {
        assert_eq!(Caddy.instancing(), Instancing::Single);
    }

    /// An artifact that unpacks and will not run is one the user meets against their own site,
    /// which is T20a's finding and the reason `Installer::install` takes a smoke test at all.
    ///
    /// `caddy version` and not `caddy --version`: Caddy's is a subcommand, and the flag that is not
    /// one exits non-zero — which would fail every install of a perfectly good archive.
    #[test]
    fn caddy_proves_itself_by_running() {
        let smoke = Caddy.smoke_test().expect("a server proves that it runs");

        assert_eq!(smoke.executable, PACKAGE);
        assert_eq!(smoke.args, ["version"]);
    }

    /// What the file renders to, for `overrides`.
    fn caddyfile(overrides: &str) -> String {
        let documents = recipe::render(&Caddy, &context(overrides)).expect("a rendering");

        assert_eq!(documents.len(), 1, "Caddy renders one file");
        assert_eq!(documents[0].relative(), Path::new(CADDYFILE));

        documents[0].contents().to_owned()
    }

    #[test]
    fn the_rendering_says_what_the_row_and_the_defaults_say() {
        let rendered = caddyfile("{}");

        assert!(rendered.contains("admin 127.0.0.1:2019"), "{rendered}");
        assert!(rendered.contains("http_port 80"), "{rendered}");
        assert!(rendered.contains("https_port 443"), "{rendered}");
        assert!(rendered.contains("auto_https off"), "{rendered}");
        assert!(rendered.contains("import sites/*.caddy"), "{rendered}");

        // Not a preference. Without it Caddy writes the configuration it last loaded to the user's
        // own config directory and reads it back on the next start, which is both a write outside
        // MIXENGINE_HOME and a second source of truth for a file rendered from the database.
        assert!(rendered.contains("persist_config off"), "{rendered}");
    }

    /// **Every path this file writes has to survive being a Windows path**, which is the finding
    /// that decided how they are quoted: a Caddyfile token in double quotes treats `\"` and `\\` as
    /// escapes, so `C:\srv\caddy\` ends its string one character early and the parse error names the
    /// line after it. Inside backticks nothing is an escape.
    ///
    /// Asserted on the rendering rather than trusted from the template, because the two ways to lose
    /// this are a quote character edited in the template and a path interpolated somewhere new.
    #[test]
    fn every_path_in_the_rendering_is_quoted_the_way_a_windows_path_survives() {
        let rendered = caddyfile("{}");

        for line in rendered.lines().filter(|line| line.contains(root())) {
            let quoted = line.trim();

            assert!(
                quoted.ends_with('`') && quoted.matches('`').count() == 2,
                "a path reached the Caddyfile outside backticks: {line}"
            );
        }

        assert!(
            rendered.contains(&format!("`{}`", context("{}").data().display())),
            "the storage directory is not the one the row names: {rendered}"
        );
    }

    /// The admin endpoint is one value read by four things — the file, the readiness check, the
    /// health probe and both commands — so an override that moved it and left one of them behind
    /// would be a service that starts and can never be stopped.
    #[test]
    fn an_override_moves_the_admin_endpoint_everywhere_it_is_named() {
        let rendered = caddyfile(r#"{"admin_port": 2020}"#);
        assert!(rendered.contains("admin 127.0.0.1:2020"), "{rendered}");

        let spec = Caddy
            .spec(&context(r#"{"admin_port": 2020}"#))
            .expect("a builder")
            .build()
            .expect("a usable spec");

        assert!(
            matches!(spec.ready(), ReadyCheck::Http { url, .. } if url.contains("127.0.0.1:2020"))
        );
        assert!(matches!(
            spec.health().map(|health| &health.probe),
            Some(HealthProbe::Http { url, .. }) if url.contains("127.0.0.1:2020")
        ));
        assert!(matches!(
            spec.stop(),
            StopBehaviour::Command { args, .. } if args.contains(&"127.0.0.1:2020".to_owned())
        ));
        assert!(matches!(
            spec.reload(),
            Some(ReloadBehaviour::Command { args, .. })
                if args.contains(&"127.0.0.1:2020".to_owned())
        ));
    }

    /// **`run`, and not `start`.** `caddy start` spawns a child, hands it the parent's stdout and
    /// returns — so what the supervisor would be watching is a launcher that has already exited,
    /// and what it would be capturing is a pipe the server holds open for as long as it serves.
    #[test]
    fn the_program_is_the_one_that_serves_rather_than_the_one_that_launches_it() {
        let spec = Caddy
            .spec(&context("{}"))
            .expect("a builder")
            .build()
            .expect("a usable spec");

        assert_eq!(spec.args().first().map(String::as_str), Some("run"));
        assert!(
            spec.program()
                .ends_with(format!("{PACKAGE}{}", std::env::consts::EXE_SUFFIX))
        );
        assert!(
            spec.args().contains(&CADDYFILE.to_owned())
                || spec.args().iter().any(|arg| arg.ends_with(CADDYFILE)),
            "{:?}",
            spec.args()
        );
    }

    /// A whole number is what the merge guarantees and a port is what the recipe needs, so this is
    /// the recipe's own refusal rather than the merge's — and the message has to name the setting,
    /// because the alternative is a URL that will not parse being reported hours later as a service
    /// that never came up.
    #[test]
    fn a_number_that_is_not_a_port_is_refused_against_the_setting_that_holds_it() {
        for offered in ["70000", "0", "-1"] {
            let error = Caddy
                .spec(&context(&format!(r#"{{"admin_port": {offered}}}"#)))
                .expect_err("a number that is not a port");

            let message = error.to_string();
            assert!(message.contains("admin_port"), "{message}");
            assert!(message.contains(offered), "{message}");
        }
    }

    /// A row with no port is a Caddy that binds nothing until a site says otherwise, which is what
    /// the `services` schema means by a nullable `port` — not a rendering with an empty directive in
    /// it, and not a failure.
    #[test]
    fn a_row_with_no_port_renders_no_http_port_at_all() {
        let service = ServiceId::parse("caddy").expect("an id");
        let settings = Settings::merge(Caddy.settings(), "{}", &service).expect("defaults");
        let portless = Context::for_test(
            service,
            PACKAGE,
            Path::new(root()),
            BTreeMap::new(),
            None,
            settings,
        );

        let documents = recipe::render(&Caddy, &portless).expect("a rendering");
        let rendered = documents[0].contents();

        assert!(!rendered.contains("http_port"), "{rendered}");
        assert!(rendered.contains("https_port 443"), "{rendered}");
    }
}
