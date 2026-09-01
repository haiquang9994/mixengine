//! `mix blueprint capture` and `mix blueprint apply` against a real daemon — roadmap task **T78**.
//!
//! What is proved here is what no unit test can: that a project captured on this machine can be
//! applied under another name, that applying it twice is applying it once, and that a capture of the
//! applied project is the blueprint it came from.
//!
//! **Offline by construction.** The fixture is a static site with no runtime and no services, so
//! every install step plans as `Satisfied` and nothing reaches the package index. What that costs is
//! coverage of the install path, which is `tests/runtime.rs`' and `tests/package.rs`' already; what
//! it buys is a suite that says the same thing on a laptop and on a runner with no network.

mod harness;

use harness::{Home, json, stdout};

/// A directory to register.
fn repository() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("mixengine-blueprint")
        .tempdir()
        .expect("a temporary directory")
}

/// A project with one static site, which is the smallest thing worth capturing.
fn a_project_with_a_site(home: &Home, directory: &std::path::Path, name: &str, domain: &str) {
    let root = directory.display().to_string();

    home.mix(&["project", "create", &root, "--name", name]);
    home.mix_in(
        directory,
        &[],
        &["site", "create", "--domain", domain, "--kind", "static"],
    );
}

/// The rendered blueprint, minus the `[blueprint]` block.
///
/// The header is what a second capture is *expected* to differ in — its name, its description and
/// the moment it was taken — and everything after it is what has to be the same.
fn body(rendered: &str) -> String {
    rendered
        .split("\n[")
        .skip(1)
        .filter(|block| !block.starts_with("blueprint]"))
        .map(|block| format!("[{block}"))
        .collect()
}

/// The feature's own acceptance criterion: capture a working project, apply it under a new name,
/// and both are there afterwards with the names they should have.
#[tokio::test(flavor = "multi_thread")]
async fn a_captured_project_is_applied_under_a_new_name() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let first = repository();
    a_project_with_a_site(&home, first.path(), "blog", "blog.test");

    let captured = json(&home.mix(&[
        "blueprint",
        "capture",
        "blog-stack",
        "--project",
        "blog",
        "--json",
    ]));
    assert_eq!(captured["slug"], "blog-stack", "{captured}");

    let second = repository();
    let into = second.path().join("shop").display().to_string();

    let applied = stdout(&home.mix(&[
        "blueprint",
        "apply",
        "blog-stack",
        "--project",
        "shop",
        "--path",
        &into,
    ]));
    assert!(applied.contains("shop"), "{applied}");

    // The project is registered, and at the directory the apply was told to use — which it had to
    // create, because `project.create` takes a root that exists and an apply's does not yet.
    let shown = json(&home.mix(&["project", "show", "shop", "--json"]));
    assert_eq!(shown["project"]["name"], "shop", "{shown}");

    // **`{project}` was expanded**, which is the whole of what a blueprint is for: the captured
    // domain was `blog.test`, tokenised to `{project}.test`, and it comes back as the new name.
    let sites = json(&home.mix(&["site", "list", "--json"]));
    let listed = sites["sites"].as_array().expect("a list of sites");
    assert!(
        listed.iter().any(|site| site["domain"] == "shop.test"),
        "{sites}"
    );

    // And the first site is untouched: applying a blueprint makes a project, it does not move one.
    assert!(
        listed.iter().any(|site| site["domain"] == "blog.test"),
        "{sites}"
    );
}

/// **The proof of D2 and D3 at once.** A second apply finds nothing left to do, which is what makes
/// a failed apply resumable rather than restartable — and it is asserted on the *steps*, because
/// "it did not fail" would also be true of an apply that did everything twice.
#[tokio::test(flavor = "multi_thread")]
async fn applying_the_same_blueprint_twice_leaves_nothing_to_do_the_second_time() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let first = repository();
    a_project_with_a_site(&home, first.path(), "blog", "blog.test");
    home.mix(&["blueprint", "capture", "blog-stack", "--project", "blog"]);

    let second = repository();
    let into = second.path().join("shop").display().to_string();
    let apply = [
        "blueprint",
        "apply",
        "blog-stack",
        "--project",
        "shop",
        "--path",
        &into,
        "--json",
    ];

    home.mix(&apply);

    // **Asserted on the results and not on the dispositions.** A plan reads this home's tables and
    // a certificate is a file, so the certificate step is planned as work either way — and what
    // matters is that carrying it out found there was none. Every step reporting `already true` is
    // the whole claim.
    let again = stdout(&home.mix(&apply));
    assert!(
        !again.contains("\"result\":\"done\""),
        "a second apply did work the first one should have done: {again}"
    );
    assert!(
        again.contains("\"result\":\"already_true\""),
        "a second apply reported nothing at all: {again}"
    );
}

/// **The round trip**, which is what catches D7 and D14 cheaply: a capture of the applied project is
/// the blueprint that made it, header aside. T77 made the renderer byte-identical on purpose, and
/// this is the assertion that spends it.
#[tokio::test(flavor = "multi_thread")]
async fn a_capture_of_an_applied_project_is_the_blueprint_it_came_from() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let first = repository();
    a_project_with_a_site(&home, first.path(), "blog", "blog.test");

    let captured = json(&home.mix(&[
        "blueprint",
        "capture",
        "blog-stack",
        "--project",
        "blog",
        "--json",
    ]));

    let second = repository();
    let into = second.path().join("shop").display().to_string();
    home.mix(&[
        "blueprint",
        "apply",
        "blog-stack",
        "--project",
        "shop",
        "--path",
        &into,
    ]);

    let round_trip = json(&home.mix(&[
        "blueprint",
        "capture",
        "shop-stack",
        "--project",
        "shop",
        "--json",
    ]));

    let before = std::fs::read_to_string(captured["file"].as_str().expect("a path"))
        .expect("the blueprint that was captured");
    let after = std::fs::read_to_string(round_trip["file"].as_str().expect("a path"))
        .expect("the blueprint the applied project makes");

    assert_eq!(
        body(&before),
        body(&after),
        "applying a blueprint and capturing the result gave a different blueprint"
    );
}

/// **The gallery is applied, not just listed** — roadmap task **T79**. `static` is the one that
/// needs no runtime and no service, so this stays offline exactly as this suite's own fixture does.
///
/// **What is asserted about the certificate step is that it did not fail.** `https = true` puts one
/// in this plan and an apply really runs it; a certificate that could not be issued comes back
/// `NotRun` with a reason, on `site.create`'s standing position that a site is worth more than a
/// certificate. Pinning an outcome here would be pinning whether the machine running the suite has
/// an authority.
#[tokio::test(flavor = "multi_thread")]
async fn the_static_blueprint_from_the_gallery_applies() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let directory = repository();
    let into = directory.path().join("shop").display().to_string();

    // `stdout` rather than `json`: an apply prints three documents — the plan, the job as it runs,
    // and the result — which is what this suite's own first test reads too.
    let applied = stdout(&home.mix(&[
        "blueprint",
        "apply",
        "static",
        "--project",
        "shop",
        "--path",
        &into,
        "--json",
    ]));

    assert!(!applied.contains("\"failed\""), "a step failed: {applied}");

    let shown = json(&home.mix(&["project", "show", "shop", "--json"]));
    assert_eq!(shown["project"]["name"], "shop", "{shown}");

    let sites = json(&home.mix(&["site", "list", "--json"]));
    let listed = sites["sites"].as_array().expect("a list of sites");
    assert!(
        listed.iter().any(|site| site["domain"] == "shop.test"),
        "{sites}"
    );
}

/// A gallery blueprint says where it came from, and that this build vouches for it.
#[tokio::test(flavor = "multi_thread")]
async fn a_gallery_blueprint_is_listed_as_this_builds_own() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let listed = json(&home.mix(&["blueprint", "list", "--json"]));
    let found = listed["blueprints"]
        .as_array()
        .expect("a listing")
        .iter()
        .find(|one| one["slug"] == "laravel")
        .unwrap_or_else(|| panic!("the gallery is not listed: {listed}"));

    assert_eq!(found["source"], "builtin", "{listed}");
    assert_eq!(found["trusted"], true, "{listed}");
}
