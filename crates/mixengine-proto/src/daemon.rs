//! What `daemon.*` answers.
//!
//! Every field here is something this build genuinely knows. Services, sites and runtimes are
//! absent rather than present-and-empty: a client that renders "0 services" before the concept
//! exists is showing a fact nobody established, and adding a field in Phase 1 costs a client
//! nothing while removing one costs it a release.

use crate::{ProtocolVersion, Timestamp, Uptime};

/// Everything the daemon knows about itself, for `daemon.status`.
///
/// Paths are strings and not `PathBuf`s. serde will refuse a `PathBuf` that is not valid UTF-8, and
/// a home directory with an unusual name is a reason to see it spelled oddly in `mix status`, not a
/// reason for `mix status` to fail. They are for reading; nothing joins or opens them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DaemonStatus {
    /// The daemon's build version — `CARGO_PKG_VERSION`, the same string `mixengined --version`
    /// prints.
    pub version: String,

    /// The API version this daemon speaks. A client compares it with its own
    /// [`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION) before it trusts anything else in this struct.
    pub protocol: ProtocolVersion,

    /// The daemon's process id, so a user can find it in a task manager and a client can tell one
    /// restart from another.
    pub pid: u32,

    /// `MIXENGINE_HOME` as it was resolved. The single most useful line when somebody is talking to
    /// a daemon they did not expect to be talking to.
    pub home: String,

    /// Where it listens: a socket path, or a named pipe.
    pub endpoint: String,

    /// The SQLite file it opened. Not derivable from `home` — `[paths]` can move it.
    pub database: String,

    /// When this daemon started.
    pub started_at: Timestamp,

    /// How long ago that was, computed by the daemon rather than by the client.
    ///
    /// Redundant with [`DaemonStatus::started_at`] only if the two clocks agree, which is exactly
    /// the assumption worth avoiding: a monotonic reading here means "up 3 days" stays right across
    /// a system clock that was corrected while the daemon ran.
    pub uptime: Uptime,
}

/// The cheap half of [`DaemonStatus`], for `daemon.version`.
///
/// Its own method because a client asks this before it can safely ask anything else — a daemon from
/// another release may answer `daemon.status` with fields this client cannot decode, and finding
/// that out by failing to parse the answer is worse than asking first.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DaemonVersion {
    /// The daemon's build version.
    pub version: String,

    /// The API version it speaks.
    pub protocol: ProtocolVersion,
}

/// The body of `GET /health`.
///
/// Unauthenticated and deliberately trivial: its one job is to tell a client whether to autostart a
/// daemon (`.claude/architecture/daemon-and-ipc.md`), and it must stay answerable while everything
/// else is still coming up. The version rides along because it is free and saves the caller a second
/// round trip.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Health {
    /// Always `true` — a daemon that could not answer this does not answer at all. A field rather
    /// than an empty object so the body is self-describing in a log or a `curl`.
    pub ok: bool,

    /// The daemon's build version.
    pub version: String,

    /// The API version it speaks.
    pub protocol: ProtocolVersion,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PROTOCOL_VERSION;

    #[test]
    fn a_status_is_flat_json_with_no_nested_envelope() {
        let status = DaemonStatus {
            version: "0.1.0".to_owned(),
            protocol: PROTOCOL_VERSION,
            pid: 4123,
            home: "/home/dev/.local/share/mixengine".to_owned(),
            endpoint: "/home/dev/.local/share/mixengine/run/mixengined.sock".to_owned(),
            database: "/home/dev/.local/share/mixengine/data/mixengine.db".to_owned(),
            started_at: Timestamp(1_723_000_000_500),
            uptime: Uptime(812),
        };

        let encoded = serde_json::to_value(&status).unwrap();
        assert_eq!(encoded["protocol"], 1);
        assert_eq!(encoded["uptime"], 812);
        assert_eq!(encoded["started_at"], 1_723_000_000_500_i64);

        assert_eq!(
            serde_json::from_value::<DaemonStatus>(encoded).unwrap(),
            status
        );
    }

    #[test]
    fn health_says_which_protocol_it_speaks_so_one_request_is_enough() {
        let health = Health {
            ok: true,
            version: "0.1.0".to_owned(),
            protocol: PROTOCOL_VERSION,
        };

        assert_eq!(
            serde_json::to_string(&health).unwrap(),
            r#"{"ok":true,"version":"0.1.0","protocol":1}"#
        );
    }
}
