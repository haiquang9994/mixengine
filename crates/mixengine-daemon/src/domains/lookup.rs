//! The two instruments a domain diagnostic is made of — roadmap task **T46**.
//!
//! Both answer a question no stored fact can: what this machine *does*, right now, rather than what
//! it was configured to do. That distinction is the whole reason they exist — T45 shipped a Linux
//! wiring whose own probe agreed with it while not one name resolved, and nothing in the
//! configuration could have said so.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs as _, UdpSocket};
use std::time::Duration;

/// What this machine's ordinary resolver answers for `name`, or an empty list.
///
/// **`getaddrinfo`, and never `nslookup`.** Measured in T45: `nslookup` talks to the configured
/// server directly and does not honour the Name Resolution Policy Table, so on Windows it answers
/// NXDOMAIN for a name `getaddrinfo` resolves at the same moment. The instrument has to be the one
/// the operating system gives ordinary programs, because those are what the answer is about.
///
/// **The operating system's cache is included, deliberately.** T45's system test had to defeat it —
/// a fresh name every poll — because it was asking whether a *mechanism* works. This asks a
/// different question: what does the user's browser see right now, and the cached answer is that
/// answer.
///
/// **`bound` stops this waiting; it does not stop the lookup.** [`tokio::task::spawn_blocking`]
/// cannot be cancelled, so a resolver that hangs holds one blocked thread until it gives up on its
/// own. That is the honest cost, and it is written here rather than hidden behind a timeout that
/// reads like a cancellation (T46 design, D6).
pub(crate) async fn resolves(name: &str, bound: Duration) -> Vec<IpAddr> {
    // `getaddrinfo` resolves a *service*, so it wants a port. Every caller here is asking about a
    // name; 80 is arbitrary and never leaves this function.
    let asked = format!("{name}:80");

    let lookup = tokio::task::spawn_blocking(move || {
        asked
            .to_socket_addrs()
            .map(|found| found.map(|address| address.ip()).collect::<Vec<_>>())
            .unwrap_or_default()
    });

    tokio::time::timeout(bound, lookup)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default()
}

/// What this daemon's own DNS server answers for `name`, asked over its socket.
///
/// **Over the socket rather than of the zone** (T46 design, D7). Asking the zone proves the
/// answering logic; asking the socket proves the *listener*, which is the only fact that separates
/// "the server died" from "nothing on this machine sends it a name" — the two failures the report
/// this feeds exists to tell apart.
pub(crate) async fn server_answers(
    server: SocketAddr,
    name: &str,
    bound: Duration,
) -> Option<Ipv4Addr> {
    let query = query_for(name);

    tokio::task::spawn_blocking(move || {
        let socket = UdpSocket::bind(("127.0.0.1", 0)).ok()?;
        socket.set_read_timeout(Some(bound)).ok()?;
        socket.send_to(&query, server).ok()?;

        let mut buffer = [0u8; 512];
        let (read, _from) = socket.recv_from(&mut buffer).ok()?;

        first_a(buffer.get(..read)?)
    })
    .await
    .ok()
    .flatten()
}

/// One `A` query for `name`.
fn query_for(name: &str) -> Vec<u8> {
    let mut out = vec![0x4E, 0x36, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];

    for label in name.split('.') {
        // A label longer than 255 bytes cannot be written down at all, and `core::domains` refused
        // it long before this. Returning the truncated query rather than truncating the *label* is
        // what keeps a malformed name from becoming a well-formed question about another one.
        let Ok(length) = u8::try_from(label.len()) else {
            return out;
        };

        out.push(length);
        out.extend_from_slice(label.as_bytes());
    }

    out.extend_from_slice(&[0, 0, 1, 0, 1]);
    out
}

/// The first `A` record in a reply, or [`None`] for anything else.
///
/// [`None`] covers a reply carrying no answers, which is what `REFUSED` and `NODATA` both look like
/// from here — and both are correct answers from T44's server for a name outside a managed TLD.
fn first_a(packet: &[u8]) -> Option<Ipv4Addr> {
    let answers = u16::from_be_bytes([*packet.get(6)?, *packet.get(7)?]);
    if answers == 0 {
        return None;
    }

    // Past the header, then past the question's name, and past its type and class.
    let mut at = past_name(packet, 12)? + 4;

    for _ in 0..answers {
        at = past_name(packet, at)?;

        let kind = u16::from_be_bytes([*packet.get(at)?, *packet.get(at + 1)?]);
        let length = usize::from(u16::from_be_bytes([
            *packet.get(at + 8)?,
            *packet.get(at + 9)?,
        ]));
        at += 10;

        if kind == 1 && length == 4 {
            return Some(Ipv4Addr::new(
                *packet.get(at)?,
                *packet.get(at + 1)?,
                *packet.get(at + 2)?,
                *packet.get(at + 3)?,
            ));
        }

        at += length;
    }

    None
}

/// The offset just past a name, whether it was written out or compressed into a pointer.
fn past_name(packet: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let length = usize::from(*packet.get(at)?);

        // The two high bits set means a pointer, which is two bytes and ends the name.
        if length & 0xC0 == 0xC0 {
            return Some(at + 2);
        }

        at += 1;

        if length == 0 {
            return Some(at);
        }

        at += length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The control**, and a test of its own rather than a line inside another: every assertion
    /// below concludes something from a name that did *not* resolve, and without this that
    /// conclusion is a statement about `getaddrinfo` rather than about the machine. Four of the six
    /// CI rounds behind T45 were void for exactly that (T46 design, D9).
    #[tokio::test]
    async fn the_instrument_resolves_a_name_every_machine_has() {
        let found = resolves("localhost", Duration::from_secs(5)).await;

        assert!(!found.is_empty(), "localhost did not resolve");
    }

    /// A name under a TLD reserved to resolve nowhere, on a machine no test wires.
    #[tokio::test]
    async fn a_name_nothing_routes_resolves_to_nothing() {
        // **CONTROL, in this test and not only in the one above** — a control taken in another test
        // is a control taken at another moment.
        assert!(
            !resolves("localhost", Duration::from_secs(5))
                .await
                .is_empty(),
            "localhost did not resolve; the assertion below would mean nothing"
        );

        let found = resolves(
            &format!("t46-{}.test", std::process::id()),
            Duration::from_secs(5),
        )
        .await;

        assert!(found.is_empty(), "{found:?}");
    }

    /// A server nothing is listening on answers nothing, and says so rather than hanging.
    #[tokio::test]
    async fn a_server_that_is_not_there_answers_nothing() {
        let nowhere = UdpSocket::bind(("127.0.0.1", 0)).expect("an ephemeral port");
        let address = nowhere.local_addr().expect("its address");
        drop(nowhere);

        let answer = server_answers(address, "blog.test", Duration::from_millis(500)).await;

        assert_eq!(answer, None);
    }

    /// A server that answers is heard, which is the half the test above cannot prove.
    ///
    /// The reply is assembled by hand rather than by starting T44's server: what is under test here
    /// is the reading of a packet, and a test that needed the whole DNS server to prove it would
    /// fail for reasons that have nothing to do with the parser.
    #[tokio::test]
    async fn an_a_record_is_read_out_of_a_reply() {
        let server = UdpSocket::bind(("127.0.0.1", 0)).expect("an ephemeral port");
        let address = server.local_addr().expect("its address");

        std::thread::spawn(move || {
            let mut buffer = [0u8; 512];
            let Ok((read, peer)) = server.recv_from(&mut buffer) else {
                return;
            };

            let mut reply = buffer[..read].to_vec();
            reply[2] = 0x81;
            reply[3] = 0x80;
            reply[6] = 0;
            reply[7] = 1;
            reply.extend_from_slice(&[0xC0, 0x0C, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 127, 0, 0, 1]);

            let _ = server.send_to(&reply, peer);
        });

        let answer = server_answers(address, "blog.test", Duration::from_secs(2)).await;

        assert_eq!(answer, Some(Ipv4Addr::LOCALHOST));
    }
}
