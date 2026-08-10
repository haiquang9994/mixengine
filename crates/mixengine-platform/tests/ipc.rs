//! The local endpoint, against the real OS.
//!
//! Not `#[ignore]`d and not system tests: every one of these binds inside a `TempDir` this test
//! owns — or, on Windows, a pipe whose name is derived from one — and touches nothing else. No
//! network socket is opened anywhere in this file, which is the whole point of the transport.
//!
//! The one case that cannot be covered here is the peer check refusing somebody: it needs a second
//! account on the machine to connect from, which no unit or component test may create. What is
//! covered is that the check runs on every accept and that this account passes it.

use std::path::Path;

use mixengine_platform::ipc::{Accepted, Connection, Endpoint, Listener};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// A `run/` directory of a home that exists only for this test.
fn run_dir() -> TempDir {
    TempDir::new().expect("the system temporary directory is writable")
}

/// Take the connection an accept was supposed to trust, or fail saying what came instead.
fn trusted(accepted: Accepted) -> Connection {
    match accepted {
        Accepted::Trusted(connection) => connection,
        other => panic!("this account's own connection was not trusted: {other:?}"),
    }
}

#[test]
fn every_home_gets_an_endpoint_of_its_own() {
    // The property the whole sandbox story rests on: a daemon started with MIXENGINE_HOME pointing
    // somewhere disposable must not be reachable at the address of the real install, and must be
    // reachable at the same address twice running.
    let one = run_dir();
    let two = run_dir();

    assert_ne!(
        Endpoint::in_run_dir(one.path()).unwrap(),
        Endpoint::in_run_dir(two.path()).unwrap()
    );
    assert_eq!(
        Endpoint::in_run_dir(one.path()).unwrap(),
        Endpoint::in_run_dir(one.path()).unwrap()
    );
}

#[tokio::test]
async fn a_client_and_the_daemon_exchange_bytes() {
    let run = run_dir();
    let endpoint = Endpoint::in_run_dir(run.path()).unwrap();
    let mut listener = Listener::bind(&endpoint).unwrap();

    let client = tokio::spawn({
        let endpoint = endpoint.clone();
        async move {
            let mut connection = Connection::connect(&endpoint).await.unwrap();
            connection.write_all(b"daemon.status").await.unwrap();

            let mut answer = [0; 2];
            connection.read_exact(&mut answer).await.unwrap();
            answer
        }
    });

    let mut connection = trusted(listener.accept().await.unwrap());

    let mut request = [0; 13];
    connection.read_exact(&mut request).await.unwrap();
    assert_eq!(&request, b"daemon.status");
    connection.write_all(b"ok").await.unwrap();

    assert_eq!(&client.await.unwrap(), b"ok");
}

#[tokio::test]
async fn a_second_client_is_served_after_the_first() {
    // The case Windows can get wrong and Unix cannot: a named pipe exists only while an instance of
    // it does, so the daemon has to create the replacement before it looks at the client it just
    // took. One client proves nothing about that; two do.
    let run = run_dir();
    let endpoint = Endpoint::in_run_dir(run.path()).unwrap();
    let mut listener = Listener::bind(&endpoint).unwrap();

    for turn in [b"1", b"2"] {
        let client = tokio::spawn({
            let endpoint = endpoint.clone();
            async move {
                let mut connection = Connection::connect(&endpoint).await.unwrap();
                connection.write_all(turn).await.unwrap();
            }
        });

        let mut connection = trusted(listener.accept().await.unwrap());
        let mut request = [0; 1];
        connection.read_exact(&mut request).await.unwrap();

        assert_eq!(&request, turn);
        client.await.unwrap();
    }
}

#[tokio::test]
async fn a_second_daemon_is_told_who_has_the_endpoint() {
    let run = run_dir();
    let endpoint = Endpoint::in_run_dir(run.path()).unwrap();
    let _running = Listener::bind(&endpoint).unwrap();

    let error = Listener::bind(&endpoint).unwrap_err();

    // Not "permission denied" (Windows) and not "address already in use" (Unix), neither of which
    // says the thing worth knowing — that a daemon is already up and this one should stand down.
    assert!(
        error.to_string().contains("already listening"),
        "the second daemon was told the wrong thing: {error}"
    );
}

#[tokio::test]
async fn dropping_the_listener_releases_the_endpoint() {
    let run = run_dir();
    let endpoint = Endpoint::in_run_dir(run.path()).unwrap();

    drop(Listener::bind(&endpoint).unwrap());

    Connection::connect(&endpoint)
        .await
        .expect_err("nothing should answer at an endpoint whose listener is gone");

    // And the name is free for the next start rather than needing a cleanup pass.
    Listener::bind(&endpoint).expect("the endpoint should be available again");
}

/// Where the socket file is, taken from the endpoint itself so the test cannot disagree with it.
#[cfg(unix)]
fn socket_of(endpoint: &Endpoint) -> std::path::PathBuf {
    std::path::PathBuf::from(endpoint.to_string())
}

#[cfg(unix)]
#[tokio::test]
async fn the_socket_admits_its_owner_and_nobody_else() {
    use std::os::unix::fs::PermissionsExt as _;

    let run = run_dir();
    let endpoint = Endpoint::in_run_dir(run.path()).unwrap();
    let _listener = Listener::bind(&endpoint).unwrap();

    let mode = std::fs::metadata(socket_of(&endpoint))
        .unwrap()
        .permissions()
        .mode()
        & 0o7777;

    // `bind` creates the socket with whatever the umask allows — 0755 on most machines — so this
    // is checking that the chmod after it happened at all, not that the umask was kind.
    assert_eq!(mode, 0o600, "the socket is reachable by other accounts");
}

#[cfg(unix)]
#[tokio::test]
async fn a_socket_left_behind_by_a_dead_daemon_is_taken_over() {
    let run = run_dir();
    let endpoint = Endpoint::in_run_dir(run.path()).unwrap();
    let socket = socket_of(&endpoint);

    // Exactly what a daemon that was killed rather than stopped leaves: closing the descriptor does
    // not unlink the file, so the next `bind` meets EADDRINUSE with nobody behind it.
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    assert!(socket.exists(), "the test did not set up a stale socket");

    let _listener =
        Listener::bind(&endpoint).expect("a socket nobody is listening on should be replaced");
}

#[cfg(unix)]
#[tokio::test]
async fn a_live_socket_is_not_mistaken_for_a_stale_one() {
    // The other half of the rule above, and the dangerous half: the cleanup must not fire while a
    // daemon is listening, or the second start would unlink the first one's socket and leave it
    // serving an endpoint no client can reach.
    let run = run_dir();
    let endpoint = Endpoint::in_run_dir(run.path()).unwrap();
    let _running = Listener::bind(&endpoint).unwrap();
    let socket = socket_of(&endpoint);
    let before = std::fs::metadata(&socket).unwrap();

    Listener::bind(&endpoint).unwrap_err();

    let after = std::fs::metadata(&socket).unwrap();
    assert_eq!(
        (
            std::os::unix::fs::MetadataExt::dev(&before),
            std::os::unix::fs::MetadataExt::ino(&before)
        ),
        (
            std::os::unix::fs::MetadataExt::dev(&after),
            std::os::unix::fs::MetadataExt::ino(&after)
        ),
        "the running daemon's socket was replaced"
    );
}

#[cfg(unix)]
#[test]
fn a_home_too_deep_for_a_socket_path_says_so_in_bytes() {
    // `bind` answers this with EINVAL, "Invalid argument", naming neither the argument nor the
    // limit. The address has to refuse it first or the user has nothing to act on.
    let deep = Path::new("/tmp").join("d".repeat(200)).join("run");

    let error = Endpoint::in_run_dir(&deep).unwrap_err();

    assert!(
        error.to_string().contains("MIXENGINE_HOME"),
        "the only thing the user can change is missing from: {error}"
    );
}

#[cfg(windows)]
#[test]
fn the_pipe_is_named_after_the_account() {
    // The pipe namespace is flat and machine-wide: two accounts signed in at once each run their
    // own daemon, and the name is the only thing keeping them apart.
    let endpoint =
        Endpoint::in_run_dir(Path::new(r"C:\Users\dev\AppData\Local\MixEngine\run")).unwrap();
    let name = endpoint.to_string();

    assert!(name.starts_with(r"\\.\pipe\mixengine."), "{name}");
    assert!(
        name.contains("S-1-"),
        "the account is not in the name: {name}"
    );
}

#[cfg(windows)]
#[test]
fn two_spellings_of_one_home_reach_one_daemon() {
    // Windows paths are case-insensitive, so `--home C:\dev\sandbox` and
    // `MIXENGINE_HOME=c:\dev\sandbox` name one directory — and must fold to one pipe.
    assert_eq!(
        Endpoint::in_run_dir(Path::new(r"C:\Dev\Sandbox\run")).unwrap(),
        Endpoint::in_run_dir(Path::new(r"c:\dev\sandbox\run")).unwrap()
    );
}
