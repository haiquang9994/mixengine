//! *Does this URL answer, and with what?* — the one question both HTTP checks ask.
//!
//! `ReadyCheck::Http` and `HealthProbe::Http` are the same request with two different verdicts
//! around it: readiness retries until the status matches or the deadline passes, health asks once
//! and folds the answer into a run. So the request lives here and neither module owns it.
//!
//! # Two failures, and only one of them is an error
//!
//! A connection refused, a socket that closed, a server that answered with something unparseable:
//! none of those is a failure of the supervisor. They are the answer *no*, which is what a service
//! that is not up yet looks like from outside — [`Endpoint::ask`] reports them as [`None`] and the
//! caller decides whether to retry or to count a failure.
//!
//! A URL that is not a URL is the other kind, and it is the spec's fault rather than the service's.
//! It is found by [`Endpoint::parse`] **before anything waits**, on the same reasoning
//! [`crate::ready`] compiles a `LogPattern`'s regex up front: a check that can never pass should not
//! be reported as a service that never came up, or whoever reads it goes looking at the wrong thing.
//!
//! # Plaintext only, and it says so
//!
//! Every URL a MixEngine spec has any business naming is on the loopback interface — Caddy's admin
//! endpoint, php-fpm's status page, a runtime's own health route — and none of them is HTTPS. An
//! `https://` check therefore answers [`Error::UnsupportedCheck`] naming what is missing, rather
//! than pulling a TLS stack and a certificate store into the supervisor to verify a certificate for
//! `127.0.0.1`.

use std::time::Duration;

use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::{Method, Request, Uri};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

use crate::{Error, Result};

/// Where an HTTP check points, once it has been read and found to be readable.
///
/// Parsed rather than carried as a string so the failure happens once, at the top of a check,
/// instead of once per attempt inside a retry loop that would otherwise report a bad URL as a
/// timeout twelve hundred times.
#[derive(Debug, Clone)]
pub(crate) struct Endpoint {
    /// The host to connect to, resolved at each attempt rather than here — a name that does not
    /// resolve *yet* is a service that is not up yet, which is the retryable answer.
    ///
    /// **Not the host as the URL wrote it**: an IPv6 literal arrives here without its brackets, for
    /// the reason [`unbracketed`] sets out.
    host: String,

    /// The port, defaulted to 80 the way every client defaults it.
    port: u16,

    /// The request target in origin form — the path and query, with the `/` an empty path means.
    target: String,

    /// What goes in the `Host` header, which HTTP/1.1 requires and hyper will not invent for a
    /// request whose URI is in origin form.
    ///
    /// The host and the port and nothing else, which is the whole of what RFC 9110 allows there.
    authority: String,
}

impl Endpoint {
    /// Read the URL a check names.
    ///
    /// `check` is what the error calls this check — `"an HTTP ready check"` — so one message serves
    /// both callers without either of them re-describing the failure.
    ///
    /// # Errors
    ///
    /// [`Error::Url`] for something that is not a URL, or is one without the parts a request needs.
    /// [`Error::UnsupportedCheck`] for a scheme this build does not speak, which is every scheme
    /// except `http`.
    pub(crate) fn parse(url: &str, check: &'static str) -> Result<Self> {
        let uri: Uri = url.parse().map_err(|source| Error::Url {
            url: url.to_owned(),
            source: Box::new(source),
        })?;

        match uri.scheme_str() {
            Some("http") => {}

            Some("https") => {
                return Err(Error::UnsupportedCheck {
                    check,
                    reason: format!(
                        "{url} is HTTPS, and this build makes plaintext requests only — a check \
                         against a service on this machine should name its `http://` address"
                    ),
                });
            }

            other => {
                return Err(Error::Url {
                    url: url.to_owned(),
                    source: format!(
                        "a check's URL needs an `http://` scheme, and this one has {}",
                        other.map_or_else(|| "none".to_owned(), |scheme| format!("`{scheme}`")),
                    )
                    .into(),
                });
            }
        }

        // The empty one is refused with the absent one, and it is the case that needs saying: a URL
        // may write `http://:2019/health` — a plausible shorthand for "this machine" — and it parses
        // perfectly well into a host of `""`. What that host resolves to is then the resolver's
        // opinion rather than anybody's intention, and the two systems disagree: glibc answers
        // `EAI_NONAME`, so the check can never connect and spends its whole timeout being reported
        // as a service that never came up, while Windows answers with **this machine's LAN
        // addresses** — so the same spec quietly aims at whatever is listening on port 2019 of an
        // interface the author never named, where it can answer `200` for a different service
        // entirely. One spec, two wrong answers, neither of them the author's: `CLAUDE.md`'s
        // "cross-platform or not merged" and this module's promise to find a check that can never
        // pass *before* anything waits on it are the same rule here.
        let host = uri
            .host()
            .filter(|host| !host.is_empty())
            .ok_or_else(|| Error::Url {
                url: url.to_owned(),
                source: "a check's URL needs a host to connect to".into(),
            })?;

        // Read once, because the two fields below disagree about what to do with it: a connection
        // always needs a port and a `Host` header only carries one the URL named.
        //
        // Asked of the authority first, because `Uri::port_u16` answers `None` to two different
        // questions — a URL that named no port at all, and one that named `:99999` or a bare `:`,
        // neither of which fits a `u16`. Defaulting the second to 80 aimed the check at whatever
        // else happens to be listening on loopback, where it can pass against a different service
        // entirely; a mistyped port is the spec's error, and this module promises to find those
        // before anything waits. `:0` is the third question and is refused with them, for the
        // reason on the arm below.
        let named_port = match port_text(&uri) {
            None => None,

            // Zero fits a `u16` and is refused here anyway, which is the same rule as the line
            // above rather than an extra one: `:0` is how an operating system is asked to *choose*
            // a port, so nothing is ever listening on it and a connection to it is refused on every
            // system there is. A check aimed at it can only ever run out its whole timeout and be
            // reported as a service that never came up — the reader sent to look at the service for
            // what is a typo in the spec, which is exactly what this module promises not to do.
            Some(text) => match uri.port_u16().filter(|port| *port != 0) {
                Some(port) => Some(port),

                None => {
                    return Err(Error::Url {
                        url: url.to_owned(),
                        source: format!(
                            "a check's URL names `{text}` as its port, and a port is a number \
                             between 1 and 65535"
                        )
                        .into(),
                    });
                }
            },
        };

        Ok(Self {
            host: unbracketed(host).to_owned(),
            port: named_port.unwrap_or(80),
            target: uri
                .path_and_query()
                .map_or_else(|| "/".to_owned(), ToString::to_string),
            // Built from the host and the port rather than taken from `Uri::authority`, which also
            // carries the userinfo a URL is allowed to have: `http://user:pw@127.0.0.1:2019/` would
            // otherwise send `Host: user:pw@127.0.0.1:2019` — not a host, so matched against no
            // virtual host, and a credential put on the wire and into whatever the server logs.
            //
            // The brackets stay on this side, because this half really is the URL's authority and
            // `[::1]:2019` is how RFC 3986 spells one. The port belongs in the header when the URL
            // named one, because a service behind a virtual host answers differently for `localhost`
            // and `localhost:2019`.
            authority: named_port.map_or_else(|| host.to_owned(), |port| format!("{host}:{port}")),
        })
    }

    /// Ask once. The status it answered with, or [`None`] if it did not answer at all.
    ///
    /// **Unbounded on its own**, deliberately: the deadline belongs to the check, which is a
    /// timeout in the health case and a whole retry budget in the ready one, and a second one in
    /// here would be a limit nobody wrote in a spec. A caller that does not impose one is the bug
    /// this note exists to prevent — see [`Self::answered`] for the version that takes the deadline
    /// as an argument.
    pub(crate) async fn ask(&self) -> Option<u16> {
        // `(&str, u16)` rather than a `SocketAddr`, so a URL naming a name rather than an address
        // is resolved — every attempt, because a resolver that has no answer for a service that is
        // still starting is the retryable case.
        let stream = TcpStream::connect((self.host.as_str(), self.port))
            .await
            .ok()?;

        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .ok()?;

        let request = Request::builder()
            .method(Method::GET)
            .uri(&self.target)
            .header(hyper::header::HOST, &self.authority)
            .body(Empty::<Bytes>::new())
            .ok()?;

        // The connection is a future that has to be polled for the request to make any progress at
        // all, and there is no task here to spawn it onto — this crate has no runtime of its own on
        // purpose. Polled beside the request instead.
        let mut connection = std::pin::pin!(connection);
        let mut sending = std::pin::pin!(sender.send_request(request));

        let arrived = tokio::select! {
            biased;

            response = &mut sending => Some(response),

            // **The connection ending is not an answer, and reading it as one was a real bug.** The
            // connection future is what reads the socket, so it is the poll in which the response
            // arrives that also sees the end of a server which answers and closes at once — which is
            // every server here, since nothing asks to keep the connection alive. Both arms become
            // ready in the same turn, and taking this one reported a service that replied perfectly
            // well as one that never did.
            _ = &mut connection => None,
        };

        // So whatever the connection managed to deliver is asked for again rather than assumed
        // absent. With the connection finished this resolves without it: with the response if one
        // arrived, and with an error if the server really did hang up first.
        let response = match arrived {
            Some(response) => response,
            None => sending.await,
        };

        Some(response.ok()?.status().as_u16())
    }

    /// Ask once, and give up after `patience`. [`None`] covers both not answering and not answering
    /// in time.
    ///
    /// The health check's shape: one probe, bounded, folded into a run. A database that accepts the
    /// connection and then says nothing is the case this exists for — without the deadline it looks
    /// exactly like a service that is fine.
    pub(crate) async fn answered(&self, patience: Duration) -> Option<u16> {
        tokio::time::timeout(patience, self.ask()).await.ok()?
    }
}

/// What a URL wrote after its host's `:`, or [`None`] if it named no port at all.
///
/// [`Uri::port_u16`] cannot answer this, because it collapses "no port" and "not a port" into the
/// same [`None`] — and only one of those may be defaulted. The text rather than a number, so the
/// error can quote back what the spec actually said.
///
/// Read off the authority rather than re-parsed, which means stepping over the two things in there
/// that carry colons of their own: the userinfo a URL is allowed to have (`user:pw@host`) and an
/// IPv6 literal (`[::1]:2019`), where only the colon past the closing bracket is a separator.
fn port_text(uri: &Uri) -> Option<&str> {
    let authority = uri.authority()?.as_str();

    let after_userinfo = authority
        .rfind('@')
        .map_or(authority, |at| &authority[at + 1..]);

    let host_ends = after_userinfo.rfind(']').map_or(0, |bracket| bracket + 1);

    after_userinfo[host_ends..]
        .find(':')
        .map(|colon| &after_userinfo[host_ends + colon + 1..])
}

/// An IPv6 literal without the brackets the URL wrote it in. Anything else, unchanged.
///
/// **The URL and the resolver want different strings, and this is where they part.** `Uri::host`
/// keeps the brackets deliberately — `[::1]` is what the *authority* says, and the header below is
/// built from it — but nothing is ever resolving a URL. `getaddrinfo` is handed a host name, and
/// `[::1]` is not one: glibc and the BSDs refuse it outright, so `http://[::1]:2019/` would have
/// been a check that could never connect, reported as a service that never came up. Windows'
/// resolver happens to accept the brackets, which is exactly how a bug like this reaches a merge —
/// `CLAUDE.md`'s "cross-platform or not merged" is aimed at this shape of thing.
///
/// hyper's own connector does the same strip at the same point, for the same reason.
fn unbracketed(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|inside| inside.strip_suffix(']'))
        .unwrap_or(host)
}

/// A server that answers a list of statuses, for the tests of both HTTP checks.
///
/// Raw rather than built on a real HTTP server, and it costs a dozen lines: what these tests need is
/// a socket that says `503` twice and then `200`, which is a service coming up — and a server crate
/// would be a second HTTP stack in the build for the sake of a fixture.
#[cfg(test)]
pub(crate) mod fake {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    /// A listener answering requests, until it is dropped.
    #[derive(Debug)]
    pub(crate) struct Server {
        addr: SocketAddr,
        serving: JoinHandle<()>,
    }

    impl Server {
        /// Answer these statuses in order, repeating the last one for ever.
        ///
        /// The repetition is what makes one fixture serve both checks: a health probe asks once and
        /// reads the first entry, a ready check asks until it gets what it wanted and walks the list.
        pub(crate) async fn answering(statuses: &[u16]) -> Self {
            Self::answering_on("127.0.0.1:0", statuses)
                .await
                .expect("every machine has an IPv4 loopback")
        }

        /// As [`Self::answering`], on an address of the caller's choosing.
        ///
        /// [`None`] when this machine cannot bind it, which is how the IPv6 test skips itself on a
        /// machine that has no IPv6 rather than failing for something it is not about.
        pub(crate) async fn answering_on(bind: &str, statuses: &[u16]) -> Option<Self> {
            // Here rather than in the accept task, where the panic would reach the client as "no
            // answer" and read as the thing under test failing instead of the fixture being empty.
            assert!(!statuses.is_empty(), "a server answers at least one status");

            let listener = TcpListener::bind(bind).await.ok()?;
            let addr = listener.local_addr().expect("a bound listener has one");
            let statuses: Arc<Vec<u16>> = Arc::new(statuses.to_vec());
            let asked = AtomicUsize::new(0);

            let serving = tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let index = asked.fetch_add(1, Ordering::Relaxed);
                    let status = statuses[index.min(statuses.len() - 1)];

                    // Read something first, so this is a server answering a request rather than one
                    // shouting at a socket — hyper is entitled to notice the difference.
                    let mut request = [0_u8; 1024];
                    let _ = stream.read(&mut request).await;

                    // No `content-length` on the statuses that are defined to have no body — a
                    // client is entitled to refuse a `204` that claims a length, and the fixture
                    // should not be the reason a test fails.
                    let length = match status {
                        204 | 304 => "",
                        _ => "content-length: 0\r\n",
                    };

                    let _ = stream
                        .write_all(format!("HTTP/1.1 {status} Status\r\n{length}\r\n").as_bytes())
                        .await;
                    let _ = stream.flush().await;
                }
            });

            Some(Self { addr, serving })
        }

        /// The URL of `path` on this server.
        pub(crate) fn url(&self, path: &str) -> String {
            format!("http://{}{path}", self.addr)
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.serving.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_is_taken_apart_into_what_a_request_needs() {
        let endpoint = Endpoint::parse("http://127.0.0.1:2019/config/", "a check")
            .expect("a plain loopback URL");

        assert_eq!(endpoint.host, "127.0.0.1");
        assert_eq!(endpoint.port, 2019);
        assert_eq!(endpoint.target, "/config/");
        assert_eq!(endpoint.authority, "127.0.0.1:2019");
    }

    /// The two defaults every client applies, applied here rather than discovered by a request that
    /// went nowhere.
    #[test]
    fn a_url_with_no_port_and_no_path_gets_the_ones_every_client_assumes() {
        let endpoint = Endpoint::parse("http://localhost", "a check").expect("a bare host");

        assert_eq!(endpoint.port, 80);
        assert_eq!(endpoint.target, "/");
        assert_eq!(
            endpoint.authority, "localhost",
            "a URL that named no port must not claim one in the Host header"
        );
    }

    /// The two halves of a URL's authority go to two places that want it written differently.
    #[test]
    fn an_ipv6_literal_keeps_its_brackets_in_the_header_and_loses_them_for_the_resolver() {
        let endpoint =
            Endpoint::parse("http://[::1]:2019/config/", "a check").expect("a loopback URL");

        assert_eq!(
            endpoint.host, "::1",
            "a resolver is given a host name, and `[::1]` is not one"
        );
        assert_eq!(
            endpoint.authority, "[::1]:2019",
            "a Host header carries the authority, and that is how one is spelled"
        );
    }

    /// A URL is allowed to carry credentials; a `Host` header is not, and this is the boundary they
    /// must not cross — a header that carried them would be an invalid host *and* a secret on the
    /// wire.
    #[test]
    fn credentials_in_a_url_do_not_reach_the_host_header() {
        let endpoint =
            Endpoint::parse("http://user:pw@127.0.0.1:2019/config/", "a check").expect("a URL");

        assert_eq!(endpoint.host, "127.0.0.1");
        assert_eq!(endpoint.authority, "127.0.0.1:2019");
    }

    /// A gap this build has, reported as one — not as a service that never became ready.
    #[test]
    fn https_says_what_is_missing() {
        let error = Endpoint::parse("https://localhost/health", "an HTTP ready check")
            .expect_err("this build speaks no TLS");

        assert!(
            matches!(&error, Error::UnsupportedCheck { reason, .. } if reason.contains("HTTPS")),
            "{error:?}"
        );
    }

    /// The spec's fault, and it is found before anything waits on it.
    #[test]
    fn something_that_is_not_a_url_is_refused_up_front() {
        for url in ["localhost:2019/config", "", "ftp://localhost/x", "http://"] {
            assert!(
                matches!(Endpoint::parse(url, "a check"), Err(Error::Url { .. })),
                "`{url}` is not a URL a request can be made from"
            );
        }
    }

    /// **A port that is not a port must not become the default one.** `:99999` fits no `u16` and a
    /// bare `:` names nothing, and both used to land on port 80 — a check quietly aimed at whatever
    /// else was listening on loopback, which can answer `200` and report a service that never
    /// started as ready.
    ///
    /// `:0` is the same rule from the other side: it fits a `u16` perfectly well and is still not a
    /// port anything listens on, so a check naming one spends its whole timeout being refused and
    /// is reported as a service that never came up.
    #[test]
    fn a_port_that_is_not_one_is_refused_rather_than_defaulted() {
        for url in [
            "http://localhost:99999/health",
            "http://localhost:/health",
            "http://[::1]:99999/health",
            "http://user:pw@localhost:99999/health",
            "http://localhost:0/health",
            "http://[::1]:0/health",
        ] {
            assert!(
                matches!(Endpoint::parse(url, "a check"), Err(Error::Url { .. })),
                "`{url}` names no port a request can be made to"
            );
        }
    }

    /// **A URL with no host must not be aimed at whatever the resolver makes of one.**
    ///
    /// `http://:2019/health` is a plausible shorthand for "this machine" and parses into a host of
    /// `""`, which is a question the two systems answer differently: glibc says `EAI_NONAME`, so the
    /// check can never connect and is reported as a service that never came up, while Windows
    /// answers with this machine's *LAN* addresses — the same spec quietly aimed at port 2019 of an
    /// interface nobody named, where it can answer `200` for a different service entirely.
    #[test]
    fn a_url_with_no_host_is_refused_rather_than_left_to_the_resolver() {
        for url in [
            "http://:2019/health",
            "http://:2019",
            "http://user:pw@:2019/health",
        ] {
            assert!(
                matches!(Endpoint::parse(url, "a check"), Err(Error::Url { .. })),
                "`{url}` names no host a request can be made to"
            );
        }
    }

    /// The other half of the rule above: the ports that *are* ports still get through, past the
    /// colons that userinfo and an IPv6 literal put in the way.
    #[test]
    fn the_ports_a_url_may_name_are_read_from_wherever_the_url_put_them() {
        for (url, port) in [
            ("http://localhost/health", 80),
            ("http://localhost:2019/health", 2019),
            ("http://[::1]:2019/health", 2019),
            ("http://[::1]/health", 80),
            ("http://user:pw@localhost:2019/health", 2019),
            ("http://user:pw@localhost/health", 80),
        ] {
            let endpoint = Endpoint::parse(url, "a check").expect(url);

            assert_eq!(endpoint.port, port, "`{url}`");
        }
    }

    /// A port nothing is listening on is *no answer*, which is what a retry and a failed probe are
    /// both built on — never an error.
    #[tokio::test]
    async fn a_port_nothing_is_listening_on_is_no_answer_rather_than_a_failure() {
        let scout = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let port = scout
            .local_addr()
            .expect("a bound listener has an address")
            .port();
        drop(scout);

        let endpoint =
            Endpoint::parse(&format!("http://127.0.0.1:{port}/"), "a check").expect("a URL");

        assert_eq!(endpoint.ask().await, None);
    }

    #[tokio::test]
    async fn a_server_that_answers_gives_back_the_status_it_answered_with() {
        let server = fake::Server::answering(&[204]).await;
        let endpoint = Endpoint::parse(&server.url("/config/"), "a check").expect("a URL");

        assert_eq!(endpoint.ask().await, Some(204));
    }

    /// The parse test above, proved against a socket — which is the only place this bug ever showed:
    /// a bracketed host parses perfectly well and then connects to nothing.
    #[tokio::test]
    async fn a_url_naming_an_ipv6_literal_reaches_the_service_listening_on_it() {
        let Some(server) = fake::Server::answering_on("[::1]:0", &[200]).await else {
            // This machine has no IPv6 loopback. Nothing to prove here, and nothing broken.
            return;
        };

        // `SocketAddr`'s own rendering is the bracketed form, which is exactly the URL a spec would
        // be written with.
        let endpoint = Endpoint::parse(&server.url("/config/"), "a check").expect("a URL");

        assert_eq!(endpoint.ask().await, Some(200));
    }

    /// The case a deadline exists for: a socket that accepts and then says nothing looks exactly
    /// like a healthy service to a probe with no clock on it.
    #[tokio::test]
    async fn a_server_that_accepts_and_never_answers_runs_out_of_patience() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let addr = listener.local_addr().expect("a bound listener has one");

        // Accepted and then held, which is the whole fixture: a connection nobody answers on.
        let holding = tokio::spawn(async move {
            let _accepted = listener.accept().await;
            std::future::pending::<()>().await;
        });

        let endpoint = Endpoint::parse(&format!("http://{addr}/"), "a check").expect("a URL");

        assert_eq!(endpoint.answered(Duration::from_millis(200)).await, None);

        holding.abort();
    }
}
