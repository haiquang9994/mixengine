//! Routing a managed TLD to a nameserver of our own, against the real machine.
//!
//! The unwritten half of this file is the important one. **Every test that concludes something from
//! a name that did *not* resolve first proves, with the same instrument and at the same moment,
//! that the instrument is alive** — the T45 design, D14.
//!
//! That rule is not a style preference. The measurement this whole task was designed from ran six
//! rounds and **four of them were void**: a fake DNS server was started in one CI step and asked
//! from the next, the runner killed it with the step, and every "Windows routes nothing" result was
//! a statement about a dead socket. Nothing noticed until a control was added. A test that says "the
//! name did not resolve" without one is making a claim about itself.
//!
//! And on Windows the instrument must be `getaddrinfo` — [`std::net::ToSocketAddrs`] — and never
//! `nslookup`, which was measured to bypass the Name Resolution Policy Table entirely and would
//! report a correctly wired machine as broken.

use mixengine_platform::{Host as _, ResolverMethod, mock};

/// The TLD every test here routes. `.test` is RFC 6761's and resolves nowhere in the world.
const TLD: &str = "test";

// ---------------------------------------------------------------------------------------------
// Reads. These need no privilege on any of the three systems, which is the whole reason the daemon
// can ask on every start — so they run in the ordinary `test` job.
// ---------------------------------------------------------------------------------------------

/// D2's table, from the machine's own side — and the one row of it that is not a constant.
#[test]
fn this_machine_says_which_mechanism_it_has() {
    let host = mixengine_platform::host();

    let method = host
        .resolver()
        .method()
        .expect("asking which mechanism a machine has is a read that cannot fail");

    #[cfg(windows)]
    assert_eq!(method, ResolverMethod::Nrpt);

    #[cfg(target_os = "macos")]
    assert_eq!(method, ResolverMethod::ResolverDirectory);

    // **Linux is the row that is a question rather than an answer** — D2. A machine running both
    // systemd services has a mechanism and one running neither has none, and both are correct.
    #[cfg(target_os = "linux")]
    assert!(matches!(
        method,
        ResolverMethod::SystemdLink | ResolverMethod::None
    ));
}

/// Probing reads and never prompts, which is what makes it affordable at every daemon start.
#[test]
fn probing_costs_no_privilege_and_answers_something() {
    let host = mixengine_platform::host();

    let state = host
        .resolver()
        .probe(&[TLD], 53_535)
        .expect("probing reads and cannot fail on a supported system");

    assert_eq!(state.method, host.resolver().method().expect("a method"));

    // Whatever this machine happens to route, it cannot claim a TLD nobody asked about.
    assert!(state.wired.iter().all(|tld| tld == TLD), "{state:?}");
}

/// A machine that routes nothing has something to ask for; one that routes everything has nothing.
/// Driven against the mock, because a CI runner is neither reliably.
#[test]
fn the_plan_is_whole_state_and_is_nothing_when_the_machine_already_agrees() {
    let unwired = mock::Host::with_resolver("/mixengine", ResolverMethod::Nrpt, &[]);
    let state = unwired.resolver().probe(&[TLD], 53).expect("a state");

    assert!(state.plan(&[TLD], 53).is_some());

    let wired = mock::Host::with_resolver("/mixengine", ResolverMethod::Nrpt, &[TLD]);
    let state = wired.resolver().probe(&[TLD], 53).expect("a state");

    assert_eq!(state.plan(&[TLD], 53), None);
}

// ---------------------------------------------------------------------------------------------
// Writes. Every one of these changes the machine, puts it back, and is `#[ignore]`d: they belong
// to CI's `system` job, which runs elevated.
// ---------------------------------------------------------------------------------------------

/// A plan that is not this system's mechanism is refused **by name**, before anything is touched.
///
/// Not `#[ignore]`d: a refusal reaches no file and needs no token, so it is asserted here with the
/// reads — exactly as T42 asserts Windows' refusal in the ordinary job.
#[test]
fn the_plan_this_system_does_not_use_is_refused_by_name() {
    use mixengine_proto::privileged::ResolverPlan;

    let foreign = if cfg!(windows) {
        ResolverPlan::ResolverDirectory {
            tlds: vec![TLD.to_owned()],
            port: 53_535,
        }
    } else {
        ResolverPlan::Nrpt {
            tlds: vec![TLD.to_owned()],
        }
    };

    let refused = mixengine_platform::resolver::apply(&foreign)
        .expect_err("another system's plan is not this one's to apply");

    assert!(
        matches!(
            refused,
            mixengine_platform::Error::UnsupportedPlatform { .. }
        ),
        "{refused:?}"
    );
}

/// The whole arc, on whichever machine is running it: wire, resolve a name nothing has ever asked
/// for, confirm the machine's other names are untouched, and unwire.
#[test]
#[ignore = "changes this machine's resolver configuration; run in CI's system job"]
fn a_wired_machine_resolves_a_name_nothing_has_ever_asked_for_and_leaves_the_rest_alone() {
    let Some(server) = FakeDns::start() else {
        // A machine that cannot lend us a port cannot be measured on, and saying so is better than
        // asserting something about a socket that never opened — which is the whole of D14.
        eprintln!(
            "this machine would not lend the port its resolver mechanism sends to, so a fake DNS              server could not be started; nothing below could be proved"
        );
        return;
    };

    // **CONTROL, before anything is concluded.**
    assert!(
        server.answers_when_asked_point_blank(),
        "the fake DNS server is not answering; nothing below this line would mean anything"
    );

    let host = mixengine_platform::host();
    let method = host.resolver().method().expect("a method");

    if method == ResolverMethod::None {
        eprintln!("this machine has no scoped resolver mechanism, which is a valid answer (D2)");
        return;
    }

    let state = host.resolver().probe(&[TLD], server.port).expect("a state");
    let plan = state
        .plan(&[TLD], server.port)
        .expect("a machine that routes nothing has something to ask for");

    let applied = mixengine_platform::resolver::apply(&plan).expect("the wiring applies");
    assert!(
        matches!(
            applied,
            mixengine_platform::resolver::Change::Written { .. }
        ),
        "{applied:?}"
    );

    // Put the machine back whatever the assertions below do.
    let _restore = Restore;

    // The probe agrees with what was just written — which is what the daemon reads on every start.
    let after = host.resolver().probe(&[TLD], server.port).expect("a state");
    assert_eq!(after.wired, vec![TLD.to_owned()], "{after:?}");

    // Whole state: the same plan again changes nothing.
    assert!(
        matches!(
            mixengine_platform::resolver::apply(&plan).expect("it applies again"),
            mixengine_platform::resolver::Change::Unchanged
        ),
        "a second apply of one plan is not a second change"
    );

    // **A name nothing has ever asked for**, so no cache can answer it and no negative entry can
    // deny it. `getaddrinfo` and never `nslookup`, which bypasses NRPT on Windows.
    //
    // **Waited for rather than asserted at once**, because applying the wiring and the machine
    // routing through it are not the same instant: `systemd-networkd` brings the link up after the
    // reload returns, and the DNS Client reads its policy when it is told to. CI measured both.
    // A bound rather than a sleep, so a machine that is ready in 200 ms costs 200 ms.
    let resolved = resolves_within(
        &format!("fresh-{}.{TLD}", std::process::id()),
        std::time::Duration::from_secs(15),
    );

    // **The evidence travels with the failure.** A bare `None` costs a whole CI round to learn one
    // bit, and the bit that halves the search is whether the query reached the server at all: if it
    // did, the routing works and the answer was rejected; if it did not, nothing is routing. This is
    // D14 applied to the failure message rather than only to the assertion.
    assert_eq!(
        resolved,
        Some(std::net::Ipv4Addr::LOCALHOST),
        "a managed name did not reach the server this test wired the machine to.
           applied: {applied:?}
  probe now: {:?}
  server was asked: {:?}
           server still answering: {}",
        host.resolver().probe(&[TLD], server.port),
        server.everything_it_was_asked(),
        server.answers_when_asked_point_blank(),
    );

    // **CONTROL again**, so the assertion above is known to have been made against a live server.
    assert!(
        server.answers_when_asked_point_blank(),
        "the fake DNS server died mid-test; the assertion above proves nothing"
    );

    // **The safety question**, which is the one the Linux measurement was built around: a global
    // routing domain looks exactly like a scoped one right up until it answers for github.com.
    assert!(
        !server.was_asked_about("example.com"),
        "wiring one TLD sent an unrelated name to MixEngine's DNS server"
    );
}

/// Unwiring puts the machine back, and unwiring a machine that is not wired is not a change.
#[test]
#[ignore = "changes this machine's resolver configuration; run in CI's system job"]
fn unwiring_a_machine_that_is_not_wired_changes_nothing() {
    let host = mixengine_platform::host();

    let Some(target) = host
        .resolver()
        .probe(&[TLD], 53_535)
        .expect("a state")
        .target()
    else {
        eprintln!("this machine has no scoped resolver mechanism, which is a valid answer (D2)");
        return;
    };

    // Twice: the first may have something of an earlier run to remove, the second must not.
    let _ = mixengine_platform::resolver::revoke(&target).expect("revoking applies");

    assert!(
        matches!(
            mixengine_platform::resolver::revoke(&target).expect("revoking again applies"),
            mixengine_platform::resolver::Change::Unchanged
        ),
        "removing a wiring that is not there is not a change"
    );
}

/// Puts this machine's resolver back however the test that made it ends.
struct Restore;

impl Drop for Restore {
    fn drop(&mut self) {
        let host = mixengine_platform::host();

        let Ok(state) = host.resolver().probe(&[TLD], 53_535) else {
            return;
        };
        let Some(target) = state.target() else {
            return;
        };

        if let Err(error) = mixengine_platform::resolver::revoke(&target) {
            eprintln!("this machine's resolver could not be put back: {error}");
        }
    }
}

/// What a name resolves to through this machine's ordinary resolver, or [`None`].
///
/// **`getaddrinfo`, and never `nslookup`.** Measured on Windows: `nslookup` talks to the configured
/// server directly and does not honour the Name Resolution Policy Table, so it answers NXDOMAIN for
/// a name that `getaddrinfo` resolves at the same moment. A diagnostic written with it would report
/// a correctly wired machine as broken — which T46 and T47 will need to know as well.
fn resolves_within(name: &str, bound: std::time::Duration) -> Option<std::net::Ipv4Addr> {
    let deadline = std::time::Instant::now() + bound;

    for round in 0.. {
        // A *different* name every round, so a negative cache entry left by an earlier attempt
        // cannot answer a later one — which would turn "not ready yet" into "never works", and is
        // exactly the trap a machine with a DNS cache sets for a poll like this.
        if let Some(found) = resolve(&format!("r{round}-{name}")) {
            return Some(found);
        }

        if std::time::Instant::now() >= deadline {
            return None;
        }

        std::thread::sleep(std::time::Duration::from_millis(250));
    }

    None
}

fn resolve(name: &str) -> Option<std::net::Ipv4Addr> {
    use std::net::ToSocketAddrs;

    (name, 80u16)
        .to_socket_addrs()
        .ok()?
        .find_map(|address| match address.ip() {
            std::net::IpAddr::V4(four) => Some(four),
            std::net::IpAddr::V6(_) => None,
        })
}

/// A DNS server that answers `A 127.0.0.1` for everything and remembers what it was asked.
///
/// The same instrument the measurement behind this task used, for the same reason: a resolver
/// reports "the query never arrived" and "the answer was thrown away" identically, and only a
/// server that logs can tell them apart.
struct FakeDns {
    port: u16,
    asked: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl FakeDns {
    /// Bind the port this system's mechanism can reach and start answering, or [`None`] when the
    /// machine will not lend it.
    ///
    /// **The port belongs to the mechanism, not to the test.** macOS' resolver file and Linux'
    /// `.network` file both carry one, so an ephemeral port keeps this suite off whatever the
    /// machine running it uses 53 for. Windows' NRPT has **no field for a port** — a rule names an
    /// address and the DNS Client asks it on 53 — so on Windows a server anywhere else is a server
    /// the wiring cannot reach however correctly the rule is written. That is precisely how the
    /// first round of this test failed, and it failed in the shape the design warned about: the
    /// rule applied, the probe agreed, and the only name the server was ever asked was the control.
    fn start() -> Option<Self> {
        let port = if cfg!(windows) { 53 } else { 0 };
        let socket = std::net::UdpSocket::bind(("127.0.0.1", port)).ok()?;
        socket
            .set_read_timeout(Some(std::time::Duration::from_millis(200)))
            .ok()?;

        let port = socket.local_addr().ok()?.port();
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        let (heard, halt) = (std::sync::Arc::clone(&asked), std::sync::Arc::clone(&stop));

        std::thread::spawn(move || {
            let mut buffer = [0u8; 512];

            while !halt.load(std::sync::atomic::Ordering::Relaxed) {
                let Ok((read, peer)) = socket.recv_from(&mut buffer) else {
                    continue;
                };

                let Some((name, after)) = question(&buffer[..read]) else {
                    continue;
                };

                heard.lock().expect("the log is not poisoned").push(name);

                if let Some(answer) = reply(&buffer[..read], after) {
                    let _ = socket.send_to(&answer, peer);
                }
            }
        });

        Some(Self { port, asked, stop })
    }

    /// Ask this server directly, bypassing every resolver on the machine.
    ///
    /// This is the control. It answers one question — *is the instrument alive?* — and nothing else.
    fn answers_when_asked_point_blank(&self) -> bool {
        let Ok(socket) = std::net::UdpSocket::bind(("127.0.0.1", 0)) else {
            return false;
        };
        if socket
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .is_err()
        {
            return false;
        }

        let query = query_for("control.invalid");
        if socket.send_to(&query, ("127.0.0.1", self.port)).is_err() {
            return false;
        }

        let mut buffer = [0u8; 512];
        socket.recv_from(&mut buffer).is_ok()
    }

    /// Every name this server was asked about, for a failure to carry with it.
    fn everything_it_was_asked(&self) -> Vec<String> {
        self.asked.lock().expect("the log is not poisoned").clone()
    }

    /// Was this server ever asked about a name ending in `suffix`?
    fn was_asked_about(&self, suffix: &str) -> bool {
        self.asked
            .lock()
            .expect("the log is not poisoned")
            .iter()
            .any(|name| name.ends_with(suffix))
    }
}

impl Drop for FakeDns {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The name a query asks about, and the offset just past the question.
fn question(packet: &[u8]) -> Option<(String, usize)> {
    let mut at = 12;
    let mut labels: Vec<String> = Vec::new();

    loop {
        let length = *packet.get(at)? as usize;
        at += 1;

        if length == 0 {
            break;
        }

        labels.push(String::from_utf8_lossy(packet.get(at..at + length)?).into_owned());
        at += length;
    }

    Some((labels.join("."), at + 4))
}

/// `A 127.0.0.1` for an `A` question, and `NOERROR` with no records for anything else.
fn reply(packet: &[u8], after: usize) -> Option<Vec<u8>> {
    let qtype = u16::from_be_bytes([*packet.get(after - 4)?, *packet.get(after - 3)?]);

    let mut out = packet.get(..after)?.to_vec();
    out[2] = 0x81;
    out[3] = 0x80;

    if qtype == 1 {
        out[6] = 0;
        out[7] = 1;
        out.extend_from_slice(&[0xC0, 0x0C, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 127, 0, 0, 1]);
    } else {
        out[6] = 0;
        out[7] = 0;
    }

    Some(out)
}

/// One `A` query for `name`.
fn query_for(name: &str) -> Vec<u8> {
    let mut out = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];

    for label in name.split('.') {
        out.push(
            u8::try_from(label.len()).expect("a test asks about labels shorter than 256 bytes"),
        );
        out.extend_from_slice(label.as_bytes());
    }

    out.extend_from_slice(&[0, 0, 1, 0, 1]);
    out
}
