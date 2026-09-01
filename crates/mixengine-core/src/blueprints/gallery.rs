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

use mixengine_proto::BlueprintSource;

use crate::blueprints::trust::Trust;
use crate::blueprints::{manifest, store};
use crate::{Paths, Result, Store};

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

/// What a seed did, for the one line the daemon logs.
///
/// [`crate::shims::Refreshed`]'s shape, and its reason: the ordinary start writes nothing, so what
/// is worth logging is the exception rather than the six names.
#[derive(Debug, Default)]
pub struct Seeded {
    /// The rows this call wrote, because they were missing or held different bytes.
    pub written: Vec<String>,

    /// The renderings it wrote without touching the row, because the file had gone or drifted.
    pub rendered: Vec<String>,

    /// What it left alone: already right, or somebody else's (D6).
    pub left: Vec<String>,
}

/// Put every blueprint this build ships into this home, writing only what differs.
///
/// **One read, then only the writes that are needed** (D4). Every CLI test in this workspace starts
/// a daemon and every daemon start calls this; six file writes and six upserts on each of those is a
/// cost with nothing on the other side of it, since the bytes are identical every time. It is
/// `bin/`'s rule one object along — see [`crate::shims::refresh`].
///
/// **A row whose source is not `builtin` is left alone** (D6), whatever its slug: a capture over a
/// gallery name makes that slug this machine's own for good.
///
/// # Errors
///
/// [`crate::Error::BlueprintManifest`] for a compiled-in file that does not parse — a broken build,
/// which [`ENTRIES`]' round-trip test is what stops reaching one; [`crate::Error::Database`] when
/// the table cannot be read or written, and [`crate::Error::Io`] when a rendering cannot be.
pub async fn seed(store: &Store, paths: &Paths) -> Result<Seeded> {
    // Every row rather than the six by name: `sqlx::query!` needs its SQL literal, so an `IN` list
    // would have to be as long as `ENTRIES` and stay in step with it by hand. A home holds a
    // handful of blueprints, and this is one statement either way.
    let rows = sqlx::query!(
        r#"SELECT id AS "id!: String", source AS "source!: String",
                  manifest_toml AS "manifest_toml!: String"
           FROM blueprints"#
    )
    .fetch_all(store.pool())
    .await
    .map_err(|error| store.failure("read", error))?;

    let mut seeded = Seeded::default();

    for entry in ENTRIES {
        let filed = rows.iter().find(|row| row.id == entry.slug);

        // Somebody else's, and nothing here touches it again (D6).
        if filed.is_some_and(|row| row.source != BlueprintSource::Builtin.as_str()) {
            seeded.left.push(entry.slug.to_owned());
            continue;
        }

        if filed.is_some_and(|row| row.manifest_toml == entry.manifest) {
            // The row is right. The file beside it may not be — a home whose `blueprints/` was
            // emptied is mended by starting the daemon, which is `bin/`'s property (D5).
            let path = store::file(paths, entry.slug);

            if std::fs::read_to_string(&path).is_ok_and(|found| found == entry.manifest) {
                seeded.left.push(entry.slug.to_owned());
                continue;
            }

            std::fs::write(&path, entry.manifest).map_err(|error| crate::Error::Io {
                action: "write",
                path,
                source: error,
            })?;

            seeded.rendered.push(entry.slug.to_owned());
            continue;
        }

        let manifest = manifest::read(entry.manifest)?;

        // **Trusted without a signature check** (D3): a signature travelling inside the same binary
        // as the key it would be checked against proves nothing the binary has not already proved.
        store::save(
            store,
            paths,
            &manifest,
            entry.slug,
            BlueprintSource::Builtin,
            Trust::Inherent,
            true,
        )
        .await?;

        seeded.written.push(entry.slug.to_owned());
    }

    Ok(seeded)
}
