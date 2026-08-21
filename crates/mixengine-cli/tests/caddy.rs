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

/// The whole overrides document for a Caddy on `admin`, with `extra` pasted in if there is any.
///
/// **The whole document and not a patch**, which is what `config_overrides_json` is: a setting that
/// is not in it is not set. So every override this suite writes repeats the admin port, and one that
/// forgot would move the endpoint back to Caddy's default under a server listening on the one this
/// home chose — a reload and a stop sent to an address nothing answers on.
fn overrides(admin: u16, extra: Option<String>) -> String {
    serde_json::json!({
        "admin_port": admin,
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
