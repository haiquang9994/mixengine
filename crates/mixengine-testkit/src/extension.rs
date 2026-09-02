//! The four `extension.toml` fixtures — roadmap task **T80**.
//!
//! **These are the manifests T82 and T83 will ship**, not examples written to fit the parser. A
//! format proved against files invented for it proves only that it is self-consistent; these are
//! the four kinds as the products they describe actually need them, which is where a format finds
//! out what it forgot.
//!
//! The hashes are placeholders. Nothing in T80 downloads anything — verification arrives with the
//! registry, in T81 — and a real hash here would be a fact that goes stale with the next release.

/// A `service` that also carries a recipe — D7's case, in one file.
pub const MAILPIT: &str = include_str!("../fixtures/extensions/mailpit.toml");

/// A `web-app` on a runtime MixEngine picks, never the user's project version.
pub const PHPMYADMIN: &str = include_str!("../fixtures/extensions/phpmyadmin.toml");

/// A `desktop-app`: nothing to supervise, and detection is T83's platform work.
pub const MIXDB: &str = include_str!("../fixtures/extensions/mixdb.toml");

/// A `recipe` and nothing else.
pub const SENDMAIL: &str = include_str!("../fixtures/extensions/sendmail.toml");
