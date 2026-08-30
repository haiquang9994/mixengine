//! A PHP site, reached the way a person reaches one — roadmap task **T72a**.
//!
//! **The first request in this repository that goes through the front end and comes back from PHP.**
//! `caddy.rs` proves a rendering the server accepts and `php_fpm.rs` proves a pool that executes a
//! script; between them was the claim neither makes, which is that a browser pointed at a site gets
//! that pool's answer.
//!
//! # Why it exists now
//!
//! T72a renders `pm.status_path` into every pool file, so php-fpm publishes its own status page on
//! the socket a site's traffic arrives on. What separates the two is a *name*: both front ends hand
//! FastCGI only what matches `.php`, and the status path deliberately does not — so no URL can
//! produce a `SCRIPT_NAME` equal to it.
//!
//! **That was an argument, and this is the measurement.** It is the assertion that lets the pool file
//! carry the directive at all: if a site can be asked for its pool's status page, the arrangement is
//! wrong and no amount of prose fixes it.
//!
//! `#[ignore]`d rather than skipped, for `caddy.rs`' reason: a test that quietly returns when it
//! finds no PHP is a green suite that proved nothing on the day the download broke.

mod harness;

use harness::frontend::request_as;
use harness::php_site;

/// **A site serves PHP through the front end.**
///
/// The precondition for everything else here, and worth its own assertion: a 404 to the status page
/// means nothing if the site answers nothing either.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real Caddy and a real PHP — see the module note, and the `caddy` and `php` steps in ci.yml"]
async fn a_site_is_served_by_the_pool_behind_it() {
    let served = php_site::served(&php_site::runtimes()[..1]).await;
    let site = &served.sites[0];

    let answer = request_as(served.port, "/", &site.domain).unwrap_or_else(|| {
        panic!(
            "the front end answered nothing at all\n{}",
            served.home.daemon_log()
        )
    });

    assert!(
        answer.contains("200"),
        "a site with a running pool behind it did not answer 200: {answer}\n{}",
        served.home.daemon_log()
    );
    assert!(
        answer.contains(&site.says),
        "the body is not what this site's PHP prints: {answer}\n{}",
        served.home.daemon_log()
    );
}

/// **A site cannot be asked for its pool's status page.**
///
/// The status page shares the socket with real traffic, so what keeps it private is that both front
/// ends hand FastCGI only what matches `.php` — `/mixengine-status` never becomes a `SCRIPT_NAME`.
///
/// **A red here is not a test to adjust.** It would mean every site in every home is publishing how
/// many workers its pool has and how many requests it has served, and the answer is to change the
/// arrangement rather than the assertion.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real Caddy and a real PHP — see the module note, and the `caddy` and `php` steps in ci.yml"]
async fn a_site_cannot_be_asked_for_the_pools_status_page() {
    let served = php_site::served(&php_site::runtimes()[..1]).await;
    let site = &served.sites[0];

    for path in ["/mixengine-status", "/mixengine-status?json"] {
        let answer = request_as(served.port, path, &site.domain).unwrap_or_else(|| {
            panic!(
                "the front end answered nothing at all\n{}",
                served.home.daemon_log()
            )
        });

        // **The body and not the status line.** A front end that answered 200 with the site's own
        // `index.php` would be perfectly correct — the request was rewritten to a real script — and
        // what must never appear is php-fpm's accounting.
        assert!(
            !answer.contains("accepted conn"),
            "a site served php-fpm's status page at {path}: {answer}\n{}",
            served.home.daemon_log()
        );
        assert!(
            !answer.contains("max children reached"),
            "a site served php-fpm's status page at {path}: {answer}\n{}",
            served.home.daemon_log()
        );
    }
}
