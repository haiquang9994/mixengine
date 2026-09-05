//! Reading the signed package index: what exists, and where to get it.
//!
//! # One verification path, two sources
//!
//! An index arrives either from the network or from the cache on disk, and **both go through the
//! same function**. Re-verifying a file this process wrote a minute ago looks redundant and is not:
//! the cache is an ordinary file in the user's home that any local process can rewrite, and a client
//! that trusts it because it trusted the network once has moved the trust boundary from "we signed
//! this" to "nothing on this machine touched it". Verification costs a BLAKE2b hash and one Ed25519
//! check over about fifty kilobytes, which is nothing next to the question it answers.
//!
//! # Why the network lives here rather than in the daemon
//!
//! Whether to go to the network at all *is* the cache policy — freshness, staleness and the
//! rollback check are the same decision as "fetch or do not fetch". Splitting them would put the
//! policy in one crate and its trigger in another, and the interesting failures all live in the
//! seam. So this module owns `reqwest` the way [`crate::store`] owns `sqlx`.
//!
//! # TLS
//!
//! `reqwest`'s default rustls path uses [`rustls-platform-verifier`], which delegates to the
//! operating system's own verifier. That is what makes MixEngine work on a machine behind a
//! corporate proxy that installs its own root — where a bundled root store would fail with nothing
//! the user could do about it — and it is what lets `MIXENGINE_MIRROR_URL` point at a team mirror
//! with an internal certificate, which
//! [runtime-packaging.md](../../../.claude/operations/runtime-packaging.md) promises.
//!
//! Being permissive there is affordable precisely because TLS is **not** what decides whether an
//! index is ours: the Ed25519 signature is, end to end, and it is checked after the bytes arrive
//! regardless of how they arrived. A proxy that can read the traffic still cannot forge the
//! document.
//!
//! One consequence worth stating before somebody finds it and calls it a hole: from Phase 5 onwards
//! MixEngine installs its own CA into that same OS trust store, so its own CA becomes trusted by its
//! own downloader. The private key for it sits on the user's machine — anybody holding it already
//! owns the machine — and it cannot touch the signature above, which is the layer that actually
//! decides authenticity.
//!
//! [`rustls-platform-verifier`]: https://github.com/rustls/rustls-platform-verifier

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use minisign_verify::{PublicKey, Signature};

use crate::{Error, Result};

pub mod format;

pub use format::{
    Arch, Artifact, Channel, Extensions, Index, Os, Package, Requires, Selection, TARGETS, Target,
    Timestamp,
};

/// The key every published index is signed with, compiled in.
///
/// Rotating it needs an application release, which is the point: a key the index itself could
/// announce would be a key an attacker serving the index could announce. The same value is committed
/// as `minisign.pub` in the packaging repository, so a reader can check that what signs the index is
/// what this binary trusts.
pub const PUBLIC_KEY: &str = "RWSUOSSPLuuv4OGGJTNtxoUeKFOWBAQ8UwqucFPqcJ8hAdoRZCNgzPEW";

/// Where the index is published.
///
/// A GitHub release asset whose tag is moved rather than added to, so the URL never changes while
/// the document behind it does.
pub const DEFAULT_URL: &str =
    "https://github.com/mixnz/mixengine-packages/releases/download/index/index.json";

/// How long a fetched index is used without asking again.
///
/// Six hours is short enough that a security release is picked up the same day and long enough that
/// a person installing four runtimes in a row does it over one round trip.
pub const FRESH_FOR: Duration = Duration::from_secs(6 * 60 * 60);

/// How long a fetch may take before it counts as "no network".
///
/// Every path that touches the network has one, per `.claude/standards/rust.md`. This one is
/// generous because the alternative to waiting is falling back to a cache that may be missing, and
/// stingy enough that `mix runtime list` on a captive-portal wifi answers rather than hangs.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// The signature file sits beside the document, named the way minisign names it.
const SIGNATURE_SUFFIX: &str = ".minisig";

/// How the index in hand was obtained, and whether the caller should say so.
///
/// Returned rather than logged alone because the three cases want different words in three different
/// places — a CLI prints them, the GUI shows a badge, and `mix doctor` reports them — and a caller
/// that only wants the data can ignore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Freshness {
    /// Fetched from the network just now.
    Fetched,

    /// Read from the cache, still inside [`FRESH_FOR`].
    Cached {
        /// How long ago it was fetched.
        age: Duration,
    },

    /// Read from the cache, past [`FRESH_FOR`], because the network could not be reached.
    ///
    /// **An answer and not an error.** The alternative is a tool that can list nothing while a
    /// user's wifi is down, and a version list a few days old is still enough to install PHP 8.3.
    /// The signature was checked exactly as it would have been on a fresh copy: old is not
    /// untrusted.
    Stale {
        /// How long ago it was fetched.
        age: Duration,
    },
}

impl Freshness {
    /// Whether the caller should tell somebody.
    #[must_use]
    pub fn is_stale(self) -> bool {
        matches!(self, Self::Stale { .. })
    }
}

/// A verified document, and how it got here.
#[derive(Debug, Clone)]
pub struct Catalogue<D> {
    /// What was signed.
    pub index: D,
    /// Where it came from.
    pub freshness: Freshness,
}

/// A signed document this client knows how to read — roadmap task **T81**.
///
/// **Two documents are published, not one** (the T81 design's D1 and D3): `index.json` says what
/// can be installed, `extensions.json` says which extensions exist. They want identical treatment —
/// verify before parse, cache, refuse to be walked backwards — and they must not share a cache
/// file or a rollback mark, or a registry fetched at noon would look like an index rolled back to
/// noon. So the client is generic over the document and this trait is everything it needs to know
/// about one.
///
/// The [`Error::IndexSignature`] family is deliberately *not* generic with it: every variant
/// already carries the `url`, which is what says which document failed, and renaming them to
/// something document-neutral would touch every call site and every test asserting one in order to
/// rename something that is still accurate — `extensions.json` is an index too.
pub trait Document: serde::de::DeserializeOwned + Clone + std::fmt::Debug {
    /// The document version this build can read.
    const SCHEMA: u32;

    /// What this document is called in a log line a person reads.
    const LABEL: &'static str;

    /// What it is cached as, under the cache directory. Two documents must not share one file.
    const CACHE_FILE: &'static str;

    /// The version the document says it is.
    fn schema(&self) -> u32;

    /// When the publishing pipeline generated it — what makes a rollback detectable, since every
    /// version we ever published is signed just as validly as the newest.
    fn generated_at(&self) -> Timestamp;
}

impl Document for Index {
    const SCHEMA: u32 = format::SCHEMA;
    const LABEL: &'static str = "package index";
    const CACHE_FILE: &'static str = "index.json";

    fn schema(&self) -> u32 {
        self.schema
    }

    fn generated_at(&self) -> Timestamp {
        self.generated_at
    }
}

/// Reads a signed document, caching it and refusing to be walked backwards.
#[derive(Debug)]
pub struct Client<D = Index> {
    url: String,
    key: PublicKey,
    cache_file: PathBuf,
    http: reqwest::Client,
    document: std::marker::PhantomData<fn() -> D>,
}

impl Client<Index> {
    /// Point a client at the published package index, caching under `cache_dir`.
    ///
    /// # Errors
    ///
    /// As [`Client::with`].
    pub fn new(cache_dir: &Path) -> Result<Self> {
        Self::with(DEFAULT_URL, PUBLIC_KEY, cache_dir)
    }
}

impl<D: Document> Client<D> {
    /// Point a client at the published index, caching under `cache_dir`.
    ///
    /// The same, against a named URL and key.
    ///
    /// This is what `MIXENGINE_INDEX_URL` and a team mirror use, and what `MockRegistry` uses in
    /// tests — the key has to be injectable because a test cannot hold the production private key,
    /// and a verification path that is switched off for tests is a verification path nothing checks.
    ///
    /// # Errors
    ///
    /// As [`Client::new`].
    pub fn with(url: &str, public_key: &str, cache_dir: &Path) -> Result<Self> {
        let key = PublicKey::from_base64(public_key).map_err(|source| Error::IndexKey {
            source: Box::new(source),
        })?;

        let http = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .user_agent(concat!("mixengine/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|source| Error::IndexTransport {
                document: D::LABEL,
                url: url.to_owned(),
                source: Box::new(source),
            })?;

        Ok(Self {
            url: url.to_owned(),
            key,
            cache_file: cache_dir.join(D::CACHE_FILE),
            http,
            document: std::marker::PhantomData,
        })
    }

    /// The index, from the cache if it is fresh and from the network otherwise.
    ///
    /// # Errors
    ///
    /// As [`Client::refresh`].
    pub async fn catalogue(&self) -> Result<Catalogue<D>> {
        if let Some((index, age)) = self.cached()
            && age < FRESH_FOR
        {
            return Ok(Catalogue {
                index,
                freshness: Freshness::Cached { age },
            });
        }

        self.refresh().await
    }

    /// The index from the network, whatever the age of what is cached.
    ///
    /// **[`catalogue`](Client::catalogue) without the cache shortcut**, and public because a caller
    /// whose *own* clock is the policy has no use for a second one. The update feed's 24 h check is
    /// that caller — roadmap task **T88** — and so is `mix self-update --check`, which
    /// `.claude/features/updates.md` says forces an immediate check.
    ///
    /// Failing still falls back to the cached document, which is what makes this safe to put on a
    /// clock: a machine that goes offline keeps the last document it verified rather than losing it
    /// every time the timer fires.
    ///
    /// # Errors
    ///
    /// Only ever the reason the *last* usable index could not be obtained, because anything that
    /// goes wrong while there is a cached index falls back to it: [`Error::IndexTransport`] when the
    /// server cannot be reached, [`Error::IndexSignature`] when what it served is not ours,
    /// [`Error::IndexUnreadable`] or [`Error::IndexSchema`] when it is ours and unusable,
    /// [`Error::IndexRolledBack`] when it is older than what is already held, and [`Error::Io`] when
    /// the cache cannot be written.
    pub async fn refresh(&self) -> Result<Catalogue<D>> {
        let cached = self.cached();

        let refreshed = match self.fetch().await {
            Ok((document, signature, index)) => self
                .accept(cached.as_ref().map(|(index, _)| index), &index)
                .and_then(|()| self.store(&document, &signature))
                .map(|()| index),
            Err(unreachable) => Err(unreachable),
        };

        // **One fallback for every way of failing to get a new index**, and that is the design
        // rather than a shortcut. An unreachable server, a signature that does not verify, a
        // document offered from before the one we hold, a cache directory that has gone read-only:
        // they differ in what a person should do about them, and not at all in what this call can
        // do next. The last index we verified is still the last index we verified, so it is
        // returned and the reason is said out loud.
        match (refreshed, cached) {
            (Ok(index), _) => Ok(Catalogue {
                index,
                freshness: Freshness::Fetched,
            }),
            (Err(refusal), Some((index, age))) => {
                tracing::warn!(
                    url = %self.url,
                    age_hours = age.as_secs() / 3600,
                    error = %refusal,
                    document = D::LABEL,
                    "keeping the cached document; the published one was not usable"
                );
                Ok(Catalogue {
                    index,
                    freshness: Freshness::Stale { age },
                })
            }
            (Err(refusal), None) => Err(refusal),
        }
    }

    /// Whether a newly fetched index may replace what is cached.
    ///
    /// **A correctly signed index can still be the wrong one.** Every version we ever published was
    /// signed by us, so an older one serves perfectly against the same key — a stale CDN edge, or
    /// somebody replaying a copy from before a security release was added. The signature cannot tell
    /// them apart; `generated_at` can, and refusing to move backwards is one comparison.
    ///
    /// Done now rather than later on purpose: adding it to a fleet that already has caches means
    /// deciding what to do about the ones already holding a newer document than the server has.
    fn accept(&self, cached: Option<&D>, offered: &D) -> Result<()> {
        let Some(cached) = cached else {
            return Ok(());
        };
        if offered.generated_at() < cached.generated_at() {
            return Err(Error::IndexRolledBack {
                document: D::LABEL,
                url: self.url.clone(),
                cached: cached.generated_at().to_string(),
                offered: offered.generated_at().to_string(),
            });
        }
        Ok(())
    }

    /// Fetch the document and its signature, and verify them.
    async fn fetch(&self) -> Result<(Vec<u8>, String, D)> {
        let transport = |source: reqwest::Error| Error::IndexTransport {
            document: D::LABEL,
            url: self.url.clone(),
            source: Box::new(source),
        };

        let document = self
            .http
            .get(&self.url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(transport)?
            .bytes()
            .await
            .map_err(transport)?;

        let signature_url = format!("{}{SIGNATURE_SUFFIX}", self.url);
        let signature = self
            .http
            .get(&signature_url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|source| Error::IndexTransport {
                document: D::LABEL,
                url: signature_url,
                source: Box::new(source),
            })?
            .text()
            .await
            .map_err(transport)?;

        let index = self.verified(&document, &signature)?;
        Ok((document.to_vec(), signature, index))
    }

    /// Check a signature, then read what it covered.
    ///
    /// In that order, and the order is the point: nothing here parses a document it has not first
    /// established we wrote. A JSON parser is a far larger attack surface than an Ed25519 check, and
    /// running it on unverified bytes would hand that surface to whoever answers the URL.
    ///
    /// # Errors
    ///
    /// [`Error::IndexSignature`] when the signature does not verify against the compiled-in key,
    /// [`Error::IndexUnreadable`] when what it covered is not JSON we understand, and
    /// [`Error::IndexSchema`] when it is a document version this build cannot read.
    fn verified(&self, document: &[u8], signature: &str) -> Result<D> {
        let signature = Signature::decode(signature).map_err(|source| Error::IndexSignature {
            document: D::LABEL,
            url: self.url.clone(),
            source: Box::new(source),
        })?;

        // `false` refuses minisign's legacy algorithm. Everything this project publishes is the
        // modern pre-hashed form, so accepting the other one would widen what we trust in exchange
        // for nothing. The call checks the signature over the trusted comment as well as the one
        // over the document.
        self.key
            .verify(document, &signature, false)
            .map_err(|source| Error::IndexSignature {
                document: D::LABEL,
                url: self.url.clone(),
                source: Box::new(source),
            })?;

        let index: D =
            serde_json::from_slice(document).map_err(|source| Error::IndexUnreadable {
                document: D::LABEL,
                url: self.url.clone(),
                source,
            })?;

        if index.schema() != D::SCHEMA {
            return Err(Error::IndexSchema {
                document: D::LABEL,
                url: self.url.clone(),
                found: index.schema(),
                expected: D::SCHEMA,
            });
        }
        Ok(index)
    }

    /// The cached index and its age, or [`None`] if there is not a usable one.
    ///
    /// Every failure here is [`None`] rather than an error: a cache that is missing, unreadable,
    /// truncated, tampered with or written by a newer schema all mean the same thing to the caller,
    /// which is "go to the network". The one that is worth a word in the log is a *signature*
    /// failure, because that is the only one that cannot happen by accident.
    fn cached(&self) -> Option<(D, Duration)> {
        let document = std::fs::read(&self.cache_file).ok()?;
        let signature = std::fs::read_to_string(self.signature_file()).ok()?;

        let index = match self.verified(&document, &signature) {
            Ok(index) => index,
            Err(refusal @ Error::IndexSignature { .. }) => {
                tracing::warn!(
                    path = %self.cache_file.display(),
                    error = %refusal,
                    document = D::LABEL,
                    "the cached document is not signed by this build's key; ignoring it"
                );
                return None;
            }
            Err(_) => return None,
        };

        let modified = std::fs::metadata(&self.cache_file).ok()?.modified().ok()?;
        // A cache stamped in the future is a clock that moved, not a fresh file. Treating it as
        // age zero would pin a machine to a stale index until its clock caught up, so it is read as
        // "as old as possible" and the network decides.
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or(Duration::MAX);

        Some((index, age))
    }

    /// Write both halves, document last.
    ///
    /// The order matters for the same reason the age is read off the document: a reader takes the
    /// document's mtime as the fetch time and needs the signature already beside it. Writing the
    /// document first would leave a window where a concurrent reader sees a new document against an
    /// old signature and concludes the cache is tampered with.
    fn store(&self, document: &[u8], signature: &str) -> Result<()> {
        let write = |path: PathBuf, bytes: &[u8]| {
            std::fs::write(&path, bytes).map_err(|source| Error::Io {
                action: "write",
                path,
                source,
            })
        };
        write(self.signature_file(), signature.as_bytes())?;
        write(self.cache_file.clone(), document)
    }

    fn signature_file(&self) -> PathBuf {
        let mut name = self.cache_file.clone().into_os_string();
        name.push(SIGNATURE_SUFFIX);
        PathBuf::from(name)
    }
}
