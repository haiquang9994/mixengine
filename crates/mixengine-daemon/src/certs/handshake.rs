//! A live TLS handshake against this home's own front end — roadmap task **T53**.
//!
//! **The first thing in this repository that measures a padlock rather than inferring one.** T48
//! reads an authority off disk, T50 writes a leaf and reads it back, T51 renders a `tls` line
//! naming it and T52 replaces it before it expires — every one of those is a claim about a *file*,
//! and none of them establishes that the running server presents that file to anything. The report
//! `.claude/features/tls.md` calls the most common of all, a certificate a server still holds in
//! memory after the file beside it was replaced, is invisible to all of them and obvious to this.
//!
//! **Loopback, with the site's name as SNI, and never a resolved address** — the T53 design, D2.
//! Whether `blog.test` resolves is `mix doctor`'s question, and a handshake that resolved would
//! report "TLS failed" on a machine whose only fault is a resolver nobody wired.

use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use mixengine_proto::{Handshake, Verdict};
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, RootCertStore, SignatureScheme};

/// What this home's front end presents for `domain` on `port`.
///
/// `authority` is this home's certificate authority in DER — its **certificate**, never its key,
/// which nothing outside `mixengine_core::certs::ca` holds and which nothing here would have a use
/// for. It is the only trust root this client is given, so "trusted" here means trusted by this
/// home and by nothing else on the machine.
pub(crate) async fn against(
    domain: &str,
    port: u16,
    authority: &[u8],
    now: SystemTime,
) -> Handshake {
    let Ok(name) = ServerName::try_from(domain.to_owned()) else {
        return Handshake::NotServed {
            because: format!("`{domain}` is not a name a TLS client can ask for"),
        };
    };

    let seen: Seen = Arc::new(Mutex::new(None));

    let config = match configured(authority, Arc::clone(&seen)) {
        Ok(config) => config,
        Err(because) => return Handshake::NotServed { because },
    };

    let stream = match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
        Ok(stream) => stream,
        Err(error) => {
            return Handshake::NotServed {
                because: format!("nothing answered on 127.0.0.1:{port}: {error}"),
            };
        }
    };

    if let Err(error) = tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(name, stream)
        .await
    {
        return Handshake::Failed {
            because: error.to_string(),
        };
    }

    let presented = seen.lock().ok().and_then(|mut slot| slot.take());

    let Some((der, rejected)) = presented else {
        return Handshake::Failed {
            because: "the server completed a handshake without presenting a certificate".to_owned(),
        };
    };

    let Some(cert) = mixengine_core::certs::leaf::describe(&der, now) else {
        return Handshake::Failed {
            because: "the server presented something that is not a certificate".to_owned(),
        };
    };

    Handshake::Presented {
        cert,
        trust: match rejected {
            None => Verdict::Trusted {},
            Some(because) => Verdict::Rejected { because },
        },
    }
}

/// What the verifier writes down: the leaf that was presented, and why it was refused if it was.
type Seen = Arc<Mutex<Option<(Vec<u8>, Option<String>)>>>;

/// A client that trusts this home's authority and nothing else, and remembers what it was shown.
///
/// **The provider is named rather than left to `ClientConfig::builder`.** rustls picks a
/// process-wide default only when exactly one provider is compiled in, and this tree compiles in
/// `aws-lc-rs` alone — naming it means a tree that ever enables a second one fails to build here,
/// where the alternative is a panic inside a running daemon the first time somebody asks about a
/// certificate.
fn configured(authority: &[u8], seen: Seen) -> Result<rustls::ClientConfig, String> {
    let mut roots = RootCertStore::empty();

    roots
        .add(CertificateDer::from(authority.to_vec()))
        .map_err(|error| format!("this home's authority is not a usable trust root: {error}"))?;

    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());

    let inner = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), Arc::clone(&provider))
        .build()
        .map_err(|error| format!("this home's authority cannot verify anything: {error}"))?;

    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| format!("this build has no usable TLS protocol version: {error}"))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Capturing { inner, seen }))
        .with_no_client_auth())
}

/// A verifier that answers the real question and then gets out of the way.
///
/// **It always returns `Ok`, and that is not a weakening of anything.** The verdict written into
/// [`Self::seen`] is a real [`WebPkiServerVerifier`]'s, rooted at this home's authority alone;
/// saying `Ok` afterwards is what lets the connection complete, so that the certificate a *failing*
/// server presented can be reported instead of being replaced by an error message about it.
///
/// Nothing is sent over the connection and nothing is read from it. The handshake is the whole of
/// the request, which is also why this cannot be confused with a health check: a server that
/// completes TLS and then refuses every request still answers here, and should.
#[derive(Debug)]
struct Capturing {
    inner: Arc<WebPkiServerVerifier>,
    seen: Seen,
}

impl ServerCertVerifier for Capturing {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let rejected = self
            .inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
            .err()
            .map(|error| error.to_string());

        if let Ok(mut slot) = self.seen.lock() {
            *slot = Some((end_entity.to_vec(), rejected));
        }

        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A home with an authority and one leaf: the authority's DER, and the leaf's pair.
    fn a_home_serving(domain: &str) -> (tempfile::TempDir, Vec<u8>, Vec<u8>, Vec<u8>) {
        let home = tempfile::tempdir().expect("a temp home");
        let certs = home.path();

        let state =
            mixengine_core::certs::ca::ensure(certs, SystemTime::now()).expect("an authority");
        let mixengine_proto::CaState::Present { ca } = &state else {
            panic!("no authority: {state:?}");
        };
        let authority = mixengine_core::certs::ca::der(&ca.certificate_pem).expect("the CA's DER");

        mixengine_core::certs::leaf::ensure(certs, &[domain.to_owned()], None, SystemTime::now())
            .expect("a leaf is signed");

        let leaf = der_of(&mixengine_core::certs::leaf::certificate_path(
            certs, domain,
        ));
        let key = der_of(&mixengine_core::certs::leaf::key_path(certs, domain));

        (home, authority, leaf, key)
    }

    fn der_of(path: &std::path::Path) -> Vec<u8> {
        let text = std::fs::read_to_string(path).expect("the file is there");
        pem::parse(&text)
            .map(pem::Pem::into_contents)
            .expect("a PEM envelope")
    }

    /// Serve TLS on an ephemeral loopback port until one connection has been accepted.
    ///
    /// Port `0`, on rule 1 of `.claude/standards/testing.md`: the operating system chooses, so no
    /// test claims a number that belongs to the machine it is running on.
    async fn serving(leaf: Vec<u8>, key: Vec<u8>) -> u16 {
        let chain = vec![CertificateDer::from(leaf)];
        let key = rustls::pki_types::PrivateKeyDer::try_from(key).expect("a private key");

        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("the default protocol versions")
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .expect("a server configuration");

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("a loopback port");
        let port = listener.local_addr().expect("its address").port();

        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = acceptor.accept(stream).await;
            }
        });

        port
    }

    /// **The measurement this whole task exists for.** Every other assertion in phase 5 is about a
    /// file; this one is about what a server hands a client.
    #[tokio::test]
    async fn a_server_presenting_this_homes_leaf_is_trusted() {
        let (_home, authority, leaf, key) = a_home_serving("blog.test");
        let port = serving(leaf, key).await;

        let answer = against("blog.test", port, &authority, SystemTime::now()).await;

        let Handshake::Presented { cert, trust } = &answer else {
            panic!("nothing was presented: {answer:?}");
        };
        assert_eq!(cert.sans, ["blog.test"], "{answer:?}");
        assert_eq!(*trust, Verdict::Trusted {}, "{answer:?}");
    }

    /// **The case the cheap alternative would have got wrong** — the T53 design, D3.
    ///
    /// Comparing issuer names would notice this one, because a second authority carries a second
    /// name. What it would not notice is the same name over a different key, and the only way to be
    /// right about both is to verify — which is what this asserts is happening.
    #[tokio::test]
    async fn a_leaf_from_another_authority_is_rejected() {
        let (_ours, authority, _leaf, _key) = a_home_serving("blog.test");
        let (_theirs, _other, leaf, key) = a_home_serving("blog.test");
        let port = serving(leaf, key).await;

        let answer = against("blog.test", port, &authority, SystemTime::now()).await;

        let Handshake::Presented { trust, .. } = &answer else {
            panic!("nothing was presented: {answer:?}");
        };
        let Verdict::Rejected { because } = trust else {
            panic!("a leaf from another authority was trusted: {answer:?}");
        };
        assert!(!because.is_empty());
    }

    /// A port nothing holds is an answer rather than an error: it is the ordinary state of a home
    /// whose front end is stopped, and of every home before one is installed.
    #[tokio::test]
    async fn a_port_nothing_is_listening_on_is_not_served() {
        let (_home, authority, _leaf, _key) = a_home_serving("blog.test");

        // Bound and dropped, so this is a number nothing holds *now* — which is what a stopped
        // front end leaves behind, and is safe in a way that guessing a number is not.
        let closed = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("a loopback port");
        let port = closed.local_addr().expect("its address").port();
        drop(closed);

        let answer = against("blog.test", port, &authority, SystemTime::now()).await;

        assert!(matches!(answer, Handshake::NotServed { .. }), "{answer:?}");
    }
}
