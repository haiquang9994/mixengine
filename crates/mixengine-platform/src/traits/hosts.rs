//! What the machine's hosts file currently says MixEngine put in it.

use std::path::PathBuf;

use mixengine_proto::privileged::HostEntry;

use crate::Result;

/// The managed block, read.
///
/// **Read-only, and that is the whole trait** — the T41 design, D9. Adding and removing entries
/// cannot live here: they need an administrative token, and a capability the daemon can call is by
/// definition one it holds no token for. The write is
/// [`PrivilegedOp::HostsApply`](mixengine_proto::privileged::PrivilegedOp::HostsApply), applied by
/// `mixengine-elevate`. Reading needs no privilege on any of the three systems.
///
/// Three callers, one of them already here: the producer decides whether a change is worth a prompt
/// by comparing this against what the database says (T41), `domain.dns_status` answers "is there a
/// hosts entry?" with it (T46), and `mix doctor` reconciles against it (T47).
pub trait HostsFile: std::fmt::Debug + Send + Sync {
    /// Where this OS keeps the file, whether or not it is there.
    fn path(&self) -> PathBuf;

    /// The entries in MixEngine's block, or why the block cannot be read.
    ///
    /// An empty vector is a machine that has never run MixEngine, which is not a failure.
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) when the file cannot be read, and
    /// [`Error::MalformedBlock`](crate::Error::MalformedBlock) when the block cannot be read without
    /// guessing at what somebody else meant.
    fn managed(&self) -> Result<Vec<HostEntry>>;
}
