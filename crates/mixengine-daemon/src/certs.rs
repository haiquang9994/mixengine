//! This home's certificate authority: made once, and reported on.
//!
//! **Made at start rather than when the first HTTPS site is created.**
//! `.claude/architecture/security-model.md` promises one elevation prompt at first run, covering the
//! CA, the resolver wiring and the port grant together. An authority that first appeared with the
//! first site would put its trust-store install (roadmap task T49) in a second batch and therefore
//! behind a second prompt — which is the finding T45 already made about the resolver and wrote down
//! in `main.rs` beside the block this one sits under. Generating here costs one ECDSA key on disk in
//! a home that never serves HTTPS; the alternative costs a second prompt to everybody who does.
//!
//! Everything about what an authority *is* lives in `mixengine_core::certs::ca`. This module is the
//! two things that are the daemon's: when it happens, and which thread it happens on.

use std::path::PathBuf;
use std::time::SystemTime;

use mixengine_proto::{CaStatus, Error, ErrorCode};

use crate::error::ToWire as _;

/// Everything this needs, which is one directory.
#[derive(Debug)]
pub(crate) struct Certificates {
    certs: PathBuf,
}

impl Certificates {
    pub(crate) fn new(paths: &mixengine_core::Paths) -> Self {
        Self {
            certs: paths.certs().to_path_buf(),
        }
    }

    /// Make the authority if this home has none, and answer with what is there either way.
    ///
    /// Idempotent, and never destructive: an authority that is present and broken is left alone and
    /// reported, because replacing it would invalidate every leaf and every trust store that holds
    /// it. See `mixengine_core::certs::ca`.
    ///
    /// # Errors
    ///
    /// Whatever `certs/ca/` could not be made or written, and the case where this machine will not
    /// produce a key pair at all. Callers at start log it and carry on.
    pub(crate) async fn ensure(&self) -> Result<CaStatus, Error> {
        let certs = self.certs.clone();

        // Key generation and two file writes. `.claude/standards/rust.md`'s rule for anything that
        // touches a disk from a runtime worker — and on Windows the private key's ACL is written by
        // running `icacls`, which is a process rather than a syscall.
        //
        // **The conversion to a wire error happens inside the closure**, not after the `await`.
        // `mixengine_core::Error` is over 128 bytes, so carrying it out through the task's own
        // `Result` puts a large error in two frames and `clippy::result_large_err` says so — the
        // same boundary every other module in this crate converts at.
        let state = blocking("making", move || {
            mixengine_core::certs::ca::ensure(&certs, SystemTime::now())
                .map_err(|error| error.to_wire())
        })
        .await??;

        Ok(CaStatus { state })
    }

    /// What is on disk, without changing any of it.
    ///
    /// # Errors
    ///
    /// Only when the task reading it does not finish. A home with no authority, or one whose
    /// authority is damaged, is an answer rather than a failure — see
    /// [`CaState`](mixengine_proto::CaState).
    pub(crate) async fn status(&self) -> Result<CaStatus, Error> {
        let certs = self.certs.clone();

        let state = blocking("reading", move || {
            mixengine_core::certs::ca::read(&certs, SystemTime::now())
        })
        .await?;

        Ok(CaStatus { state })
    }
}

/// Run `work` off the runtime, and turn a task that did not finish into a sentence.
async fn blocking<T: Send + 'static>(
    what: &'static str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Error> {
    tokio::task::spawn_blocking(work).await.map_err(|_| {
        Error::new(
            ErrorCode::Internal,
            format!("the task {what} this home's certificate authority did not finish"),
        )
    })
}
