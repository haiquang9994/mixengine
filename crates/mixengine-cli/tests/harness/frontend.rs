//! The arc a front end has to walk, whichever program is being asked to be one — roadmap task
//! **T37**.
//!
//! `.claude/roadmap/phase-3-services.md` asks T37 for "a parity test suite running both generators",
//! and this is the parity: one sequence of assertions, driven twice. What each front end supplies is
//! a [`FrontEnd`] — where its archive is, what its overrides are called, which line in the rendering
//! carries its control port — and everything a *user* meets is here, once.
//!
//! **Why it is one sequence and not two files that look alike.** The claim T31 made for Caddy and
//! T37 makes for nginx is the same sentence: a row becomes a configuration the server itself
//! accepts, the server comes up, an edited override is *served* by the same process a moment later,
//! a broken one is refused with the last good configuration still live, and a stop ends it. Two
//! copies of that would drift — and the copy that drifted would be the one nobody was reading on the
//! day it mattered, because both suites would still be green.
//!
//! What is deliberately **not** shared is how each server answers its own readiness: Caddy has an
//! admin endpoint and nginx has a `server` block MixEngine renders for the purpose. Those live in
//! the recipes, and each suite asserts its own in its own file.

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use mixengine_testkit::{FakePackage, Packed, Packing};
use serde_json::Value;

use super::{Home, json};

/// How long the server is given to be serving something new after a reload.
///
/// Long for what it covers, because what it is really waiting for is a runner's next turn plus a
/// reload on a machine that may be compiling something else at the same time.
const EVENTUALLY: Duration = Duration::from_secs(30);

/// One of the two programs a site can be reached through, as this suite has to know it.
pub(crate) struct FrontEnd {
    /// The `packages.name`, which is also the service id: a front end exists once and its id carries
    /// no `@`.
    pub package: &'static str,

    /// The environment variable naming the directory an unpacked archive is in, as the CI step and a
    /// developer both set it.
    pub variable: &'static str,

    /// The version the index publishes this as, and the one `mix service create` names.
    ///
    /// Nothing compares it against what the binary reports — a recipe is found by `packages.name` —
    /// but an index entry has to say something.
    pub version: &'static str,

    /// The file the recipe renders, under `etc/<service-id>/`.
    pub config: &'static str,

    /// How much of the unpacked archive the fixture has to carry. See [`Archive`].
    pub archive: Archive,

    /// Data files out of the archive that the generated configuration reaches by absolute path,
    /// as `provides` name and the relative path the index publishes for it.
    ///
    /// Empty for Caddy, which includes nothing; two for nginx — `mime.types`, which every rendering
    /// includes, and `fastcgi_params`, which a php-fpm site's `location ~ \.php$` does.
    pub data_files: &'static [(&'static str, &'static str)],

    /// The whole overrides document for a server whose control port is `control` and which is
    /// serving nothing.
    pub alone: fn(control: u16) -> String,

    /// The same, with a site pasted into the free-form override: `says` on `site`.
    pub serving: fn(control: u16, site: u16, says: &str) -> String,

    /// An overrides document this server's own checker has to refuse.
    pub broken: fn(control: u16) -> String,

    /// The line the rendering carries when the control port is `control`, which is how a test says
    /// "the file on disk is the one this row asked for" in each program's own spelling.
    pub control_line: fn(control: u16) -> String,

    /// What this server answers `200` on, and the one thing about a front end that is genuinely not
    /// shared: Caddy has an admin endpoint of its own, and nginx has a `server` block MixEngine
    /// renders precisely because it has none. Both are what the recipe's readiness check asks, and
    /// asking it here is what says the endpoint a *person* was told about is the endpoint that
    /// answers.
    pub control_path: &'static str,
}

/// How much of an unpacked archive a fixture has to carry.
pub(crate) enum Archive {
    /// One executable at the root and nothing else — how `mixengine-packages` publishes Caddy.
    OneProgram,

    /// The whole tree, because the generated configuration reaches a data file inside it — nginx,
    /// whose `conf/mime.types` every `Content-Type` it serves comes out of.
    WholeTree,
}

impl FrontEnd {
    /// Where an unpacked copy of this server is, or the reason there is none.
    ///
    /// **A panic and not a skip.** A test that quietly returns when it cannot find a server is a
    /// green suite that proved nothing on the day the download broke; the suites that call this are
    /// `#[ignore]`d, which is *visibly* not run, and CI fetches a real archive on all three systems.
    pub(crate) fn package_directory(&self) -> PathBuf {
        let directory = std::env::var_os(self.variable).unwrap_or_else(|| {
            panic!(
                "{} is not set, so there is no {} to judge this recipe against. The `{}` step in \
                 .github/workflows/ci.yml fetches one; by hand, unpack any {} and point {} at the \
                 directory holding the binary.",
                self.variable, self.package, self.package, self.package, self.variable
            )
        });

        let directory = PathBuf::from(directory);
        let binary = directory.join(self.binary());

        assert!(
            binary.is_file(),
            "{} is {}, which holds no {} binary",
            self.variable,
            directory.display(),
            self.package
        );

        directory
    }

    /// What the executable inside the archive is called on this system.
    pub(crate) fn binary(&self) -> String {
        format!("{}{}", self.package, std::env::consts::EXE_SUFFIX)
    }

    /// The real server, packed the way the index publishes it.
    pub(crate) fn pack(&self) -> Packed {
        let packing = match cfg!(windows) {
            true => Packing::Zip,
            false => Packing::TarZst,
        };

        let directory = self.package_directory();
        let stem = format!("{}-{}", self.package, self.version);

        match self.archive {
            Archive::OneProgram => FakePackage::new(packing)
                .program(&self.binary(), &directory.join(self.binary()))
                .build(&stem),
            Archive::WholeTree => FakePackage::new(packing).directory(&directory).build(&stem),
        }
    }

    /// An index offering exactly this server, for this machine.
    fn index(&self, packed: &Packed, url: &str) -> Value {
        let mut provides = serde_json::Map::new();
        provides.insert(self.package.to_owned(), Value::String(self.binary()));

        for (name, relative) in self.data_files {
            provides.insert((*name).to_owned(), Value::String((*relative).to_owned()));
        }

        serde_json::json!({
            "schema": 1,
            "generated_at": "2026-08-21T06:55:12Z",
            "packages": [{
                "kind": self.package,
                "version": self.version,
                "channel": "stable",
                "artifacts": [{
                    "os": std::env::consts::OS,
                    "arch": std::env::consts::ARCH,
                    "url": url,
                    "sha256": packed.sha256,
                    "size": packed.size(),
                    "provides": provides,
                }],
            }],
        })
    }
}

/// How many numbers are tried before this gives up on the machine rather than on the port.
///
/// **Five hundred and twelve, and the size is the finding.** Windows hands out ephemeral ports
/// *sequentially*, and its UDP exclusion ranges are blocks of a hundred that sit next to one
/// another — `49667–49766`, `49767–49866` and `49867–49966` are three in a row, three hundred
/// consecutive numbers. So a retry is not an independent draw: entering such a block means every
/// following candidate is inside it too, and a small budget fails on all of them. The first draft of
/// this loop tried twenty-five and panicked on a machine sitting at 65319, in the middle of
/// `65285–65484`.
///
/// What the budget therefore has to clear is the longest run of adjacent blocks, not the odd bad
/// port. Twice the longest run measured, so the loop walks out of the block and keeps going.
const CANDIDATES: usize = 512;

/// A port nothing is listening on, by listening on it and then not.
///
/// The usual race is the usual price: between the drop and the server's bind, another process on the
/// machine could take it. Nothing better exists — the alternative is a fixed port, which two runs of
/// this suite on one machine would fight over.
///
/// **A TCP bind alone is not enough to hand the number to a front end**, which is what this used to
/// do. From T51 a site with a certificate makes Caddy open an **HTTP/3 listener**, which is UDP on
/// the TLS port — and Windows keeps port exclusion ranges *per protocol*, so a number TCP is welcome
/// on can refuse UDP outright with `An attempt was made to access a socket in a way forbidden by its
/// access permissions`. A server that cannot bind one of its listeners refuses its whole
/// configuration, so what that looked like from out here was Caddy in a crash loop and
/// `system (windows-latest)` red in `caddy.rs` — twice, hours apart, on branches that had not
/// touched a front end.
///
/// So the number is proved for **both** protocols before it is handed out, and the TCP listener is
/// held while the UDP half is tried: dropping it first would open a window for another process to
/// take the number between the two checks.
/// What a service wrote to its own log, or an empty string.
///
/// **`daemon.log` is not where a front end says what it did.** Output travels on its own stream and
/// into `logs/services/<id>/current.log`, per ADR 0009.
pub(crate) fn service_log(home: &Home, service: &str) -> String {
    std::fs::read_to_string(
        home.path()
            .join("logs")
            .join("services")
            .join(service)
            .join("current.log"),
    )
    .unwrap_or_default()
}

pub(crate) fn free_port() -> u16 {
    for _ in 0..CANDIDATES {
        let held = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = held.local_addr().expect("the port it was given").port();

        if takes_udp(port) {
            return port;
        }
    }

    panic!(
        "this machine refused the udp half of {CANDIDATES} ports in a row. On Windows that is \
         `netsh interface ipv4 show excludedportrange udp` covering most of the ephemeral range — \
         a reboot releases the dynamic ones."
    );
}

/// Whether this port can be had for UDP at all.
///
/// **Loopback and not every interface, deliberately, and it costs nothing.** An exclusion range
/// refuses `127.0.0.1:49424` exactly as it refuses `0.0.0.0:49424` — measured, both with the same
/// message — so binding the wildcard buys no accuracy. What it does buy is a **Windows Firewall
/// prompt per test binary**, since this harness is compiled into every suite in the crate and each
/// one is a different executable listening on the network. The first draft did that and produced a
/// row of dialogs on a developer's screen.
fn takes_udp(port: u16) -> bool {
    std::net::UdpSocket::bind(("127.0.0.1", port)).is_ok()
}

/// **A port a front end could not take is not one to hand it.**
///
/// The deterministic half of this fix: hold the UDP side of a port, and the check must refuse it
/// even though the TCP side is completely free. That is exactly the shape the CI failure had — TCP
/// welcome, UDP not — arrived at by holding the port rather than by hoping the machine has one.
#[test]
fn a_port_whose_udp_half_is_taken_is_refused() {
    // **The port comes from the socket that holds it, and never from `free_port`.** An earlier
    // version asked `free_port` for a number and then bound its udp half, on the reasoning that a
    // number just proved free on both protocols is the cleanest starting point. It is not a
    // starting point at all: `free_port` answers by binding and then *letting go*, so between the
    // number coming back and the bind below, any of the dozen other test binaries in this crate can
    // take it — and this test failed on `bench`-free ubuntu with `AddrInUse` for exactly that
    // reason. The tcp half that reasoning was protecting is not looked at here anyway: what is
    // asserted is `takes_udp`, one protocol, on a port this test never stops holding.
    let _held = std::net::UdpSocket::bind(("127.0.0.1", 0)).expect("a loopback udp port");
    let port = _held.local_addr().expect("the port it was given").port();

    assert!(
        !takes_udp(port),
        "port {port} has its udp half held and was still offered to a front end"
    );
}

// **There was a second test here, and removing it is the point.** It took twenty-five ports from
// `free_port` and asserted each still took a udp bind — and since it ran in the same binary as the
// test above, which deliberately holds the udp half of a port it was just handed, the two raced:
// whichever ran second could be handed the number the first was holding, in the window between
// `free_port` proving it free and the assertion asking again. Green on Windows, red on Linux and
// macOS, and flaky by construction — the exact thing this whole change exists to stop the harness
// producing. What it was meant to catch, `free_port` losing its check, is six lines above and reads
// plainly enough.

/// `GET /` on a loopback port, as raw as it can be, and whatever came back.
///
/// A hand-written request rather than an HTTP client, because what is being asked is small enough
/// that a client would be the bigger thing to get wrong: one connection, one request, read until the
/// server closes. `Connection: close` is what makes the read terminate.
///
/// **The `Host` header has to be the site's own address.** A Caddyfile block written as
/// `http://127.0.0.1:8080` matches on that host, so a request carrying any other one reaches a
/// listening Caddy and is answered `404` — which reads as a reload that did not happen and is a
/// header that did not match.
pub(crate) fn get(port: u16) -> Option<String> {
    request(port, "/")
}

/// The same, for a path that is not the root — the control endpoint each front end answers on.
pub(crate) fn request(port: u16, path: &str) -> Option<String> {
    request_as(port, path, &format!("127.0.0.1:{port}"))
}

/// The same again, addressed to a name rather than to the loopback address.
///
/// **The `Host` header is what a site is matched on**, and it is what T43's own steps need: a site
/// declared as `blog.test` is a Caddyfile block written `http://blog.test` and an nginx
/// `server_name blog.test`, and a request carrying any other host reaches a listening server and is
/// answered `404` — which reads as a reload that did not happen and is a header that did not match.
///
/// **The header rather than the name.** CI has no elevation, so no hosts entry exists, and what this
/// suite is for is proving that the rendering is right and the server is reading it — not that a
/// name resolves. Resolution is T44 and T45's, and has its own suites.
pub(crate) fn request_as(port: u16, path: &str, host: &str) -> Option<String> {
    request_at(std::net::Ipv4Addr::LOCALHOST, port, path, host)
}

/// The same again, at an address that is not loopback — roadmap task **T76**.
///
/// What a shared site is for: the request arrives at the machine's LAN address, carrying that
/// address as its `Host`, which is what a phone handed a URL actually sends.
pub(crate) fn request_at(
    address: std::net::Ipv4Addr,
    port: u16,
    path: &str,
    host: &str,
) -> Option<String> {
    let mut stream = TcpStream::connect((address, port)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a read deadline");

    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .ok()?;

    let mut answer = String::new();
    stream.read_to_string(&mut answer).ok()?;

    Some(answer)
}

/// Caddy, as the suites that need a real front end have to know it.
///
/// **Here rather than in `tests/caddy.rs`** since T72: the idle-footprint budget needs a home
/// with a real Caddy running in it and nothing else, which is this const and `declared` and
/// nothing more. A second copy of it would be a second answer to what version the index
/// publishes and what file the recipe renders.
pub(crate) const CADDY: FrontEnd = FrontEnd {
    package: "caddy",
    // Where an unpacked Caddy is, as the CI step and a developer both set it: the directory holding
    // the binary, since `mixengine-packages` publishes Caddy as one executable with nothing around
    // it. That is also what a `packages` row's `install_path` is.
    variable: "MIXENGINE_CADDY_PACKAGE",
    version: "2.x",
    config: "Caddyfile",
    archive: Archive::OneProgram,
    // A Caddyfile includes nothing out of its own archive.
    data_files: &[],
    alone: |admin| overrides(admin, None),
    serving: |admin, port, says| {
        overrides(
            admin,
            Some(format!(
                "http://127.0.0.1:{port} {{\n\trespond \"{says}\"\n}}\n"
            )),
        )
    },
    broken: |admin| overrides(admin, Some("this is not a Caddyfile {".to_owned())),
    control_line: |admin| format!("admin 127.0.0.1:{admin}"),
    // Caddy's own admin endpoint: `GET /config/` answers `200` with the running configuration, which
    // is a stronger statement than a TCP accept and is what the recipe's readiness check asks.
    control_path: "/config/",
};

/// **A free TLS port, and not the 443 the preset carries** — roadmap task T51.
///
/// From T51 a front end actually binds `https_port`, because a site with a certificate renders a TLS
/// listener. These suites run a real server as an unprivileged user, where 443 is refused — and both
/// servers reject the *whole* configuration over one listener they cannot bind, so the failure is
/// not "no HTTPS" but "the reload was refused and the old configuration is still running". The HTTP
/// port was already a free one for the same reason; this is its other half.
fn free_tls_port() -> u16 {
    free_port()
}
/// The whole overrides document for a Caddy on `admin`, with `extra` pasted in if there is any.
///
/// **The whole document and not a patch**, which is what `config_overrides_json` is: a setting that
/// is not in it is not set. So every override this suite writes repeats the admin port, and one that
/// forgot would move the endpoint back to Caddy's default under a server listening on the one this
/// home chose — a reload and a stop sent to an address nothing answers on.
fn overrides(admin: u16, extra: Option<String>) -> String {
    serde_json::json!({
        "admin_port": admin,
        "https_port": free_tls_port(),
        "extra": extra.unwrap_or_default(),
    })
    .to_string()
}

/// A home with this server **installed** in it, on ports nothing else is using, and a daemon over it.
///
/// The archive is packed out of what the `ci.yml` step fetched, served by a registry that signs its
/// own index, and installed through `package.install` — so this covers the whole T31a path against a
/// real artifact on all three systems at no extra cost, and the service it then creates is one
/// `service.create` wrote rather than one a fixture inserted.
///
/// The control port is moved off the recipe's default for the reason the site port is chosen rather
/// than fixed: a developer running this suite may well have a Caddy of their own on 2019, and a test
/// that took it over would be a test that stops somebody's work.
pub(crate) async fn declared(
    front: &FrontEnd,
) -> (
    Home,
    super::Daemon,
    mixengine_testkit::MockRegistry,
    u16,
    u16,
) {
    let (site, control) = (free_port(), free_port());

    let packed = front.pack();
    let registry = mixengine_testkit::MockRegistry::start(&serde_json::json!({
        "schema": 1, "generated_at": "2026-08-21T06:55:12Z", "packages": []
    }))
    .await;
    let url = registry.publish_asset(&packed.path(), packed.bytes.clone());
    registry.publish(&front.index(&packed, &url));

    let home = Home::new();
    let daemon = home.start_daemon_reading_index(&registry.url(), registry.public_key());

    let installed =
        json(&home.mix(&["package", "install", front.package, front.version, "--json"]));
    assert_eq!(
        installed["state"],
        "succeeded",
        "{installed}
{}",
        home.daemon_log()
    );

    // **No `@`**, which is the instancing rule seen from a recipe that has it: there is one of these,
    // and an id carrying an instance name would be refused here.
    let created = json(&home.mix(&[
        "service",
        "create",
        front.package,
        front.version,
        "--port",
        &site.to_string(),
        "--json",
    ]));
    assert_eq!(
        created["service"]["id"],
        front.package,
        "{created}
{}",
        home.daemon_log()
    );

    mixengine_testkit::declare::reconfigure(
        &home.database_file(),
        front.package,
        &(front.alone)(control),
    )
    .await;

    (home, daemon, registry, site, control)
}

/// **The whole of what a front end is, in the order a user meets it.**
///
/// One test rather than five, deliberately: each step is the previous one's precondition, and five
/// tests would be five real servers started to re-reach the state this one is already in. What each
/// assertion proves is written beside it.
pub(crate) async fn is_generated_validated_started_reloaded_and_stopped(front: &FrontEnd) {
    let (home, _daemon, _registry, site_port, control) = declared(front).await;
    let id = front.package;

    // --- generated, and judged by the server itself ----------------------------------------------
    //
    // `service start` renders the configuration and runs the recipe's validator over the staged copy
    // before installing it, so a start that completes is a configuration the real program accepted.
    let started = json(&home.mix(&["service", "start", id, "--json"]));
    assert_eq!(
        started["complete"],
        true,
        "{started}\n{}",
        home.daemon_log()
    );

    let config = home.path().join("etc").join(id).join(front.config);
    let rendered = std::fs::read_to_string(&config).expect("the generated configuration");
    assert!(
        rendered.contains(&(front.control_line)(control)),
        "{rendered}"
    );

    // --- started, and proved up by whatever this server answers ----------------------------------
    //
    // The readiness check in the spec is an HTTP request the recipe chose, so a service the daemon
    // reports as running is one that answered it.
    let up = status(&home, id);
    assert_eq!(up["state"], "running", "{up}\n{}", home.daemon_log());
    let pid = up["pid"].as_u64().expect("a running service has a pid");

    assert!(
        get(site_port).is_none(),
        "a front end with no sites answered on the port sites are served on"
    );

    // The one mechanism the two front ends do not share, asked in each one's own spelling: Caddy's
    // admin endpoint, and the `server` block MixEngine renders for nginx because nginx has none.
    // This is what the recipe's readiness check asks, so a `running` service has already answered it
    // — what is added here is that it is answering on the port a *person* was told about.
    assert!(
        frontend_control(front, control).is_some_and(|answer| answer.contains(" 200 ")),
        "{id}'s control endpoint on {control} did not answer 200 on {}:\n{}",
        front.control_path,
        home.daemon_log()
    );

    // --- reloaded ---------------------------------------------------------------------------------
    //
    // A site pasted into the free-form override, and then nothing but a listing: the configuration is
    // rendered at the top of every `service.*` call, and a rendering that moved under a running
    // service is handed to it. Nothing here restarts anything.
    mixengine_testkit::declare::reconfigure(
        &home.database_file(),
        id,
        &(front.serving)(control, site_port, "mixengine reloaded me"),
    )
    .await;

    let listed = json(&home.mix(&["service", "list", "--json"]));
    assert_eq!(listed["services"][0]["state"], "running", "{listed}");

    let deadline = Instant::now() + EVENTUALLY;
    loop {
        if get(site_port).is_some_and(|answer| answer.contains("mixengine reloaded me")) {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "the running {id} never served the site the reload gave it\n--- {} ---\n{}\n\
             --- daemon.log ---\n{}",
            front.config,
            std::fs::read_to_string(&config).unwrap_or_default(),
            home.daemon_log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    let reloaded = status(&home, id);
    assert_eq!(
        reloaded["pid"].as_u64(),
        Some(pid),
        "the server was replaced rather than reloaded, which is the cost the whole task avoids: \
         {reloaded}"
    );

    // **No front end MixEngine runs may install a certificate authority** — found by T76.
    //
    // Caddy provisions a local CA of its own and installs its root into the user's trust store the
    // first time it starts, unless told not to: `auto_https off` stops it *obtaining* certificates
    // and says nothing about this. Five `Caddy Local Authority` roots were found in
    // `CurrentUser\Root` on the machine this was written on, none of them asked for — and on a CI
    // runner the install blocked for the whole readiness budget with the server half provisioned,
    // because adding to that store wants a consent nobody is there to give.
    //
    // MixEngine reaches a trust store exactly once, through `mixengine-elevate`, for its own
    // authority and with the user's agreement (T48, T49a). A second one arriving because a front
    // end's default said so is that design undone, so it is asserted against the running server
    // rather than trusted from the template.
    let said = service_log(&home, id);
    assert!(
        !said.contains("installing root certificate"),
        "{id} installed a certificate authority on this machine, which only \
         `mixengine-elevate` may do\n--- current.log ---\n{said}"
    );

    // --- a site, declared the way a person declares one ------------------------------------------
    //
    // Everything above went through a free-form override, which proves the *reload* and says nothing
    // about T43. This is the task itself: a `sites` row becomes a file in the front end's own
    // document set, judged by the server's own checker as part of that set, and served by the
    // process that was already running.
    let project = tempfile::Builder::new()
        .prefix("mixengine-site")
        .tempdir()
        .expect("a directory to serve");
    std::fs::write(
        project.path().join("index.html"),
        "<h1>mixengine serves blog.test</h1>\n",
    )
    .expect("something to serve");

    let root = project.path().display().to_string();
    let registered = json(&home.mix(&["project", "create", &root, "--name", "blog", "--json"]));
    assert_eq!(registered["project"]["name"], "blog", "{registered}");

    let declared_site = json(&home.mix(&[
        "site",
        "create",
        "--project",
        "blog",
        "--domain",
        "blog.test",
        "--kind",
        "static",
        "--json",
    ]));
    assert_eq!(
        declared_site["site"]["site"]["domain"],
        "blog.test",
        "{declared_site}\n{}",
        home.daemon_log()
    );

    let sites = home.path().join("etc").join(id).join("sites");
    assert!(
        std::fs::read_dir(&sites)
            .expect("the sites directory")
            .filter_map(std::result::Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().starts_with("blog.test")),
        "no file was rendered for the site\n{}",
        home.daemon_log()
    );

    // The `Host` header and not the name: CI has no elevation, so no hosts entry exists — and what is
    // under test is the rendering and the server reading it, not resolution.
    let deadline = Instant::now() + EVENTUALLY;
    loop {
        let answer = request_as(site_port, "/", "blog.test");

        if answer
            .as_deref()
            .is_some_and(|body| body.contains("mixengine serves blog.test"))
        {
            break;
        }

        // The rendering and the answer, both, because either one alone leaves the reader guessing: a
        // file that was never written and a file the server would not match look identical from out
        // here.
        assert!(
            Instant::now() < deadline,
            "{id} never served the site it was given\n--- answered ---\n{}\n--- rendered ---\n{}\n\
             --- daemon.log ---\n{}",
            answer.unwrap_or_else(|| "nothing at all".to_owned()),
            rendered_sites(&home, id),
            home.daemon_log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    // --- stopped, which is the sweep and the reload together --------------------------------------
    //
    // D9 and D4 in one step: the flag goes down, the file goes with it because `sites/` holds exactly
    // what was rendered into it, and the removal is what makes the walk count as changed — without
    // that the site would go on being served by a server nobody told.
    let stopped_site = json(&home.mix(&["site", "stop", "blog.test", "--json"]));
    assert_eq!(stopped_site["site"]["state"], "disabled", "{stopped_site}");

    let deadline = Instant::now() + EVENTUALLY;
    loop {
        let answer = request_as(site_port, "/", "blog.test");

        if !answer.is_some_and(|body| body.contains("mixengine serves blog.test")) {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "{id} went on serving a site nothing declares any more\n{}",
            home.daemon_log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    // And the *other* site — the one pasted into the free-form override — is untouched, which is what
    // says the sweep took the site file and nothing around it.
    //
    // **Waited for rather than asked once**, and the loop above is why. `request_as` reports a
    // connection it could not make and a site that is gone identically, so the wait for `blog.test`
    // to stop being served can be satisfied by the reload itself rather than by its result — and an
    // assertion made in that instant reads a server mid-reload as a server that swept both sites.
    let deadline = Instant::now() + EVENTUALLY;
    loop {
        if get(site_port).is_some_and(|answer| answer.contains("mixengine reloaded me")) {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "the sweep took more than the site it was about\n{}",
            home.daemon_log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    // --- refused, with the last good configuration still live -------------------------------------
    //
    // The half of validation that matters. The server's own checker refuses the staged rendering, so
    // nothing is installed — and the process goes on serving what it was serving, which is what a
    // user whose typo would otherwise have taken every site on the machine down needs to be true.
    mixengine_testkit::declare::reconfigure(&home.database_file(), id, &(front.broken)(control))
        .await;

    let refused = home.mix(&["service", "list", "--json"]);
    assert!(
        !refused.status.success(),
        "a configuration {id} cannot parse was accepted: {}",
        String::from_utf8_lossy(&refused.stdout)
    );

    assert!(
        std::fs::read_to_string(&config)
            .expect("the configuration is still there")
            .contains("mixengine reloaded me"),
        "the refused rendering was installed anyway"
    );

    // **Waited for, and this was the one read in the whole arc that was not.** A refused rendering
    // installs nothing and reloads nothing, so a site it did not take down is answering already and
    // the first attempt returns — while a one-shot connect on a loaded runner reports "not there"
    // for reasons that have nothing to do with any configuration, which is what failed a macOS leg
    // on 2026-08-28. **The wait cannot hide a real failure of this property**: nothing is coming to
    // restore a site the refusal did take down — no reload was ordered and none will be — so a site
    // that is gone is gone for the whole of `EVENTUALLY` and the assertion still fails.
    let deadline = Instant::now() + EVENTUALLY;
    loop {
        if get(site_port).is_some_and(|answer| answer.contains("mixengine reloaded me")) {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "a configuration that was never installed stopped the site that was being served\n{}",
            home.daemon_log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    // --- stopped ----------------------------------------------------------------------------------
    //
    // The spec's `StopBehaviour::Command`, which is each server's own way of being asked. The port
    // going quiet is what says the process really went, rather than the row having been written.
    mixengine_testkit::declare::reconfigure(
        &home.database_file(),
        id,
        &(front.serving)(control, site_port, "mixengine reloaded me"),
    )
    .await;

    let stopped = json(&home.mix(&["service", "stop", id, "--json"]));
    assert_eq!(
        stopped["complete"],
        true,
        "{stopped}\n{}",
        home.daemon_log()
    );
    assert_eq!(status(&home, id)["state"], "stopped");

    let deadline = Instant::now() + EVENTUALLY;
    while get(site_port).is_some() {
        assert!(
            Instant::now() < deadline,
            "something is still serving the site port after {id} was stopped\n--- daemon.log ---\n{}",
            home.daemon_log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Every file in this front end's `sites/` directory, named and quoted, or why there are none.
///
/// What a "the site was not served" failure needs beside the daemon's own log: the file the server
/// was given, which is the difference between a rendering that never happened and one the server
/// would not match.
fn rendered_sites(home: &Home, id: &str) -> String {
    let directory = home.path().join("etc").join(id).join("sites");

    let Ok(entries) = std::fs::read_dir(&directory) else {
        return format!("{} does not exist", directory.display());
    };

    entries
        .filter_map(std::result::Result::ok)
        .map(|entry| {
            format!(
                "--- {} ---\n{}",
                entry.path().display(),
                std::fs::read_to_string(entry.path())
                    .unwrap_or_else(|error| format!("unreadable: {error}"))
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// This front end's control endpoint, asked whatever it answers on.
fn frontend_control(front: &FrontEnd, control: u16) -> Option<String> {
    request(control, front.control_path)
}

/// What `mix service status <id>` says.
pub(crate) fn status(home: &Home, id: &str) -> Value {
    json(&home.mix(&["service", "status", id, "--json"]))
}
