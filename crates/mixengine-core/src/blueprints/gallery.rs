//! The blueprints this build ships — roadmap task **T79**.
//!
//! **A table compiled into the binary** (the T79 design, D1), on [`crate::shims::COMMANDS`]'
//! precedent: a set this build ships is a constant of this build, not a document it fetches. A
//! gallery that arrived over the network would be absent on a fresh machine with no connection, and
//! `builtin` would stop meaning *this build's own* — which is the only thing that makes trusting one
//! without a signature check sound (D3).
//!
//! **Each file is exactly what [`crate::blueprints::manifest::render`] would write** (D2). That
//! costs the files their comments — a gallery blueprint is a thing people read to learn the format,
//! and it cannot carry a word of commentary; `[blueprint] description` is what carries it instead,
//! because that survives the round trip. What it buys is that the file here and the file in a
//! user's home are the same bytes, so a `diff` between them means something.

/// One blueprint this build ships.
#[derive(Debug)]
pub struct Entry {
    /// What it is filed under: the row's key, the rendered file's stem, and what a person types.
    pub slug: &'static str,

    /// The manifest, canonical (D2).
    pub manifest: &'static str,
}

/// Every blueprint this build ships, in slug order — which is the order a listing shows them in.
pub const ENTRIES: &[Entry] = &[
    Entry {
        slug: "django",
        manifest: include_str!("gallery/django.toml"),
    },
    Entry {
        slug: "laravel",
        manifest: include_str!("gallery/laravel.toml"),
    },
    Entry {
        slug: "nextjs",
        manifest: include_str!("gallery/nextjs.toml"),
    },
    Entry {
        slug: "static",
        manifest: include_str!("gallery/static.toml"),
    },
    Entry {
        slug: "symfony",
        manifest: include_str!("gallery/symfony.toml"),
    },
    Entry {
        slug: "wordpress",
        manifest: include_str!("gallery/wordpress.toml"),
    },
];
