//! What a shared home is listening on — roadmap task **T76**, the first of two enforcement tests.
//!
//! **This test proves what is listening, not what is allowed through**, and the distinction is not a
//! quibble to skip over. Every connection here is made from the machine being scanned to its own
//! address, so none of them crosses the firewall: a machine with the web port open and everything
//! else blocked, and a machine with no firewall at all, pass this identically.
//!
//! What it *does* prove is the promise T74's first real run found broken — that sharing one site
//! does not put the rest of what MixEngine runs on the network. That defect was found with `netstat`
//! and not with a firewall, which is exactly the half this suite covers. The rule half is
//! `mixengine-core`'s `tests/firewall.rs` and the manual run recorded in the T76 design.
//!
//! **`#[ignore]`d rather than skipped**, on `caddy.rs`' reasoning: a test that quietly returns when
//! it cannot find a front end is a green suite that proved nothing. Only a running server can be
//! asked what it bound.

mod harness;

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use harness::frontend::{self, CADDY};

/// How long to wait for a connection that is expected to be refused.
///
/// Short: a refusal on loopback is immediate, and this budget only bounds the case where something
/// filters instead of refusing — which would be a failure either way, but should not be a slow one.
const REACHABLE: Duration = Duration::from_millis(500);

/// How long a reload is given to take effect.
///
/// The harness' own `EVENTUALLY`, and for its reason: a reload is asynchronous, so what is really
/// being waited for is a runner's next turn on a machine that may be compiling something else at the
/// same time.
const EVENTUALLY: Duration = Duration::from_secs(30);

/// Whether anything accepts a TCP connection there.
///
/// A refusal and a timeout are the same answer for this suite's purpose: nothing is serving.
fn accepts(address: Ipv4Addr, port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from((address, port)), REACHABLE).is_ok()
}

/// Whether that port stops accepting within [`EVENTUALLY`].
///
/// **Polled rather than asserted on the instant, because a reload is not synchronous.** `mix site
/// unshare` returns once the daemon has re-rendered and told the front end; the server then closes
/// the listener in its own time, and on a busy runner that is not the same millisecond. Asserting
/// immediately would be asserting about scheduling — CI failed on exactly that, having passed on a
/// developer's machine.
fn stops_accepting(address: Ipv4Addr, port: u16) -> bool {
    let deadline = std::time::Instant::now() + EVENTUALLY;

    while std::time::Instant::now() < deadline {
        if !accepts(address, port) {
            return true;
        }

        std::thread::sleep(Duration::from_millis(200));
    }

    false
}

/// An interface this machine can share on, by name.
///
/// **Named rather than left to the daemon's default, and read rather than guessed.** `site.share`
/// refuses to choose where more than one interface is up — the T74 design, D5 — and a CI runner with
/// virtual adapters is exactly that machine. The name comes from the same enumeration the daemon
/// will make, so this suite is about what gets bound rather than about how many adapters the runner
/// happens to have.
///
/// **And [`None`] is a real answer rather than a broken machine.** A machine with only loopback
/// cannot share at all, by design — CI's Linux leg runs its whole test job inside exactly such a
/// namespace — so a suite that *asserted* an interface existed would be asserting a property of the
/// machine rather than of MixEngine.
fn a_shareable_interface() -> Option<mixengine_platform::Interface> {
    mixengine_platform::host()
        .network()
        .interfaces()
        .expect("this machine can be asked about its own interfaces")
        .into_iter()
        .find(|interface| !interface.loopback && bindable(interface.address))
}

/// What a service wrote to its own log, or an empty string.
///
/// **`daemon.log` is not where a front end says why it would not start.** Output travels on its own
/// stream and into `logs/services/<id>/current.log`, per ADR 0009 — so a failure that attached only
/// the daemon's log would carry the readiness timeout and none of the reason for it, which is what
/// this suite's first CI run produced.
fn service_log(home: &harness::Home, service: &str) -> String {
    std::fs::read_to_string(
        home.path()
            .join("logs")
            .join("services")
            .join(service)
            .join("current.log"),
    )
    .unwrap_or_default()
}

/// Whether a listener can be opened on that address at all.
///
/// **An interface that is *up* is not the same as an address a server can bind**, and this suite
/// needs the second. `if-addrs` reports every adapter carrying an IPv4 address — a tunnel, a
/// hypervisor's internal switch, an adapter mid-configuration — and a front end told to bind one it
/// cannot use never becomes ready, which arrives as a readiness timeout thirty seconds later with
/// nothing in it about the address.
///
/// Asked here, with the same system call the front end will make, so that a machine whose first
/// adapter cannot host a listener is skipped rather than mis-reported as a broken feature.
fn bindable(address: Ipv4Addr) -> bool {
    std::net::TcpListener::bind(SocketAddr::from((address, 0))).is_ok()
}

/// **The shared site answers on the LAN address, and nothing else MixEngine runs does.**
///
/// The control port is the one that matters most here and the one a reader might not expect to see
/// scanned: Caddy's admin endpoint accepts configuration, so a front end that put it on the network
/// because one site was shared would be handing the local network the ability to reconfigure this
/// machine's web server. It is bound to loopback by the recipe, and this is what says so after a
/// share rather than before one.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real Caddy — see the module note, and the `caddy` step in ci.yml"]
async fn a_shared_home_listens_on_the_web_port_and_nothing_else() {
    let (home, _daemon, _registry, site_port, control) = frontend::declared(&CADDY).await;

    let repository = tempfile::Builder::new()
        .prefix("mixengine-t76")
        .tempdir()
        .expect("a temporary directory");
    let root = repository.path().display().to_string();

    home.mix(&["project", "create", &root, "--name", "blog"]);
    home.mix_in(
        repository.path(),
        &[],
        &[
            "site",
            "create",
            "--domain",
            "blog.test",
            "--kind",
            "static",
        ],
    );

    let Some(interface) = a_shareable_interface() else {
        eprintln!(
            "skipped: this machine has no interface to share on, only loopback — there is nothing \
             to assert about what a share puts on a network that does not exist"
        );
        return;
    };

    // The domain is positional here, unlike `site create`'s `--domain`. Worth the note: the two
    // commands sit next to each other in this test and take it differently.
    let shared = harness::json(&home.mix(&[
        "site",
        "share",
        "blog.test",
        "--interface",
        &interface.name,
        "--json",
    ]));

    let address: Ipv4Addr = shared["address"]
        .as_str()
        .unwrap_or_else(|| panic!("no address in {shared}\n{}", home.daemon_log()))
        .parse()
        .expect("an IPv4 address");

    assert_eq!(address, interface.address, "{shared}");

    // **Not `harness::json`**, which panics with the command's own output and nothing else: a front
    // end that failed to come up says why in its *own* log — output travels on its own stream, per
    // ADR 0009 — and a failure carrying neither that nor the rendering cost a CI round trip on this
    // suite's first run.
    let start = home.mix(&["service", "start", CADDY.package, "--json"]);
    assert!(
        start.status.success(),
        "the front end did not start with a site shared on {address}\n\
         --- mix ---\n{}\n--- current.log ---\n{}\n--- daemon.log ---\n{}",
        harness::stdout(&start),
        service_log(&home, CADDY.package),
        home.daemon_log()
    );

    // **The site answers at the address, by the address.** T74's first real run found the empty 200
    // that happens when a block is bound to an address it does not also answer *for*, so the request
    // carries the address as its `Host` — which is what a phone sends.
    let answered = frontend::request_at(address, site_port, "/", &format!("{address}:{site_port}"))
        .unwrap_or_else(|| {
            panic!(
                "the shared site did not answer on {address}:{site_port}\n{}",
                home.daemon_log()
            )
        });
    assert!(answered.contains("HTTP/1.1"), "{answered}");

    // **And nothing else MixEngine runs is on that address.** The control port is Caddy's admin
    // endpoint: it accepts configuration, so a share that put it on the network would hand the local
    // network this machine's web server.
    assert!(
        !accepts(address, control),
        "the front end's control port answers on the LAN address, and sharing a site must never \
         put it there\n{}",
        home.daemon_log()
    );

    // **And the listener goes away with the share**, which is the half `mix site unshare` owns and
    // the precondition for everything T76 does automatically.
    home.mix(&["site", "unshare", "blog.test"]);

    assert!(
        stops_accepting(address, site_port),
        "unsharing must take the listener off the LAN address again\n{}",
        home.daemon_log()
    );
}
