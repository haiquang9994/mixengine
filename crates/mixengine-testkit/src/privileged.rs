//! A row in the queue of privileged operations, for a build that has no producer of them.
//!
//! **Scaffolding with an expiry date, like [`mod@crate::declare`] before it.** Roadmap task
//! **T40b** shipped the queue with nothing able to fill it, deliberately, and there is no
//! `mix elevation enqueue` and never will be: what needs an administrator's permission is decided
//! by the operation that needs it, and a command that let a person put an arbitrary privileged
//! operation in the queue would be a client deciding what runs as root. So a suite that wants to
//! see the screen T64 puts in front of the prompt has to write the row itself.
//!
//! **T41's `HostsApply` is the first producer, and the day it lands this module should go**: a
//! suite that creates a site and *then* finds an operation waiting proves something this cannot —
//! that the queue is filled by the product rather than by a fixture.
//!
//! `op` is the operation's serialisation, which is what a caller with `mixengine-proto` already
//! has. It is also the `dedupe_key`, because that is what `mixengine_core::elevation::canonical`
//! answers today; a fixture that recomputed the canonical form would be a second opinion on the
//! column whose whole job is to be the first.

use std::path::Path;

use crate::declare::open;

/// Put one operation in the queue of a home whose daemon is running.
///
/// # Panics
///
/// If the database cannot be opened, or if the row cannot be written — a fixture that half worked,
/// which would otherwise fail later as an assertion about the daemon.
pub async fn enqueue(database: &Path, op: &str, at: i64) {
    let pool = open(database).await;

    sqlx::query(
        "INSERT INTO pending_privileged_ops (op, dedupe_key, requested_at)
         VALUES (?, ?, ?)
         ON CONFLICT (dedupe_key) DO NOTHING",
    )
    .bind(op)
    .bind(op)
    .bind(at)
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("`{op}` can be queued: {error}"));

    pool.close().await;
}

/// [`enqueue`], for a test that has no runtime of its own.
///
/// # Panics
///
/// As [`enqueue`], and if a runtime cannot be started.
pub fn enqueue_blocking(database: &Path, op: &str, at: i64) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime")
        .block_on(enqueue(database, op, at));
}
