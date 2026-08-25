//! The nginx recipe against a **real** nginx — roadmap task **T37**.
//!
//! [`caddy.rs`](../caddy.rs)'s file for the other front end, and deliberately almost all of it is
//! this page's four constants: the sequence a front end has to walk lives in [`harness::frontend`]
//! and is driven twice, which is what the roadmap means by "a parity test suite running both
//! generators". A copy of the arc written out again here would be two things to keep in step, and
//! the one that drifted would be green while it drifted.
//!
//! **It is `#[ignore]`d rather than skipped**, for Caddy's reason: a test that quietly returns when
//! it cannot find an nginx is a green suite that proved nothing on the day the download broke. The
//! `nginx` step in `.github/workflows/ci.yml` fetches a real archive on all three systems.
//!
//! # What only a real nginx can answer
//!
//! Three of this recipe's decisions are guesses until this suite runs, and each one fails silently
//! in a different way if it is wrong:
//!
//! - **Whether `nginx -t` accepts a generated configuration at all**, with a Windows path in every
//!   directive that names one. That is the question the forward-slashed quoting exists for, and only
//!   nginx's own parser answers it.
//! - **Whether the prefix makes `include sites/*.conf` resolve where the recipe says it does** — in
//!   the staging directory while the rendering is being judged, and in `etc/nginx/` once it is
//!   installed. Get it wrong and validation passes over a directory nothing is in.
//! - **Whether `-s reload` reaches the master this daemon started.** A signal that found no pid file
//!   exits non-zero and is reported; one that found the *wrong* one would be worse, which is why the
//!   pid path is written into the same configuration every invocation is given.
//!
//! # The archive, whole
//!
//! Unlike Caddy, the fixture packs the entire unpacked tree rather than one binary: a generated
//! `nginx.conf` `include`s the archive's own `conf/mime.types` by absolute path, and a package
//! without it is one this recipe refuses while rendering. Packing the whole tree is also what makes
//! the `provides` map the suite publishes the same shape `mixengine-packages` publishes.

mod harness;

use harness::frontend::{self, Archive, FrontEnd};

/// nginx, as this suite has to know it.
const NGINX: FrontEnd = FrontEnd {
    package: "nginx",
    // Where an unpacked nginx is, as the CI step and a developer both set it: the directory holding
    // the binary, which for this package is also the root of the tree `conf/` sits in.
    variable: "MIXENGINE_NGINX_PACKAGE",
    version: "1.x",
    config: "nginx.conf",
    archive: Archive::WholeTree,
    // The data files a generated configuration reaches into the archive for, under the names the
    // recipe asks for them by.
    data_files: &[
        ("mime.types", "conf/mime.types"),
        ("fastcgi_params", "conf/fastcgi_params"),
    ],
    alone: |status| overrides(status, None),
    serving: |status, port, says| {
        overrides(
            status,
            // Inside `http { }`, which is where this template renders `extra` — a `server` block at
            // the top level of an nginx configuration is a parse error, and that difference from
            // Caddy is the reason the free-form override is rendered where each format wants it.
            Some(format!(
                "server {{\n        listen 127.0.0.1:{port};\n        \
                 location / {{\n            return 200 \"{says}\";\n        }}\n    }}\n"
            )),
        )
    },
    broken: |status| overrides(status, Some("this is not nginx {".to_owned())),
    control_line: |status| format!("listen 127.0.0.1:{status};"),
    // The endpoint this recipe renders *because* nginx has no admin one. See the module note on
    // `mixengine_core::generate::recipes::nginx`: a TCP accept cannot tell a serving nginx from one
    // whose workers have all died, because the master holds the listening socket either way.
    control_path: "/mixengine/health",
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
/// The whole overrides document for an nginx on `status`, with `extra` pasted in if there is any.
///
/// **The whole document and not a patch**, which is what `config_overrides_json` is: a setting that
/// is not in it is not set. So every override this suite writes repeats the status port, and one
/// that forgot would move the endpoint back to the recipe's default under a server listening on the
/// one this home chose — a readiness check and a health probe pointed at a port nothing answers on.
fn overrides(status: u16, extra: Option<String>) -> String {
    serde_json::json!({
        "status_port": status,
        "https_port": free_tls_port(),
        "extra": extra.unwrap_or_default(),
    })
    .to_string()
}

/// **The whole of T37, in the order a user meets it.**
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real nginx — see the module note, and the `nginx` step in ci.yml"]
async fn nginx_is_generated_validated_started_reloaded_and_stopped() {
    frontend::is_generated_validated_started_reloaded_and_stopped(&NGINX).await;
}

/// **And a home that has one front end is refused the other** — the rule `Recipe::role` exists for.
///
/// Here rather than in a unit test because what the unit tests know is that
/// `core::services::front_end` finds a front end by its role; what a *user* meets is a
/// `service.create` that says which one is already there. Nothing is started: this costs one
/// install and one create.
///
/// **No Caddy is installed, and that is the assertion.** The refusal is deliberately ordered before
/// the check that the named package exists, because installing the second front end would not help
/// — so a home with no Caddy at all still hears about the nginx it has, rather than being told to go
/// and fetch the thing it is about to be refused.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real nginx — see the module note, and the `nginx` step in ci.yml"]
async fn a_home_that_already_has_a_front_end_is_refused_the_other_one() {
    let (home, _daemon, _registry, _site, _status) = frontend::declared(&NGINX).await;

    let refused = home.mix(&["service", "create", "caddy", "2.x", "--json"]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );

    assert!(
        !refused.status.success(),
        "a second front end was created beside the first: {said}"
    );
    assert!(
        said.contains("nginx"),
        "the refusal does not name the front end this home already has: {said}"
    );
    assert!(
        !said.contains("is not installed"),
        "the refusal was about the package rather than about there being one front end, which is \
         the ordering this test is for: {said}"
    );
}
