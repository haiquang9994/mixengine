//! Extensions — roadmap task **T80**.
//!
//! **Not [`crate::runtimes::extensions`]**, which is about a PHP extension being switched on for
//! one installed runtime. These are MixEngine's own: Mailpit, phpMyAdmin, MixDB.
//!
//! T80 reads a manifest and renders it into the thing that would run. Nothing here installs, stores
//! or starts anything — that is T81, which is deliberately handed a format already proved to make
//! sense rather than one it discovers is wrong five actions into an install.

pub mod manifest;
pub mod render;
