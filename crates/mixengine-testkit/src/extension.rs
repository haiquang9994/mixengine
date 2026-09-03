//! The five `extension.toml` fixtures — roadmap task **T80**, made true by **T82**.
//!
//! **These are the manifests T82 shipped**, not examples written to fit the parser. A format proved
//! against files invented for it proves only that it is self-consistent; these are the kinds as the
//! products they describe actually need them, which is where a format finds out what it forgot —
//! and it did: T80's `[web-app].root = "{install_dir}/app"` and its `template` field were both wrong
//! about the real artifacts, which is what T82's design D1 and its roadmap line record.
//!
//! **The hashes are still placeholders, and now they are the only thing that is.** Everything else
//! here is what `mixnz/mixengine-packages` publishes under `data/extensions/`; a real hash would be
//! a fact that goes stale with the next upstream release, and nothing in this workspace downloads
//! one of these. What proves the published roster is that repository's own `check-extensions.yml`.

/// A `service` that also carries a recipe — D7's case, in one file.
pub const MAILPIT: &str = include_str!("../fixtures/extensions/mailpit.toml");

/// A `web-app` on a runtime MixEngine picks, never the user's project version.
pub const PHPMYADMIN: &str = include_str!("../fixtures/extensions/phpmyadmin.toml");

/// A `web-app` whose artifact is **one file** rather than an archive — the T82 design's D3 — and
/// whose generated `index.php` is what gives Adminer a default server.
pub const ADMINER: &str = include_str!("../fixtures/extensions/adminer.toml");

/// A `desktop-app`: nothing to supervise, and detection is T83's platform work.
pub const MIXDB: &str = include_str!("../fixtures/extensions/mixdb.toml");

/// A `recipe` and nothing else.
pub const SENDMAIL: &str = include_str!("../fixtures/extensions/sendmail.toml");
