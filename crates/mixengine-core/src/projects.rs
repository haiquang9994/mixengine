//! The `projects` table: a directory this home has been told about, and the versions it pins.
//!
//! [`crate::packages`]' shape one table across — this module owns every write to `projects` and
//! nothing else — with one addition that is the point of the task: [`find`] is the walk
//! [`crate::resolve`] used to hold privately. Two implementations of "which project is this
//! directory in?" would be two answers to a question that has exactly one, which is the same rule
//! that put `resolve` in this crate to begin with.
//!
//! # One directory is one project, on both sides of the comparison
//!
//! `root_path` is `UNIQUE`, and it is normalised through
//! [`mixengine_platform::paths::in_full`] **before it is written** — that is what makes
//! `C:\Users\RUNNER~1\blog` and `C:\Users\runneradmin\blog` one project rather than two. The query
//! side is normalised as well, once, before the walk starts: a row normalised on the way in and a
//! caller's `cwd` that was not are two different strings for one directory, and step 3 of the
//! resolution order would miss on the very day it first had a row to hit.
//!
//! `in_full` expands 8.3 aliases and settles case. It does **not** follow symlinks or junctions, so
//! two paths reaching one directory through a junction can still register as two projects. That is
//! a known limit rather than an oversight: `std::fs::canonicalize` on Windows answers with a `\\?\`
//! verbatim path, a spelling nothing else in this workspace uses and which would leak into every
//! message and every rendered file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mixengine_platform::paths::in_full;
use mixengine_proto::{
    PackageVersion, PinSource, ProjectPin, ProjectRef, RuntimeKind, ServiceId, Timestamp,
    VersionConstraint,
};

use crate::{Error, Result, Store};

/// The longest a project's name may be.
///
/// A handle typed on a command line, and T39a will take a site's default domain from it.
const NAME_LIMIT: usize = 64;

/// One row of `projects`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecord {
    /// The rowid, which stays inside this crate: the wire handle is the name (spec D4).
    pub id: i64,

    /// What it is called.
    pub name: String,

    /// Its root, spelled the way the filesystem spells it.
    pub root: PathBuf,

    /// The versions it wants, by language, with anything this build cannot read left out.
    pub pins: BTreeMap<RuntimeKind, VersionConstraint>,

    /// When it was registered, as ISO-8601 text.
    pub created_at: String,

    /// Whether this project's services are held out of idle shutdown — roadmap task **T69**.
    ///
    /// *The one project being worked on all day*, in the feature doc's words. What it reaches is
    /// [`kept_warm`], which today is the PHP pool this project's sites name.
    pub keep_warm: bool,
}

/// Everything registering a project has to write down.
#[derive(Debug, Clone)]
pub struct Registration {
    /// What to call it, already fallen through the manifest and the directory name.
    pub name: String,

    /// The root, as the caller spelled it. Normalised here.
    pub root: PathBuf,

    /// The pins, already fallen through the manifest.
    pub pins: BTreeMap<RuntimeKind, VersionConstraint>,
}

/// What an update is changing, where [`None`] means "leave it".
#[derive(Debug, Clone, Default)]
pub struct Change {
    /// A new name.
    pub name: Option<String>,

    /// A new root.
    pub root: Option<PathBuf>,

    /// The pins, **replacing** what the row held. An empty map clears them.
    pub pins: Option<BTreeMap<RuntimeKind, VersionConstraint>>,

    /// Whether to hold this project's services out of idle shutdown — roadmap task **T69**.
    ///
    /// A field on the change every other project setting travels through rather than a
    /// `project.keep_warm` method of its own: a whole RPC for one boolean is a second door onto one
    /// row, and the rules would then have two places to be applied differently in.
    pub keep_warm: Option<bool>,
}

/// A name, trimmed, or the reason it is not one.
///
/// # Errors
///
/// [`Error::InvalidProjectName`] for an empty name, one over sixty-four characters, one holding a
/// control character, and one holding `/` or `\` — a name that carries a path separator can be
/// neither a command-line handle nor the domain T39a will make of it.
pub fn validated_name(name: &str) -> Result<String> {
    let trimmed = name.trim();

    let refusal = if trimmed.is_empty() {
        Some("it is empty")
    } else if trimmed.chars().count() > NAME_LIMIT {
        Some("it is longer than sixty-four characters")
    } else if trimmed.chars().any(char::is_control) {
        Some("it holds a control character")
    } else if trimmed.contains('/') || trimmed.contains('\\') {
        Some("it holds a path separator")
    } else {
        None
    };

    match refusal {
        Some(because) => Err(Error::InvalidProjectName {
            name: name.to_owned(),
            because,
        }),
        None => Ok(trimmed.to_owned()),
    }
}

/// Register a directory as a project.
///
/// # Errors
///
/// [`Error::InvalidProjectName`] for a name that is not one; [`Error::ProjectNameTaken`] and
/// [`Error::ProjectRootTaken`] for the two unique columns, the second naming the project already
/// holding the directory; and [`Error::Database`] when the row cannot be written.
pub async fn create(
    store: &Store,
    registration: &Registration,
    at: Timestamp,
) -> Result<ProjectRecord> {
    let name = validated_name(&registration.name)?;
    let root = in_full(&registration.root);
    let root_column = root.display().to_string();
    let created_at = at.to_rfc3339();
    let pins = encode(&registration.pins);

    // Asked before the insert so the answer can name the project that is in the way, which a unique
    // index cannot — and asked again by the index underneath, which is what makes two clients
    // racing produce a refusal rather than two rows.
    if let Some((_, holder)) = holder(store, &root_column).await? {
        return Err(Error::ProjectRootTaken {
            root: root_column,
            holder,
        });
    }

    let inserted = sqlx::query!(
        "INSERT INTO projects (name, root_path, runtime_pins_json, created_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT DO NOTHING",
        name,
        root_column,
        pins,
        created_at
    )
    .execute(store.pool())
    .await
    .map_err(|source| store.failure("write", source))?;

    if inserted.rows_affected() == 0 {
        return match holder(store, &root_column).await? {
            Some((_, holder)) => Err(Error::ProjectRootTaken {
                root: root_column,
                holder,
            }),
            None => Err(Error::ProjectNameTaken { name }),
        };
    }

    tracing::info!(%name, root = %root_column, "a project was registered");

    Ok(ProjectRecord {
        id: inserted.last_insert_rowid(),
        name,
        root,
        pins: registration.pins.clone(),
        created_at,
        // The column's default, said here rather than read back: a project is registered because
        // somebody wants to work on it eventually, not because they are working on it now.
        keep_warm: false,
    })
}

/// Every service a keep-warm project reaches, and which project reaches it — roadmap task **T69**.
///
/// **One join, and it is `sites.php_service_id`.** That is the whole of what today's schema knows
/// about which services a project uses, so this keeps a project's PHP pool warm and does **not**
/// keep the database it queries warm. A `project_services` table would be a second description of a
/// relationship `sites` already half-holds, written by nobody and read by one sweeper.
///
/// The missing half belongs to **T77**, where a project's blueprint declares what it needs; when
/// that lands this query widens rather than being replaced. Until then the gap costs nothing,
/// because with no recipe offering an idle default no database is a candidate for stopping anyway.
///
/// **The project's name travels with the service** rather than being looked up again by whoever
/// renders the exemption. It is already in this join, and the alternative is a second query per
/// sweep to say something this one has in hand. Two projects keeping one pool warm is one entry:
/// what a person asked was *why is this still running*, and the first answer is a whole answer.
///
/// # Errors
///
/// [`Error::Database`] when the tables cannot be read.
pub async fn kept_warm(store: &Store) -> Result<BTreeMap<ServiceId, String>> {
    let rows = sqlx::query!(
        r#"SELECT s.php_service_id AS "service!: String", p.name AS "project!: String"
           FROM sites s
           JOIN projects p ON p.id = s.project_id
           WHERE p.keep_warm = 1 AND s.php_service_id IS NOT NULL
           ORDER BY p.name"#
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    // A row this build cannot parse is skipped rather than raised. `php_service_id` is a foreign key
    // into `services`, so an unreadable one is a database somebody edited by hand — and the cost of
    // being wrong here is a service that keeps running, which is the direction every reading in this
    // task errs in.
    Ok(rows
        .into_iter()
        .filter_map(|row| Some((ServiceId::parse(&row.service).ok()?, row.project)))
        .collect())
}

/// Every registered project, in name order.
///
/// The order a listing is scanned in, on [`crate::packages::records`]' reasoning: a table somebody
/// looks for a row in should be in the order the eye can predict.
///
/// # Errors
///
/// [`Error::UnreadableProjectRow`] for a row this build cannot read at all, and [`Error::Database`]
/// when the table cannot be read.
pub async fn records(store: &Store) -> Result<Vec<ProjectRecord>> {
    let rows = sqlx::query!(
        r#"SELECT id, name, root_path, runtime_pins_json, created_at,
                  keep_warm AS "keep_warm: bool"
           FROM projects ORDER BY name"#
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    rows.into_iter()
        .map(|row| {
            Ok(ProjectRecord {
                id: row.id,
                pins: decode(&row.root_path, &row.runtime_pins_json)?,
                root: PathBuf::from(row.root_path),
                name: row.name,
                created_at: row.created_at,
                keep_warm: row.keep_warm,
            })
        })
        .collect()
}

/// The project a reference names, or [`None`].
///
/// A [`ProjectRef::Path`] is answered by **walking up**: the nearest registered root at or above
/// that directory, which is what a shell three directories deep inside a repository means. One
/// `in_full` call before the walk rather than one per ancestor — the answer is the same and the
/// walk is the hot half.
///
/// # Errors
///
/// The errors [`records`] gives.
pub async fn find(store: &Store, reference: &ProjectRef) -> Result<Option<ProjectRecord>> {
    let known = records(store).await?;

    match reference {
        ProjectRef::Name(name) => Ok(known.into_iter().find(|project| &project.name == name)),

        ProjectRef::Path(path) => Ok(nearest(&known, Path::new(path), |_| true)),
    }
}

/// The nearest project at or above a directory **that pins this language**.
///
/// Step 3 of the resolution order, and a different question from [`find`] rather than a wrapper
/// over it: a project silent about PHP is not an answer about PHP, so the walk carries on to the
/// project above it. That is [`crate::resolve`]'s rule one step higher up as well — a nearer
/// `mixengine.toml` naming only Node does not shadow an outer one naming PHP — and a repository
/// with a package of its own registered inside it is exactly the shape that meets it.
///
/// # Errors
///
/// The errors [`records`] gives.
pub async fn pinning(
    store: &Store,
    kind: RuntimeKind,
    directory: &Path,
) -> Result<Option<ProjectRecord>> {
    let known = records(store).await?;

    Ok(nearest(&known, directory, |project| {
        project.pins.contains_key(&kind)
    }))
}

/// The nearest of `known` at or above `directory` that `accept` agrees to.
///
/// The one walk both questions above are asked through, which is what keeps "which project is this
/// directory in?" from having two answers. `in_full` once before it starts rather than once per
/// ancestor: the rows were normalised on the way in, the caller's directory was not, and comparing
/// the two unnormalised is how step 3 would miss on the very day it first had a row to hit.
fn nearest(
    known: &[ProjectRecord],
    directory: &Path,
    accept: impl Fn(&ProjectRecord) -> bool,
) -> Option<ProjectRecord> {
    let directory = in_full(directory);

    directory.ancestors().find_map(|ancestor| {
        known
            .iter()
            .find(|project| project.root == ancestor && accept(project))
            .cloned()
    })
}

/// Change a project's name, root or pins.
///
/// # Errors
///
/// [`Error::NotFound`] when the row is gone, the two taken-column errors [`create`] gives, and
/// [`Error::Database`] when the row cannot be written.
pub async fn update(store: &Store, id: i64, change: &Change) -> Result<ProjectRecord> {
    let mut project = records(store)
        .await?
        .into_iter()
        .find(|project| project.id == id)
        .ok_or_else(|| missing(&id.to_string()))?;

    if let Some(name) = &change.name {
        project.name = validated_name(name)?;
    }

    if let Some(root) = &change.root {
        project.root = in_full(root);
    }

    if let Some(pins) = &change.pins {
        project.pins = pins.clone();
    }

    if let Some(keep_warm) = change.keep_warm {
        project.keep_warm = keep_warm;
    }

    let root_column = project.root.display().to_string();
    let pins = encode(&project.pins);

    // The same reading `create` does, and for the same reason: the answer has to be able to name
    // the project in the way. Compared by rowid rather than by name, so this one never accuses the
    // row of being in its own way — a rename is written while the row still holds the directory.
    if let Some((occupant, holder)) = holder(store, &root_column).await?
        && occupant != id
    {
        return Err(Error::ProjectRootTaken {
            root: root_column,
            holder,
        });
    }

    let written = sqlx::query!(
        "UPDATE projects SET name = ?, root_path = ?, runtime_pins_json = ?, keep_warm = ?
         WHERE id = ?",
        project.name,
        root_column,
        pins,
        project.keep_warm,
        id
    )
    .execute(store.pool())
    .await;

    match written {
        Ok(_) => {}
        // The name is the other unique column, and SQLite's own message names an index rather than
        // the project. Classified rather than passed through, so a client meets one vocabulary.
        Err(sqlx::Error::Database(failure)) if failure.is_unique_violation() => {
            return Err(Error::ProjectNameTaken { name: project.name });
        }
        Err(source) => return Err(store.failure("write", source)),
    }

    tracing::info!(name = %project.name, root = %root_column, "a project was changed");

    Ok(project)
}

/// Forget a project. The directory is not touched.
///
/// # Errors
///
/// [`Error::NotFound`] when there is no such row, and [`Error::Database`] when it cannot be
/// written.
pub async fn delete(store: &Store, id: i64) -> Result<()> {
    let removed = sqlx::query!("DELETE FROM projects WHERE id = ?", id)
        .execute(store.pool())
        .await
        .map_err(|source| store.failure("write", source))?;

    if removed.rows_affected() == 0 {
        return Err(missing(&id.to_string()));
    }

    tracing::info!(project = id, "a project was forgotten");

    Ok(())
}

/// The project already holding this exact directory, if any: its rowid and its name.
///
/// **Both halves, because the two callers need different ones.** [`create`] names the project in
/// the way, which a unique index cannot do. [`update`] has to tell "somebody else has it" from
/// "this row has it", and the row's own name is not the test: a rename changes the name being
/// written while the row still holds the directory, so comparing names would make every rename
/// refuse itself.
async fn holder(store: &Store, root: &str) -> Result<Option<(i64, String)>> {
    let found = sqlx::query!("SELECT id, name FROM projects WHERE root_path = ?", root)
        .fetch_optional(store.pool())
        .await
        .map_err(|source| store.failure("read", source))?;

    Ok(found.map(|row| (row.id, row.name)))
}

/// The pins column, as a map this build can use.
///
/// **Read as strings and parsed one value at a time**, which is `resolve`'s rule and its reason: a
/// row written by a build that manages a fifth language must not stop this one reading the project.
/// A key or a value this build cannot read is left out rather than fatal.
fn decode(root: &str, column: &str) -> Result<BTreeMap<RuntimeKind, VersionConstraint>> {
    let raw: BTreeMap<String, String> =
        serde_json::from_str(column).map_err(|_| Error::UnreadableProjectRow {
            root: root.to_owned(),
            column: "runtime_pins_json",
            value: column.to_owned(),
        })?;

    Ok(raw
        .into_iter()
        .filter_map(|(kind, constraint)| {
            Some((
                RuntimeKind::parse(&kind)?,
                VersionConstraint::parse(constraint).ok()?,
            ))
        })
        .collect())
}

/// The map, as the column holds it.
///
/// Serialising a map of strings cannot fail; written as a fallback rather than an `expect` because
/// nothing in this crate panics, and an empty object is what a project with no pins already means —
/// which is what `packages::remember` says beside the same call.
fn encode(pins: &BTreeMap<RuntimeKind, VersionConstraint>) -> String {
    let raw: BTreeMap<&str, &str> = pins
        .iter()
        .map(|(kind, constraint)| (kind.as_str(), constraint.as_str()))
        .collect();

    serde_json::to_string(&raw).unwrap_or_else(|_| "{}".to_owned())
}

/// The failure of looking one up, named the way the wire mapping expects.
fn missing(id: &str) -> Error {
    Error::NotFound {
        kind: "project",
        id: id.to_owned(),
    }
}

/// One project's pins in **effective** order, with what each resolves to today.
///
/// Effective means the manifest's entry where the root has one and the row's where it does not,
/// because that is what the shim will actually do (spec D1) — a panel showing anything else is a
/// panel that lies. Nothing here refuses an unsatisfiable pin: `project.create` on a colleague's
/// freshly cloned repository has to succeed on the machine that most needs telling what to install
/// (spec D6).
///
/// # Errors
///
/// [`Error::Manifest`] for a manifest at the root that does not parse, and [`Error::Database`] when
/// the installed set cannot be read.
pub async fn effective_pins(store: &Store, project: &ProjectRecord) -> Result<Vec<ProjectPin>> {
    let path = crate::manifest::at(&project.root);
    let manifest = crate::manifest::read(&path)?;
    let manifest_path = path.display().to_string();

    let mut effective: BTreeMap<RuntimeKind, (VersionConstraint, PinSource)> = project
        .pins
        .iter()
        .map(|(kind, constraint)| (*kind, (constraint.clone(), PinSource::Registered)))
        .collect();

    if let Some(manifest) = manifest {
        for (kind, constraint) in manifest.runtimes {
            effective.insert(
                kind,
                (
                    constraint,
                    PinSource::Manifest {
                        path: manifest_path.clone(),
                    },
                ),
            );
        }
    }

    let mut pins = Vec::with_capacity(effective.len());

    for (kind, (constraint, source)) in effective {
        let resolved = newest_matching(store, kind, &constraint, None).await?;

        pins.push(ProjectPin {
            hint: resolved
                .is_none()
                .then(|| crate::resolve::install_command(kind, &constraint)),
            kind,
            constraint,
            source,
            resolved,
        });
    }

    Ok(pins)
}

/// A project whose pin a removal would leave with no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokenPin {
    /// Which project, by the name a person would type at it.
    pub project: String,

    /// Which language.
    pub kind: RuntimeKind,

    /// What it asks for.
    pub constraint: VersionConstraint,
}

/// The projects whose pin **goes from having an answer to having none** if this version is removed.
///
/// The transition is the whole of it (spec D7). A pin that is already unsatisfiable stays
/// unsatisfiable and refuses nothing, or one stale pin would make unremovable a runtime it never
/// mentions; and a pin three installed versions answer is not broken by losing one of them.
///
/// Reading every project's manifest here is affordable *here*: an uninstall deletes hundreds of
/// megabytes and runs perhaps monthly. It is not affordable anywhere near the shim, which is why
/// [`crate::resolve`] reads one file per ancestor and stops at the first answer.
///
/// # Errors
///
/// The errors [`effective_pins`] gives, and [`Error::Database`] when `projects` cannot be read.
pub async fn pins_broken_by(
    store: &Store,
    kind: RuntimeKind,
    version: &PackageVersion,
) -> Result<Vec<BrokenPin>> {
    let mut broken = Vec::new();

    for project in records(store).await? {
        let Some(pin) = effective_pins(store, &project)
            .await?
            .into_iter()
            .find(|pin| pin.kind == kind)
        else {
            continue;
        };

        // Already unsatisfiable, so this removal is not what breaks it.
        if pin.resolved.is_none() {
            continue;
        }

        if newest_matching(store, kind, &pin.constraint, Some(version))
            .await?
            .is_none()
        {
            broken.push(BrokenPin {
                project: project.name,
                kind,
                constraint: pin.constraint,
            });
        }
    }

    Ok(broken)
}

/// The newest installed version of `kind` that answers `constraint`, ignoring `without`.
///
/// The resolver's own choice — [`PackageVersion::cmp_precedence`], the newest as upstream means
/// newest rather than as ASCII does — so a pin is judged here exactly as it will be judged when
/// somebody `cd`s into the directory.
async fn newest_matching(
    store: &Store,
    kind: RuntimeKind,
    constraint: &VersionConstraint,
    without: Option<&PackageVersion>,
) -> Result<Option<PackageVersion>> {
    Ok(crate::runtimes::records(store, Some(kind))
        .await?
        .into_iter()
        .map(|runtime| runtime.version)
        .filter(|version| without != Some(version))
        .filter(|version| constraint.matches(version))
        .max_by(PackageVersion::cmp_precedence))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Timestamp = Timestamp(1_760_000_000_000);

    async fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&home.path().join(crate::paths::DATABASE_FILE_NAME))
            .await
            .expect("a database");
        (home, store)
    }

    /// A keep-warm project reaches the pool its sites name, and nothing else.
    ///
    /// **The second half is the assertion worth having.** Today the only edge from a project to a
    /// service is `sites.php_service_id`, so a project's *database* is not reached — and the day
    /// T77 gives a project a way to declare what it needs, this test is what says the reach
    /// changed rather than a user discovering it.
    #[tokio::test]
    async fn a_keep_warm_project_reaches_the_pool_its_sites_name() {
        let (_home, store) = store().await;
        let (_tree, root) = tree(&["shop"]);

        let project = create(&store, &registration("shop", &root), NOW)
            .await
            .expect("a project");

        service_row(&store, "php-fpm@8.3").await;
        service_row(&store, "mariadb@main").await;
        site_row(&store, project.id, "php-fpm@8.3").await;

        assert!(
            kept_warm(&store).await.expect("the warm set").is_empty(),
            "a project nobody asked to keep warm keeps nothing warm"
        );

        update(
            &store,
            project.id,
            &Change {
                keep_warm: Some(true),
                ..Change::default()
            },
        )
        .await
        .expect("the project is updated");

        let warm = kept_warm(&store).await.expect("the warm set");

        assert_eq!(
            warm.get(&ServiceId::parse("php-fpm@8.3").expect("an id")),
            Some(&"shop".to_owned()),
            "the pool this project's site names is kept warm, and by a project with a name: {warm:?}"
        );
        assert!(
            !warm.contains_key(&ServiceId::parse("mariadb@main").expect("an id")),
            "no schema relates a project to its database yet — that edge is T77's"
        );
    }

    /// A `packages` row and a `services` row for it, which is all a site needs to point at one.
    async fn service_row(store: &Store, id: &str) {
        sqlx::query(
            "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
             VALUES (?, '1.0.0', '/packages/x', '2026-08-26T00:00:00Z', 'https://example', 'ab')",
        )
        .bind(id)
        .execute(store.pool())
        .await
        .expect("a package");

        sqlx::query(
            "INSERT INTO services (id, package_id, instance_name, state)
             VALUES (?, (SELECT id FROM packages WHERE name = ?), 'main', 'stopped')",
        )
        .bind(id)
        .bind(id)
        .execute(store.pool())
        .await
        .expect("a service");
    }

    /// A php-fpm site belonging to `project`, pointing at `pool`.
    async fn site_row(store: &Store, project: i64, pool: &str) {
        sqlx::query(
            "INSERT INTO sites (project_id, doc_root, kind, php_service_id, state)
             VALUES (?, '/srv/shop/public', 'php-fpm', ?, 'enabled')",
        )
        .bind(project)
        .bind(pool)
        .execute(store.pool())
        .await
        .expect("a site");
    }

    /// A real directory to register, because `create` normalises what it is given.
    fn tree(depth: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().expect("a temporary directory");
        let mut path = root.path().to_path_buf();
        for name in depth {
            path = path.join(name);
        }
        std::fs::create_dir_all(&path).expect("a directory");
        (root, path)
    }

    fn pins(entries: &[(RuntimeKind, &str)]) -> BTreeMap<RuntimeKind, VersionConstraint> {
        entries
            .iter()
            .map(|(kind, text)| {
                (
                    *kind,
                    VersionConstraint::parse((*text).to_owned()).expect("a constraint"),
                )
            })
            .collect()
    }

    fn registration(name: &str, root: &Path) -> Registration {
        Registration {
            name: name.to_owned(),
            root: root.to_path_buf(),
            pins: BTreeMap::new(),
        }
    }

    /// Write the rows an install would have written, without the eighty megabytes.
    async fn install(store: &Store, kind: RuntimeKind, versions: &[&str]) {
        for version in versions {
            crate::runtimes::remember(
                store,
                &crate::runtimes::Installation {
                    kind,
                    version: PackageVersion::parse(*version).expect("a version"),
                    channel: mixengine_proto::PackageChannel::Stable,
                    path: PathBuf::from("/home/runtimes")
                        .join(kind.as_str())
                        .join(version),
                    bytes: 41_000_000,
                    url: format!("https://example.invalid/{kind}-{version}.tar.zst"),
                    sha256: "00".to_owned(),
                    provides: [(kind.as_str().to_owned(), format!("bin/{kind}"))]
                        .into_iter()
                        .collect(),
                    extension_dir: None,
                    extensions: crate::index::Extensions::default(),
                },
                NOW,
            )
            .await
            .expect("a row");
        }
    }

    /// **D1 and D6 together.** The manifest outranks the row, so a contradicting row pin is inert —
    /// and saying which one is in charge is the whole reason this answer carries a source.
    #[tokio::test]
    async fn a_manifest_pin_outranks_the_rows_and_says_so() {
        let (_home, store) = store().await;
        install(&store, RuntimeKind::Php, &["8.2.20", "8.3.33"]).await;
        let (_root, blog) = tree(&["blog"]);
        std::fs::write(crate::manifest::at(&blog), "[runtimes]\nphp = \"8.3\"\n")
            .expect("a manifest");

        let mut asked = registration("blog", &blog);
        asked.pins = pins(&[(RuntimeKind::Php, "8.2"), (RuntimeKind::Node, "22")]);
        let project = create(&store, &asked, NOW).await.expect("a project");

        let effective = effective_pins(&store, &project).await.expect("the pins");

        let php = effective
            .iter()
            .find(|pin| pin.kind == RuntimeKind::Php)
            .expect("php is pinned");
        assert_eq!(php.constraint.as_str(), "8.3", "the file wins");
        assert!(matches!(php.source, PinSource::Manifest { .. }), "{php:?}");
        assert_eq!(
            php.resolved.as_ref().map(PackageVersion::as_str),
            Some("8.3.33")
        );
        assert_eq!(php.hint, None, "a pin that resolves needs no advice");

        let node = effective
            .iter()
            .find(|pin| pin.kind == RuntimeKind::Node)
            .expect("node is pinned by the row");
        assert_eq!(node.source, PinSource::Registered);
        assert_eq!(node.resolved, None, "no node is installed");
        assert!(
            node.hint
                .as_deref()
                .is_some_and(|hint| hint.contains("runtime install node")),
            "a pin nothing satisfies carries the command that would fix it: {node:?}"
        );
    }

    /// **The test the refusal is worth having.** Same pin, same command, two outcomes — and only a
    /// re-resolution against what would be left produces both.
    #[tokio::test]
    async fn a_removal_breaks_a_pin_only_when_it_takes_the_last_version_that_answers_it() {
        let (_home, store) = store().await;
        install(&store, RuntimeKind::Php, &["8.3.33", "8.3.34"]).await;
        let (_root, blog) = tree(&["blog"]);

        let mut asked = registration("blog", &blog);
        asked.pins = pins(&[(RuntimeKind::Php, "^8.3")]);
        create(&store, &asked, NOW).await.expect("a project");

        let first = pins_broken_by(
            &store,
            RuntimeKind::Php,
            &PackageVersion::parse("8.3.33").expect("a version"),
        )
        .await
        .expect("a reading");
        assert!(
            first.is_empty(),
            "^8.3 still has 8.3.34 to answer it: {first:?}"
        );

        crate::runtimes::forget(
            &store,
            RuntimeKind::Php,
            &PackageVersion::parse("8.3.33").expect("a version"),
        )
        .await
        .expect("the row");

        let second = pins_broken_by(
            &store,
            RuntimeKind::Php,
            &PackageVersion::parse("8.3.34").expect("a version"),
        )
        .await
        .expect("a reading");
        assert_eq!(second.len(), 1, "{second:?}");
        assert_eq!(second[0].project, "blog");
        assert_eq!(second[0].constraint.as_str(), "^8.3");
    }

    /// A pin that is already unsatisfiable must refuse nothing — otherwise one stale pin would make
    /// unremovable a runtime it never mentions.
    #[tokio::test]
    async fn a_pin_nothing_already_satisfies_breaks_over_nothing() {
        let (_home, store) = store().await;
        install(&store, RuntimeKind::Php, &["8.3.33"]).await;
        let (_root, blog) = tree(&["blog"]);

        let mut asked = registration("blog", &blog);
        asked.pins = pins(&[(RuntimeKind::Php, "8.1")]);
        create(&store, &asked, NOW).await.expect("a project");

        let broken = pins_broken_by(
            &store,
            RuntimeKind::Php,
            &PackageVersion::parse("8.3.33").expect("a version"),
        )
        .await
        .expect("a reading");

        assert!(broken.is_empty(), "{broken:?}");
    }

    /// **D7 reads the pin in effective order**, so a refusal is never based on a row the file
    /// overrides.
    #[tokio::test]
    async fn a_row_pin_the_manifest_overrides_refuses_nothing() {
        let (_home, store) = store().await;
        install(&store, RuntimeKind::Php, &["8.2.20", "8.3.33"]).await;
        let (_root, blog) = tree(&["blog"]);
        std::fs::write(crate::manifest::at(&blog), "[runtimes]\nphp = \"8.2\"\n")
            .expect("a manifest");

        let mut asked = registration("blog", &blog);
        asked.pins = pins(&[(RuntimeKind::Php, "8.3.33")]);
        create(&store, &asked, NOW).await.expect("a project");

        let broken = pins_broken_by(
            &store,
            RuntimeKind::Php,
            &PackageVersion::parse("8.3.33").expect("a version"),
        )
        .await
        .expect("a reading");

        assert!(
            broken.is_empty(),
            "the file pins 8.2, so removing 8.3.33 breaks nothing that would ever take effect: \
             {broken:?}"
        );
    }

    /// What was written is what comes back.
    #[tokio::test]
    async fn a_registered_project_is_listed_with_the_pins_it_was_given() {
        let (_home, store) = store().await;
        let (_root, blog) = tree(&["blog"]);

        let mut asked = registration("blog", &blog);
        asked.pins = pins(&[(RuntimeKind::Php, "^8.3")]);

        let written = create(&store, &asked, NOW).await.expect("a project");

        assert_eq!(written.name, "blog");
        assert_eq!(written.pins, asked.pins);
        assert_eq!(written.created_at, NOW.to_rfc3339());
        assert_eq!(records(&store).await.expect("a listing"), vec![written]);
    }

    /// **D3.** The walk `resolve::in_a_project` used to hold: the nearest registered root at or
    /// above the directory, and never a further one when a nearer one exists.
    #[tokio::test]
    async fn a_directory_finds_the_nearest_project_above_it() {
        let (_home, store) = store().await;
        let (root, deep) = tree(&["blog", "packages", "theme", "src"]);
        let inner = root.path().join("blog").join("packages").join("theme");

        create(&store, &registration("outer", root.path()), NOW)
            .await
            .expect("the outer project");
        create(&store, &registration("theme", &inner), NOW)
            .await
            .expect("the inner project");

        let found = find(&store, &ProjectRef::Path(deep.display().to_string()))
            .await
            .expect("a lookup")
            .expect("something above it");

        assert_eq!(found.name, "theme", "the nearer root wins");

        // And a root inside another project's root is allowed: nesting has a defined answer.
        let outer = find(
            &store,
            &ProjectRef::Path(root.path().join("blog").display().to_string()),
        )
        .await
        .expect("a lookup")
        .expect("the outer project");
        assert_eq!(outer.name, "outer");
    }

    /// **D5, the half that would otherwise be silently broken on Windows.** A row normalised on the
    /// way in and a directory that was not are two strings for one directory — so the query side is
    /// normalised too, and the test asks it the way a caller does.
    #[tokio::test]
    async fn a_directory_is_found_however_this_filesystem_spells_it() {
        let (_home, store) = store().await;
        let (root, blog) = tree(&["blog"]);

        create(&store, &registration("blog", &blog), NOW)
            .await
            .expect("a project");

        // The temporary root as the OS handed it over — which on Windows may be an 8.3 alias — and
        // the same directory spelled in full. Both are the project.
        for spelling in [root.path().join("blog"), in_full(&root.path().join("blog"))] {
            assert!(
                find(&store, &ProjectRef::Path(spelling.display().to_string()))
                    .await
                    .expect("a lookup")
                    .is_some(),
                "{} did not find the project",
                spelling.display()
            );
        }
    }

    /// One directory is one project, and the refusal names the project already holding it.
    #[tokio::test]
    async fn the_same_directory_twice_is_refused_by_the_name_that_has_it() {
        let (_home, store) = store().await;
        let (_root, blog) = tree(&["blog"]);

        create(&store, &registration("blog", &blog), NOW)
            .await
            .expect("the first");
        let error = create(&store, &registration("other", &blog), NOW)
            .await
            .expect_err("the second");

        assert!(
            matches!(&error, Error::ProjectRootTaken { holder, .. } if holder == "blog"),
            "{error:?}"
        );
    }

    /// The other unique column, whose repair is different: a name is not freed by moving anything.
    #[tokio::test]
    async fn the_same_name_twice_is_refused() {
        let (_home, store) = store().await;
        let (_first, one) = tree(&["blog"]);
        let (_second, two) = tree(&["shop"]);

        create(&store, &registration("blog", &one), NOW)
            .await
            .expect("the first");
        let error = create(&store, &registration("blog", &two), NOW)
            .await
            .expect_err("the second");

        assert!(matches!(error, Error::ProjectNameTaken { .. }), "{error:?}");
    }

    /// **D4.** A handle typed on a command line, and T39a will make a domain out of it.
    #[test]
    fn a_name_is_a_handle_rather_than_free_text() {
        assert_eq!(validated_name("  blog  ").expect("trimmed"), "blog");

        for refused in [
            "",
            "   ",
            "blog/site",
            "blog\\site",
            "a\u{7}b",
            &"x".repeat(65),
        ] {
            assert!(
                validated_name(refused).is_err(),
                "{refused:?} should not be a project name"
            );
        }
    }

    /// **D6.** An absent map leaves the pins alone; an empty one clears them.
    #[tokio::test]
    async fn updating_pins_replaces_them_and_an_empty_map_clears_them() {
        let (_home, store) = store().await;
        let (_root, blog) = tree(&["blog"]);
        let mut asked = registration("blog", &blog);
        asked.pins = pins(&[(RuntimeKind::Php, "8.3"), (RuntimeKind::Node, "22")]);
        let written = create(&store, &asked, NOW).await.expect("a project");

        let renamed = update(
            &store,
            written.id,
            &Change {
                name: Some("weblog".to_owned()),
                root: None,
                pins: None,
                keep_warm: None,
            },
        )
        .await
        .expect("a rename");
        assert_eq!(renamed.name, "weblog");
        assert_eq!(renamed.pins, asked.pins, "an absent map changed nothing");

        let replaced = update(
            &store,
            written.id,
            &Change {
                name: None,
                root: None,
                pins: Some(pins(&[(RuntimeKind::Php, "^8.4")])),
                keep_warm: None,
            },
        )
        .await
        .expect("a replacement");
        assert_eq!(
            replaced.pins,
            pins(&[(RuntimeKind::Php, "^8.4")]),
            "replacing is not merging: node is gone"
        );

        let cleared = update(
            &store,
            written.id,
            &Change {
                name: None,
                root: None,
                pins: Some(BTreeMap::new()),
                keep_warm: None,
            },
        )
        .await
        .expect("a clearing");
        assert!(cleared.pins.is_empty());
    }

    /// A row written by a build that manages a fifth language must not stop this one reading the
    /// project — `resolve`'s own rule, kept where the reading moved to.
    #[tokio::test]
    async fn a_pin_naming_a_language_this_build_does_not_manage_is_ignored_rather_than_fatal() {
        let (_home, store) = store().await;
        let (_root, blog) = tree(&["blog"]);
        let root = in_full(&blog).display().to_string();

        sqlx::query(
            "INSERT INTO projects (name, root_path, runtime_pins_json, created_at)
             VALUES ('blog', ?, '{\"php\": \"8.3\", \"go\": \"1.22\"}', '2026-08-22T06:55:12Z')",
        )
        .bind(&root)
        .execute(store.pool())
        .await
        .expect("a project");

        let found = find(&store, &ProjectRef::Name("blog".to_owned()))
            .await
            .expect("a lookup")
            .expect("the project");

        assert_eq!(found.pins, pins(&[(RuntimeKind::Php, "8.3")]));
    }

    /// A reference matching nothing is nothing, not an error — the daemon is what turns it into
    /// `not_found`, because only it knows which of the two words a person typed.
    #[tokio::test]
    async fn a_reference_that_matches_nothing_answers_nothing() {
        let (_home, store) = store().await;

        assert!(
            find(&store, &ProjectRef::Name("blog".to_owned()))
                .await
                .expect("a lookup")
                .is_none()
        );
    }
}
