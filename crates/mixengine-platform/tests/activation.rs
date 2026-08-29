//! The listener an activator holds — roadmap task **T70**.
//!
//! **One test file that names no operating system.** What a service listens on differs by system —
//! a Unix socket where there are sockets, a TCP port on Windows — and the whole point of
//! [`Listen`] is that everything above this crate can carry either without knowing which. So the
//! address these tests use comes out of one helper, and every assertion below holds on all three.

#![cfg(feature = "ipc")]

use std::net::{Ipv4Addr, TcpListener};

use mixengine_platform::activation::{Activation, Listen, dial};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// An address of the shape this system's services use.
///
/// A path inside a directory the test owns where there are Unix sockets, and a loopback port
/// nothing holds where there are not — chosen by binding, because a number this file merely hoped
/// for is one another program on the machine is entitled to have.
fn somewhere(home: &tempfile::TempDir) -> Listen {
    if cfg!(windows) {
        let port = (24_500..25_400)
            .find(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, *port)).is_ok())
            .expect("a free port in the window");

        Listen::Tcp((Ipv4Addr::LOCALHOST, port).into())
    } else {
        Listen::Socket(home.path().join("activate.sock"))
    }
}

/// **The activator carries bytes and never reads them** — the design's D1.
///
/// FastCGI, the MySQL protocol and RESP have nothing in common except that a client connects and
/// something is said. This is the whole of what the listener has to do for all three.
#[tokio::test]
async fn what_is_written_to_one_end_arrives_at_the_other() {
    let home = tempfile::tempdir().expect("a directory");
    let listen = somewhere(&home);

    let listener = Activation::bind(&listen).await.expect("a listener");

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
    let listen = somewhere(&home);

    let listener = Activation::bind(&listen).await.expect("a listener");

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
    let listen = somewhere(&home);

    let first = Activation::bind(&listen).await.expect("a listener");
    drop(first);

    Activation::bind(&listen)
        .await
        .expect("the second bind is the one this test is about");
}
