//! The sockets, and the task that answers on them — roadmap task **T44**.
//!
//! Everything about *what* to answer is [`super::answer`]'s, and everything here is about getting
//! bytes to it and back: two sockets on loopback, hickory's UDP and TCP framing, and a task that
//! ends with the daemon's root cancellation token.
//!
//! **Loopback and nothing else** — the T44 design, D8. Binding `127.0.0.1` rather than the wildcard
//! address is the whole of this server's access control: a query from off the machine cannot
//! arrive, so there is no source address to check and — with no forwarder behind it — nothing to
//! abuse if one did.
//!
//! **Both transports, always.** A DNS client that receives a truncated answer retries over TCP, and
//! a server that registered only a UDP socket fails that retry by refusing the connection, which
//! reads as a network problem rather than as a missing listener. They are bound as a pair or not at
//! all.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use hickory_proto::op::{Edns, Header, HeaderCounts, MessageType, Metadata, ResponseCode};
use hickory_server::net::runtime::Time;
use hickory_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo};
use hickory_server::zone_handler::MessageResponseBuilder;
use tokio::net::{TcpListener, UdpSocket};
use tokio_util::sync::CancellationToken;

use super::answer;

/// How long a TCP connection may stay open without sending a request.
///
/// A stub resolver opens one, asks, reads and goes; five seconds is generous for that and short
/// enough that a connection nobody is using does not sit there. hickory closes it on expiry.
const TCP_IDLE: Duration = Duration::from_secs(5);

/// How many responses may be queued on one TCP connection at once.
///
/// A DNS-over-TCP connection carries one question at a time in every stub resolver in ordinary use;
/// the buffer only means anything to a client that pipelines, and thirty-two outstanding answers is
/// far past what any of them do.
const TCP_RESPONSE_BUFFER: usize = 32;

/// How many times [`bind`] may try again when the operating system picked the port.
///
/// Only reachable with `port = 0`, where UDP is bound first and TCP has to be asked for *the same*
/// number afterwards — which can lose a race with anything else on the machine binding ephemeral
/// ports. A configured port never retries: there is nothing to try differently.
const EPHEMERAL_ATTEMPTS: usize = 8;

/// The policy, as something hickory can call.
///
/// Holds nothing: every answer is a function of the question ([`super::answer`]), so there is no
/// zone, no cache and no state to share between requests.
#[derive(Debug, Clone, Copy)]
struct Handler;

#[async_trait::async_trait]
impl RequestHandler for Handler {
    async fn handle_request<R: ResponseHandler, T: Time>(
        &self,
        request: &Request,
        mut response_handle: R,
    ) -> ResponseInfo {
        let reply = answer::reply(request.metadata.op_code, request.queries.queries());

        let mut metadata = Metadata::response_from_request(&request.metadata);
        metadata.authoritative = reply.authoritative;
        // Never set, and it is not an oversight: this server does not recurse (D1), so saying it
        // could would be inviting a client to ask it to.
        metadata.recursion_available = false;
        metadata.response_code = reply.code;

        let mut builder = MessageResponseBuilder::from_message_request(request);

        // Declared out here so the borrow the builder takes outlives the branch that made it.
        let response_edns;
        if let Some(requested) = request.edns.as_ref() {
            let mut edns = Edns::new();
            edns.set_version(0);
            // Never below the 512 bytes a message without EDNS is limited to, however small the
            // client claimed its buffer was.
            edns.set_max_payload(requested.max_payload().max(512));

            response_edns = edns;
            builder.edns(&response_edns);
        }

        let response = builder.build(
            metadata,
            &reply.answers,
            std::iter::empty(),
            // hickory chains this onto the authority section, which is where a negative answer's
            // SOA belongs (RFC 2308).
            &reply.authority,
            std::iter::empty(),
        );

        match response_handle.send_response(response).await {
            Ok(info) => info,
            Err(error) => {
                // The client is gone, or the socket refused the write. There is nobody left to tell,
                // so this is a log line and a header for hickory's own accounting.
                tracing::debug!(%error, "a DNS answer could not be sent");

                let mut metadata = Metadata::new(
                    request.metadata.id,
                    MessageType::Response,
                    request.metadata.op_code,
                );
                metadata.response_code = ResponseCode::ServFail;

                ResponseInfo::from(Header {
                    metadata,
                    counts: HeaderCounts::default(),
                })
            }
        }
    }
}

/// Bind both transports on `127.0.0.1:port`, or say why not.
///
/// The two have to share a port number, which is why UDP is bound first and TCP is asked for
/// whatever UDP was given: with `port = 0` the operating system would otherwise hand out two
/// different ones.
async fn bind(port: u16) -> io::Result<(UdpSocket, TcpListener)> {
    let mut last: Option<io::Error> = None;

    for _ in 0..EPHEMERAL_ATTEMPTS {
        let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, port)).await?;
        let address = udp.local_addr()?;

        match TcpListener::bind(address).await {
            Ok(tcp) => return Ok((udp, tcp)),
            // A configured port that is half-taken is a failure to report, not one to work around:
            // answering on a different number than the one the resolver was wired to would be
            // worse than not answering at all.
            Err(error) if port != 0 => return Err(error),
            Err(error) => last = Some(error),
        }
    }

    Err(last.unwrap_or_else(|| io::Error::other("no ephemeral port was free for both UDP and TCP")))
}

/// Start answering on `port`, and stop when `shutdown` is cancelled.
///
/// Returns the address actually bound, which is not always the one asked for: `port = 0` lets the
/// operating system choose, and the caller has to be told which one it chose.
///
/// # Errors
///
/// Whatever the bind refused with. **The caller is expected to carry on** — a daemon that would not
/// start because its DNS port was taken is a worse machine than one that says so and falls back to
/// the hosts file (D6).
pub(super) async fn start(port: u16, shutdown: CancellationToken) -> io::Result<SocketAddr> {
    let (udp, tcp) = bind(port).await?;
    let address = udp.local_addr()?;

    let mut server = hickory_server::Server::new(Handler);
    server.register_socket(udp);
    server.register_listener(tcp, TCP_IDLE, TCP_RESPONSE_BUFFER);

    tokio::spawn(async move {
        tokio::select! {
            () = shutdown.cancelled() => {
                if let Err(error) = server.shutdown_gracefully().await {
                    tracing::debug!(%error, "the DNS server did not stop cleanly");
                }
            }
            result = server.block_until_done() => {
                // Both sockets gave up on their own, which is not something a shutdown asked for.
                if let Err(error) = result {
                    tracing::warn!(%error, "the DNS server stopped answering");
                }
            }
        }
    });

    Ok(address)
}

#[cfg(test)]
mod tests {
    use hickory_proto::op::{Message, MessageType, OpCode, Query};
    use hickory_proto::rr::rdata::A;
    use hickory_proto::rr::{Name, RData, RecordType};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpStream;

    use super::*;

    /// How long a test waits for an answer before calling it a failure rather than a hang.
    const PATIENCE: Duration = Duration::from_secs(5);

    /// A server on a port the operating system chose, and the token that stops it.
    ///
    /// **Never the configured default**: `.claude/standards/testing.md` forbids a test binding 53,
    /// and an ephemeral port is also what lets several of these run beside each other in one
    /// `cargo test --workspace`.
    async fn answering() -> (SocketAddr, CancellationToken) {
        let shutdown = CancellationToken::new();
        let address = start(0, shutdown.clone())
            .await
            .expect("an ephemeral port is free for both transports");

        (address, shutdown)
    }

    /// An ordinary question, encoded.
    fn asking(name: &str, record_type: RecordType) -> Message {
        let mut query = Query::new();
        query
            .set_name(Name::from_ascii(name).expect("a name"))
            .set_query_type(record_type);

        let mut message = Message::query();
        message.add_query(query);
        message
    }

    /// Send one message over UDP and parse what comes back.
    async fn over_udp(address: SocketAddr, message: &Message) -> Message {
        let client = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("a client socket");
        client.connect(address).await.expect("a connected socket");
        client
            .send(&message.to_vec().expect("a message encodes"))
            .await
            .expect("the question goes");

        let mut buffer = [0_u8; 4_096];
        let read = tokio::time::timeout(PATIENCE, client.recv(&mut buffer))
            .await
            .expect("an answer arrives")
            .expect("an answer is read");

        Message::from_vec(&buffer[..read]).expect("an answer parses")
    }

    /// The same, over TCP, which carries a two-byte length in front of every message.
    async fn over_tcp(address: SocketAddr, message: &Message) -> Message {
        let mut stream = TcpStream::connect(address).await.expect("a connection");
        let question = message.to_vec().expect("a message encodes");

        let length = u16::try_from(question.len()).expect("a short question");
        stream
            .write_all(&length.to_be_bytes())
            .await
            .expect("the length goes");
        stream
            .write_all(&question)
            .await
            .expect("the question goes");

        let mut header = [0_u8; 2];
        tokio::time::timeout(PATIENCE, stream.read_exact(&mut header))
            .await
            .expect("an answer arrives")
            .expect("a length is read");

        let mut answer = vec![0_u8; usize::from(u16::from_be_bytes(header))];
        stream
            .read_exact(&mut answer)
            .await
            .expect("an answer is read");

        Message::from_vec(&answer).expect("an answer parses")
    }

    fn answered(message: &Message) -> Vec<RData> {
        message
            .answers
            .iter()
            .map(|record| record.data.clone())
            .collect()
    }

    /// The whole feature, over the transport a browser's resolver actually uses.
    #[tokio::test]
    async fn a_managed_name_answers_loopback_over_udp() {
        let (address, shutdown) = answering().await;

        let answer = over_udp(address, &asking("api.blog.test.", RecordType::A)).await;

        assert_eq!(answer.metadata.response_code, ResponseCode::NoError);
        assert!(answer.metadata.authoritative);
        assert!(
            !answer.metadata.recursion_available,
            "this server does not recurse and must not claim it could"
        );
        assert_eq!(answered(&answer), vec![RData::A(A(Ipv4Addr::LOCALHOST))]);

        shutdown.cancel();
    }

    /// **TCP is asserted separately because a truncated answer sends a client there**, and a server
    /// registered on UDP alone refuses that connection — which reads as a network fault rather than
    /// as a listener nobody bound.
    #[tokio::test]
    async fn the_same_question_is_answered_over_tcp() {
        let (address, shutdown) = answering().await;

        let answer = over_tcp(address, &asking("blog.test.", RecordType::A)).await;

        assert_eq!(answer.metadata.response_code, ResponseCode::NoError);
        assert_eq!(answered(&answer), vec![RData::A(A(Ipv4Addr::LOCALHOST))]);

        shutdown.cancel();
    }

    /// D1 on the wire: a name outside the managed TLDs is refused — not forwarded, and not answered
    /// with an `NXDOMAIN` this server has no standing to give.
    #[tokio::test]
    async fn a_name_outside_the_managed_tlds_is_refused_on_the_wire() {
        let (address, shutdown) = answering().await;

        let answer = over_udp(address, &asking("example.com.", RecordType::A)).await;

        assert_eq!(answer.metadata.response_code, ResponseCode::Refused);
        assert!(answer.answers.is_empty());

        shutdown.cancel();
    }

    /// D3 and D10 on the wire: no `AAAA` record, and an `SOA` so a resolver can cache the absence
    /// instead of asking again on every connection.
    #[tokio::test]
    async fn aaaa_comes_back_empty_with_an_soa_to_cache_it_by() {
        let (address, shutdown) = answering().await;

        let answer = over_udp(address, &asking("blog.test.", RecordType::AAAA)).await;

        assert_eq!(answer.metadata.response_code, ResponseCode::NoError);
        assert!(answer.answers.is_empty());
        assert!(
            answer
                .authorities
                .iter()
                .any(|record| matches!(record.data, RData::SOA(_))),
            "{answer:?}"
        );

        shutdown.cancel();
    }

    /// An `OPT` record survives the round trip, which is the half of the protocol this crate hands
    /// to hickory rather than implementing. A feature trimmed off `hickory-server` would break this
    /// and nothing else in the suite.
    #[tokio::test]
    async fn a_question_carrying_edns_is_answered_with_edns() {
        let (address, shutdown) = answering().await;

        let mut message = asking("blog.test.", RecordType::A);
        message.set_edns(hickory_proto::op::Edns::new());

        let answer = over_udp(address, &message).await;

        assert_eq!(answer.metadata.response_code, ResponseCode::NoError);
        assert!(answer.edns.is_some(), "{answer:?}");

        shutdown.cancel();
    }

    /// The opcode row of the table, reached the only way it can be: through a real encoder. There
    /// is no zone here to update, and saying so is cheaper than deciding what an update would mean.
    #[tokio::test]
    async fn an_update_is_refused_rather_than_attempted() {
        let (address, shutdown) = answering().await;

        let mut message = Message::new(4_242, MessageType::Query, OpCode::Update);
        message.add_query(asking("blog.test.", RecordType::A).queries[0].clone());

        let answer = over_udp(address, &message).await;

        assert_eq!(answer.metadata.response_code, ResponseCode::Refused);

        shutdown.cancel();
    }

    /// The token every other task in this daemon hangs off stops this one too — otherwise a
    /// `daemon.shutdown` would leave two sockets bound and the next start would find its own port
    /// taken.
    #[tokio::test]
    async fn a_cancelled_token_stops_the_server_answering() {
        let (address, shutdown) = answering().await;

        // Answering first, so that what is asserted below is the shutdown rather than a server that
        // never came up.
        let before = over_udp(address, &asking("blog.test.", RecordType::A)).await;
        assert_eq!(before.metadata.response_code, ResponseCode::NoError);

        shutdown.cancel();

        // The listener goes with the task, so a fresh connection is refused. Bounded rather than
        // looped forever: "refused" is the assertion, and a hang should fail here rather than as a
        // timeout on whichever CI runner is slowest that day.
        let closed = tokio::time::timeout(PATIENCE, async {
            while TcpStream::connect(address).await.is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await;

        assert!(closed.is_ok(), "the server went on accepting connections");
    }

    /// A configured port that is taken fails rather than quietly answering somewhere else: a
    /// resolver is wired to a number, and a server on a different one is worse than none.
    #[tokio::test]
    async fn a_configured_port_that_is_taken_is_reported_rather_than_moved() {
        let (address, shutdown) = answering().await;

        let error = start(address.port(), CancellationToken::new())
            .await
            .expect_err("the port is already this server's");

        assert!(
            !format!("{error}").is_empty(),
            "a bind failure has to carry the operating system's own words"
        );

        shutdown.cancel();
    }
}
