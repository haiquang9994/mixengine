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
//! **The fourth was found by a rotation and is `--force` on the reload.** Caddy adapts the Caddyfile
//! to JSON and the adapter throws comments away — so the line `site.caddy` carries a certificate
//! fingerprint on, which is the only thing a reissue changes, is invisible to the server. The
//! configuration it is handed is identical to the one it is running, its admin endpoint skips the
//! load, and it goes on presenting the certificate it holds in memory. See the test that names the
//! flag.
//!
//! # It renders every site, and into its own set
//!
//! Roadmap task **T43**. `sites/*.caddy` is imported by the Caddyfile above and rendered by
//! [`Recipe::sites`] below, into the *same* `Vec<Document>` — never by a second path that runs
//! afterwards. The import resolves against the directory holding the file it is written in, which is
//! the staging directory while `caddy validate` is looking and `etc/caddy/` afterwards, so a site
//! file written anywhere else would be invisible to the checker and present at run time: the one
//! arrangement whose correctness cannot be checked. T31 put the import here rather than in Phase 4
//! for exactly that reason, and this is the half that was waiting.
//!
//! [`swept`](Recipe::swept) is the other half. A site that stops being declared has to lose its file
//! on the same walk, or it goes on being served — so `sites/` holds exactly what was rendered into
//! it and nothing else.
//!
//! # What this recipe deliberately does not do
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

use crate::generate::document::{CONFIG, Document, Validator};
use crate::generate::recipe::{Context, Instancing, Recipe, Role, TemplateFile, Upstream};
use crate::generate::served::{Served, ServedKind};
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

/// One rendered site, under `etc/<service-id>/sites/`.
const SITE: &str = include_str!("caddy/site.caddy");

/// The directory the sites go in, which is also the one this recipe sweeps.
///
/// The name is in the Caddyfile's `import sites/*.caddy` as well, and the two have to agree — but
/// they are a template and a directory listing rather than two constants, so this is the one place
/// in Rust that spells it.
const SITES: &str = "sites";

/// Where the admin endpoint listens. Loopback always — see the template.
const ADMIN_HOST: &str = "127.0.0.1";

/// The addresses every site answers on before anything is shared — roadmap task **T74**.
///
/// **Written on every site block, and that is the fix rather than the decoration.** Caddy's own
/// default is every interface, so before T74 a site was already reachable from the network on any
/// machine whose firewall allowed the port — it simply answered nothing, because no site block
/// matched an address-shaped `Host`. T74 opens that port deliberately, which turns "answers
/// nothing" into a promise this code has to keep: without these two lines a site nobody shared is
/// listening on the LAN, and the only thing between it and a stranger is which `Host` header they
/// send.
///
/// Both loopback families, because `blog.test` resolves to 127.0.0.1 through the hosts file while
/// `localhost` resolves to ::1 first on Windows — binding one would break whichever the developer
/// typed.
const LOOPBACK: [&str; 2] = ["127.0.0.1", "::1"];

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
    /// contending for port 80 — which is not a configuration anybody meant to ask for.
    fn instancing(&self) -> Instancing {
        Instancing::Single
    }

    /// And it is one of the two programs a site is reached through — roadmap task **T37**.
    ///
    /// What `.claude/features/services.md` calls "exactly one active front end" needs both halves:
    /// this one is about the *job*, and it is what [`instancing`](Self::instancing) cannot say —
    /// one Caddy and one nginx are two rows that each obey their own recipe and still leave a home
    /// with two front ends. [`nginx`](super::nginx) is the other side of it.
    fn role(&self) -> Role {
        Role::FrontEnd
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

    /// Exactly `sites/`, and only because this recipe is a front end — D4.
    ///
    /// Without it a deleted site keeps the file it had and goes on being served, which is what T31
    /// left open in as many words. Nothing sweeps `etc/caddy/` itself: a directory belonging to a
    /// *deleted service* is `service.delete`'s problem.
    fn swept(&self) -> &'static [&'static str] {
        &[SITES]
    }

    /// One file per site, named after its primary domain — D12.
    ///
    /// Rendered into the set the Caddyfile's own `import sites/*.caddy` picks up, which is what makes
    /// the whole rendering judged by `caddy validate` where it is staged.
    fn sites(&self, context: &Context, served: &[Served]) -> Result<Vec<Document>> {
        served
            .iter()
            .map(|site| {
                let rendering = SiteRendering {
                    primary: site.primary(),
                    domains: &site.domains,
                    doc_root: &site.doc_root,
                    kind: kind(&site.kind),
                    upstream: upstream(&site.kind),
                    activator: activator(&site.kind),
                    certificate: site.certificate.as_ref().map(Certificate::from),
                    bind: bound(site.shared.as_ref().map(|shared| shared.address)),
                    lan: site
                        .shared
                        .as_ref()
                        .map(|shared| shared.address.to_string()),
                    mdns: site.shared.as_ref().and_then(|shared| shared.name.clone()),
                };

                let contents = crate::generate::served::render(
                    SITE,
                    "caddy/site.caddy",
                    context.service(),
                    &rendering,
                )?;

                Ok(Document::new(
                    format!("{SITES}/{}.caddy", site.primary()),
                    contents,
                ))
            })
            .collect()
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
                    // **The certificate line this reload exists for is a comment**, and the
                    // Caddyfile adapter removes comments — so the JSON Caddy is handed after a
                    // reissue is identical to the one it is running, and without this it answers by
                    // skipping the load and goes on serving the certificate it holds in memory. See
                    // the test that names this flag.
                    "--force".to_owned(),
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

/// One site, as `caddy/site.caddy` reads it.
#[derive(Debug, serde::Serialize)]
struct SiteRendering<'a> {
    primary: &'a str,
    domains: &'a [String],
    doc_root: &'a std::path::Path,
    kind: &'static str,

    /// Empty for a static site, whose branch does not read it — but `Strict` undefined behaviour
    /// means the key has to be there whichever branch is taken.
    upstream: String,

    /// The activator to fall back to, or [`None`] for a pool nothing can wake — T70.
    ///
    /// Present whichever branch is taken, for `upstream`'s reason: `Strict` makes a missing key an
    /// error, and `None` serialises to `null`, which `{% if %}` reads as false.
    activator: Option<String>,

    /// [`None`] renders no TLS block at all — the T51 design, D4.
    ///
    /// **The key is always present**, for `upstream`'s reason: `UndefinedBehavior::Strict` makes a
    /// missing key an error rather than a falsy value. `None` serialises to `null`, which `{% if %}`
    /// reads as false.
    certificate: Option<Certificate>,

    /// Every address this site's listeners bind, loopback first — roadmap task **T74**.
    ///
    /// **Never empty.** A site that is not shared binds the two loopback addresses and nothing
    /// else; a shared one adds its interface address. Caddy's `bind` *replaces* the default rather
    /// than adding to it, which is what makes both halves work: loopback has to be named or a
    /// shared site would go down in the browser on this machine, and the LAN address has to be
    /// absent or an unshared site would be listening on the network.
    bind: Vec<String>,

    /// The address a shared site also answers to by name, or [`None`] — roadmap task **T74**.
    ///
    /// **A site block matches on `Host`, and a phone sends the address it was given.** Binding the
    /// interface makes the connection arrive; without this line it arrives and matches no site, and
    /// Caddy answers 200 with an empty body — which is exactly what the first phone to try this saw.
    lan: Option<String>,

    /// The mDNS name a shared site also answers to, or [`None`] — roadmap task **T75**.
    ///
    /// **The same lesson as [`lan`](Self::lan), with a name.** The daemon advertises the name and
    /// this line is what makes the site *reply* to it; a name that resolves to a block which does
    /// not match it is the blank page T74 spent a phone finding.
    mdns: Option<String>,
}

/// What this site's listeners bind: loopback always, and the interface address when shared.
fn bound(shared: Option<std::net::Ipv4Addr>) -> Vec<String> {
    let mut bound: Vec<String> = LOOPBACK
        .iter()
        .map(|address| (*address).to_owned())
        .collect();
    bound.extend(shared.map(|address| address.to_string()));

    bound
}

/// A certificate as the template writes it — roadmap task **T51**.
///
/// **Strings and not `Path`s**, because a template writes text: `Path`'s `Serialize` is lossy on a
/// path that is not UTF-8, and `display()` is what the rest of this module already writes.
#[derive(Debug, serde::Serialize)]
struct Certificate {
    certificate: String,
    key: String,
    fingerprint: String,
}

impl From<&crate::generate::served::SiteCertificate> for Certificate {
    fn from(certificate: &crate::generate::served::SiteCertificate) -> Self {
        Self {
            certificate: certificate.certificate.display().to_string(),
            key: certificate.key.display().to_string(),
            fingerprint: certificate.fingerprint.clone(),
        }
    }
}

/// Which branch of the template this kind takes.
///
/// Three and not four: a `node-app` renders as a reverse proxy to loopback and that is all it is —
/// D7. Nothing in this build starts a node process, and a fourth branch would be a difference the
/// rendering does not have.
const fn kind(kind: &ServedKind) -> &'static str {
    match kind {
        ServedKind::PhpFpm { .. } => "php-fpm",
        ServedKind::Static => "static",
        ServedKind::ReverseProxy { .. } | ServedKind::NodeApp { .. } => "proxy",
    }
}

/// The address this kind is proxied or passed to, as **Caddy** spells one.
///
/// `unix/` and then the path, which is Caddy's own spelling of a socket and is not nginx's — the
/// same value renders as `unix:/run/…` there. That difference is the reason
/// [`Upstream`] is a value rather than a string.
fn upstream(kind: &ServedKind) -> String {
    match kind {
        ServedKind::PhpFpm { upstream, .. } => address(upstream),
        ServedKind::ReverseProxy { upstream } => upstream.clone(),
        ServedKind::NodeApp { port } => format!("http://127.0.0.1:{port}"),
        ServedKind::Static => String::new(),
    }
}

/// The activator's address for this kind, as Caddy spells one — roadmap task **T70**.
///
/// [`None`] for everything nothing can start by connecting to it, which renders the site exactly as
/// it rendered before T70.
fn activator(kind: &ServedKind) -> Option<String> {
    match kind {
        ServedKind::PhpFpm { activator, .. } => activator.as_ref().map(address),
        _ => None,
    }
}

/// One [`Upstream`] in Caddy's spelling.
fn address(upstream: &Upstream) -> String {
    match upstream {
        Upstream::Socket(path) => format!("unix/{}", path.display()),
        Upstream::Tcp(address) => address.to_string(),
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

    use mixengine_platform::PortBinding;
    use mixengine_proto::ServiceId;

    use super::*;
    use crate::generate::recipe;
    use crate::generate::served::Shared;
    use crate::generate::settings::Settings;

    /// D8: the row keeps the port a browser asks for, and the file carries the port the process
    /// must bind. On macOS those differ; on the other two they are the same number, and neither the
    /// row nor this recipe knows which system it is on.
    #[test]
    fn the_global_block_carries_the_port_the_process_binds_and_the_row_keeps_the_one_it_answers() {
        let redirecting = vec![
            PortBinding {
                answer: 80,
                bind: 8080,
            },
            PortBinding {
                answer: 443,
                bind: 8443,
            },
        ];

        let redirected = context("{}").with_bindings(redirecting);
        let rendered = recipe::render(&Caddy, &redirected).expect("a Caddyfile")[0]
            .contents()
            .to_owned();

        assert!(rendered.contains("http_port 8080"), "{rendered}");
        assert!(rendered.contains("https_port 8443"), "{rendered}");
        assert_eq!(
            redirected.port(),
            Some(80),
            "the row still answers on 80, which is what LAN sharing and `mix site show` read"
        );

        let direct = context("{}");
        let plain = recipe::render(&Caddy, &direct).expect("a Caddyfile")[0]
            .contents()
            .to_owned();

        assert!(plain.contains("http_port 80"), "{plain}");
        assert!(plain.contains("https_port 443"), "{plain}");
    }

    /// One site per file, named after the primary domain — D12. The row's integer id was the
    /// alternative: `etc/caddy/sites/7.caddy` tells whoever is reading the directory nothing, and
    /// the directory is one of the first places somebody looks when a site does not answer.
    #[test]
    fn each_kind_renders_a_block_naming_what_it_was_given() {
        let served = vec![
            Served {
                shared: None,
                domains: vec!["blog.test".to_owned(), "www.blog.test".to_owned()],
                doc_root: doc_root(),
                kind: ServedKind::Static,
                https: true,
                certificate: None,
            },
            Served {
                shared: None,
                domains: vec!["php.test".to_owned()],
                doc_root: doc_root(),
                kind: ServedKind::PhpFpm {
                    upstream: Upstream::Tcp("127.0.0.1:9000".parse().expect("an address")),
                    activator: None,
                },
                https: true,
                certificate: None,
            },
            Served {
                shared: None,
                domains: vec!["proxy.test".to_owned()],
                doc_root: doc_root(),
                kind: ServedKind::ReverseProxy {
                    upstream: "http://127.0.0.1:4000".to_owned(),
                },
                https: true,
                certificate: None,
            },
            Served {
                shared: None,
                domains: vec!["node.test".to_owned()],
                doc_root: doc_root(),
                kind: ServedKind::NodeApp { port: 3000 },
                https: true,
                certificate: None,
            },
        ];

        let documents = Caddy
            .sites(&context("{}"), &served)
            .expect("four site files");

        assert_eq!(documents.len(), 4);
        assert_eq!(
            documents[0].relative(),
            Path::new("sites").join("blog.test.caddy")
        );

        let statically = documents[0].contents();
        assert!(statically.contains("http://blog.test"), "{statically}");
        assert!(
            statically.contains("http://www.blog.test"),
            "an alias is not reachable: {statically}"
        );
        assert!(
            statically.contains("file_server"),
            "a static site serves files: {statically}"
        );

        let php = documents[1].contents();
        assert!(php.contains("php_fastcgi 127.0.0.1:9000"), "{php}");

        let proxy = documents[2].contents();
        assert!(
            proxy.contains("reverse_proxy http://127.0.0.1:4000"),
            "{proxy}"
        );

        // D7, asserted rather than described: a `node-app` is a reverse proxy to loopback and
        // nothing else. Nothing in this build starts `npm run dev`.
        let node = documents[3].contents();
        assert!(
            node.contains("reverse_proxy http://127.0.0.1:3000"),
            "{node}"
        );
    }

    /// A document root on whichever system this is compiled for.
    fn doc_root() -> std::path::PathBuf {
        if cfg!(windows) {
            std::path::PathBuf::from(r"C:\src\blog\public")
        } else {
            std::path::PathBuf::from("/src/blog/public")
        }
    }

    /// A socket is spelled Caddy's way, which is why `Upstream` is a value and not a string: nginx
    /// writes the same socket differently.
    #[test]
    fn a_pool_on_a_socket_is_spelled_the_way_caddy_spells_one() {
        let served = vec![Served {
            shared: None,
            domains: vec!["php.test".to_owned()],
            doc_root: doc_root(),
            kind: ServedKind::PhpFpm {
                upstream: Upstream::Socket(std::path::PathBuf::from(
                    "/home/me/run/php-fpm-8.3.sock",
                )),
                activator: None,
            },
            https: true,
            certificate: None,
        }];

        let rendered = Caddy.sites(&context("{}"), &served).expect("one site")[0]
            .contents()
            .to_owned();

        assert!(
            rendered.contains("php_fastcgi unix//home/me/run/php-fpm-8.3.sock"),
            "{rendered}"
        );
    }

    /// A site shared on the LAN — roadmap task **T74**.
    fn a_shared_site(address: [u8; 4]) -> Served {
        Served {
            shared: Some(Shared {
                address: address.into(),
                name: Some("blog-mixengine.local".to_owned()),
            }),
            domains: vec!["blog.test".to_owned()],
            doc_root: doc_root(),
            kind: ServedKind::Static,
            https: false,
            certificate: None,
        }
    }

    /// **Every address on one directive, loopback first** — the T74 design, D2. Caddy's `bind`
    /// replaces the default rather than adding to it, so a block naming only the LAN address would
    /// come up on the phone and go down in the browser on this machine.
    #[test]
    fn a_shared_site_binds_loopback_and_the_lan_address() {
        let served = vec![a_shared_site([192, 168, 1, 10])];

        let rendered = Caddy.sites(&context("{}"), &served).expect("one site")[0]
            .contents()
            .to_owned();

        assert!(
            rendered.contains("bind 127.0.0.1 ::1 192.168.1.10"),
            "{rendered}"
        );
    }

    /// **A shared site answers to its address by name as well as binding it.**
    ///
    /// The bug a phone found. Binding the interface makes the connection arrive, and a site block
    /// matches on `Host` — a phone sends the address it was handed, so without that address in the
    /// block's own list Caddy matches no site and answers 200 with an empty body. Which is what the
    /// URL `mix site share` had just printed did.
    #[test]
    fn a_shared_site_answers_to_the_address_a_phone_sends() {
        let served = vec![a_shared_site([192, 168, 1, 10])];

        let rendered = Caddy.sites(&context("{}"), &served).expect("one site")[0]
            .contents()
            .to_owned();

        assert!(
            rendered.contains("http://blog.test, http://192.168.1.10"),
            "{rendered}"
        );
    }

    /// **The name is in the block's address list, not only on the network** — the T75 design, D3.
    ///
    /// T74 learned this with an address: binding an interface says where a connection is accepted,
    /// and the block's address list says which site replies. A phone that resolves the name and is
    /// answered 200 with an empty body is the slowest failure this feature has, and it is the one
    /// this line prevents.
    #[test]
    fn a_shared_site_answers_to_its_mdns_name() {
        let served = vec![a_shared_site([192, 168, 1, 10])];

        let rendered = Caddy.sites(&context("{}"), &served).expect("one site")[0]
            .contents()
            .to_owned();

        assert!(
            rendered.contains("http://blog-mixengine.local"),
            "{rendered}"
        );
    }

    /// **Opt-in per site, and this is where it is actually enforced.**
    ///
    /// Caddy's default is every interface, so an unshared site was already listening on the network
    /// before T74 — it simply matched no `Host` anyone would send. T74 opens the port deliberately,
    /// which turns that from an accident nobody could reach into a promise this rendering has to
    /// keep: loopback, both families, and nothing else.
    #[test]
    fn a_site_that_is_not_shared_binds_loopback_only() {
        let served = vec![Served {
            shared: None,
            ..a_shared_site([192, 168, 1, 10])
        }];

        let rendered = Caddy.sites(&context("{}"), &served).expect("one site")[0]
            .contents()
            .to_owned();

        assert!(rendered.contains("bind 127.0.0.1 ::1"), "{rendered}");
        assert!(!rendered.contains("192.168.1.10"), "{rendered}");
    }

    /// A shared site with a certificate renders two blocks and both carry the addresses — a padlock
    /// that worked everywhere except the device the site was shared for would be worse than none.
    #[test]
    fn both_blocks_of_a_shared_https_site_carry_the_addresses() {
        let served = vec![Served {
            shared: Some(Shared {
                address: [192, 168, 1, 10].into(),
                name: Some("blog-mixengine.local".to_owned()),
            }),
            ..a_site_with_a_certificate()
        }];

        let rendered = Caddy.sites(&context("{}"), &served).expect("one site")[0]
            .contents()
            .to_owned();

        assert!(
            rendered.contains("https://blog.test, https://192.168.1.10"),
            "{rendered}"
        );
        assert_eq!(
            rendered.matches("bind 127.0.0.1 ::1 192.168.1.10").count(),
            2,
            "{rendered}"
        );
    }

    /// The front end is the only recipe that sweeps, and it sweeps exactly `sites/`.
    #[test]
    fn the_front_end_sweeps_its_sites_directory_and_nothing_else() {
        assert_eq!(Caddy.swept(), &["sites"]);
    }

    /// A static site at `blog.test` with a certificate — roadmap task **T51**.
    ///
    /// Paths that do not exist, deliberately: this module renders text and never reads a disk.
    /// Whether the pair is there was decided in `generate::served`, and asking again here would be a
    /// second answer to one question.
    fn a_site_with_a_certificate() -> Served {
        Served {
            shared: None,
            domains: vec!["blog.test".to_owned()],
            doc_root: doc_root(),
            kind: ServedKind::Static,
            https: true,
            certificate: Some(crate::generate::served::SiteCertificate {
                certificate: std::path::PathBuf::from(
                    "/home/someone/.mixengine/certs/sites/blog.test.crt",
                ),
                key: std::path::PathBuf::from("/home/someone/.mixengine/certs/sites/blog.test.key"),
                fingerprint: "ab".repeat(32),
            }),
        }
    }

    /// **The three directives are one unit, and any two of them are worse than none** — T70, D2.
    ///
    /// Measured against a real Caddy 2.10.0 before this was written, twenty requests each with the
    /// pool's address dead and the activator's live:
    ///
    /// | Rendering | 200s |
    /// | --- | --- |
    /// | both addresses, nothing else | 8 of 20 — Caddy load-balances between them |
    /// | `+ lb_policy first`, `lb_try_duration` | 0 of 20, each burning the full budget |
    /// | `+ fail_duration` | 20 of 20 |
    ///
    /// So the bare two-address form is not a missing retry, it is a site sending half its traffic to
    /// the activator while the pool is up and well; and `first` without passive health checking keeps
    /// choosing the address that is already refusing until the budget runs out. This asserts all
    /// three together because that is the only combination that works.
    #[test]
    fn a_pool_that_can_be_woken_renders_the_activator_after_it_under_a_policy_that_prefers_the_pool()
     {
        let rendered = render_site(&Served {
            shared: None,
            domains: vec!["php.test".to_owned()],
            doc_root: doc_root(),
            kind: ServedKind::PhpFpm {
                upstream: Upstream::Tcp("127.0.0.1:9000".parse().expect("an address")),
                activator: Some(Upstream::Tcp("127.0.0.1:9500".parse().expect("an address"))),
            },
            https: false,
            certificate: None,
        });

        assert!(
            rendered.contains("php_fastcgi 127.0.0.1:9000 127.0.0.1:9500 {"),
            "the pool must come first — the fallback is what is tried when it refuses:\n{rendered}"
        );

        for directive in ["lb_policy first", "lb_try_duration", "fail_duration"] {
            assert!(
                rendered.contains(directive),
                "without `{directive}` the other two are worse than no fallback at all:\n{rendered}"
            );
        }
    }

    /// **A pool with no activator renders exactly what it rendered before T70.**
    ///
    /// A home whose pool listens on TCP and whose row predates the `activation_port` column has no
    /// activator, and the site it serves must be the site it served yesterday — a `lb_policy` over
    /// one upstream would be a behaviour change bought for nothing.
    #[test]
    fn a_pool_with_no_activator_renders_the_one_line_it_always_did() {
        let rendered = render_site(&Served {
            shared: None,
            domains: vec!["php.test".to_owned()],
            doc_root: doc_root(),
            kind: ServedKind::PhpFpm {
                upstream: Upstream::Tcp("127.0.0.1:9000".parse().expect("an address")),
                activator: None,
            },
            https: false,
            certificate: None,
        });

        assert!(
            rendered.contains("php_fastcgi 127.0.0.1:9000\n"),
            "a pool nothing can wake renders one address and no block:\n{rendered}"
        );
        assert!(
            !rendered.contains("lb_policy"),
            "there is nothing to choose between:\n{rendered}"
        );
    }

    /// One site through the real recipe.
    ///
    /// `Caddy.sites` and not `served::render`, because that is the path `sites()` actually takes —
    /// a mistake in how the rendering is assembled is caught here rather than only by the
    /// integration suite.
    fn render_site(site: &Served) -> String {
        Caddy
            .sites(&context("{}"), std::slice::from_ref(site))
            .expect("one site file")[0]
            .contents()
            .to_owned()
    }

    /// A site with a certificate renders **two** blocks: one plaintext, one TLS — the T51 design,
    /// D2. One block naming both schemes is refused by Caddy itself, which is why this asserts the
    /// shape and not the directive alone.
    #[test]
    fn an_https_site_renders_a_plaintext_block_and_a_tls_block() {
        let rendered = render_site(&a_site_with_a_certificate());

        assert!(rendered.contains("http://blog.test"), "{rendered}");
        assert!(rendered.contains("https://blog.test"), "{rendered}");
        assert_eq!(rendered.matches("\n\ttls ").count(), 1, "{rendered}");
        assert_eq!(rendered.matches("\n\troot *").count(), 2, "{rendered}");
    }

    /// **A site with no certificate renders one block and no `tls`** — the T51 design, D4. A `tls`
    /// at a path that is not there fails `caddy validate`, and validation judges the whole staged
    /// rendering, so this one site would cost every other site its new configuration.
    #[test]
    fn a_site_with_no_certificate_renders_no_tls_at_all() {
        let mut site = a_site_with_a_certificate();
        site.certificate = None;

        let rendered = render_site(&site);

        assert!(rendered.contains("http://blog.test"), "{rendered}");
        assert!(!rendered.contains("https://"), "{rendered}");
        assert!(!rendered.contains("\n\ttls "), "{rendered}");
    }

    /// A site that never declared HTTPS renders exactly what it rendered before T51.
    #[test]
    fn a_plain_site_renders_one_block() {
        let mut site = a_site_with_a_certificate();
        site.https = false;
        site.certificate = None;

        let rendered = render_site(&site);

        assert_eq!(rendered.matches("\n\troot *").count(), 1, "{rendered}");
        assert!(!rendered.contains("\n\ttls "), "{rendered}");
    }

    /// **The fingerprint is what makes a reissue reload** — the T51 design, D5. T50 writes to the
    /// same path, so without it the rendering after a reissue is byte-identical, `install` finds no
    /// difference, and the server goes on serving the certificate it holds in memory.
    #[test]
    fn a_different_certificate_renders_a_different_file() {
        let site = a_site_with_a_certificate();
        let before = render_site(&site);

        let mut reissued = site.clone();
        reissued
            .certificate
            .as_mut()
            .expect("a certificate")
            .fingerprint = "cd".repeat(32);

        assert_ne!(
            before,
            render_site(&reissued),
            "a reissued certificate rendered the same bytes, so nothing would reload"
        );
    }

    /// **And the other half, which matters as much**: nothing changing renders identical bytes. A
    /// rendering that had become unstable would reload the front end on every unrelated
    /// `service.*` call.
    #[test]
    fn rendering_twice_with_nothing_changed_is_identical() {
        let site = a_site_with_a_certificate();

        assert_eq!(render_site(&site), render_site(&site));
    }

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

    /// There is one Caddy, which is what stops `service.create` being asked for a second Caddy.
    #[test]
    fn caddy_exists_once() {
        assert_eq!(Caddy.instancing(), Instancing::Single);
    }

    /// And what stops it being asked for an nginx beside this one is the other answer — roadmap task
    /// **T37**. Two recipes, one job; [`Instancing`] is about a package and cannot say it.
    #[test]
    fn caddy_is_a_front_end() {
        assert_eq!(Caddy.role(), Role::FrontEnd);
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

    /// **`--force`, and the reason is that the line a reissue changes is a comment.**
    ///
    /// `site.caddy` carries `# Certificate sha256:…` so that a reissued certificate renders a file
    /// that *differs* — without it `document::install` compares byte-identical bytes, finds no
    /// change, and never asks anybody to re-read anything. That half works. The other half is that
    /// `caddy reload` adapts the Caddyfile to JSON, **and the adapter throws comments away**: the
    /// configuration Caddy is handed is identical to the one it is already running, so its admin
    /// endpoint skips the load and the process goes on serving the certificate it holds in memory.
    ///
    /// Measured, not read: CI's `system` job rotated the authority on Windows and macOS, and both
    /// legs found the server still presenting the leaf signed by the authority that had just been
    /// replaced — `problem: served_certificate_differs`, `trust: rejected`. `caddy reload --help`
    /// on the pinned 2.11.4 names the flag for exactly this: *Force config reload, even if it is
    /// the same*.
    ///
    /// It costs nothing on the ordinary path. A reload is only ever asked for after the rendering
    /// changed, so "even if it is the same" is a statement about Caddy's view of the file and never
    /// about ours.
    #[test]
    fn a_reload_is_forced_because_the_line_a_reissued_certificate_changes_is_a_comment() {
        let spec = Caddy
            .spec(&context("{}"))
            .expect("a builder")
            .build()
            .expect("a usable spec");

        let Some(ReloadBehaviour::Command { args, .. }) = spec.reload() else {
            panic!("a front end that cannot be reloaded drops every connection to edit one site");
        };

        assert!(
            args.contains(&"--force".to_owned()),
            "a certificate is reissued to the same path, so the only thing that changes in this \
             file is a comment the adapter removes: {args:?}"
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
