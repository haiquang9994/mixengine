//! Who is listening on a local TCP port, against the real OS.
//!
//! Not `#[ignore]`d and not a system test: every port here is an ephemeral one the OS handed this
//! process for the length of one test. `.claude/standards/testing.md` rules out 53, 80 and 443,
//! which is a different question from binding whatever is free.
//!
//! **The subject is this process.** A test that went looking for somebody else's server would be
//! asserting about whatever the machine running it happens to have installed; a test that binds a
//! port itself knows the pid and the program name the answer has to carry, on every OS.

use std::net::TcpListener;

use mixengine_platform::host;

/// A listening socket on a port nobody else can hold, because the OS just chose it for us.
fn listening() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("the loopback interface accepts a bind");
    let port = listener
        .local_addr()
        .expect("a bound listener has a local address")
        .port();

    (listener, port)
}

#[test]
fn the_process_listening_on_a_port_is_named() {
    let (_listener, port) = listening();

    let holder = host()
        .port_owner()
        .listening_on(port)
        .expect("this machine can be asked who is listening")
        .expect("this test process is listening on the port it just bound");

    assert_eq!(
        holder.pid,
        Some(std::process::id()),
        "the port belongs to this process, so the answer has to be this pid"
    );
    assert!(
        holder
            .name
            .as_deref()
            .is_some_and(|name| name.contains("ports")),
        "the holder is this test binary, whose name starts with the file it is built from; got {:?}",
        holder.name
    );
}

#[test]
fn a_port_nobody_is_listening_on_has_no_holder() {
    let (listener, port) = listening();
    drop(listener);

    assert!(
        host()
            .port_owner()
            .listening_on(port)
            .expect("this machine can be asked who is listening")
            .is_none(),
        "the listener was dropped before the question was asked, so nothing holds the port — \
         a holder here means the answer is reading a table that outlives the socket"
    );
}
