//! Blueprints: what a project is made of, written down so another machine can make one like it.
//!
//! Roadmap task **T77**, and half a feature by design — this module *describes* and *plans*, and
//! **T78** is what carries a plan out. The split is not tidiness: the acceptance criterion for the
//! whole feature is that `--dry-run` matches exactly what the real run performs, which is only
//! enforceable while one place decides what the actions are.
//!
//! Six files, one responsibility each:
//!
//! - [`manifest`] is the file format, its reader and its deterministic writer;
//! - [`store`] is the row the truth lives in, and the rendering beside it;
//! - [`capture`] turns a project that already works into a manifest;
//! - [`gallery`] is the set this build ships, and how it reaches a home;
//! - [`plan`] turns a manifest and this home into the list of things an apply would do;
//! - [`trust`] answers the one question T78a added: did the gallery sign this, or did somebody
//!   hand it over.

pub mod capture;
pub mod gallery;
pub mod manifest;
pub mod plan;
pub mod store;
pub mod trust;

pub use manifest::{BlueprintManifest, BlueprintService, BlueprintSite, Header, Php, Provenance};
