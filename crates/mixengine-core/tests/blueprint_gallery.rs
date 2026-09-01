//! The blueprints this build ships — roadmap task **T79**.
//!
//! Out here rather than beside the code because these are assertions about the *shipped set*: that
//! the six files are readable, that each one is its own rendering, and that seeding a real home
//! with them is idempotent.

use mixengine_core::blueprints::gallery::{self, ENTRIES};
use mixengine_core::blueprints::manifest;
use mixengine_core::blueprints::plan::plan;
use mixengine_core::blueprints::store as blueprint_store;
use mixengine_core::blueprints::store::Filed;
use mixengine_core::blueprints::trust::Trust;
use mixengine_core::{Paths, Store, open_home};
use mixengine_proto::{BlueprintSource, Disposition, PlanAction, RuntimeKind};
use tempfile::TempDir;

/// **Every gallery file is exactly what the renderer would write** — the T79 design, D2. Without
/// this the file in this repository, the `manifest_toml` column and the file in a user's home are
/// three different texts for one blueprint, and a `diff` between any two of them means nothing.
#[test]
fn every_gallery_blueprint_is_its_own_rendering() {
    for entry in ENTRIES {
        let manifest = manifest::read(entry.manifest)
            .unwrap_or_else(|error| panic!("{} does not parse: {error}", entry.slug));

        assert_eq!(
            manifest::render(&manifest),
            entry.manifest,
            "{} is not canonical — replace the file with what `render` returns",
            entry.slug
        );
    }
}

/// The set the roadmap names, spelled the way a person types it on a command line.
#[test]
fn the_gallery_is_the_six_the_roadmap_names() {
    let slugs: Vec<_> = ENTRIES.iter().map(|entry| entry.slug).collect();

    assert_eq!(
        slugs,
        [
            "django",
            "laravel",
            "nextjs",
            "static",
            "symfony",
            "wordpress"
        ],
        "the gallery is listed in slug order, which is the order a listing shows it in"
    );

    for entry in ENTRIES {
        mixengine_core::blueprints::store::validated_slug(entry.slug)
            .unwrap_or_else(|error| panic!("{} cannot be a filename: {error}", entry.slug));
    }
}

/// **Three carry a command and three do not** — D8. Asserted rather than left to a reading of the
/// files, because a scaffold added to `wordpress` or `django` by a later edit is exactly the change
/// this task decided against.
#[test]
fn only_the_three_that_can_run_a_command_carry_one() {
    for entry in ENTRIES {
        let manifest = manifest::read(entry.manifest).expect("a gallery blueprint");
        let expected = matches!(entry.slug, "laravel" | "symfony" | "nextjs");

        assert_eq!(
            manifest.scaffold.is_some(),
            expected,
            "{} carries the wrong answer about a scaffold command",
            entry.slug
        );
    }
}

/// An opened home with its database, both in a directory the test owns — `tests/store.rs`' helper.
async fn home() -> (TempDir, Paths, Store) {
    let temp = TempDir::new().expect("a temporary directory");
    let opened = open_home(
        None,
        &mixengine_platform::mock::Host::with_home(temp.path().join("MixEngine")),
    )
    .expect("a home");
    let store = Store::open(opened.paths.database_file())
        .await
        .expect("a database");

    (temp, opened.paths, store)
}

/// **A home nobody has touched holds the gallery, trusted** — the T79 design, D1 and D3.
#[tokio::test]
async fn a_fresh_home_is_seeded_with_the_whole_gallery() {
    let (_temp, paths, store) = home().await;

    let seeded = gallery::seed(&store, &paths).await.expect("a seeded home");
    assert_eq!(seeded.written.len(), ENTRIES.len(), "{seeded:?}");

    let listed = blueprint_store::records(&store, &paths)
        .await
        .expect("a listing");
    assert_eq!(listed.len(), ENTRIES.len());

    for summary in &listed {
        assert_eq!(summary.source, BlueprintSource::Builtin, "{summary:?}");
        assert!(summary.trusted, "{summary:?}");
        assert!(
            std::path::Path::new(&summary.file).exists(),
            "the rendering is missing: {summary:?}"
        );
    }
}

/// **The second start writes nothing at all** — D4. This is the assertion the decision exists for:
/// every CLI test in this workspace starts a daemon, and six file writes on each of those is a cost
/// with nothing on the other side of it.
#[tokio::test]
async fn seeding_a_home_that_is_already_seeded_writes_nothing() {
    let (_temp, paths, store) = home().await;

    gallery::seed(&store, &paths).await.expect("a seeded home");
    let again = gallery::seed(&store, &paths).await.expect("a second seed");

    assert!(again.written.is_empty(), "it wrote rows again: {again:?}");
    assert!(again.rendered.is_empty(), "it wrote files again: {again:?}");
    assert_eq!(again.left.len(), ENTRIES.len(), "{again:?}");
}

/// **A row somebody else owns is never touched** — D6. Capturing over `laravel` takes `--overwrite`
/// and makes the row this machine's own; no upgrade takes that slug back.
#[tokio::test]
async fn a_captured_row_survives_a_seed() {
    let (_temp, paths, store) = home().await;
    let mine = manifest::read(
        r#"schema = 1

[blueprint]
name = "Mine"
created_at = "2026-09-01T00:00:00Z"

[blueprint.created_on]
os = "windows"
version = "0.1.0"
"#,
    )
    .expect("a manifest of my own");

    blueprint_store::save(
        &store,
        &paths,
        &mine,
        "laravel",
        BlueprintSource::Captured,
        Trust::Inherent,
        false,
    )
    .await
    .expect("a capture under the gallery's slug");

    gallery::seed(&store, &paths).await.expect("a seed");

    let filed = blueprint_store::filed_of(&store, "laravel")
        .await
        .expect("the row");
    assert_eq!(filed.source, BlueprintSource::Captured);
    assert_eq!(filed.manifest.blueprint.name, "Mine");
}

/// **A builtin row that drifted is put back** — D5's repair property: a home whose gallery was
/// edited or emptied is mended by starting the daemon, exactly as `bin/` is.
#[tokio::test]
async fn a_builtin_row_that_was_edited_is_restored() {
    let (_temp, paths, store) = home().await;
    gallery::seed(&store, &paths).await.expect("a seed");

    sqlx::query("UPDATE blueprints SET manifest_toml = 'schema = 1' WHERE id = 'static'")
        .execute(store.pool())
        .await
        .expect("an edited row");

    let again = gallery::seed(&store, &paths).await.expect("a second seed");
    assert_eq!(again.written, vec!["static".to_owned()], "{again:?}");

    let filed = blueprint_store::filed_of(&store, "static")
        .await
        .expect("the row");
    assert_eq!(filed.manifest.blueprint.name, "Static site");
}

/// A rendering deleted from `blueprints/` comes back without the row being rewritten.
#[tokio::test]
async fn a_deleted_rendering_is_written_again() {
    let (_temp, paths, store) = home().await;
    gallery::seed(&store, &paths).await.expect("a seed");

    std::fs::remove_file(blueprint_store::file(&paths, "django")).expect("the rendering");

    let again = gallery::seed(&store, &paths).await.expect("a second seed");
    assert!(again.written.is_empty(), "the row was rewritten: {again:?}");
    assert_eq!(again.rendered, vec!["django".to_owned()], "{again:?}");
}

/// What each gallery blueprint plans on a machine holding nothing at all — which is the ordinary
/// case for one, since a person applying `laravel` has very often never installed PHP.
async fn planned(slug: &str) -> mixengine_proto::BlueprintPlan {
    let (_temp, _paths, store) = home().await;
    let entry = ENTRIES
        .iter()
        .find(|entry| entry.slug == slug)
        .expect("a gallery blueprint");

    let filed = Filed {
        manifest: manifest::read(entry.manifest).expect("a manifest"),
        source: BlueprintSource::Builtin,
        trusted: true,
        signature: None,
    };

    plan(
        &store,
        slug,
        &filed,
        "shop",
        std::path::Path::new("/projects/shop"),
        &[],
    )
    .await
    .expect("a plan")
}

/// **Every one of the six plans without a blocked step** — nothing in the gallery asks for
/// something this build cannot do on a machine that has nothing installed.
#[tokio::test]
async fn every_gallery_blueprint_plans_on_a_machine_with_nothing_installed() {
    for entry in ENTRIES {
        let planned = planned(entry.slug).await;

        assert!(
            !planned.steps.is_empty(),
            "{} planned nothing at all",
            entry.slug
        );
        assert!(
            !planned
                .steps
                .iter()
                .any(|step| matches!(step.disposition, Disposition::Blocked { .. })),
            "{} plans a step this build cannot carry out: {:?}",
            entry.slug,
            planned.steps
        );
    }
}

/// **`{project}` is expanded everywhere it appears, and nowhere is it left as a token** — the T78a
/// D6 property, held for the shipped set: a gallery blueprint that planned the literal `{project}`
/// would create a database called `{project}` on somebody's machine.
#[tokio::test]
async fn no_gallery_plan_carries_an_unexpanded_token() {
    for entry in ENTRIES {
        let planned = planned(entry.slug).await;

        assert!(
            !format!("{:?}", planned.steps).contains("{project}"),
            "{} left a token in its plan: {:?}",
            entry.slug,
            planned.steps
        );
    }
}

/// The headline blueprint, step by step: a machine with nothing on it installs two languages, two
/// servers, makes a database, a site, a name and a certificate, turns an extension on, and offers
/// the command.
#[tokio::test]
async fn laravel_plans_the_whole_stack() {
    let planned = planned("laravel").await;
    let has =
        |wanted: fn(&PlanAction) -> bool| planned.steps.iter().any(|step| wanted(&step.action));

    assert!(has(|action| matches!(
        action,
        PlanAction::RegisterProject { .. }
    )));
    assert!(has(|action| matches!(
        action,
        PlanAction::InstallRuntime { kind, .. } if *kind == RuntimeKind::Php
    )));
    assert!(has(|action| matches!(
        action,
        PlanAction::InstallRuntime { kind, .. } if *kind == RuntimeKind::Node
    )));
    assert!(has(
        |action| matches!(action, PlanAction::CreateDatabase { database, .. } if database == "shop")
    ));
    assert!(has(|action| matches!(
        action,
        PlanAction::CreateSite { .. }
    )));
    assert!(has(
        |action| matches!(action, PlanAction::AddDomain { domain, .. } if domain == "shop.test")
    ));
    assert!(has(|action| matches!(
        action,
        PlanAction::IssueCertificate { .. }
    )));
    assert!(has(
        |action| matches!(action, PlanAction::SetPhpExtension { name, .. } if name == "redis")
    ));
    assert!(has(|action| matches!(
        action,
        PlanAction::RunScaffold { .. }
    )));
}

/// **The three without a command plan no command** — D8, asserted on the plan rather than on the
/// file, because the plan is what an apply carries out.
#[tokio::test]
async fn the_blueprints_with_no_command_plan_no_command() {
    for slug in ["wordpress", "django", "static"] {
        let planned = planned(slug).await;

        assert!(
            !planned
                .steps
                .iter()
                .any(|step| matches!(step.action, PlanAction::RunScaffold { .. })),
            "{slug} planned a command it does not carry"
        );
    }
}

/// **The display names are deliberately not slugs** — roadmap task **T79a**, its design's D9. This
/// is what makes the daemon's hand-import test mean something: if somebody renamed `Static site` to
/// `static`, that test would keep passing for a reason that had stopped being true, and the fallback
/// it guards would be untested again.
#[test]
fn the_gallery_display_names_are_not_slugs() {
    let names: Vec<String> = ENTRIES
        .iter()
        .map(|entry| {
            manifest::read(entry.manifest)
                .expect("a gallery blueprint")
                .blueprint
                .name
        })
        .collect();

    assert!(
        names.iter().any(|name| name.contains('.')),
        "no gallery name carries a dot any more: {names:?}"
    );
    assert!(
        names.iter().any(|name| name.contains(' ')),
        "no gallery name carries a space any more: {names:?}"
    );

    for name in &names {
        assert!(
            blueprint_store::validated_slug(name).is_err(),
            "{name} is spelled as a slug, which is not what `[blueprint] name` is for"
        );
    }
}
