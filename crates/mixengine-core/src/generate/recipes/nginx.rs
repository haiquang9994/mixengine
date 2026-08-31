//! Nginx: the alternative front end — roadmap task **T37**.
//!
//! The second of the two programs `.claude/features/services.md` will let a site be reached through,
//! and the one that makes "exactly one active front end" a rule somebody can break — which is why
//! [`Role`] arrived with it. Everything else here is [`caddy`](super::caddy)'s shape answered by a
//! server that has none of Caddy's mechanisms:
//!
//! - **There is no admin endpoint, so the recipe renders one.** A loopback `server` block that
//!   answers `200` on `/mixengine/health` and `404` on everything else is both the readiness check
//!   and the health probe. The obvious alternative — a TCP connect on the port — is not a weaker
//!   version of that, it is a different question: the master process holds the listening socket, so
//!   a connection is accepted in exactly the same way when every worker has died. A request that
//!   comes back is one a worker reading this configuration served.
//! - **`nginx -t` judges the rendering before it is installed**, over the staging directory T30
//!   builds, exactly as `caddy validate` does. What makes the two of them work differently is where
//!   an `include` resolves: Caddy's `import` is relative to the file, nginx's is relative to the
//!   **prefix** — so `-p` is passed in both places, pointing at the staging directory while the
//!   configuration is being judged and at `etc/<service-id>/` once it is installed.
//! - **A changed rendering is reloaded rather than restarted**, through `-s reload` against the
//!   running master. That is a signal in nginx's own spelling rather than an OS one, so it works on
//!   Windows, where the supervisor's own `signal` does not — and it is why the pid file goes where
//!   this configuration says rather than where nginx would compile it: `-s` finds a master through
//!   the configuration it is given.
//!
//! # Judged against the real server
//!
//! `crates/mixengine-cli/tests/nginx.rs` runs the whole of that against a real nginx, and it runs it
//! through the same harness [`caddy`](super::caddy)'s suite does — which is the parity half of T37.
//! Two findings are in the template beside the lines they explain: forward-slashed quoted paths, and
//! five temp directories that are children of one that exists.
//!
//! # What this recipe deliberately does not do
//!
//! **It renders no site, and listens on nothing a site would be reached on.** `include sites/*.conf`
//! matches nothing until Phase 4 (T39, T43), and the row's own port is written into no `listen` at
//! all: binding 80 needs the port grant T42 has not built on macOS and Linux, and a front end
//! holding a port it serves nothing on is worse than one that has not taken it yet. Caddy says the
//! same thing by writing `http_port` into a global block and binding neither.
//!
//! **It terminates no TLS.** Phase 5 owns the certificate, and a `listen ... ssl` with no
//! certificate is a configuration nginx refuses outright rather than one it starts without.
//!
//! [`Role`]: crate::generate::recipe::Role

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};

use mixengine_proto::{
    HealthCheck, HealthProbe, Millis, ReadyCheck, ReloadBehaviour, ServiceSpec, ServiceSpecBuilder,
    StopBehaviour,
};

use crate::generate::document::{CONFIG, Document, Reason, Validator};
use crate::generate::recipe::{
    Context, Endpoints, Instancing, Recipe, Role, TemplateFile, Upstream,
};
use crate::generate::served::{Served, ServedKind};
use crate::generate::settings::{Preset, Setting};
use crate::install::SmokeTest;
use crate::{Error, Result};

/// The `packages.name` this recipe is for, which is also the name the binary is published under.
const PACKAGE: &str = "nginx";

/// The rendered configuration, under `etc/<service-id>/`.
const CONFIG_FILE: &str = "nginx.conf";

/// The data file out of the archive that a generated configuration cannot do without.
///
/// Every `Content-Type` nginx serves comes out of it, and a generated file has no `conf/` of its own
/// to reach it through — see [`Endpoints::includes`].
const MIME_TYPES: &str = "mime.types";

/// The other file out of the archive a generated configuration includes: what a `fastcgi_pass` to
/// PHP-FPM needs, and the reason MixEngine renders an nginx configuration at all — D6.
///
/// `mixengine-packages`' `tools/nginx.py` publishes it under `CONF_FILES` in as many words.
const FASTCGI_PARAMS: &str = "fastcgi_params";

/// One rendered site, under `etc/<service-id>/sites/`.
const SITE: &str = include_str!("nginx/site.conf");

/// The directory the sites go in, which is also the one this recipe sweeps. The name is in
/// `nginx.conf`'s `include sites/*.conf;` as well.
const SITES: &str = "sites";

/// The port a front end answers on when its row names none.
///
/// nginx's own configuration carries no listen for sites, so unlike Caddy there is no server default
/// to fall through to — this is that default, written down, and it is the same 80 Caddy would use.
const DEFAULT_HTTP_PORT: u16 = 80;

/// The port a site with a certificate is served on over TLS — roadmap task **T51**.
///
/// **A setting and not a constant**, which was not the first design: nginx has no global listen, so
/// a constant looked like enough. It is not. This is the first port a front end binds that no test
/// could move, and `tests/nginx.rs` runs a real server as an unprivileged user — where 443 is
/// refused, and nginx rejects the whole configuration rather than the one listener. The same is
/// true of any machine that has not been granted the ports, so a setting is what the *product*
/// needs as well as the suite.
///
/// `Context::bound` still turns whatever this is into its bound half on macOS.
const HTTPS_PORT: &str = "https_port";

/// Where the status endpoint listens. Loopback always — see the template.
const STATUS_HOST: &str = "127.0.0.1";

/// What the status endpoint answers on, and the one path in this configuration that is MixEngine's
/// rather than a user's.
const HEALTH_PATH: &str = "/mixengine/health";

/// The port that endpoint listens on.
///
/// **One above Caddy's 2019**, which is the whole of the reasoning: it is the same thing for the
/// other front end, the two can never both be running ([`Role::FrontEnd`]), and a person who
/// remembers one number has remembered both. nginx publishes no default of its own to borrow.
const STATUS_PORT: &str = "status_port";

/// How many worker processes to start.
const WORKER_PROCESSES: &str = "worker_processes";

/// How many connections each of them may hold at once.
const WORKER_CONNECTIONS: &str = "worker_connections";

/// The largest request body nginx will accept, in nginx's own units: `64m`, `1g`.
const CLIENT_MAX_BODY_SIZE: &str = "client_max_body_size";

/// The level nginx logs at, in its own spelling: `debug`, `info`, `notice`, `warn`, `error`,
/// `crit`, `alert`, `emerg`.
const LOG_LEVEL: &str = "log_level";

/// How long the status endpoint is given to answer before the start is a failure, in milliseconds.
const READY_TIMEOUT: &str = "ready_timeout_ms";

/// How long `-s quit` is given before the process group is killed, in milliseconds.
const STOP_GRACE: &str = "stop_grace_ms";

/// How often the status endpoint is asked whether the server is still there.
const HEALTH_INTERVAL: Millis = Millis(10_000);

/// How long one of those may take. Well inside the interval, which [`ServiceSpec::validate`]
/// insists on.
const HEALTH_TIMEOUT: Millis = Millis(2_000);

/// How long a reload is waited for.
///
/// `-s reload` returns as soon as the master has accepted the new configuration, while the old
/// workers finish what they are serving behind it — so this covers a master under load rather than
/// a graceful shutdown. Nothing is killed when it expires; see [`ReloadBehaviour::Command`].
const RELOAD_PATIENCE: Millis = Millis(30_000);

/// Nginx, as MixEngine runs it.
#[derive(Debug)]
pub struct Nginx;

impl Recipe for Nginx {
    fn package(&self) -> &'static str {
        PACKAGE
    }

    /// There is one nginx, for the reason there is one Caddy: a second is two processes contending
    /// for the ports every site on the machine is reached through.
    fn instancing(&self) -> Instancing {
        Instancing::Single
    }

    /// And it is the other answer to the same question, which [`Instancing`] cannot express.
    fn role(&self) -> Role {
        Role::FrontEnd
    }

    fn smoke_test(&self) -> Option<SmokeTest> {
        Some(SmokeTest {
            executable: PACKAGE.to_owned(),
            // `-v` and not `-t`: the second reads a configuration, and at the moment an archive is
            // being installed there is no service and therefore nothing rendered to read.
            args: vec!["-v".to_owned()],
        })
    }

    fn settings(&self) -> &'static [Setting] {
        &[
            Setting {
                key: CLIENT_MAX_BODY_SIZE,
                default: Preset::Text("64m"),
            },
            Setting {
                key: LOG_LEVEL,
                default: Preset::Text("error"),
            },
            Setting {
                // Thirty seconds, for Caddy's reason rather than nginx's: the server itself is up in
                // milliseconds, and what this is really waiting for is a first start on Windows with
                // Defender reading the binary.
                key: READY_TIMEOUT,
                default: Preset::Number(30_000),
            },
            Setting {
                key: STATUS_PORT,
                default: Preset::Number(2020),
            },
            Setting {
                key: HTTPS_PORT,
                default: Preset::Number(443),
            },
            Setting {
                key: STOP_GRACE,
                default: Preset::Number(10_000),
            },
            Setting {
                key: WORKER_CONNECTIONS,
                default: Preset::Number(1024),
            },
            Setting {
                // One, not `auto`. See the template: on the borrowed Windows build the extra workers
                // do nothing, and one developer's machine has nothing for them to do anywhere else.
                key: WORKER_PROCESSES,
                default: Preset::Number(1),
            },
        ]
    }

    fn files(&self) -> &'static [TemplateFile] {
        &[TemplateFile {
            path: CONFIG_FILE,
            source: include_str!("nginx/nginx.conf"),
        }]
    }

    /// Exactly `sites/`, and only because this recipe is a front end — D4.
    fn swept(&self) -> &'static [&'static str] {
        &[SITES]
    }

    /// One file per site, named after its primary domain — D12.
    ///
    /// Rendered into the set `nginx.conf`'s own `include sites/*.conf;` picks up, which resolves
    /// against the prefix — the staging directory while `nginx -t` is judging it, and `etc/nginx/`
    /// afterwards. That is why the validator passes `-p .`.
    ///
    /// # Errors
    ///
    /// [`Error::TemplateBroken`] naming the site template, and
    /// [`Error::ServiceProvidesNothing`] for an install that
    /// publishes no `fastcgi_params` — a package problem reported while rendering rather than an
    /// `include` of a file that is not there.
    fn sites(&self, context: &Context, served: &[Served]) -> Result<Vec<Document>> {
        let fastcgi_params = forward_slashed(&context.provided(FASTCGI_PARAMS)?);

        // The row's port is what a browser asks for; this is what the process must listen on. A row
        // with no port answers on 80, exactly as Caddy's own default does.
        let listen = listening(
            context.bind(),
            context.bound(context.port().unwrap_or(DEFAULT_HTTP_PORT)),
        );

        // The same function and the same mapping, a second time: `mixengine-platform` stays the only
        // thing that knows which system moves a port.
        let https_port = port(context, HTTPS_PORT)?;
        let listen_tls = listening(context.bind(), context.bound(https_port));

        served
            .iter()
            .map(|site| {
                let rendering = SiteRendering {
                    primary: site.primary(),
                    domains: &site.domains,
                    doc_root: forward_slashed(&site.doc_root),
                    kind: kind(&site.kind),
                    upstream: upstream(&site.kind),
                    activator: activator(&site.kind),
                    group: group(site.primary()),
                    fastcgi_params: &fastcgi_params,
                    listen: &listen,
                    listen_tls: &listen_tls,
                    certificate: site.certificate.as_ref().map(Certificate::from),
                    // The LAN listener binds the *bound* port, exactly as loopback's does: a
                    // machine that redirects 80 to 8080 redirects it for every address, and a
                    // listener on the number a browser types would answer nothing at all.
                    lan: site.shared.as_ref().map(|shared| {
                        listening(
                            &shared.address.to_string(),
                            context.bound(context.port().unwrap_or(DEFAULT_HTTP_PORT)),
                        )
                    }),
                    lan_tls: site.shared.as_ref().map(|shared| {
                        listening(&shared.address.to_string(), context.bound(https_port))
                    }),
                    mdns: site.shared.as_ref().and_then(|shared| shared.name.clone()),
                };

                let contents = crate::generate::served::render(
                    SITE,
                    "nginx/site.conf",
                    context.service(),
                    &rendering,
                )?;

                Ok(Document::new(
                    format!("{SITES}/{}.conf", site.primary()),
                    contents,
                ))
            })
            .collect()
    }

    /// The archive's own `mime.types`, by the absolute path the index publishes it at.
    ///
    /// Resolved here rather than joined in the template, which is what [`Endpoints`] is for: a
    /// package that publishes no `mime.types` fails while this recipe is being rendered, naming what
    /// the install does provide, instead of producing an `include` of a file that is not there.
    fn endpoints(&self, context: &Context) -> Result<Endpoints> {
        Ok(Endpoints {
            includes: BTreeMap::from([
                (MIME_TYPES.to_owned(), context.provided(MIME_TYPES)?),
                (FASTCGI_PARAMS.to_owned(), context.provided(FASTCGI_PARAMS)?),
            ]),
            ..Endpoints::default()
        })
    }

    /// `nginx -t`, pointed at the staged configuration with the staging directory as its prefix.
    ///
    /// **`-p .` and not the installed directory**, which is the whole reason this is not a copy of
    /// Caddy's: an `include` inside an nginx configuration resolves against the prefix, so a checker
    /// given the installed one would judge a staged file against the sites that are already live.
    /// The validator runs with the staging directory as its working directory, which is what `.`
    /// is — and what makes the rendering judged as a whole, includes and all.
    ///
    /// `-e stderr` so that a complaint arrives on the pipe the error is read from rather than in a
    /// `logs/error.log` under a prefix that is about to be thrown away.
    ///
    /// **[`Reason::First`], because nginx says why and then says that it failed.** A refusal is two
    /// lines — `nginx: [emerg] <what is wrong> in <file>:<line>` and then
    /// `nginx: configuration file <path> test failed` — and the second names the file the message
    /// around it already names. Reported by its last line, every nginx configuration error a person
    /// ever meets would read as *something is wrong somewhere*.
    fn validator(&self, context: &Context) -> Option<Validator> {
        Some(
            Validator::new(context.program(PACKAGE), CONFIG_FILE)
                .args(["-t", "-p", ".", "-c", CONFIG, "-e", "stderr"])
                .reason(Reason::First),
        )
    }

    fn spec(&self, context: &Context) -> Result<ServiceSpecBuilder> {
        let settings = context.settings();

        let nginx = context.program(PACKAGE);
        let prefix = context.etc().to_string_lossy().into_owned();
        let config = context.config(CONFIG_FILE).to_string_lossy().into_owned();
        let status_port = port(context, STATUS_PORT)?;
        let health = format!("http://{STATUS_HOST}:{status_port}{HEALTH_PATH}");

        // What every invocation of this binary says, whether it is the server or a signal sent to
        // one: which prefix, which configuration, and where the errors go. A signal that named a
        // different configuration would look for a pid file this instance never wrote.
        let invocation = |trailing: &[&str]| {
            let mut args = vec![
                "-p".to_owned(),
                prefix.clone(),
                "-c".to_owned(),
                config.clone(),
                "-e".to_owned(),
                "stderr".to_owned(),
            ];
            args.extend(trailing.iter().map(|arg| (*arg).to_owned()));
            args
        };

        Ok(ServiceSpec::builder(context.service().clone(), &nginx)
            .args(invocation(&[]))
            // The configuration directory, which is also the prefix: a relative path inside a site
            // — a document root somebody wrote by hand — resolves against it, and so does the
            // `include` that will bring that site in.
            .cwd(context.etc())
            // What a failed start is diagnosed against (T38), and it is the status endpoint alone:
            // nothing here listens on the port sites are served on until T43 renders one.
            .ports([status_port])
            .ready(ReadyCheck::Http {
                url: health.clone(),
                expect_status: 200,
                timeout: millis(settings.number(READY_TIMEOUT)),
            })
            .health(HealthCheck {
                probe: HealthProbe::Http {
                    url: health,
                    expect_status: 200,
                },
                interval: HEALTH_INTERVAL,
                timeout: HEALTH_TIMEOUT,
                // Three intervals rather than one, as Caddy has: a master that is reloading is
                // finishing requests on the old workers, which is a busy front end and not a sick
                // one.
                failures_before_degraded: 3,
                successes_before_running: 1,
            })
            .reload(ReloadBehaviour::Command {
                program: nginx.clone(),
                args: invocation(&["-s", "reload"]),
                patience: RELOAD_PATIENCE,
            })
            // `-s quit` and not `-s stop`: the first lets the workers finish what they are serving,
            // the second cuts every connection where it stands. A front end being stopped is a
            // developer restarting their own machine's web server, not an emergency.
            .stop(StopBehaviour::Command {
                program: nginx,
                args: invocation(&["-s", "quit"]),
                grace: millis(settings.number(STOP_GRACE)),
            }))
    }
}

/// One of this recipe's port settings, as a port.
///
/// [`caddy`](super::caddy)'s reasoning, and deliberately its own copy rather than a shared helper:
/// what makes the message useful is that it names the setting, and the two recipes have different
/// settings.
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
fn millis(number: i64) -> Millis {
    Millis(u64::try_from(number).unwrap_or_default())
}

/// One site, as `nginx/site.conf` reads it.
#[derive(Debug, serde::Serialize)]
struct SiteRendering<'a> {
    primary: &'a str,
    domains: &'a [String],
    doc_root: String,
    kind: &'static str,

    /// Empty for the kinds whose branch does not read it — `Strict` undefined behaviour means the
    /// key has to be there whichever branch is taken.
    upstream: String,

    /// The activator to fall back to, or [`None`] for a pool nothing can wake — T70.
    ///
    /// Present whichever branch is taken, for `upstream`'s reason. [`None`] renders a
    /// `fastcgi_pass` straight at the pool, which is what this file rendered before T70.
    activator: Option<String>,

    /// What the `upstream` group holding the two is called, when there is one.
    group: String,
    fastcgi_params: &'a str,
    listen: &'a str,

    /// What the TLS listener binds. **Always present**, for `upstream`'s reason: `Strict` undefined
    /// behaviour makes a missing key an error whichever branch the template takes.
    listen_tls: &'a str,

    /// [`None`] renders no TLS at all — the T51 design, D4.
    certificate: Option<Certificate>,

    /// What a shared site's second listener binds, or [`None`] for a site that is not shared —
    /// roadmap task **T74**.
    ///
    /// A second `listen` line rather than a changed one: loopback keeps working, which is what the
    /// browser on this machine is using while a phone looks at the same site.
    lan: Option<String>,

    /// The same for TLS. Present whichever branch is taken, for `upstream`'s reason.
    lan_tls: Option<String>,

    /// The mDNS name a shared site also answers to, or [`None`] — roadmap task **T75**.
    ///
    /// It joins `server_name` rather than adding a listener: a name is matched after nginx has
    /// picked a listener group, so this is the half that decides which site replies.
    mdns: Option<String>,
}

/// A certificate as the template writes it — roadmap task **T51**.
///
/// **Strings and not `Path`s**, because a template writes text: `Path`'s `Serialize` is lossy on a
/// path that is not UTF-8, and this module already forward-slashes and stringifies every path it
/// renders.
#[derive(Debug, serde::Serialize)]
struct Certificate {
    certificate: String,
    key: String,
    fingerprint: String,
}

impl From<&crate::generate::served::SiteCertificate> for Certificate {
    fn from(certificate: &crate::generate::served::SiteCertificate) -> Self {
        Self {
            certificate: forward_slashed(&certificate.certificate),
            key: forward_slashed(&certificate.key),
            fingerprint: certificate.fingerprint.clone(),
        }
    }
}

/// What a site's `listen` says: the address the row asked for, and the port the process must bind.
///
/// **The address and not the port alone**, which is nginx's own dispatch rule showing through: it
/// groups servers by listen *address* first and consults `server_name` only inside a group, so a
/// site left on the wildcard `*:8080` is unreachable beside anything that took `127.0.0.1:8080`
/// — the name is never looked at. Writing the address the row carries is also what makes LAN
/// sharing (T74) a change to one column rather than to this template.
///
/// [`SocketAddr`] does the spelling, so an IPv6 `bind_addr` arrives bracketed the way nginx needs.
/// A `bind_addr` that is not an address at all renders as the bare port — nginx's "any" — rather
/// than as an invented loopback: a front end that had quietly stopped answering on the LAN is the
/// worse of the two failures.
fn listening(bind: &str, port: u16) -> String {
    bind.parse::<IpAddr>().map_or_else(
        |_| port.to_string(),
        |address| SocketAddr::new(address, port).to_string(),
    )
}

/// A path as nginx has to read it: forward slashes, on every system.
///
/// nginx accepts `/` on Windows and its own tokeniser eats `\` inside the quotes these paths are
/// written in, so one spelling works on all three. `nginx.conf` does the same thing with a Jinja
/// filter; this is the Rust half, for the values this recipe computes.
fn forward_slashed(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Which branch of the template this kind takes.
///
/// Three and not four: a `node-app` renders as a reverse proxy to loopback and that is all it is —
/// D7.
const fn kind(kind: &ServedKind) -> &'static str {
    match kind {
        ServedKind::PhpFpm { .. } => "php-fpm",
        ServedKind::Static => "static",
        ServedKind::ReverseProxy { .. } | ServedKind::NodeApp { .. } => "proxy",
    }
}

/// The address this kind is proxied or passed to, as **nginx** spells one.
///
/// `unix:` and then the path, which is not Caddy's `unix/` — the difference is the reason
/// [`Upstream`] is a value rather than a string.
fn upstream(kind: &ServedKind) -> String {
    match kind {
        ServedKind::PhpFpm { upstream, .. } => address(upstream),
        ServedKind::ReverseProxy { upstream } => upstream.clone(),
        ServedKind::NodeApp { port } => format!("http://127.0.0.1:{port}"),
        ServedKind::Static => String::new(),
    }
}

/// The activator's address for this kind, as nginx spells one — roadmap task **T70**.
///
/// [`None`] for everything nothing can start by connecting to it, which renders the site exactly as
/// it rendered before T70.
fn activator(kind: &ServedKind) -> Option<String> {
    match kind {
        ServedKind::PhpFpm { activator, .. } => activator.as_ref().map(address),
        _ => None,
    }
}

/// One [`Upstream`] in nginx's spelling.
fn address(upstream: &Upstream) -> String {
    match upstream {
        Upstream::Socket(path) => format!("unix:{}", forward_slashed(path)),
        Upstream::Tcp(address) => address.to_string(),
    }
}

/// What to call the `upstream` group holding a site's pool and its activator — roadmap task **T70**.
///
/// **Named after the site and never after the pool.** Two sites sharing one pool are the ordinary
/// case, and nginx refuses a configuration that declares one upstream name twice — which takes the
/// whole front end down rather than the one site, since a refused configuration is a server that
/// does not start. A site's primary domain is already unique across this home, which is what
/// `sites/<primary>.conf` relies on to be one file per site.
///
/// Everything but letters, digits and `_` becomes `_`: a group name is a bare token to nginx's
/// parser, and a domain carries dots and hyphens.
fn group(primary: &str) -> String {
    let sanitised: String = primary
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect();

    format!("mixengine_{sanitised}")
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use mixengine_platform::PortBinding;
    use mixengine_proto::{HealthProbe, ReadyCheck, ReloadBehaviour, ServiceId, StopBehaviour};

    use super::*;
    use crate::generate::recipe;
    use crate::generate::served::Shared;
    use crate::generate::settings::Settings;

    /// D6: nginx's `fastcgi_params` comes out of the package rather than being written into this
    /// template by hand. `mixengine-packages`' `tools/nginx.py` publishes it under `CONF_FILES` for
    /// exactly this reason, and copying seventeen `fastcgi_param` lines in here would be this
    /// repository maintaining a second copy of a file the server already reads.
    #[test]
    fn the_package_supplies_the_fastcgi_parameters_a_php_site_needs() {
        let endpoints = Nginx
            .endpoints(&context("{}"))
            .expect("a package publishing what a generated configuration includes");

        assert!(
            endpoints.includes.contains_key("fastcgi_params"),
            "a generated nginx configuration has no conf/ beside it, so the file has to be reached \
             where the artifact keeps it"
        );
    }

    /// D8, nginx's half. Caddy takes the port from its global block; nginx has none, so each site's
    /// `server` block declares its own `listen` — and it is the port the process **binds**, which on
    /// macOS is 8080 for a front end answering on 80.
    #[test]
    fn a_site_listens_on_the_port_the_process_binds() {
        let context = context_on("{}", Some(80)).with_bindings(vec![PortBinding {
            answer: 80,
            bind: 8080,
        }]);

        let served = vec![Served {
            shared: None,
            domains: vec!["blog.test".to_owned(), "www.blog.test".to_owned()],
            doc_root: doc_root(),
            kind: ServedKind::Static,
            https: true,
            certificate: None,
        }];

        let rendered = Nginx.sites(&context, &served).expect("one site")[0]
            .contents()
            .to_owned();

        assert!(rendered.contains("listen 127.0.0.1:8080;"), "{rendered}");
        assert!(
            rendered.contains("server_name blog.test www.blog.test;"),
            "{rendered}"
        );
    }

    /// A document root on whichever system this is compiled for.
    fn doc_root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\src\blog\public")
        } else {
            PathBuf::from("/src/blog/public")
        }
    }

    /// Each kind renders the directive that kind needs, and a `node-app` renders exactly what a
    /// reverse proxy to loopback renders — D7 asserted rather than described.
    #[test]
    fn each_kind_renders_a_server_block_naming_what_it_was_given() {
        let served = vec![
            Served {
                shared: None,
                domains: vec!["php.test".to_owned()],
                doc_root: doc_root(),
                kind: ServedKind::PhpFpm {
                    upstream: Upstream::Socket(PathBuf::from("/home/me/run/php-fpm-8.3.sock")),
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

        let documents = Nginx
            .sites(&context("{}"), &served)
            .expect("three site files");

        assert_eq!(
            documents[0].relative(),
            Path::new("sites").join("php.test.conf")
        );

        let php = documents[0].contents();
        assert!(
            php.contains("fastcgi_pass unix:/home/me/run/php-fpm-8.3.sock;"),
            "a socket is spelled nginx's way, which is not Caddy's: {php}"
        );
        assert!(php.contains("include \""), "{php}");

        assert!(
            documents[1]
                .contents()
                .contains("proxy_pass http://127.0.0.1:4000;")
        );
        assert!(
            documents[2]
                .contents()
                .contains("proxy_pass http://127.0.0.1:3000;")
        );
    }

    /// One site per file, and the directory holding them is swept.
    #[test]
    fn the_front_end_sweeps_its_sites_directory_and_nothing_else() {
        assert_eq!(Nginx.swept(), &["sites"]);
    }

    /// An absolute path on whichever system this is compiled for.
    const fn root() -> &'static str {
        if cfg!(windows) {
            r"C:\MixEngine"
        } else {
            "/opt/mixengine"
        }
    }

    /// Where the binary sits inside the archive, as the index publishes it.
    fn nginx_binary() -> String {
        format!("nginx{}", std::env::consts::EXE_SUFFIX)
    }

    /// A static site at `blog.test` with a certificate — roadmap task **T51**.
    ///
    /// Paths that do not exist, deliberately: this module renders text and never reads a disk.
    /// Whether the pair is there was decided in `generate::served`.
    fn a_site_with_a_certificate() -> Served {
        Served {
            shared: None,
            domains: vec!["blog.test".to_owned()],
            doc_root: doc_root(),
            kind: ServedKind::Static,
            https: true,
            certificate: Some(crate::generate::served::SiteCertificate {
                certificate: PathBuf::from("/home/someone/.mixengine/certs/sites/blog.test.crt"),
                key: PathBuf::from("/home/someone/.mixengine/certs/sites/blog.test.key"),
                fingerprint: "ab".repeat(32),
            }),
        }
    }

    /// One site through the real recipe.
    fn render_site(site: &Served) -> String {
        Nginx
            .sites(&context("{}"), std::slice::from_ref(site))
            .expect("one site file")[0]
            .contents()
            .to_owned()
    }

    /// **nginx says in one directive what Caddy needs three for** — roadmap task **T70**, D2.
    ///
    /// `backup` in an `upstream` group *is* "only when the others have refused", so there is no
    /// policy to state and no load balancing to switch off. Measured against a real nginx 1.24.0
    /// with the pool's address dead: 200 on the first request, in 7.9 ms. What has to be right here
    /// is the shape — a group, the pool plain, the activator marked `backup`, and `fastcgi_pass`
    /// pointing at the group rather than at either address.
    #[test]
    fn a_pool_that_can_be_woken_renders_a_group_whose_second_server_is_a_backup() {
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
            rendered.contains("server 127.0.0.1:9000;"),
            "the pool is the ordinary member of the group:\n{rendered}"
        );
        assert!(
            rendered.contains("server 127.0.0.1:9500 backup;"),
            "the activator is reached only once the pool has refused:\n{rendered}"
        );
        assert!(
            !rendered.contains("fastcgi_pass 127.0.0.1:9000;"),
            "passing straight to the pool bypasses the group that makes the fallback work:\n\
             {rendered}"
        );
    }

    /// **Two sites sharing one pool must not declare one `upstream` name twice.**
    ///
    /// nginx refuses a configuration with a duplicate upstream name outright, so the group is named
    /// after the *site* and not after the pool — and this is what would catch a name derived from
    /// the pool instead. The whole front end fails to start when it is wrong, not the one site.
    #[test]
    fn two_sites_on_one_pool_declare_two_differently_named_groups() {
        let pool = Upstream::Tcp("127.0.0.1:9000".parse().expect("an address"));
        let activator = Some(Upstream::Tcp("127.0.0.1:9500".parse().expect("an address")));

        let served: Vec<Served> = ["one.test", "two.test"]
            .into_iter()
            .map(|domain| Served {
                shared: None,
                domains: vec![domain.to_owned()],
                doc_root: doc_root(),
                kind: ServedKind::PhpFpm {
                    upstream: pool.clone(),
                    activator: activator.clone(),
                },
                https: false,
                certificate: None,
            })
            .collect();

        let documents = Nginx
            .sites(&context("{}"), &served)
            .expect("two site files");

        let names: Vec<String> = documents
            .iter()
            .map(|document| {
                // The *directive* and not the word: this file explains itself in prose that says
                // "upstream" too, and a search that found the comment would compare two identical
                // sentences and pass whatever the names were.
                document
                    .contents()
                    .lines()
                    .find(|line| line.starts_with("upstream "))
                    .expect("a group at the top level")
                    .trim_end_matches(" {")
                    .to_owned()
            })
            .collect();

        assert_ne!(
            names[0], names[1],
            "nginx refuses a duplicate upstream name and the whole front end fails to start"
        );
    }

    /// A pool with no activator renders what it rendered before T70: a `fastcgi_pass` straight at
    /// the pool, and no group at all.
    #[test]
    fn a_pool_with_no_activator_is_passed_to_directly() {
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
            rendered.contains("fastcgi_pass 127.0.0.1:9000;"),
            "{rendered}"
        );
        assert!(
            !rendered.contains("upstream "),
            "a group of one is a group for nothing:\n{rendered}"
        );
    }

    /// A site shared on the LAN, without a certificate — roadmap task **T74**.
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

    /// **A second `listen`, not a changed one** — the T74 design, D2. The browser on this machine
    /// and the phone are looking at the same site at the same time, so loopback stays.
    #[test]
    fn a_shared_site_listens_on_loopback_and_the_lan_address() {
        let rendered = render_site(&a_shared_site([192, 168, 1, 10]));

        assert!(rendered.contains("listen 127.0.0.1:80;"), "{rendered}");
        assert!(rendered.contains("listen 192.168.1.10:80;"), "{rendered}");
        assert_eq!(
            rendered
                .matches(
                    "
    listen "
                )
                .count(),
            2,
            "{rendered}"
        );
    }

    /// **The name joins `server_name`** — the T75 design, D3. nginx picks a listener group by
    /// address and only then consults the names, so this is the half that decides which site
    /// replies to `blog-mixengine.local` once the responder has said where it resolves.
    #[test]
    fn a_shared_site_names_itself_in_server_name() {
        let rendered = render_site(&a_shared_site([192, 168, 1, 10]));

        assert!(
            rendered.contains("server_name blog.test blog-mixengine.local;"),
            "{rendered}"
        );
    }

    /// **An unshared site carries no name at all**, which is what makes "opt-in per site" a
    /// property of the rendering rather than a promise made about it.
    #[test]
    fn an_unshared_site_carries_no_mdns_name() {
        let rendered = render_site(&Served {
            shared: None,
            domains: vec!["shop.test".to_owned()],
            doc_root: doc_root(),
            kind: ServedKind::Static,
            https: false,
            certificate: None,
        });

        // The directive rather than the whole file: the comment above it names the shape a
        // shared site's name takes, and a comment is not a name this site answers to.
        assert!(rendered.contains("server_name shop.test;"), "{rendered}");
    }

    /// **Two sites shared on one address** — the assumption T74 recorded and could not yet reach.
    ///
    /// nginx groups servers by listen address before it consults `server_name`, so a request
    /// carrying `Host: <ip>` is answered by that group's *default* — the first server block on the
    /// address — while each site's own mDNS name is matched by name. Asserted so that a later
    /// change cannot quietly move which site an address-shaped `Host` reaches.
    #[test]
    fn two_sites_shared_on_one_address_are_told_apart_by_name() {
        let shop = Served {
            shared: Some(Shared {
                address: [192, 168, 1, 10].into(),
                name: Some("shop-mixengine.local".to_owned()),
            }),
            domains: vec!["shop.test".to_owned()],
            doc_root: doc_root(),
            kind: ServedKind::Static,
            https: false,
            certificate: None,
        };

        let rendered = Nginx
            .sites(&context("{}"), &[a_shared_site([192, 168, 1, 10]), shop])
            .expect("two site files");

        assert!(
            rendered[0].contents().contains("blog-mixengine.local"),
            "{}",
            rendered[0].contents()
        );
        assert!(
            rendered[1].contents().contains("shop-mixengine.local"),
            "{}",
            rendered[1].contents()
        );

        // Both on the one address, which is what makes the group's default the question it is.
        assert_eq!(
            rendered
                .iter()
                .filter(|document| document.contents().contains("listen 192.168.1.10:80;"))
                .count(),
            2
        );
    }

    /// Sharing is opt-in per site: the site beside it is untouched.
    #[test]
    fn an_unshared_site_listens_once_per_scheme() {
        let rendered = render_site(&Served {
            shared: None,
            ..a_shared_site([192, 168, 1, 10])
        });

        assert_eq!(
            rendered
                .matches(
                    "
    listen "
                )
                .count(),
            1,
            "{rendered}"
        );
        assert!(!rendered.contains("192.168.1.10"), "{rendered}");
    }

    /// A shared HTTPS site listens on the LAN address over TLS as well, because the certificate
    /// covers that address as an IP SAN — T74, D9.
    #[test]
    fn a_shared_https_site_offers_tls_on_the_lan_address_too() {
        let rendered = render_site(&Served {
            shared: Some(Shared {
                address: [192, 168, 1, 10].into(),
                name: Some("blog-mixengine.local".to_owned()),
            }),
            ..a_site_with_a_certificate()
        });

        assert_eq!(
            rendered
                .matches(
                    "
    listen "
                )
                .count(),
            4,
            "{rendered}"
        );
        assert_eq!(
            rendered.matches("192.168.1.10").count(),
            2,
            "one plaintext listener and one TLS one:
{rendered}"
        );
    }

    /// **The consequence of D2 and D3, asserted rather than discovered.**
    ///
    /// nginx groups servers by listen address before it consults `server_name`, so the LAN address
    /// has exactly one server block in its group: a request arriving from the network with another
    /// site's `Host` is answered by the shared site as that group's default, not by the site it
    /// named. That is the intended outcome — no unshared site is served over the LAN — and this
    /// test exists so a later change cannot quietly turn it into the other one.
    #[test]
    fn the_lan_address_belongs_to_the_shared_site_alone() {
        let shared = a_shared_site([192, 168, 1, 10]);
        let other = Served {
            shared: None,
            domains: vec!["shop.test".to_owned()],
            ..a_shared_site([192, 168, 1, 10])
        };

        let documents = Nginx
            .sites(&context("{}"), &[shared, other])
            .expect("two site files");

        // One file per site, so the question is which files carry the address rather than which
        // blocks do — and the answer has to be exactly the shared one.
        let carrying: Vec<&str> = documents
            .iter()
            .map(|document| document.contents())
            .filter(|contents| contents.contains("192.168.1.10"))
            .collect();

        assert_eq!(carrying.len(), 1, "{documents:?}");
        assert!(carrying[0].contains("blog.test"), "{}", carrying[0]);
        assert!(!carrying[0].contains("shop.test"), "{}", carrying[0]);
    }

    /// An HTTPS site listens twice and names its certificate — roadmap task **T51**.
    ///
    /// **One `server` block and not two, unlike Caddy**, and that was measured rather than assumed:
    /// nginx 1.24 answers `syntax is ok` to a block carrying a plaintext listener and a TLS one,
    /// where Caddy refuses the equivalent outright. TLS attaches to a listener here and to a site
    /// block there.
    #[test]
    fn an_https_site_listens_on_tls_and_names_its_certificate() {
        let rendered = render_site(&a_site_with_a_certificate());

        assert_eq!(rendered.matches("\n    listen ").count(), 2, "{rendered}");
        assert!(rendered.contains(" ssl;"), "{rendered}");
        assert!(
            rendered.contains(
                "ssl_certificate \"/home/someone/.mixengine/certs/sites/blog.test.crt\";"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "ssl_certificate_key \"/home/someone/.mixengine/certs/sites/blog.test.key\";"
            ),
            "{rendered}"
        );
        assert_eq!(rendered.matches("server {").count(), 1, "{rendered}");
    }

    /// A site with no certificate listens once and names none — the T51 design, D4.
    #[test]
    fn a_site_with_no_certificate_has_one_listener() {
        let mut site = a_site_with_a_certificate();
        site.certificate = None;

        let rendered = render_site(&site);

        assert_eq!(rendered.matches("\n    listen ").count(), 1, "{rendered}");
        assert!(!rendered.contains("ssl"), "{rendered}");
    }

    /// **The fingerprint is what makes a reissue reload** — the T51 design, D5.
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

    /// And nothing changing renders identical bytes — a rendering that had become unstable would
    /// reload the front end on every unrelated `service.*` call.
    #[test]
    fn rendering_twice_with_nothing_changed_is_identical() {
        let site = a_site_with_a_certificate();

        assert_eq!(render_site(&site), render_site(&site));
    }
    /// An nginx on port 80 in a home at [`root`], with `overrides` applied.
    ///
    /// The root is a plain string rather than a temporary directory, for [`super::super::caddy`]'s
    /// reason: nothing here writes a file, and what the assertions are about is the *text* a path
    /// becomes. On Windows that text contains backslashes, which is the subject of one of these.
    fn context(overrides: &str) -> Context {
        context_on(overrides, Some(80))
    }

    /// The same, with the port the row carries spelled out — what D8's half of this recipe is about.
    fn context_on(overrides: &str, port: Option<u16>) -> Context {
        let service = ServiceId::parse("nginx").expect("an id");
        let settings =
            Settings::merge(Nginx.settings(), overrides, &service).expect("usable overrides");

        let context = Context::for_test(
            service,
            PACKAGE,
            Path::new(root()),
            // What `mixengine-packages` publishes: the server, and the data files a generated
            // configuration includes. `mime.types` is the one nothing works without.
            [
                ("nginx".to_owned(), nginx_binary()),
                (MIME_TYPES.to_owned(), "conf/mime.types".to_owned()),
                (FASTCGI_PARAMS.to_owned(), "conf/fastcgi_params".to_owned()),
            ]
            .into_iter()
            .collect(),
            port,
            settings,
        );

        // What `Generator::render` does before it renders anything, and this template needs it: the
        // `include` of the archive's own `mime.types` is resolved here rather than joined in the
        // file.
        let endpoints = Nginx
            .endpoints(&context)
            .expect("a package publishing what a generated configuration includes");

        context.with_endpoints(endpoints)
    }

    /// What the file renders to, for `overrides`.
    fn conf(overrides: &str) -> String {
        let documents = recipe::render(&Nginx, &context(overrides)).expect("a rendering");

        assert_eq!(documents.len(), 1, "nginx renders one file");
        assert_eq!(documents[0].relative(), Path::new(CONFIG_FILE));

        documents[0].contents().to_owned()
    }

    /// The spec this recipe builds for `overrides`.
    fn spec(overrides: &str) -> ServiceSpec {
        Nginx
            .spec(&context(overrides))
            .expect("a builder")
            .build()
            .expect("a usable spec")
    }

    /// There is one nginx, which is what stops `service.create` being asked for a second one.
    ///
    /// The same answer as Caddy's and for the same sentence in `.claude/features/services.md`:
    /// exactly one active front end. What stops a *Caddy* being created beside this one is
    /// [`Recipe::role`], which is a different rule about a different mistake.
    #[test]
    fn nginx_exists_once() {
        assert_eq!(Nginx.instancing(), Instancing::Single);
    }

    /// And it is a front end, which is the half `service.create` reads.
    #[test]
    fn nginx_is_a_front_end() {
        assert_eq!(Nginx.role(), Role::FrontEnd);
    }

    /// What a failed start is diagnosed against — roadmap task **T38**.
    ///
    /// **The status endpoint alone.** The row's own port is what sites will be served on, and this
    /// recipe writes no listener for it until sites exist (T43) — so an nginx that failed to start
    /// never wanted 80, and declaring it would put another program's IIS into the reason for a
    /// failure that was not about it.
    #[test]
    fn the_spec_declares_the_status_endpoint_it_will_bind() {
        assert_eq!(spec("{}").ports(), [2020]);
    }

    /// An artifact that unpacks and will not run is one the user meets against their own site.
    ///
    /// `-v` and not `-t`: the second reads a configuration, and there is none to read at the moment
    /// an archive is being installed.
    #[test]
    fn nginx_proves_itself_by_running() {
        let smoke = Nginx.smoke_test().expect("a server proves that it runs");

        assert_eq!(smoke.executable, PACKAGE);
        assert_eq!(smoke.args, ["-v"]);
    }

    #[test]
    fn the_rendering_says_what_the_row_and_the_defaults_say() {
        let rendered = conf("{}");

        assert!(rendered.contains("worker_processes 1;"), "{rendered}");
        assert!(rendered.contains("worker_connections 1024;"), "{rendered}");
        assert!(
            rendered.contains("listen 127.0.0.1:2020;"),
            "the status endpoint is not in the file: {rendered}"
        );
        assert!(rendered.contains(HEALTH_PATH), "{rendered}");
        assert!(rendered.contains("client_max_body_size 64m;"), "{rendered}");
        assert!(rendered.contains("include sites/*.conf;"), "{rendered}");

        // The one data file nothing works without, reached by the absolute path the index publishes
        // it at rather than through nginx's own `conf/` — which a generated configuration has none
        // of. Asserted as the whole path, because "contains mime.types" would also be true of a
        // template that joined one itself and got the layout wrong.
        let published = context("{}")
            .provided(MIME_TYPES)
            .expect("a published mime.types")
            .to_string_lossy()
            .replace('\\', "/");
        assert!(
            rendered.contains(&format!("include \"{published}\";")),
            "{rendered}"
        );
    }

    /// **In the foreground, with its errors on the stream.**
    ///
    /// `daemon off;` is nginx's spelling of the decision `caddy run` is Caddy's: the default forks a
    /// master and returns, so what a supervisor would be watching is a launcher that has already
    /// exited. `-e stderr` is the other half — an error *before* the configuration has been read
    /// goes to the compiled-in `logs/error.log` under the prefix otherwise, which is a file nobody
    /// is reading.
    #[test]
    fn the_program_stays_in_the_foreground_and_says_so_on_the_stream() {
        let rendered = conf("{}");
        assert!(rendered.contains("daemon off;"), "{rendered}");

        let spec = spec("{}");
        let args = spec.args().join(" ");

        assert!(args.contains("-e stderr"), "{args}");
        assert!(args.contains("-c "), "{args}");
        assert!(args.contains("-p "), "{args}");
    }

    /// **Every path this file writes is forward-slashed and quoted**, which is MariaDB's finding in
    /// nginx's spelling: `ngx_conf_read_token` treats `\` inside a quoted string as an escape, so a
    /// home under `C:\Users\Nguyen Hai Quang` loses every separator — and unquoted, the directive
    /// stops at the space instead. nginx accepts `/` on Windows, so one spelling works on all three
    /// systems.
    #[test]
    fn every_path_in_the_rendering_is_written_the_way_a_windows_path_survives() {
        let rendered = conf("{}");
        let home = root().replace('\\', "/");

        assert!(
            !rendered.contains(root()) || !cfg!(windows),
            "a path reached nginx.conf with backslashes in it: {rendered}"
        );

        for line in rendered.lines().filter(|line| line.contains(&home)) {
            assert!(
                line.matches('"').count() == 2,
                "a path reached nginx.conf outside quotes: {line}"
            );
        }
    }

    /// The five temp directories nginx makes for itself are children of one that already exists.
    ///
    /// The finding is `mixengine-packages`' and it is the reason this is asserted rather than
    /// assumed: nginx creates `client_body_temp` and the four beside it with a **single** `mkdir`,
    /// so a missing parent is `[emerg] CreateDirectory() failed (3)` on a configuration that passed
    /// `nginx -t` one line earlier. The data directory is made by `Generator::render` (T35); putting
    /// the five leaves directly inside it is what makes a `temp/` nobody creates unnecessary.
    #[test]
    fn every_temp_directory_nginx_makes_has_a_parent_that_already_exists() {
        let context = context("{}");
        let data = context.data().to_string_lossy().replace('\\', "/");
        let rendered = conf("{}");

        for directive in [
            "client_body_temp_path",
            "proxy_temp_path",
            "fastcgi_temp_path",
            "scgi_temp_path",
            "uwsgi_temp_path",
        ] {
            let line = rendered
                .lines()
                .find(|line| line.trim_start().starts_with(directive))
                .unwrap_or_else(|| panic!("{directive} is not in the rendering: {rendered}"));

            let path = line
                .split('"')
                .nth(1)
                .unwrap_or_else(|| panic!("{directive} names no quoted path: {line}"));

            assert_eq!(
                path.rsplit_once('/').map(|(parent, _)| parent),
                Some(data.as_str()),
                "{directive} is not a child of the data directory, which is the one that exists"
            );
        }
    }

    /// The status endpoint is one value read by four things — the file, the readiness check, the
    /// health probe and what a failed start is diagnosed against — so an override that moved it and
    /// left one behind would be a service that starts and is never reported up.
    #[test]
    fn an_override_moves_the_status_endpoint_everywhere_it_is_named() {
        let moved = r#"{"status_port": 2121}"#;
        let rendered = conf(moved);

        assert!(rendered.contains("listen 127.0.0.1:2121;"), "{rendered}");

        let spec = spec(moved);

        assert!(
            matches!(spec.ready(), ReadyCheck::Http { url, .. } if url.contains("127.0.0.1:2121")),
            "{:?}",
            spec.ready()
        );
        assert!(matches!(
            spec.health().map(|health| &health.probe),
            Some(HealthProbe::Http { url, .. }) if url.contains("127.0.0.1:2121")
        ));
        assert_eq!(spec.ports(), [2121]);
    }

    /// A reload is `-s reload` and a stop is `-s quit`, both through the same configuration the
    /// server was started with — which is how either one finds the pid file this instance wrote.
    #[test]
    fn a_reload_and_a_stop_are_sent_through_this_instances_own_configuration() {
        let spec = spec("{}");
        let config = context("{}").config(CONFIG_FILE).display().to_string();

        let Some(ReloadBehaviour::Command { args, .. }) = spec.reload() else {
            panic!("a reload that is not a command: {:?}", spec.reload());
        };
        assert!(args.contains(&"reload".to_owned()), "{args:?}");
        assert!(args.contains(&config), "{args:?}");

        let StopBehaviour::Command { args, .. } = spec.stop() else {
            panic!("a stop that is not a command: {:?}", spec.stop());
        };
        assert!(args.contains(&"quit".to_owned()), "{args:?}");
        assert!(args.contains(&config), "{args:?}");
    }

    /// **Nothing is served on the row's own port yet**, and that is T43's to add rather than a gap.
    ///
    /// A front end that bound 80 here would need the port grant T42 has not built on macOS and
    /// Linux, and would be serving nothing on it. Caddy says the same thing by writing `http_port`
    /// into a global block and binding nothing until a site asks it to.
    #[test]
    fn the_rendering_listens_on_nothing_a_site_would_be_reached_on() {
        let rendered = conf("{}");

        assert!(
            !rendered.contains("listen 80"),
            "a front end with no sites is listening on the port sites are served on: {rendered}"
        );
    }

    /// A whole number is what the merge guarantees and a port is what the recipe needs.
    #[test]
    fn a_number_that_is_not_a_port_is_refused_against_the_setting_that_holds_it() {
        for offered in ["70000", "0", "-1"] {
            let error = Nginx
                .spec(&context(&format!(r#"{{"status_port": {offered}}}"#)))
                .expect_err("a number that is not a port");

            let message = error.to_string();
            assert!(message.contains("status_port"), "{message}");
            assert!(message.contains(offered), "{message}");
        }
    }
}
