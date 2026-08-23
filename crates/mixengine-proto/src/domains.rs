//! What a domain may be, and which TLDs MixEngine manages.
//!
//! **Here rather than in `mixengine-core` because two processes need it and only one of them may
//! ask the other** — the T41 design, D4. `mixengine-elevate` must refuse a domain outside a managed
//! TLD *itself*: being handed a list of permitted TLDs inside a request would be the helper
//! trusting the daemon, which is the one thing the security model does not allow. Sharing a
//! compile-time constant is not that.
//!
//! **Syntax lives here; policy does not.** Whether `.local` needs saying so out loud, which error a
//! refusal becomes, what a project's name slugs to — all of it stays in `mixengine_core::domains`,
//! which reads the table from here.
//!
//! # The table is compiled in
//!
//! `.test` is reserved by RFC 6761 for exactly this and is the default. `.localhost` is accepted
//! because many resolvers already map it. `.local` is mDNS territory and needs acknowledging. Every
//! other TLD is refused: the ones a person reaches for — `.dev`, `.app` — are HSTS-preloaded, and a
//! browser would refuse plain HTTP before any of this was consulted.
//!
//! **The helper's table can be older than the daemon's**, because `mixengine-elevate` is excluded
//! from auto-update. That is the correct failure: a TLD a future build manages is refused by the
//! installed helper, loudly, at its own index — never applied because the caller said it was fine.

/// The TLD a site gets when nobody says otherwise.
pub const DEFAULT_TLD: &str = "test";

/// Every TLD MixEngine will answer for.
pub const MANAGED_TLDS: [&str; 3] = [DEFAULT_TLD, "localhost", "local"];

/// The longest one label may be, in bytes. RFC 1035.
const LABEL_LIMIT: usize = 63;

/// The longest a whole name may be, in bytes.
const NAME_LIMIT: usize = 253;

/// Why `name` is not a domain, or [`None`] when it is one.
///
/// **Syntax alone, and nothing about policy**: it says nothing about which TLD the name is on, and
/// nothing about `.local`. The name is checked exactly as given — an uppercase letter is a refusal
/// rather than something to fix, because the one caller that cannot lowercase first is the helper,
/// which is validating somebody else's request.
#[must_use]
pub fn domain_syntax(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        Some("it is empty")
    } else if name.len() > NAME_LIMIT {
        Some("it is longer than two hundred and fifty-three bytes")
    } else if name.contains('*') {
        Some("a wildcard is answered by the DNS server rather than owned by a site")
    } else if name.split('.').count() < 2 {
        Some("it has only one label")
    } else {
        name.split('.').find_map(bad_label)
    }
}

/// [`domain_syntax`], as the question the helper asks.
#[must_use]
pub fn is_domain_syntax(name: &str) -> bool {
    domain_syntax(name).is_none()
}

/// Why this label is not one, or [`None`].
fn bad_label(label: &str) -> Option<&'static str> {
    if label.is_empty() {
        Some("it has an empty label")
    } else if label.len() > LABEL_LIMIT {
        Some("one of its labels is longer than sixty-three bytes")
    } else if !label
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        // Uppercase reaches this too, deliberately: the caller that lowercases is policy's, and the
        // caller that cannot is the helper's. Punycode is recorded as unsupported rather than
        // half-handled.
        Some("it holds something other than lowercase ASCII letters, digits and hyphens")
    } else if label.starts_with('-') || label.ends_with('-') {
        Some("one of its labels starts or ends with a hyphen")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Syntax alone. Every row here is a rule the helper applies to a request it does not trust.
    #[test]
    fn a_domain_is_lowercase_ascii_labels_with_at_least_two_of_them() {
        for good in [
            "blog.test",
            "api.blog.test",
            "x.localhost",
            "printer.local",
            "a-b.test",
        ] {
            assert!(is_domain_syntax(good), "{good} was refused");
        }

        for wrong in [
            "",            // nothing at all
            "blog",        // one label is a hostname
            "BLOG.TEST",   // policy lowercases; the helper is handed the result and checks it
            "-blog.test",  // a label starting with a hyphen
            "blog-.test",  // or ending with one
            "bl og.test",  // a space
            "*.blog.test", // a wildcard is the DNS server's answer, not a hosts line
            "blög.test",   // IDN, refused by name rather than mangled
            "blog..test",  // an empty label
        ] {
            assert!(!is_domain_syntax(wrong), "{wrong} was accepted");
        }

        let long_label = format!("{}.test", "a".repeat(64));
        assert!(!is_domain_syntax(&long_label));
        assert!(!is_domain_syntax(&format!("{}.test", "a.".repeat(130))));
    }

    /// The reason is the predicate's answer, so there is one place a refusal is worded.
    #[test]
    fn a_refusal_says_which_rule_was_broken() {
        assert_eq!(domain_syntax("blog.test"), None);
        assert!(domain_syntax("blog").is_some_and(|why| why.contains("one label")));
        assert!(domain_syntax("").is_some_and(|why| why.contains("empty")));
    }

    /// The table the helper reads, and the default a site gets.
    #[test]
    fn the_table_names_the_three_tlds_this_product_manages() {
        assert_eq!(DEFAULT_TLD, "test");
        assert!(MANAGED_TLDS.contains(&DEFAULT_TLD));
        assert!(MANAGED_TLDS.contains(&"localhost"));
        assert!(MANAGED_TLDS.contains(&"local"));
        assert!(!MANAGED_TLDS.contains(&"dev"));
    }
}
