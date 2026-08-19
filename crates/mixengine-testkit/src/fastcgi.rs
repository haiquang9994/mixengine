//! A FastCGI responder client, for the one suite that has to prove a pool is serving PHP.
//!
//! **Because connecting to the socket proves nothing.** A php-fpm that is listening and cannot
//! execute a script — a missing SAPI, a `security.limit_extensions` that refuses the file, a
//! `SCRIPT_FILENAME` the pool cannot see — accepts a connection exactly like one that works. The
//! only claim worth making about a pool is that a request went in and a body came out, and that
//! takes speaking the protocol.
//!
//! Small, because the responder role is small: one `BEGIN_REQUEST`, one block of CGI parameters, an
//! empty `STDIN`, and then records read back until `END_REQUEST`. Nothing here handles multiplexing,
//! filters, authorizers or a request body — a test that needed any of those would be testing this
//! client.
//!
//! **A dev-dependency like everything else in this crate**, so none of it ships.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

/// The protocol version, which has been 1 since 1996.
const VERSION: u8 = 1;

/// A web server asking for a request to begin.
const BEGIN_REQUEST: u8 = 1;

/// The server saying this request is over.
const END_REQUEST: u8 = 3;

/// The CGI parameter block, as a stream.
const PARAMS: u8 = 4;

/// The request body, as a stream. Empty for every question this client asks.
const STDIN: u8 = 5;

/// What the script wrote — headers and body.
const STDOUT: u8 = 6;

/// What it wrote to its error stream.
const STDERR: u8 = 7;

/// The role a web server asks for: run the script and give me its output.
const RESPONDER: u16 = 1;

/// The one request this client ever has in flight. Multiplexing is what the id is for and this does
/// not multiplex.
const REQUEST_ID: u16 = 1;

/// How long a pool is given to answer.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

/// Where a pool listens, in whichever of the two ways this system has.
#[derive(Debug, Clone)]
pub enum Pool {
    /// A Unix domain socket — how a pool listens everywhere php-fpm exists.
    #[cfg(unix)]
    Socket(PathBuf),

    /// A loopback port — how `php-cgi.exe -b` listens on Windows.
    Port(SocketAddr),
}

impl Pool {
    /// A pool on a Unix socket.
    #[cfg(unix)]
    #[must_use]
    pub fn socket(path: impl Into<PathBuf>) -> Self {
        Self::Socket(path.into())
    }

    /// A pool on a TCP port.
    #[must_use]
    pub fn port(addr: SocketAddr) -> Self {
        Self::Port(addr)
    }

    /// Run one script and read what it wrote.
    ///
    /// `GET`, no query string, no body — which is every question this suite asks. The parameters are
    /// the CGI ones php-fpm insists on: without `SCRIPT_FILENAME` there is nothing to run, and
    /// **without `REDIRECT_STATUS` a `php-cgi` built with `cgi.force_redirect` on refuses the
    /// request outright** with a message about being called directly, which is the failure that
    /// costs an afternoon on Windows.
    ///
    /// # Errors
    ///
    /// Whatever the connection or the read reported. A pool that answered something this cannot
    /// parse is an [`std::io::ErrorKind::InvalidData`].
    pub fn get(&self, script: &Path) -> std::io::Result<Response> {
        let script = script.display().to_string();

        let mut request = Vec::new();

        let mut begin = Vec::with_capacity(8);
        begin.extend_from_slice(&RESPONDER.to_be_bytes());
        begin.extend_from_slice(&[0; 6]);
        record(&mut request, BEGIN_REQUEST, &begin);

        let mut params = Vec::new();
        pair(&mut params, "GATEWAY_INTERFACE", "CGI/1.1");
        pair(&mut params, "REQUEST_METHOD", "GET");
        pair(&mut params, "SCRIPT_FILENAME", &script);
        pair(&mut params, "SCRIPT_NAME", "/index.php");
        pair(&mut params, "REQUEST_URI", "/index.php");
        pair(&mut params, "QUERY_STRING", "");
        pair(&mut params, "CONTENT_LENGTH", "0");
        pair(&mut params, "SERVER_PROTOCOL", "HTTP/1.1");
        pair(&mut params, "SERVER_SOFTWARE", "mixengine-testkit");
        pair(&mut params, "REMOTE_ADDR", "127.0.0.1");
        pair(&mut params, "REDIRECT_STATUS", "200");
        record(&mut request, PARAMS, &params);
        // An empty record of a stream type is what closes it.
        record(&mut request, PARAMS, &[]);
        record(&mut request, STDIN, &[]);

        parse(&self.exchange(&request)?)
    }

    /// Send the whole request and read until the connection closes.
    fn exchange(&self, request: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut answer = Vec::new();

        match self {
            #[cfg(unix)]
            Self::Socket(path) => {
                let mut stream = std::os::unix::net::UnixStream::connect(path)?;
                stream.set_read_timeout(Some(PATIENCE))?;
                stream.write_all(request)?;
                stream.read_to_end(&mut answer)?;
            }

            Self::Port(addr) => {
                let mut stream = TcpStream::connect(addr)?;
                stream.set_read_timeout(Some(PATIENCE))?;
                stream.write_all(request)?;
                stream.read_to_end(&mut answer)?;
            }
        }

        Ok(answer)
    }
}

/// What a script wrote, split where CGI splits it.
#[derive(Debug, Clone)]
pub struct Response {
    /// Everything before the blank line — `Content-type`, and a `Status` if the script set one.
    pub headers: String,

    /// Everything after it.
    pub body: String,
}

/// One record: an eight-byte header and a body, with no padding.
fn record(out: &mut Vec<u8>, kind: u8, body: &[u8]) {
    let length = u16::try_from(body.len()).expect("a record body under 64 KiB");

    out.push(VERSION);
    out.push(kind);
    out.extend_from_slice(&REQUEST_ID.to_be_bytes());
    out.extend_from_slice(&length.to_be_bytes());
    // Padding length, then one reserved byte. Nothing here pads: alignment is an optimisation for a
    // server reading millions of these, and this one sends four.
    out.push(0);
    out.push(0);
    out.extend_from_slice(body);
}

/// One name-value pair, in the protocol's one-or-four-byte length encoding.
///
/// A length under 128 is one byte; anything longer is four with the top bit set, which is why the
/// short case is not merely an optimisation — a 200-byte value written as one byte would be read as
/// a 72-byte one and everything after it would be garbage.
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

/// Pull the `STDOUT` stream out of a stack of records, and split it where CGI splits it.
///
/// `STDERR` is read and dropped rather than ignored: a pool that wrote a PHP fatal error there and
/// nothing to `STDOUT` should produce an empty body, which is what the caller then asserts against —
/// not a parse failure that says nothing about PHP.
fn parse(answer: &[u8]) -> std::io::Result<Response> {
    let invalid = |what: &str| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("the pool answered something this is not: {what}"),
        )
    };

    let mut stdout = Vec::new();
    let mut at = 0;

    while at + 8 <= answer.len() {
        let kind = answer[at + 1];
        let length = usize::from(u16::from_be_bytes([answer[at + 4], answer[at + 5]]));
        let padding = usize::from(answer[at + 6]);
        let body = at + 8;

        if body + length > answer.len() {
            return Err(invalid(
                "a record whose body is shorter than its header says",
            ));
        }

        match kind {
            STDOUT => stdout.extend_from_slice(&answer[body..body + length]),

            // Read and dropped rather than left out of the match: a script's notices are not the
            // body, and naming the type here is what says so.
            STDERR => {}

            END_REQUEST => break,

            // Anything a future server sends that this does not know about.
            _ => {}
        }

        at = body + length + padding;
    }

    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let (headers, body) = stdout
        .split_once("\r\n\r\n")
        .or_else(|| stdout.split_once("\n\n"))
        .ok_or_else(|| invalid("output with no blank line between headers and body"))?;

    Ok(Response {
        headers: headers.to_owned(),
        body: body.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name-value pair under 128 bytes takes one length byte at each end, and a longer one takes
    /// four with the top bit set.
    ///
    /// The encoding is the only part of this client that can be silently wrong: a wrong length is a
    /// pool that reads a parameter block it cannot parse and answers nothing, which from a test's
    /// side is indistinguishable from a pool that is not there.
    #[test]
    fn a_parameter_is_encoded_the_way_the_protocol_says() {
        let mut short = Vec::new();
        pair(&mut short, "A", "b");
        assert_eq!(short, [1, 1, b'A', b'b']);

        let long = "x".repeat(200);
        let mut wide = Vec::new();
        pair(&mut wide, "A", &long);
        assert_eq!(&wide[..1], &[1]);
        assert_eq!(&wide[1..5], &[0x80, 0, 0, 200]);
    }

    /// A record carries its body length in two big-endian bytes after the request id.
    #[test]
    fn a_record_header_is_eight_bytes() {
        let mut out = Vec::new();
        record(&mut out, STDIN, b"hi");

        assert_eq!(out, [1, STDIN, 0, 1, 0, 2, 0, 0, b'h', b'i']);
    }

    /// What a pool's answer looks like coming back, and where the body starts.
    ///
    /// Assembled here rather than read off a real pool because what is being checked is the *reader*
    /// — that the `STDERR` a script wrote does not end up in the body, and that a record after
    /// `END_REQUEST` is not read as more output.
    #[test]
    fn the_body_is_what_the_script_wrote_after_the_blank_line() {
        let mut answer = Vec::new();
        record(&mut answer, STDERR, b"a notice nobody asked for");
        record(&mut answer, STDOUT, b"Content-type: text/html\r\n\r\nhello");
        record(&mut answer, END_REQUEST, &[0; 8]);
        record(&mut answer, STDOUT, b"after the end");

        let response = parse(&answer).expect("a well formed answer");

        assert_eq!(response.headers, "Content-type: text/html");
        assert_eq!(response.body, "hello");
    }
}
