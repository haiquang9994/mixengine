//! What a domain may be, and which TLDs this home will answer for — roadmap task T39a.
//!
//! One module owns the policy for the same reason [`crate::projects`] owns the project walk: a
//! second copy of "is this a domain?" is a second answer to a question with exactly one. `site.*`
//! asks it here, T46's `domain.*` will ask it here, and T44's DNS server answers only for what this
//! module let through.
//!
//! # The table itself is in `mixengine-proto`
//!
//! Moved there by T41 (design D4), because `mixengine-elevate` has to refuse a domain outside a
//! managed TLD *itself* and being handed the permitted list in a request would be the helper
//! trusting the daemon. What stays here is everything that is not the table: which error each
//! refusal becomes, `.local` needing `accept_risky_tld`, and the slug a project's name makes.

use mixengine_proto::domains::{MANAGED_TLDS, domain_syntax};

use crate::{Error, Result};

/// The TLD a site gets when nobody says otherwise.
///
/// Re-exported rather than restated: one table, in [`mixengine_proto::domains`].
pub use mixengine_proto::domains::DEFAULT_TLD;

/// The TLD that works until somebody plugs in a printer.
const RISKY_TLD: &str = "local";

/// A domain, lowercased and checked, or the reason it is not one.
///
/// # Errors
///
/// [`Error::InvalidDomain`] for a name that is not one at all; [`Error::UnmanagedTld`] for a public
/// suffix; [`Error::RiskyTld`] for `.local` without `accept_risky_tld`.
pub fn normalised(domain: &str, accept_risky_tld: bool) -> Result<String> {
    let name = domain.trim().to_ascii_lowercase();

    if let Some(because) = domain_syntax(&name) {
        return Err(Error::InvalidDomain {
            domain: domain.to_owned(),
            because,
        });
    }

    let tld = name.rsplit('.').next().unwrap_or_default().to_owned();

    if !MANAGED_TLDS.contains(&tld.as_str()) {
        return Err(Error::UnmanagedTld {
            domain: domain.to_owned(),
            tld,
        });
    }

    if tld == RISKY_TLD && !accept_risky_tld {
        return Err(Error::RiskyTld {
            domain: domain.to_owned(),
        });
    }

    Ok(name)
}

/// A project's name as a domain label, or [`None`] when there is nothing left to make one of.
#[must_use]
pub fn slug(name: &str) -> Option<String> {
    let mut slug = String::with_capacity(name.len());

    for character in name.chars() {
        match character {
            'a'..='z' | '0'..='9' => slug.push(character),
            'A'..='Z' => slug.push(character.to_ascii_lowercase()),
            _ if slug.ends_with('-') => {}
            _ => slug.push('-'),
        }
    }

    let trimmed = slug.trim_matches('-');

    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// The domain a project gets when nobody named one: `<slug>.test`.
///
/// **A collision is not this function's business** — it is refused by the caller, naming the site
/// that holds the domain, because appending `-2` would hand somebody a domain they never typed and
/// would not remember (spec D10).
///
/// # Errors
///
/// [`Error::InvalidDomain`] for a name with no ASCII in it to slug.
pub fn default_for(project_name: &str) -> Result<String> {
    let slug = slug(project_name).ok_or_else(|| Error::InvalidDomain {
        domain: project_name.to_owned(),
        because: "there is nothing in the project's name a domain label can be made of",
    })?;

    normalised(&format!("{slug}.{DEFAULT_TLD}"), false)
}

/// This site's domains with `domain` on the end — roadmap task **T46**.
///
/// **On the end, never at the head.** The head is the primary, which decides the site's canonical
/// URL and, from phase 5, the name on its certificate; a verb that says "add a domain" does not get
/// to change that (T46 design, D3).
///
/// Adding a name the site already has is not an error. The caller asked for a state, and it is the
/// state they get — which is also what makes a client retrying a request harmless.
///
/// The name is not checked here. [`normalised`] is the one place that decides what a domain may be,
/// and the caller runs it: two checks would be two answers to a question with one.
#[must_use]
pub fn after_adding(current: &[String], domain: &str) -> Vec<String> {
    let mut after = current.to_vec();

    if !after.iter().any(|held| held == domain) {
        after.push(domain.to_owned());
    }

    after
}

/// This site's domains without `domain` — roadmap task **T46**.
///
/// # Errors
///
/// [`Error::LastDomain`] when it is the only one, which is the "at least one" invariant
/// `0001_initial.sql` records as this layer's to uphold because SQLite cannot express it; and
/// [`Error::PrimaryDomain`] when it is the head of the list.
pub fn after_removing(current: &[String], domain: &str) -> Result<Vec<String>> {
    if current.len() <= 1 {
        return Err(Error::LastDomain {
            domain: domain.to_owned(),
        });
    }

    if current.first().is_some_and(|primary| primary == domain) {
        return Err(Error::PrimaryDomain {
            domain: domain.to_owned(),
        });
    }

    Ok(current
        .iter()
        .filter(|held| *held != domain)
        .cloned()
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The policy table, row by row. Each line is a rule somebody would otherwise re-decide in a
    /// second place.
    #[test]
    fn a_domain_is_lowercase_ascii_labels_on_a_managed_tld() {
        assert_eq!(normalised("BLOG.test", false).unwrap(), "blog.test");
        assert_eq!(
            normalised("  api.blog.test  ", false).unwrap(),
            "api.blog.test"
        );
        assert_eq!(
            normalised("shop.localhost", false).unwrap(),
            "shop.localhost"
        );

        for wrong in [
            "blog",        // one label is a hostname, not a domain
            "-blog.test",  // a label starting with a hyphen
            "blog-.test",  // or ending with one
            "bl og.test",  // a space
            "*.blog.test", // a wildcard is T44's answer, not a row
            "blög.test",   // IDN, refused by name rather than mangled
            "blog..test",  // an empty label
            "",
        ] {
            assert!(
                matches!(normalised(wrong, false), Err(Error::InvalidDomain { .. })),
                "{wrong} was accepted"
            );
        }

        // 64 bytes in one label, and 254 across the whole name.
        let long_label = format!("{}.test", "a".repeat(64));
        assert!(matches!(
            normalised(&long_label, false),
            Err(Error::InvalidDomain { .. })
        ));
    }

    /// A public TLD is refused with the one this home does manage in the message.
    #[test]
    fn a_public_tld_is_refused_and_test_is_offered() {
        for public in ["blog.dev", "blog.app", "blog.com"] {
            let error = normalised(public, false).expect_err("a public suffix");
            assert!(
                matches!(&error, Error::UnmanagedTld { tld, .. } if !tld.is_empty()),
                "{error:?}"
            );
        }
    }

    /// `.local` in both directions: refused without the acknowledgement, accepted with it.
    #[test]
    fn dot_local_needs_saying_so_out_loud() {
        assert!(matches!(
            normalised("blog.local", false),
            Err(Error::RiskyTld { .. })
        ));
        assert_eq!(normalised("blog.local", true).unwrap(), "blog.local");
    }

    /// The default domain, and the names that cannot make one.
    #[test]
    fn a_project_name_becomes_a_domain_or_says_it_cannot() {
        assert_eq!(slug("Blog").as_deref(), Some("blog"));
        assert_eq!(slug("My Shop  v2").as_deref(), Some("my-shop-v2"));
        assert_eq!(slug("--blog--").as_deref(), Some("blog"));
        assert_eq!(slug("   "), None);
        assert_eq!(
            slug("日本語"),
            None,
            "there is no ASCII left to make a label of"
        );

        assert_eq!(default_for("My Shop").unwrap(), "my-shop.test");
        assert!(matches!(
            default_for("日本語"),
            Err(Error::InvalidDomain { .. })
        ));
    }

    /// The new name goes on the end, so the primary stays the primary.
    #[test]
    fn adding_never_disturbs_the_primary() {
        let after = after_adding(&["blog.test".to_owned()], "www.blog.test");

        assert_eq!(
            after,
            vec!["blog.test".to_owned(), "www.blog.test".to_owned()]
        );
    }

    /// Idempotent rather than an error: a client that adds a name twice has the state it asked for,
    /// which is what makes a retried request harmless.
    #[test]
    fn adding_a_name_a_site_already_has_changes_nothing() {
        let current = vec!["blog.test".to_owned(), "www.blog.test".to_owned()];

        assert_eq!(after_adding(&current, "www.blog.test"), current);
    }

    /// The invariant `0001_initial.sql` says SQLite cannot express, upheld where it can be.
    #[test]
    fn a_sites_last_domain_cannot_be_removed() {
        let refused = after_removing(&["blog.test".to_owned()], "blog.test")
            .expect_err("a site needs a domain");

        assert!(matches!(refused, Error::LastDomain { .. }), "{refused:?}");
    }

    /// Removing the primary would change what the site *is* under a verb that says "remove a
    /// domain" — the T46 design, D3.
    #[test]
    fn the_primary_cannot_be_removed() {
        let current = vec!["blog.test".to_owned(), "www.blog.test".to_owned()];

        let refused = after_removing(&current, "blog.test").expect_err("the primary stays");

        assert!(
            matches!(refused, Error::PrimaryDomain { .. }),
            "{refused:?}"
        );
    }

    /// An alias goes, and the order of what is left is untouched — the head is still the head.
    #[test]
    fn an_alias_is_removed_and_the_rest_keep_their_order() {
        let current = vec![
            "blog.test".to_owned(),
            "www.blog.test".to_owned(),
            "old.blog.test".to_owned(),
        ];

        let after = after_removing(&current, "www.blog.test").expect("it removes");

        assert_eq!(
            after,
            vec!["blog.test".to_owned(), "old.blog.test".to_owned()]
        );
    }
}
