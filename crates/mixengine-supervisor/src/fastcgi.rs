//! A FastCGI responder client, for the one probe that has to ask php-fpm about itself — roadmap
//! task **T72a**.
//!
//! **Because there is no cross-platform way to count connections to a Unix socket.** Linux
//! publishes them in `/proc/net/unix`, one row per connection with the path on it; macOS's `lsof`
//! has no state filter for a Unix socket the way `-sTCP:ESTABLISHED` is one for a port, and the
//! honest alternative there is `libproc` through FFI. So the question is not put to the operating
//! system at all — php-fpm has kept the answer since PHP 5, and this asks it. **Nothing here is
//! per-OS**: what shape the address is belongs to [`Listen`], which the platform crate owns.
//!
//! Small, because the responder role is small: one `BEGIN_REQUEST`, one block of CGI parameters, an
//! empty `STDIN`, then records read back until `END_REQUEST`. No multiplexing, no request body, no
//! filters — a probe that needed any of those would be testing this file.
//!
//! **There is a second FastCGI client in this workspace**, `mixengine-testkit`'s, and it cannot be
//! this one: testkit is a dev-dependency and `mixengine-proto`'s `workspace_layering.rs` enforces
//! that nothing shipped links it. Its client is also blocking, which this cannot be. The framing has
//! been eight fixed bytes since 1996 and both sides have tests over it.

use std::time::Duration;

use mixengine_platform::activation::{Listen, dial};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// The protocol version, which has been 1 since 1996.
const VERSION: u8 = 1;

/// A web server asking for a request to begin.
const BEGIN_REQUEST: u8 = 1;

/// The server saying this request is over.
const END_REQUEST: u8 = 3;

/// The CGI parameter block, as a stream.
const PARAMS: u8 = 4;

/// The request body, as a stream. Empty for the one question this client asks.
const STDIN: u8 = 5;

/// What the script wrote — headers and body.
const STDOUT: u8 = 6;

/// What it wrote to its error stream.
const STDERR: u8 = 7;

/// The role a caller asks for: run it and give me the output.
const RESPONDER: u16 = 1;

/// The one request this client ever has in flight. Multiplexing is what the id is for.
const REQUEST_ID: u16 = 1;

/// A pool's own listening socket, as the call that dials it needs to see it.
///
/// **Two types for one idea, deliberately**, exactly as the daemon's activator has it: a probe
/// carries a path because that is what a recipe rendered, and [`Listen`] is what the platform dials.
/// Neither crate depends on the other in the direction that would let them share one type.
pub(crate) fn at(socket: &std::path::Path) -> Listen {
    Listen::Socket(socket.to_path_buf())
}

/// Ask a pool for its status page and give back the body it answered with.
///
/// **The whole exchange is under one `patience`**, rather than a timeout per read: what the caller
/// is budgeting is a sweep, and a pool that answers its header quickly and then stalls has spent
/// that sweep either way.
///
/// # Errors
///
/// Whatever the dial or the exchange reported, [`std::io::ErrorKind::TimedOut`] when the pool did
/// not finish answering in time, and [`std::io::ErrorKind::InvalidData`] for an answer this cannot
/// parse.
pub(crate) async fn status(
    listen: &Listen,
    path: &str,
    patience: Duration,
) -> std::io::Result<Vec<u8>> {
    let exchange = async {
        let mut connection = dial(listen)
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))?;

        connection.write_all(&request(path)).await?;
        connection.flush().await?;

        let mut answer = Vec::new();
        connection.read_to_end(&mut answer).await?;

        Ok::<_, std::io::Error>(answer)
    };

    let answer = tokio::time::timeout(patience, exchange)
        .await
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the pool did not answer in time",
            )
        })??;

    Ok(body(&stdout(&answer)?).to_vec())
}

/// The whole request, as bytes: begin, parameters, an empty body.
///
/// **`SCRIPT_NAME` is what php-fpm matches `pm.status_path` against**, so it is the one parameter
/// this cannot get wrong. `QUERY_STRING=json` turns the status page from a paragraph meant for a
/// person into a document with numbers in it.
///
/// `REDIRECT_STATUS` is here for the reason testkit's client documents: a PHP built with
/// `cgi.force_redirect` refuses a request without it outright, and the message is about being
/// called directly rather than about the header.
fn request(path: &str) -> Vec<u8> {
    let mut out = Vec::new();

    let mut begin = Vec::with_capacity(8);
    begin.extend_from_slice(&RESPONDER.to_be_bytes());
    begin.extend_from_slice(&[0; 6]);
    record(&mut out, BEGIN_REQUEST, &begin);

    let mut params = Vec::new();
    pair(&mut params, "GATEWAY_INTERFACE", "CGI/1.1");
    pair(&mut params, "REQUEST_METHOD", "GET");
    pair(&mut params, "SCRIPT_NAME", path);
    pair(&mut params, "SCRIPT_FILENAME", path);
    pair(&mut params, "REQUEST_URI", path);
    pair(&mut params, "QUERY_STRING", "json");
    pair(&mut params, "CONTENT_LENGTH", "0");
    pair(&mut params, "SERVER_PROTOCOL", "HTTP/1.1");
    pair(&mut params, "SERVER_SOFTWARE", "mixengined");
    pair(&mut params, "REMOTE_ADDR", "127.0.0.1");
    pair(&mut params, "REDIRECT_STATUS", "200");
    record(&mut out, PARAMS, &params);
    // An empty record of a stream type is what closes that stream.
    record(&mut out, PARAMS, &[]);
    record(&mut out, STDIN, &[]);

    out
}

/// One record: an eight-byte header and a body, with no padding.
///
/// Nothing here pads. Alignment is an optimisation for a server reading millions of these, and this
/// one sends four.
fn record(out: &mut Vec<u8>, kind: u8, body: &[u8]) {
    let length = u16::try_from(body.len()).expect("a record body under 64 KiB");

    out.push(VERSION);
    out.push(kind);
    out.extend_from_slice(&REQUEST_ID.to_be_bytes());
    out.extend_from_slice(&length.to_be_bytes());
    out.push(0);
    out.push(0);
    out.extend_from_slice(body);
}

/// One name-value pair, in the protocol's one-or-four-byte length encoding.
///
/// A length under 128 is one byte; anything longer is four with the top bit set. **Not an
/// optimisation**: a 200-byte value written as one byte is read as a 72-byte one, and every byte
/// after it is garbage.
fn pair(out: &mut Vec<u8>, name: &str, value: &str) {
    for length in [name.len(), value.len()] {
        let length = u32::try_from(length).expect("a parameter under 4 GiB");

        if let Ok(short) = u8::try_from(length)
            && short < 128
        {
            out.push(short);
        } else {
            out.extend_from_slice(&(length | 0x8000_0000).to_be_bytes());
        }
    }

    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(value.as_bytes());
}

/// Pull the `STDOUT` stream out of a stack of records.
///
/// `STDERR` is read and dropped rather than left out of the match: a pool that wrote a PHP notice
/// there has not written a body, and naming the type is what says the difference was seen rather
/// than missed.
fn stdout(answer: &[u8]) -> std::io::Result<Vec<u8>> {
    let invalid = |what: &str| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("the pool answered something this is not: {what}"),
        )
    };

    let mut out = Vec::new();
    let mut at = 0;

    while at + 8 <= answer.len() {
        let kind = answer[at + 1];
        let length = usize::from(u16::from_be_bytes([answer[at + 4], answer[at + 5]]));
        let padding = usize::from(answer[at + 6]);
        let start = at + 8;

        if start + length > answer.len() {
            return Err(invalid(
                "a record whose body is shorter than its header says",
            ));
        }

        match kind {
            STDOUT => out.extend_from_slice(&answer[start..start + length]),

            STDERR => {}

            END_REQUEST => break,

            // Anything a future server sends that this does not know about.
            _ => {}
        }

        at = start + length + padding;
    }

    Ok(out)
}

/// What follows the CGI blank line — the headers a status page sets are not its answer.
///
/// A body with no blank line in it at all is returned whole: that is a pool that answered something
/// unusual, and handing the caller nothing would turn it into "a status page without numbers",
/// which is a different diagnosis from the true one.
fn body(out: &[u8]) -> &[u8] {
    out.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map_or(out, |at| &out[at + 4..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parameter block carries what php-fpm decides a status request by: `SCRIPT_NAME`.
    #[test]
    fn a_status_request_names_the_path_php_fpm_matches_on() {
        let request = request("/mixengine-status");
        let bytes = String::from_utf8_lossy(&request).into_owned();

        assert!(bytes.contains("SCRIPT_NAME"), "{bytes}");
        assert!(bytes.contains("/mixengine-status"), "{bytes}");
        assert!(
            bytes.contains("json"),
            "the status page answers a paragraph unless asked for json: {bytes}"
        );
    }

    /// **A value of 128 bytes or more is four bytes of length, not one** — the protocol's rule, and
    /// the one that turns a long home's socket path into garbage when it is got wrong.
    #[test]
    fn a_long_parameter_is_written_in_the_four_byte_form() {
        let long = "x".repeat(200);
        let mut out = Vec::new();

        pair(&mut out, "SCRIPT_FILENAME", &long);

        assert_eq!(
            out[1], 0x80,
            "a 200-byte value must set the top bit of a four-byte length"
        );
    }

    /// `STDOUT` is collected across records, `STDERR` is dropped, and `END_REQUEST` ends the read.
    #[test]
    fn stdout_is_gathered_and_the_error_stream_is_not() {
        let mut answer = Vec::new();
        record(
            &mut answer,
            STDOUT,
            b"Content-type: application/json\r\n\r\n{\"pool\"",
        );
        record(&mut answer, STDERR, b"PHP Notice: something");
        record(&mut answer, STDOUT, b":\"www\"}");
        record(&mut answer, END_REQUEST, &[0; 8]);

        let out = stdout(&answer).expect("a well-formed stack of records");

        assert_eq!(
            String::from_utf8_lossy(&out),
            "Content-type: application/json\r\n\r\n{\"pool\":\"www\"}"
        );
    }

    /// A truncated record is an error and never a short body: half a JSON document parsed as a whole
    /// one is how a probe reports a pool as idle on the strength of nothing.
    #[test]
    fn a_record_shorter_than_its_header_says_is_refused() {
        let mut answer = Vec::new();
        record(&mut answer, STDOUT, b"{\"pool\":\"www\"}");
        answer.truncate(answer.len() - 4);

        assert!(stdout(&answer).is_err());
    }

    /// The body is what follows the CGI blank line, headers dropped.
    #[test]
    fn the_body_starts_after_the_blank_line() {
        let out = b"Content-type: application/json\r\n\r\n{\"accepted conn\":3}";

        assert_eq!(body(out), b"{\"accepted conn\":3}");
    }

    /// The whole exchange, against a server that speaks the protocol back.
    ///
    /// **On a port rather than a socket**, deliberately: what is under test is the framing and the
    /// read, neither of which knows which shape it dialled, and a Unix-socket fixture would leave
    /// all of it untested on Windows.
    #[tokio::test]
    async fn a_status_request_comes_back_as_a_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let address = listener.local_addr().expect("its number");

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("the probe connects");

            // Read the request so the client's write completes, then answer and let the stream drop
            // — `read_to_end` is waiting for the close.
            let mut seen = vec![0; 4096];
            let _ = stream.read(&mut seen).await;

            let mut answer = Vec::new();
            record(
                &mut answer,
                STDOUT,
                b"Content-type: application/json\r\n\r\n{\"accepted conn\":7}",
            );
            record(&mut answer, END_REQUEST, &[0; 8]);

            stream
                .write_all(&answer)
                .await
                .expect("the fixture answers");
        });

        let body = status(
            &Listen::Tcp(address),
            "/mixengine-status",
            Duration::from_secs(5),
        )
        .await
        .expect("a pool that answered");

        assert_eq!(String::from_utf8_lossy(&body), "{\"accepted conn\":7}");
    }

    /// A pool that accepts and then says nothing costs the patience and nothing more.
    #[tokio::test]
    async fn a_pool_that_never_answers_times_out() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let address = listener.local_addr().expect("its number");

        tokio::spawn(async move {
            let _held = listener.accept().await.expect("the probe connects");

            // Held open, answering nothing, until this task is dropped with the test.
            std::future::pending::<()>().await;
        });

        let error = status(
            &Listen::Tcp(address),
            "/mixengine-status",
            Duration::from_millis(200),
        )
        .await
        .expect_err("a pool that said nothing is not a reading");

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }
}
