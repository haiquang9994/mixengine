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

use std::time::{Duration, Instant};

use harness::frontend::{self, CADDY};

/// **The whole of T31, in the order a user meets it.**
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real Caddy — see the module note, and the `caddy` step in ci.yml"]
async fn caddy_is_generated_validated_started_reloaded_and_stopped() {
    frontend::is_generated_validated_started_reloaded_and_stopped(&CADDY).await;
}

/// **Caddy judges and then serves an extension's front-end fragment** — roadmap task **T81c**.
///
/// The claim no unit test can make: a fragment is arbitrary Caddyfile text, so the only thing that
/// can say whether one is a configuration is Caddy. The sequence is [`frontend`]'s, driven twice for
/// T37's reason.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real Caddy — see the module note, and the `caddy` step in ci.yml"]
async fn caddy_serves_what_an_extension_s_fragment_adds() {
    frontend::serves_what_an_extension_s_fragment_adds(&CADDY).await;
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

    // **And it installs no authority of its own — asked of Caddy rather than of the file** (T76).
    //
    // The template says `skip_install_trust`, and for one release of this branch that was the whole
    // fix and it did nothing: the Caddyfile adapter applies the option to the certificate
    // authorities a configuration names, names none on its own, and quietly produced JSON with no
    // `pki` app in it — so the CA Caddy provisioned at run time was the implicit one, which
    // installs. A CI runner then spent its entire readiness budget inside that install.
    //
    // A `contains("skip_install_trust")` on the rendering passed throughout. What separates the two
    // is Caddy's own reading of the file, so that is what is asked for here — the same adapter the
    // server runs, over the whole installed configuration, imports and all.
    let adapted = std::process::Command::new(CADDY.package_directory().join(CADDY.binary()))
        .args(["adapt", "--config"])
        .arg(
            home.path()
                .join("etc")
                .join(CADDY.package)
                .join("Caddyfile"),
        )
        .args(["--adapter", "caddyfile"])
        .output()
        .expect("a real Caddy can adapt the configuration it was given");

    let json = String::from_utf8_lossy(&adapted.stdout);
    assert!(
        json.contains("\"install_trust\":false"),
        "Caddy reads this configuration as one that installs a certificate authority, so the \
         running server will put a root of its own in this machine's trust store\n\
         --- adapt ---\n{json}\n--- stderr ---\n{}\n--- Caddyfile ---\n{global}",
        String::from_utf8_lossy(&adapted.stderr)
    );

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

/// How long a rotation is given to reach the running front end.
///
/// A reload is asynchronous everywhere in this product — the registry notifies, the service's own
/// task acts — so this is the one number that says how long "afterwards" is in the criterion below.
/// Generous rather than tight: it bounds a failure, and a runner under load must not be reported as
/// a rotation that never arrived.
const SETTLE: Duration = Duration::from_secs(30);

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

    // **Polled and bounded rather than read once**, and what `ca-rotate` promises is why. Its last
    // step is `progress(90, "telling the front end")`: `services::Registry::reconfigure` writes the
    // new rendering and notifies the runner, and the reload itself happens in that runner's own
    // task. So the command returns when the front end has been *told*, and a reading taken the
    // instant it returns measures the race rather than the criterion. What this asserts is the state
    // a rotation leaves behind, and `SETTLE` is how long it is given to arrive — polled and not
    // slept through, in `crates/mixengine-platform/tests/connections.rs`'s shape and for its reason.
    //
    // **Not a precaution: measured.** Read once, this found the server still holding the leaf signed
    // by the authority that had just been replaced, on CI's Windows and macOS legs both, with
    // `problem: served_certificate_differs` naming it exactly.
    let since = Instant::now();

    loop {
        let answer = harness::json(&home.mix(&["cert", "status", "--json"]));
        let site = &answer["sites"][0];

        // The front end was told, so it ends up serving the leaf signed by the *new* authority —
        // which is T51's fingerprint-in-header doing its job and nothing T54 added.
        if site["handshake"]["trust"]["trust"] == "trusted" && site["problem"].is_null() {
            break;
        }

        assert!(
            since.elapsed() < SETTLE,
            "the padlock is not green {SETTLE:?} after a rotation: {answer}\n{}",
            home.daemon_log()
        );

        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    // And the machine is left as it was found: this suite installed an authority, so it takes it
    // back out rather than leaving one in the trust store of whoever ran it.
    home.mix(&["cert", "ca-uninstall", "--yes", "--json"]);
}
