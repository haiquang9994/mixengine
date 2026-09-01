//! The blueprints this build ships — roadmap task **T79**.
//!
//! Out here rather than beside the code because these are assertions about the *shipped set*: that
//! the six files are readable, that each one is its own rendering, and that seeding a real home
//! with them is idempotent.

use mixengine_core::blueprints::gallery::ENTRIES;
use mixengine_core::blueprints::manifest;

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
