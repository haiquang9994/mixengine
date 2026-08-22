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
}
