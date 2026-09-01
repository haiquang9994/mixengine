//! Where a captured blueprint lives: one row, and one file rendered beside it.
//!
//! **The row is the truth; the file is a rendering** — the T77 design, D7. `blueprints.manifest_toml`
//! is the blueprint. `blueprints/<slug>.toml` exists so a person can read and copy it, and is never
//! parsed back into state — the rule everything under `etc/` has lived by since the beginning.
//!
//! # The name is validated because it becomes a filename
//!
//! [`validated_slug`] is a security boundary and not a nicety: the slug is joined onto
//! [`crate::Paths::blueprints`], so `../../etc/x` would write outside the home. The refusal is what
//! makes the join safe, rather than anything the join itself does.

use std::path::PathBuf;

use mixengine_proto::{BlueprintSource, BlueprintSummary, SignatureCheck};

use crate::blueprints::manifest::{self, BlueprintManifest};
use crate::blueprints::trust::Trust;
use crate::{Error, Paths, Result, Store};

/// The longest a blueprint name may be.
///
/// [`crate::projects`]' limit, for the same reason: a handle typed on a command line. It is also a
/// filename stem, and every target this product ships to is comfortable well below this.
const NAME_LIMIT: usize = 64;

/// Where a blueprint's rendering goes.
#[must_use]
pub fn file(paths: &Paths, slug: &str) -> PathBuf {
    paths.blueprints().join(format!("{slug}.toml"))
}

/// Hold a name to what a filename stem and a command-line handle can both be.
///
/// Lower case, because two blueprints differing only in case would be one file on Windows and two
/// rows in SQLite — a disagreement the schema cannot settle and the filesystem decides silently.
///
/// # Errors
///
/// [`Error::InvalidBlueprintName`] naming which rule was broken, in the words the user is shown.
pub fn validated_slug(name: &str) -> Result<String> {
    let refuse = |reason: &'static str| {
        Err(Error::InvalidBlueprintName {
            name: name.to_owned(),
            reason,
        })
    };

    if name.is_empty() {
        return refuse("it is empty");
    }
    if name.len() > NAME_LIMIT {
        return refuse("it is longer than 64 characters");
    }
    if !name.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
    }) {
        return refuse("only lower-case letters, digits and hyphens are allowed");
    }
    if name.starts_with('-') || name.ends_with('-') {
        return refuse("it starts or ends with a hyphen");
    }

    Ok(name.to_owned())
}

/// Write one down, and render it beside the row.
///
/// **The file is written after the commit**, because the row is the truth: a file written for a
/// transaction that then rolled back would be a blueprint that exists on disk and nowhere else, and
/// the next `blueprint.list` would not know about it.
///
/// `trust` decides whether this build will later offer to run the manifest's own `[scaffold]`
/// command — roadmap task **T78a**, its design's D1. It is the caller's to settle, once, from where
/// the blueprint came from: a capture is this machine's own, and an import earns it only from a
/// signature that verified. **Nothing raises it afterwards**, which is why it is a parameter here
/// rather than something a later call can set.
///
/// **One value, two columns** — roadmap task **T79b**. The answer and the reason for it are written
/// from the same [`Trust`], so nothing can set one without the other, and the `ON CONFLICT` list
/// refreshes both: a row keeping the reason of a file it no longer holds would be exactly the
/// self-contradiction the readers below decline to reconcile.
///
/// # Errors
///
/// [`Error::InvalidBlueprintName`] for a slug that cannot be a filename;
/// [`Error::BlueprintExists`] when something is already filed under it and `overwrite` is not set;
/// [`Error::Database`] when the row cannot be written, and [`Error::Io`] when the rendering cannot.
pub async fn save(
    store: &Store,
    paths: &Paths,
    manifest: &BlueprintManifest,
    slug: &str,
    source: BlueprintSource,
    trust: Trust,
    overwrite: bool,
) -> Result<BlueprintSummary> {
    let slug = validated_slug(slug)?;
    let rendered = manifest::render(manifest);
    let name = manifest.blueprint.name.clone();
    let description = manifest.blueprint.description.clone();
    let created_at = manifest.blueprint.created_at.clone();
    let source_word = source.as_str();
    let trusted = trust.trusted();
    let vouched = i64::from(trusted);

    // The word, or NULL where no check happened. One vocabulary for the column and the wire, on
    // `BlueprintSource::as_str`'s rule.
    let signature = trust.signature();
    let recorded = signature.map(SignatureCheck::as_str);

    // `BEGIN IMMEDIATE` because this reads and then writes: two captures racing for one name would
    // otherwise both find it free.
    let mut tx = store
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|error| store.failure("write", error))?;

    let existing = sqlx::query_scalar!("SELECT id FROM blueprints WHERE id = ?", slug)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| store.failure("read", error))?;

    if existing.is_some() && !overwrite {
        return Err(Error::BlueprintExists { name: slug });
    }

    sqlx::query!(
        "INSERT INTO blueprints (id, name, description, manifest_toml, created_at, source, trusted,
                                 signature)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET
             name = excluded.name,
             description = excluded.description,
             manifest_toml = excluded.manifest_toml,
             created_at = excluded.created_at,
             source = excluded.source,
             trusted = excluded.trusted,
             signature = excluded.signature",
        slug,
        name,
        description,
        rendered,
        created_at,
        source_word,
        vouched,
        recorded,
    )
    .execute(&mut *tx)
    .await
    .map_err(|error| store.failure("write", error))?;

    tx.commit()
        .await
        .map_err(|error| store.failure("write", error))?;

    let path = file(paths, &slug);
    std::fs::write(&path, &rendered).map_err(|error| Error::Io {
        action: "write",
        path: path.clone(),
        source: error,
    })?;

    tracing::info!(blueprint = %slug, "a blueprint was written down");

    Ok(BlueprintSummary {
        slug,
        name,
        description,
        created_at,
        source,
        trusted,
        signature,
        file: path.display().to_string(),
    })
}

/// Every blueprint this home holds, in slug order.
///
/// # Errors
///
/// [`Error::Database`] when the table cannot be read, and [`Error::UnknownBlueprintSource`] for a
/// row whose `source` this build does not know — which is a hand-edited database or one written by
/// a newer build, and either way not something to answer with a guess.
pub async fn records(store: &Store, paths: &Paths) -> Result<Vec<BlueprintSummary>> {
    let rows = sqlx::query!(
        r#"SELECT id AS "id!: String", name AS "name!: String",
                  description AS "description!: String", created_at AS "created_at!: String",
                  source AS "source!: String", trusted AS "trusted!: bool",
                  signature AS "signature?: String"
           FROM blueprints
           ORDER BY id"#
    )
    .fetch_all(store.pool())
    .await
    .map_err(|error| store.failure("read", error))?;

    rows.into_iter()
        .map(|row| {
            let source = BlueprintSource::parse(&row.source).ok_or_else(|| {
                Error::UnknownBlueprintSource {
                    name: row.id.clone(),
                    value: row.source.clone(),
                }
            })?;

            let signature = recorded(&row.id, row.signature.as_deref());

            Ok(BlueprintSummary {
                file: file(paths, &row.id).display().to_string(),
                slug: row.id,
                name: row.name,
                description: row.description,
                created_at: row.created_at,
                source,
                trusted: row.trusted,
                signature,
            })
        })
        .collect()
}

/// One blueprint as it was filed: the manifest, and where it came from.
///
/// **One read rather than two.** The manifest is what a plan is built from and the source and trust
/// are what a client is told about it (roadmap task **T78a**, its design's D5), and reading the row
/// twice would be two chances to read two different things — the rule
/// [`crate::blueprints::plan`]'s one planning path already lives by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filed {
    /// What the blueprint says.
    pub manifest: BlueprintManifest,

    /// Where it came from.
    pub source: BlueprintSource,

    /// Whether its `[scaffold]` command may be offered — decided when it arrived, and never since.
    pub trusted: bool,

    /// Why, where anything was checked at all — roadmap task **T79b**.
    ///
    /// Read from its own column beside the answer rather than derived from it: a hand-edited row
    /// that disagrees with itself is answered with `trusted`, which is what decides, and this
    /// beside it. Reconciling the two here would be this build re-opening a question T78a settled.
    pub signature: Option<SignatureCheck>,
}

/// Read one back out of the column rather than off the disk (D7).
///
/// # Errors
///
/// [`Error::NotFound`] for a slug nothing is filed under, [`Error::UnknownBlueprintSchema`] and
/// [`Error::BlueprintManifest`] from the reader, [`Error::UnknownBlueprintSource`] for a source word
/// this build does not know, and [`Error::Database`] when the row cannot be read.
pub async fn filed_of(store: &Store, slug: &str) -> Result<Filed> {
    let row = sqlx::query!(
        r#"SELECT manifest_toml AS "manifest_toml!: String", source AS "source!: String",
                  trusted AS "trusted!: bool", signature AS "signature?: String"
           FROM blueprints WHERE id = ?"#,
        slug
    )
    .fetch_optional(store.pool())
    .await
    .map_err(|error| store.failure("read", error))?;

    let row = row.ok_or_else(|| Error::NotFound {
        kind: "blueprint",
        id: slug.to_owned(),
    })?;

    let source =
        BlueprintSource::parse(&row.source).ok_or_else(|| Error::UnknownBlueprintSource {
            name: slug.to_owned(),
            value: row.source.clone(),
        })?;

    Ok(Filed {
        manifest: manifest::read(&row.manifest_toml)?,
        source,
        trusted: row.trusted,
        signature: recorded(slug, row.signature.as_deref()),
    })
}

/// Whether anything is filed under this slug, without reading its manifest.
///
/// # Errors
///
/// [`Error::Database`] when the table cannot be read.
pub async fn exists(store: &Store, slug: &str) -> Result<bool> {
    let found = sqlx::query_scalar!("SELECT id FROM blueprints WHERE id = ?", slug)
        .fetch_optional(store.pool())
        .await
        .map_err(|error| store.failure("read", error))?;

    Ok(found.is_some())
}

/// The reason a row records, or [`None`] for a word this build does not know — roadmap task
/// **T79b**, its design's D3.
///
/// Where [`Error::UnknownBlueprintSource`] refuses to guess, this shrugs, and what each column is
/// for is the difference: `source` decides what a plan does, and this decides one sentence. A
/// listing killed by the one field on it that is decoration is the wrong price, so an unreadable
/// word costs a row its explanation and nothing else.
fn recorded(slug: &str, word: Option<&str>) -> Option<SignatureCheck> {
    let check = word.and_then(SignatureCheck::parse);

    if let (Some(word), None) = (word, check) {
        tracing::warn!(
            blueprint = %slug,
            %word,
            "a blueprint records a trust reason this build does not know"
        );
    }

    check
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::blueprints::{Header, Provenance};

    async fn home() -> (tempfile::TempDir, Store, Paths) {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&temp.path().join("mixengine.db"))
            .await
            .expect("a database");
        let paths = Paths::new(temp.path().to_path_buf(), &Default::default());
        std::fs::create_dir_all(paths.blueprints()).expect("a blueprints directory");

        (temp, store, paths)
    }

    fn a_manifest(name: &str) -> BlueprintManifest {
        BlueprintManifest {
            schema: manifest::SCHEMA,
            blueprint: Header {
                name: name.to_owned(),
                description: String::new(),
                created_at: "2026-09-01T09:00:00Z".to_owned(),
                created_on: Provenance {
                    os: "linux".to_owned(),
                    version: "0.1.0".to_owned(),
                },
            },
            runtimes: std::collections::BTreeMap::new(),
            site: None,
            services: Vec::new(),
            php: None,
            scaffold: None,
        }
    }

    /// **Trust is a column, not a derivation** — roadmap task **T78a**, its design's D1. A capture
    /// is this machine's own, and the flag it is saved with is what a scaffold consent is later
    /// checked against.
    #[tokio::test]
    async fn a_saved_blueprint_carries_the_trust_it_was_saved_with() {
        let (_temp, store, paths) = home().await;

        let saved = save(
            &store,
            &paths,
            &a_manifest("blog"),
            "blog",
            BlueprintSource::Captured,
            Trust::Inherent,
            false,
        )
        .await
        .expect("it saves");

        assert!(saved.trusted, "a capture is this machine's own");

        let listed = records(&store, &paths).await.expect("it lists");
        assert_eq!(listed.len(), 1);
        assert!(
            listed[0].trusted,
            "and the column is what the listing reads"
        );
    }

    /// An untrusted blueprint comes back untrusted, which is the half of D1 with teeth: nothing in
    /// this build raises the flag once it is written.
    #[tokio::test]
    async fn an_untrusted_blueprint_comes_back_untrusted() {
        let (_temp, store, paths) = home().await;

        save(
            &store,
            &paths,
            &a_manifest("borrowed"),
            "borrowed",
            BlueprintSource::Imported,
            Trust::Unsigned,
            false,
        )
        .await
        .expect("it saves");

        let listed = records(&store, &paths).await.expect("it lists");
        assert!(!listed[0].trusted);
    }

    /// **The reason survives in the row, not in the call** — roadmap task **T79b**. A file whose
    /// signature did not verify is untrusted for the same reason an unsigned one is, and the two
    /// are not the same event; both readers of the table say which one it was.
    #[tokio::test]
    async fn a_rejected_signature_is_remembered_beside_the_answer() {
        let (_temp, store, paths) = home().await;

        save(
            &store,
            &paths,
            &a_manifest("borrowed"),
            "borrowed",
            BlueprintSource::Imported,
            Trust::Rejected,
            false,
        )
        .await
        .expect("it saves");

        let listed = records(&store, &paths).await.expect("it lists");
        assert!(!listed[0].trusted);
        assert_eq!(listed[0].signature, Some(SignatureCheck::Rejected));

        let filed = filed_of(&store, "borrowed").await.expect("it reads back");
        assert!(!filed.trusted);
        assert_eq!(
            filed.signature,
            Some(SignatureCheck::Rejected),
            "the plan reads this one, and it is the same row"
        );
    }

    /// **The overwrite path refreshes the reason too** — roadmap task **T79b**, its design's D4.
    /// Left out of the `ON CONFLICT` list, this build would manufacture the self-contradicting row
    /// D3 declines to reconcile: `trusted = 0` beside `signature = 'verified'`.
    #[tokio::test]
    async fn re_importing_without_a_signature_forgets_the_old_reason() {
        let (_temp, store, paths) = home().await;

        for trust in [Trust::Verified, Trust::Unsigned] {
            save(
                &store,
                &paths,
                &a_manifest("borrowed"),
                "borrowed",
                BlueprintSource::Imported,
                trust,
                true,
            )
            .await
            .expect("it saves");
        }

        let listed = records(&store, &paths).await.expect("it lists");
        assert!(!listed[0].trusted);
        assert_eq!(
            listed[0].signature,
            Some(SignatureCheck::Missing),
            "the row kept the reason of a file it no longer holds"
        );
    }

    /// A capture is this machine's own and the gallery is this build's own: no check happened, so
    /// there is nothing to explain, and `source` is what says why.
    #[tokio::test]
    async fn a_blueprint_nobody_checked_carries_no_reason() {
        let (_temp, store, paths) = home().await;

        let saved = save(
            &store,
            &paths,
            &a_manifest("blog"),
            "blog",
            BlueprintSource::Captured,
            Trust::Inherent,
            false,
        )
        .await
        .expect("it saves");

        assert!(saved.trusted);
        assert_eq!(saved.signature, None);
    }

    /// **A word this build does not know reads as no reason** — roadmap task **T79b**, its design's
    /// D3. Where [`Error::UnknownBlueprintSource`] refuses to guess, this shrugs: `source` decides
    /// what a plan does and this decides one sentence, so a newer build's word costs a listing its
    /// explanation rather than costing it the listing.
    #[tokio::test]
    async fn a_reason_this_build_does_not_know_reads_as_no_reason() {
        let (_temp, store, paths) = home().await;

        save(
            &store,
            &paths,
            &a_manifest("borrowed"),
            "borrowed",
            BlueprintSource::Imported,
            Trust::Rejected,
            false,
        )
        .await
        .expect("it saves");

        sqlx::query!("UPDATE blueprints SET signature = 'revoked' WHERE id = 'borrowed'")
            .execute(store.pool())
            .await
            .expect("a row from a build this one has never met");

        let listed = records(&store, &paths).await.expect("it lists");
        assert_eq!(listed[0].signature, None);
        assert!(
            !listed[0].trusted,
            "the answer is read from its own column, not from the reason"
        );
    }

    /// **D7.** What is on disk is exactly what is in the column, byte for byte — which is what makes
    /// the file safe to hand somebody and pointless to read back.
    #[tokio::test]
    async fn a_saved_blueprint_is_a_row_and_a_file_that_agree() {
        let (_temp, store, paths) = home().await;

        let summary = save(
            &store,
            &paths,
            &a_manifest("blog"),
            "blog",
            BlueprintSource::Captured,
            Trust::Inherent,
            false,
        )
        .await
        .expect("it saves");

        assert_eq!(summary.slug, "blog");
        assert_eq!(summary.source, BlueprintSource::Captured);

        let on_disk = std::fs::read_to_string(file(&paths, "blog")).expect("the rendered file");
        let in_row: String =
            sqlx::query_scalar("SELECT manifest_toml FROM blueprints WHERE id = 'blog'")
                .fetch_one(store.pool())
                .await
                .expect("the row");

        assert_eq!(on_disk, in_row);
    }

    /// A second capture under one name is refused, because there is no `blueprint.delete` in this
    /// build and a typo would otherwise be permanent.
    #[tokio::test]
    async fn a_name_already_taken_is_refused_unless_overwriting() {
        let (_temp, store, paths) = home().await;
        let manifest = a_manifest("blog");

        save(
            &store,
            &paths,
            &manifest,
            "blog",
            BlueprintSource::Captured,
            Trust::Inherent,
            false,
        )
        .await
        .expect("the first one");

        assert!(matches!(
            save(
                &store,
                &paths,
                &manifest,
                "blog",
                BlueprintSource::Captured,
                Trust::Inherent,
                false
            )
            .await,
            Err(Error::BlueprintExists { .. })
        ));

        save(
            &store,
            &paths,
            &manifest,
            "blog",
            BlueprintSource::Captured,
            Trust::Inherent,
            true,
        )
        .await
        .expect("overwriting is allowed when it is asked for");

        assert_eq!(records(&store, &paths).await.expect("a listing").len(), 1);
    }

    /// **The name becomes a filename**, so it is refused at the boundary rather than resolved: this
    /// is the difference between an error message and a write outside the home.
    #[test]
    fn a_name_that_is_not_a_slug_is_refused_before_anything_touches_the_disk() {
        assert_eq!(
            validated_slug("laravel-php82").expect("a slug"),
            "laravel-php82"
        );

        for bad in [
            "../../etc/x",
            "with space",
            "UPPER",
            "",
            "a/b",
            "a\\b",
            "..",
            "-leading",
            "trailing-",
            "dot.",
        ] {
            assert!(
                matches!(validated_slug(bad), Err(Error::InvalidBlueprintName { .. })),
                "{bad:?} was accepted"
            );
        }
    }

    /// Reading back something nobody saved names the namespace, which is what gives the daemon its
    /// `mix blueprint list` hint.
    #[tokio::test]
    async fn an_unknown_slug_is_not_found() {
        let (_temp, store, _paths) = home().await;

        assert!(matches!(
            filed_of(&store, "nothing").await,
            Err(Error::NotFound {
                kind: "blueprint",
                ..
            })
        ));
    }

    /// A row from a build that knew a fourth source is reported rather than guessed at.
    #[tokio::test]
    async fn a_source_this_build_does_not_know_is_refused() {
        let (_temp, store, paths) = home().await;

        save(
            &store,
            &paths,
            &a_manifest("blog"),
            "blog",
            BlueprintSource::Captured,
            Trust::Inherent,
            false,
        )
        .await
        .expect("it saves");

        sqlx::query("UPDATE blueprints SET source = 'borrowed' WHERE id = 'blog'")
            .execute(store.pool())
            .await
            .expect("a hand-edited row");

        assert!(matches!(
            records(&store, &paths).await,
            Err(Error::UnknownBlueprintSource { .. })
        ));
    }
}
