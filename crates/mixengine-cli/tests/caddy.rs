//! The Caddy recipe against a **real** Caddy — roadmap task **T31**, driven through T37's harness.
//!
//! Everything else about this recipe is provable in one process and is proved there: the template
//! renders, the settings merge, the spec builds, the registry hands a changed rendering to a running
//! service. None of that says the thing the task is actually about, which is that *Caddy accepts
//! what MixEngine generates and does what MixEngine asks it to*. That claim can only be made against
//! the program, so this suite is made against the program.
//!
//! **It is `#[ignore]`d rather than skipped**, and the difference is the point. A test that quietly
//! returns when it cannot find a Caddy is a green suite that proved nothing on the day the download
//! broke; an ignored one is *visibly* not run, and the `caddy` step in `.github/workflows/ci.yml`
//! fetches a real archive on all three systems and runs it. Without one, everything here panics
//! saying so.
//!
//! **The arc itself lives in [`harness::frontend`]**, and this file is what makes it Caddy's: where
//! the archive is, what its overrides are called, and which line of a Caddyfile carries the admin
//! port. `nginx.rs` is the same file for the other front end, and the two running the same sequence
//! is what T37 means by a parity suite. What is Caddy's alone — the admin endpoint answering for
//! both readiness and health — is asserted here.

mod harness;

use harness::frontend::{self, Archive, FrontEnd};

/// Caddy, as this suite has to know it.
const CADDY: FrontEnd = FrontEnd {
    package: "caddy",
    // Where an unpacked Caddy is, as the CI step and a developer both set it: the directory holding
    // the binary, since `mixengine-packages` publishes Caddy as one executable with nothing around
    // it. That is also what a `packages` row's `install_path` is.
    variable: "MIXENGINE_CADDY_PACKAGE",
    version: "2.x",
    config: "Caddyfile",
    archive: Archive::OneProgram,
    // A Caddyfile includes nothing out of its own archive.
    data_files: &[],
    alone: |admin| overrides(admin, None),
    serving: |admin, port, says| {
        overrides(
            admin,
            Some(format!(
                "http://127.0.0.1:{port} {{\n\trespond \"{says}\"\n}}\n"
            )),
        )
    },
    broken: |admin| overrides(admin, Some("this is not a Caddyfile {".to_owned())),
    control_line: |admin| format!("admin 127.0.0.1:{admin}"),
    // Caddy's own admin endpoint: `GET /config/` answers `200` with the running configuration, which
    // is a stronger statement than a TCP accept and is what the recipe's readiness check asks.
    control_path: "/config/",
};

/// **A free TLS port, and not the 443 the preset carries** — roadmap task T51.
///
/// From T51 a front end actually binds `https_port`, because a site with a certificate renders a TLS
/// listener. These suites run a real server as an unprivileged user, where 443 is refused — and both
/// servers reject the *whole* configuration over one listener they cannot bind, so the failure is
/// not "no HTTPS" but "the reload was refused and the old configuration is still running". The HTTP
/// port was already a free one for the same reason; this is its other half.
fn free_tls_port() -> u16 {
    frontend::free_port()
}
/// The whole overrides document for a Caddy on `admin`, with `extra` pasted in if there is any.
///
/// **The whole document and not a patch**, which is what `config_overrides_json` is: a setting that
/// is not in it is not set. So every override this suite writes repeats the admin port, and one that
/// forgot would move the endpoint back to Caddy's default under a server listening on the one this
/// home chose — a reload and a stop sent to an address nothing answers on.
fn overrides(admin: u16, extra: Option<String>) -> String {
    serde_json::json!({
        "admin_port": admin,
        "https_port": free_tls_port(),
        "extra": extra.unwrap_or_default(),
    })
    .to_string()
}

/// **The whole of T31, in the order a user meets it.**
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real Caddy — see the module note, and the `caddy` step in ci.yml"]
async fn caddy_is_generated_validated_started_reloaded_and_stopped() {
    frontend::is_generated_validated_started_reloaded_and_stopped(&CADDY).await;
}

/// **Caddy accepts a rendering with TLS in it** — roadmap task **T51**.
///
/// The one assertion no unit test can make. The first draft of this task rendered a single site
/// block naming both schemes with a `tls` inside it: every unit test would have passed it, and Caddy
/// refuses it outright — `server listening on [:80] is HTTP, but attempts to configure TLS
/// connection policies`. What is proved here is that the shape the templates settled on is a shape
/// the program will load.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real Caddy — see the module note, and the `caddy` step in ci.yml"]
async fn caddy_accepts_a_site_served_over_tls() {
    let (home, _daemon, _registry, _site_port, _control) = frontend::declared(&CADDY).await;

    let repository = tempfile::Builder::new()
        .prefix("mixengine-t51")
        .tempdir()
        .expect("a temporary directory");
    let root = repository.path().display().to_string();

    home.mix(&["project", "create", &root, "--name", "blog"]);
    home.mix_in(
        repository.path(),
        &[],
        &[
            "site",
            "create",
            "--domain",
            "blog.test",
            "--kind",
            "static",
        ],
    );

    // The daemon's own producer (T50) signed the certificate as the site was created, and the walk
    // that followed rendered and installed the configuration. Nothing here asks for either.
    let site_file = home
        .path()
        .join("etc")
        .join(CADDY.package)
        .join("sites")
        .join("blog.test.caddy");

    let rendered = std::fs::read_to_string(&site_file).unwrap_or_else(|error| {
        panic!(
            "no site file at {}: {error}\n{}",
            site_file.display(),
            home.daemon_log()
        )
    });

    assert!(rendered.contains("https://blog.test"), "{rendered}");
    assert!(rendered.contains("tls "), "{rendered}");

    // **Both addresses**, the T51 design's D9: a client pointed at plaintext keeps working.
    assert!(rendered.contains("http://blog.test"), "{rendered}");

    // **And the global block still refuses to obtain one of its own** — D1. A later change to the
    // preset would re-enable a public certificate request for a name resolving nowhere, and this is
    // where that gets caught.
    let global = std::fs::read_to_string(
        home.path()
            .join("etc")
            .join(CADDY.package)
            .join("Caddyfile"),
    )
    .expect("the Caddyfile");
    assert!(global.contains("auto_https off"), "{global}");

    // The strongest assertion here is the implicit one: the file above exists, which means
    // `document::install` staged this rendering, ran `caddy validate` over it and only then
    // installed it. A rendering Caddy refuses never reaches that path — this test would have failed
    // at `read_to_string`, with the validator's own words in `daemon.log`.
}

/// **The first assertion in this repository that measures a green padlock** — roadmap task **T53**.
///
/// Everything phase 5 asserts elsewhere is about a file: that a certificate was written, that a
/// `tls` line names it, that the rendering validates. None of that establishes that the running
/// server presents it to anything. This starts a real Caddy over the site MixEngine generated and
/// then asks MixEngine itself what that server hands a client — which is the only thing a browser
/// ever sees.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real Caddy — see the module note, and the `caddy` step in ci.yml"]
async fn cert_status_measures_a_trusted_handshake_against_a_running_caddy() {
    let (home, _daemon, _registry, _site_port, _control) = frontend::declared(&CADDY).await;

    let repository = tempfile::Builder::new()
        .prefix("mixengine-t53")
        .tempdir()
        .expect("a temporary directory");
    let root = repository.path().display().to_string();

    home.mix(&["project", "create", &root, "--name", "blog"]);
    home.mix_in(
        repository.path(),
        &[],
        &[
            "site",
            "create",
            "--domain",
            "blog.test",
            "--kind",
            "static",
        ],
    );

    let started = harness::json(&home.mix(&["service", "start", CADDY.package, "--json"]));
    assert_eq!(
        started["complete"],
        true,
        "{started}\n{}",
        home.daemon_log()
    );

    let answer = harness::json(&home.mix(&["cert", "status", "--json"]));
    let site = &answer["sites"][0];

    assert_eq!(
        site["handshake"]["handshake"],
        "presented",
        "{answer}\n{}",
        home.daemon_log()
    );
    assert_eq!(
        site["handshake"]["trust"]["trust"],
        "trusted",
        "{answer}\n{}",
        home.daemon_log()
    );
    assert_eq!(site["problem"], serde_json::Value::Null, "{answer}");
}

/// **A server still holding the certificate that was replaced under it** — roadmap task **T53**,
/// and the report `.claude/features/tls.md` says most "the padlock is broken" messages really are.
///
/// Everything that reads files calls this machine healthy: the certificate is present, it covers
/// the right names, it has eighty days left and `mix doctor` is green. Only the handshake sees it.
///
/// **Reissued through `mix` rather than written by this test**, which is what keeps it honest about
/// the mechanism: `cert.issue` writes a certificate and tells nothing, so the running server goes
/// on holding the one it loaded at start.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real Caddy — see the module note, and the `caddy` step in ci.yml"]
async fn cert_status_notices_a_server_holding_the_previous_certificate() {
    let (home, _daemon, _registry, _site_port, _control) = frontend::declared(&CADDY).await;

    let repository = tempfile::Builder::new()
        .prefix("mixengine-t53-stale")
        .tempdir()
        .expect("a temporary directory");
    let root = repository.path().display().to_string();

    home.mix(&["project", "create", &root, "--name", "blog"]);
    home.mix_in(
        repository.path(),
        &[],
        &[
            "site",
            "create",
            "--domain",
            "blog.test",
            "--kind",
            "static",
        ],
    );

    let started = harness::json(&home.mix(&["service", "start", CADDY.package, "--json"]));
    assert_eq!(
        started["complete"],
        true,
        "{started}\n{}",
        home.daemon_log()
    );

    // Removed and then reissued, so what lands on disk is a *different* certificate rather than the
    // same one again — `cert.issue` reuses anything still usable, which is T50's whole guarantee.
    let sites = home.path().join("certs").join("sites");
    std::fs::remove_file(sites.join("blog.test.crt")).expect("the certificate is removed");
    std::fs::remove_file(sites.join("blog.test.key")).expect("the key is removed");

    let reissued = harness::json(&home.mix(&["cert", "issue", "--site", "blog.test", "--json"]));
    assert_eq!(
        reissued["sites"][0]["outcome"]["outcome"], "issued",
        "{reissued}"
    );

    let answer = harness::json(&home.mix(&["cert", "status", "--json"]));
    let site = &answer["sites"][0];

    assert_eq!(
        site["handshake"]["handshake"],
        "presented",
        "{answer}\n{}",
        home.daemon_log()
    );
    assert_eq!(
        site["problem"],
        "served_certificate_differs",
        "{answer}\n{}",
        home.daemon_log()
    );
}

/// **The acceptance criterion, measured** — *"`mix cert ca-rotate` completes with all sites still
/// trusted afterwards"*, from `.claude/features/tls.md`. Roadmap task **T54**.
///
/// Every other assertion T54 makes is about a file, a queue or a probe. This is the only one that
/// asks the running server what it presents *after* a rotation, which is the only thing a browser
/// ever sees — and the only way to notice a front end still holding the chain that was replaced
/// underneath it.
///
/// **Gated on `MIXENGINE_SYSTEM_TESTS=1` as well as `#[ignore]`d**, unlike everything else in this
/// file, and rule 1 of `.claude/standards/testing.md` is why: a rotation writes this *machine's*
/// trust store, where the tests above only ever start a server. The `caddy` step in
/// `.github/workflows/ci.yml` runs this suite with `--ignored`, so without the second gate this
/// would install and remove a certificate authority on every macOS and Windows runner — and on
/// Windows it can raise a dialog, which a CI job has nobody to answer. That is not hypothetical: an
/// earlier draft of the T54 suite raised a real UAC prompt in the middle of `cargo test`.
///
/// **The job that does set it is `system`, on Windows and macOS**, and that is the whole of where
/// this test runs. Both hold a token that can grant — Windows a full administrator one, macOS by
/// running the suite as root — so the rotation below is a real one and `outcome == "rotated"` is an
/// assertion with something behind it. A Linux runner has no polkit agent, so a rotation there is
/// refused rather than granted and this test could only ever fail on it; what Linux answers instead
/// is the refusal, in `tests/cert.rs`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real Caddy and writes this machine's trust store — set MIXENGINE_SYSTEM_TESTS=1"]
async fn a_rotated_authority_still_gives_every_site_a_green_padlock() {
    if std::env::var("MIXENGINE_SYSTEM_TESTS").as_deref() != Ok("1") {
        return;
    }

    let (home, _daemon, _registry, _site_port, _control) = frontend::declared(&CADDY).await;

    let repository = tempfile::Builder::new()
        .prefix("mixengine-t54")
        .tempdir()
        .expect("a temporary directory");
    let root = repository.path().display().to_string();

    home.mix(&["project", "create", &root, "--name", "blog"]);
    home.mix_in(
        repository.path(),
        &[],
        &[
            "site",
            "create",
            "--domain",
            "blog.test",
            "--kind",
            "static",
        ],
    );

    let started = harness::json(&home.mix(&["service", "start", CADDY.package, "--json"]));
    assert_eq!(
        started["complete"],
        true,
        "{started}\n{}",
        home.daemon_log()
    );

    let was = harness::json(&home.mix(&["cert", "ca-status", "--json"]));
    let rotated = harness::json(&home.mix(&["cert", "ca-rotate", "--yes", "--json"]));

    assert_eq!(
        rotated["outcome"],
        "rotated",
        "{rotated}\n{}",
        home.daemon_log()
    );
    assert_ne!(
        was["ca"]["key_id"], rotated["status"]["ca"]["key_id"],
        "a rotation over the same key would not be one: {rotated}"
    );

    let answer = harness::json(&home.mix(&["cert", "status", "--json"]));
    let site = &answer["sites"][0];

    // The front end was told, so it is serving the leaf signed by the *new* authority — which is
    // T51's fingerprint-in-header doing its job and nothing T54 added.
    assert_eq!(
        site["handshake"]["trust"]["trust"],
        "trusted",
        "the padlock is not green after a rotation: {answer}\n{}",
        home.daemon_log()
    );
    assert_eq!(
        site["problem"],
        serde_json::Value::Null,
        "{answer}\n{}",
        home.daemon_log()
    );

    // And the machine is left as it was found: this suite installed an authority, so it takes it
    // back out rather than leaving one in the trust store of whoever ran it.
    home.mix(&["cert", "ca-uninstall", "--yes", "--json"]);
}
