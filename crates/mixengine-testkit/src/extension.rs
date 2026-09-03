//! The five `extension.toml` fixtures — roadmap task **T80**, made true by **T82** and **T82a**.
//!
//! **These are the manifests T82 shipped**, not examples written to fit the parser. A format proved
//! against files invented for it proves only that it is self-consistent; these are the kinds as the
//! products they describe actually need them, which is where a format finds out what it forgot —
//! and it did: T80's `[web-app].root = "{install_dir}/app"` and its `template` field were both wrong
//! about the real artifacts, which is what T82's design D1 and its roadmap line record.
//!
//! **Three of them carry the real hashes**, which T80 said they never would. That rule was written
//! when a hash here could only go stale; what changed is that these are now the same bytes
//! `mixnz/mixengine-packages` publishes under `data/extensions/`, and a fixture that agreed with the
//! roster about everything except the one field somebody would copy is a trap rather than a
//! precaution. Nothing in this workspace downloads one of these, so a hash that goes stale costs a
//! diff and no red test; what proves the published roster against upstream is that repository's own
//! `check-extensions.yml`.
//!
//! `mixdb.toml` has no hash at all — T83 has not chosen a release — and `sendmail.toml` needs none:
//! a `recipe` downloads nothing.

/// A `service` that also carries a recipe — D7's case, in one file.
pub const MAILPIT: &str = include_str!("../fixtures/extensions/mailpit.toml");

/// A `web-app` on a runtime MixEngine picks, never the user's project version — and, since roadmap
/// task **T82a**, the one manifest that declares `[web-app.database].signs_in`.
pub const PHPMYADMIN: &str = include_str!("../fixtures/extensions/phpmyadmin.toml");

/// A `web-app` whose artifact is **one file** rather than an archive — the T82 design's D3 — and
/// whose generated `index.php` is what gives Adminer a default server.
pub const ADMINER: &str = include_str!("../fixtures/extensions/adminer.toml");

/// A `desktop-app`: nothing to supervise, and detection is T83's platform work.
pub const MIXDB: &str = include_str!("../fixtures/extensions/mixdb.toml");

/// A `recipe` and nothing else.
pub const SENDMAIL: &str = include_str!("../fixtures/extensions/sendmail.toml");
