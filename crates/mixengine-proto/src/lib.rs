//! Wire types shared by `mixengined` and every client.
//!
//! This crate is the single source of truth for the API surface: requests, responses, events and
//! the wire error. It is `serde`-only on purpose — no I/O, no platform code, no domain logic — so
//! that a client can depend on it without pulling in the daemon's world, and so the TypeScript
//! bindings can be generated from it (see roadmap task T56).

#![warn(missing_docs)]

/// Version of the JSON-RPC protocol spoken over the local IPC transport.
///
/// The daemon and every client negotiate this on connect, and so do the daemon and
/// `mixengine-elevate`. Bump it when a change is not backwards compatible for an older peer.
pub const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(1);

/// A protocol version, exchanged during the handshake.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct ProtocolVersion(pub u32);

impl std::fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_wire_transparent() {
        let encoded = serde_json::to_string(&PROTOCOL_VERSION).unwrap();
        assert_eq!(encoded, "1");
        assert_eq!(
            serde_json::from_str::<ProtocolVersion>(&encoded).unwrap(),
            PROTOCOL_VERSION
        );
    }
}
