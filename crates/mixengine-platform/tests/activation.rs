//! The listener an activator holds — roadmap task **T70**.
//!
//! **One test file that names no operating system.** What a service listens on differs by system —
//! a Unix socket where there are sockets, a TCP port on Windows — and the whole point of
//! [`Listen`] is that everything above this crate can carry either without knowing which. So the
//! address these tests use comes out of one helper, and every assertion below holds on all three.

#![cfg(feature = "ipc")]

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU16, Ordering};

use mixengine_platform::activation::{Activation, Listen, dial};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// An address of the shape this system's services use, **already bound**.
///
/// A path inside a directory the test owns where there are Unix sockets, and a loopback port
/// nothing holds where there are not.
///
/// **The bind is the search**, and each caller searches a window of its own. Both halves were
/// measured rather than imagined, and each fixed a different failure.
///
/// An earlier version chose the port with a probe — bind a `TcpListener`, drop it, hand back the
/// number — and a port that was free a moment ago is not a port anybody holds: the tests in this
/// file run at the same moment and started their search at the same number, so two of them took it.
/// That is why the bind that keeps the address is the one that chooses it.
///
/// Searching *one* window was not enough either, and the test it broke says why:
/// [`a_stale_address_does_not_stop_the_next_bind`] releases its address on purpose and takes it back
/// — and in between, a neighbour walking the same range from below is entitled to it. Disjoint
/// windows mean a released port can only be retaken by the test that released it. Ports another
/// program on the machine holds are still skipped, because the number was never this file's to
/// assume.
async fn bound(home: &tempfile::TempDir) -> (Listen, Activation) {
    /// How many callers have taken a window so far.
    static TAKEN: AtomicU16 = AtomicU16::new(0);

    /// How many ports each caller gets to search.
    const WINDOW: u16 = 100;

    if cfg!(windows) {
        let base = 24_500 + TAKEN.fetch_add(1, Ordering::Relaxed) * WINDOW;

        for port in base..base + WINDOW {
            let listen = Listen::Tcp((Ipv4Addr::LOCALHOST, port).into());

            if let Ok(activation) = Activation::bind(&listen).await {
                return (listen, activation);
            }
        }

        panic!("no free port between {base} and {}", base + WINDOW);
    }

    // One address and no search: the directory is this test's own, so nothing else can be holding
    // the socket inside it.
    let listen = Listen::Socket(home.path().join("activate.sock"));
    let activation = Activation::bind(&listen).await.expect("a listener");

    (listen, activation)
}

/// **The activator carries bytes and never reads them** — the design's D1.
///
/// FastCGI, the MySQL protocol and RESP have nothing in common except that a client connects and
/// something is said. This is the whole of what the listener has to do for all three.
#[tokio::test]
async fn what_is_written_to_one_end_arrives_at_the_other() {
    let home = tempfile::tempdir().expect("a directory");
    let (listen, listener) = bound(&home).await;

    let dialling = tokio::spawn({
        let listen = listen.clone();
        async move {
            let mut client = dial(&listen).await.expect("a connection");
            client.write_all(b"ping").await.expect("a write");

            let mut back = [0_u8; 4];
            client.read_exact(&mut back).await.expect("a read");
            back
        }
    });

    let mut accepted = listener.accept().await.expect("a connection");

    let mut said = [0_u8; 4];
    accepted.read_exact(&mut said).await.expect("a read");
    assert_eq!(&said, b"ping");

    accepted.write_all(b"pong").await.expect("a write");

    assert_eq!(&dialling.await.expect("the client"), b"pong");
}

/// **A client that waits to be greeted is carried by the same code** — D1, the other order.
///
/// MySQL sends nothing until the server has spoken. A listener that assumed the client speaks
/// first would serve every web request and hang on every database connection, which is exactly the
/// half T70a depends on and exactly the half a web-only test would never reach.
#[tokio::test]
async fn a_client_that_waits_to_be_greeted_is_carried_too() {
    let home = tempfile::tempdir().expect("a directory");
    let (listen, listener) = bound(&home).await;

    let dialling = tokio::spawn({
        let listen = listen.clone();
        async move {
            let mut client = dial(&listen).await.expect("a connection");

            let mut greeting = [0_u8; 5];
            client.read_exact(&mut greeting).await.expect("a read");
            client.write_all(b"auth").await.expect("a write");
            greeting
        }
    });

    let mut accepted = listener.accept().await.expect("a connection");
    accepted.write_all(b"hello").await.expect("the greeting");

    let mut answered = [0_u8; 4];
    accepted.read_exact(&mut answered).await.expect("a read");
    assert_eq!(&answered, b"auth");

    assert_eq!(&dialling.await.expect("the client"), b"hello");
}

/// **A socket file the last daemon left behind does not stop the next one.**
///
/// `bind` on a path that still holds a socket file fails with `AddrInUse` even when nothing is
/// listening on it, so a daemon that was killed would leave every activator address unusable until
/// somebody deleted the files by hand — and what a user would see is sites that 502 until they
/// find out that a file in `run/` is why.
#[tokio::test]
async fn a_stale_address_does_not_stop_the_next_bind() {
    let home = tempfile::tempdir().expect("a directory");
    let (listen, first) = bound(&home).await;

    drop(first);

    Activation::bind(&listen)
        .await
        .expect("the second bind is the one this test is about");
}
