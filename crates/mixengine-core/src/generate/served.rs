//! What a front end has to serve, assembled out of the three site tables — roadmap task **T43**.
//!
//! [`Served`] is a site as a *template* needs it and not as the database holds it: the doc root
//! joined onto its project's root, the domains already ordered with the primary at the head, and a
//! php-fpm site's pool already resolved to the address that pool listens on. Everything a recipe
//! would otherwise have to look up itself is looked up once, here, for the same reason
//! [`Endpoints`](super::recipe::Endpoints) exists — a template that computed a path would be a
//! second place for it to be computed differently.
//!
//! # One place reads these tables
//!
//! [`crate::sites::records`] is that place, and this module asks it rather than writing a query of
//! its own. `sites.rs`' module note is explicit about why: a second door onto a table is a second
//! answer to a question that has one. What is added here is the join onto `projects`, which is the
//! one thing a doc root cannot be made absolute without.

use std::path::PathBuf;

use super::recipe::Upstream;

/// One site, as the thing that renders it needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    /// Ordered; the head is the primary.
    pub domains: Vec<String>,

    /// Absolute: the project's root joined to the row's relative doc root.
    pub doc_root: PathBuf,

    /// What it serves, and what that kind needs to know.
    pub kind: ServedKind,

    /// Whether HTTPS is declared. **Read by Phase 5. Rendered by nothing here** — a site address is
    /// written `http://` today, and rendering half of TLS now would leave a site redirecting to a
    /// port serving nothing.
    pub https: bool,
}

impl Served {
    /// The domain this site is named after, in a listing and in a filename.
    ///
    /// The head of the list, which `core::sites` guarantees is the primary — and guarantees is
    /// there: a site with no domain is not a row this build can write.
    #[must_use]
    pub fn primary(&self) -> &str {
        self.domains.first().map_or("", String::as_str)
    }
}

/// What a site serves, with everything a template would otherwise have to look up resolved.
///
/// [`SiteKind`](mixengine_proto::SiteKind)'s shape with one difference, and it is the difference
/// this type exists for: a php-fpm site carries the *address* its pool listens on rather than the
/// pool's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServedKind {
    /// PHP through a pool, at the address that pool listens on.
    PhpFpm {
        /// Where the pool is, in this system's shape.
        upstream: Upstream,
    },

    /// Files, and nothing running.
    Static,

    /// Everything forwarded to an address the user already has listening.
    ReverseProxy {
        /// An absolute `http` or `https` URL with a host, as the row holds it.
        upstream: String,
    },

    /// A node process the user runs, on a loopback port.
    ///
    /// **Rendered exactly as a reverse proxy to `127.0.0.1:<port>`, and that is all it is.** Nothing
    /// in this build starts `npm run dev`; what distinguishes this from
    /// [`ReverseProxy`](Self::ReverseProxy) is the scope of the address rather than a mechanism, and
    /// writing that down is more honest than a kind that pretends to more.
    NodeApp {
        /// The loopback port it listens on.
        port: u16,
    },
}
