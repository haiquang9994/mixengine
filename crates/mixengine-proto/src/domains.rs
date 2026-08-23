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
//! because many resolvers already map it. `.internal` was reserved by ICANN in July 2024 as the
//! private-use TLD — RFC 1918 for names. `.local` is mDNS territory and needs acknowledging. Every
//! other TLD is refused: the ones a person reaches for — `.dev`, `.app` — are HSTS-preloaded, and a
//! browser would refuse plain HTTP before any of this was consulted.
//!
//! **Two tables, not one** (T45, D9). [`MANAGED_TLDS`] is what a site may be named on;
//! [`WIRED_TLDS`] is what a resolver may be pointed at, and `.local` is in the first and not the
//! second.
//!
//! **The helper's table can be older than the daemon's**, because `mixengine-elevate` is excluded
//! from auto-update. That is the correct failure: a TLD a future build manages is refused by the
//! installed helper, loudly, at its own index — never applied because the caller said it was fine.

/// The TLD a site gets when nobody says otherwise.
pub const DEFAULT_TLD: &str = "test";

/// Every TLD MixEngine will answer for.
///
/// **`internal` is here rather than added after a release** — the T45 design, D9. ICANN reserved it
/// in July 2024 as the private-use TLD, which makes it safe in the sense the reserved ones are:
/// never delegated, never publicly resolvable. Adding it later would be far more expensive than
/// adding it now, because the helper is excluded from auto-update and every installed copy would
/// refuse a TLD it had never heard of until the user reinstalled it.
pub const MANAGED_TLDS: [&str; 4] = [DEFAULT_TLD, "localhost", "internal", "local"];

/// The TLDs a resolver on this machine may be pointed at — the T45 design, D9.
///
/// **`local` is managed and is not here, and two constants rather than one is the whole point.** A
/// site may be declared on `.local`, with `--i-know`, and it gets one exact hosts entry like any
/// other name. Wiring the TLD is a different act: the DNS server answers `A 127.0.0.1` for *every*
/// name beneath a managed TLD at any depth, so an `/etc/resolver/local` would send `printer.local`
/// and every other Bonjour name on the user's network to loopback, machine-wide.
pub const WIRED_TLDS: [&str; 3] = [DEFAULT_TLD, "localhost", "internal"];

/// May a resolver on this machine be pointed at `tld`?
///
/// Read by the planner in `mixengine-platform` and, independently, by `mixengine-elevate` — which
/// asks again rather than trusting that the caller did, for the reason this module exists.
#[must_use]
pub fn is_wired_tld(tld: &str) -> bool {
    WIRED_TLDS.contains(&tld)
}

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

    /// The table the helper reads, the default a site gets, and the subset that may ever be wired.
    #[test]
    fn the_table_names_every_tld_this_product_manages() {
        assert_eq!(DEFAULT_TLD, "test");

        for managed in ["test", "localhost", "internal", "local"] {
            assert!(MANAGED_TLDS.contains(&managed), "{managed} is managed");
        }

        assert!(
            !MANAGED_TLDS.contains(&"dev"),
            ".dev is delegated and HSTS-preloaded"
        );
        assert!(!MANAGED_TLDS.contains(&"lc"), ".lc is a real ccTLD");
    }

    /// The T45 design, D9. A site may be declared on `.local`; a resolver is never pointed at it,
    /// because the server answers every name under a wired TLD and `printer.local` is Bonjour's.
    #[test]
    fn local_is_managed_and_is_never_wired() {
        assert!(MANAGED_TLDS.contains(&"local"));
        assert!(!is_wired_tld("local"));

        for wired in WIRED_TLDS {
            assert!(
                MANAGED_TLDS.contains(&wired),
                "{wired} must be managed to be wired"
            );
            assert!(is_wired_tld(wired));
        }
    }

    /// A TLD nobody manages is not wired either, which is the branch a planner relies on.
    #[test]
    fn a_tld_outside_the_table_is_not_wired() {
        assert!(!is_wired_tld("dev"));
        assert!(!is_wired_tld(""));
    }
}
