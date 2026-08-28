//! Does the state this build filters on match what a real established connection reports?
//!
//! The unit tests in `linux/ports.rs` parse a captured table, which proves the parsing and says
//! nothing about the constant. `01`, `MIB_TCP_STATE_ESTAB` and `-sTCP:ESTABLISHED` are three claims
//! about three unrelated mechanisms, and the only thing that can check any of them is a socket that
//! really is connected. So this suite is the one part of the capability CI is the first place to
//! learn about — it runs in the `test` job, on all three runners.
//!
//! **The `test` job and not `system`**, which is what this note said for as long as no job ran the
//! suite at all. `system` is the job for what an unprivileged process cannot prove, and counting
//! connections is not that: what it needs is a real socket on each of the three mechanisms, and the
//! ordinary test job is the one with a leg on each of them.

use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

/// How long a closed connection is given to stop being established.
///
/// A close leaves `TIME_WAIT` behind on some systems, which is neither established nor instant. The
/// wait is bounded and polled rather than slept through: a fixed pause is flaky on a loaded runner
/// and wasteful on an idle one.
const SETTLE: Duration = Duration::from_secs(10);

/// A real connection is counted, and its absence is counted too.
///
/// **One test rather than two.** The second assertion only means anything because the first ran in
/// the same process against the same port: a reader that always answered zero would pass a test that
/// only checked an idle port, and a reader that counted listeners would pass one that only checked a
/// busy one.
#[test]
#[ignore = "opens a real socket; the test job runs it with --ignored"]
fn a_real_connection_is_counted_and_a_closed_one_stops_being() {
    let host = mixengine_platform::host();

    // Port 0: the OS picks. A test may not assume a port is free — that is a property of the machine
    // and not of MixEngine.
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
    let port = listener.local_addr().expect("the bound address").port();

    assert_eq!(
        host.connections()
            .established_on(port)
            .expect("this machine can count connections"),
        0,
        "a listener nobody has connected to has no established connections, and the listening \
         socket itself is not one"
    );

    let client = TcpStream::connect(("127.0.0.1", port)).expect("a connection to the listener");
    let (accepted, _) = listener.accept().expect("the server side of it");

    assert_eq!(
        host.connections()
            .established_on(port)
            .expect("this machine can count connections"),
        1,
        "one connection, established at both ends, counted once — not twice for its two sockets"
    );

    drop(client);
    drop(accepted);

    let since = Instant::now();

    loop {
        let count = host
            .connections()
            .established_on(port)
            .expect("this machine can count connections");

        if count == 0 {
            break;
        }

        assert!(
            since.elapsed() < SETTLE,
            "a closed connection was still established after {SETTLE:?}: {count}"
        );

        std::thread::sleep(Duration::from_millis(50));
    }
}
