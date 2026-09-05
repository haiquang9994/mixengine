//! What the corpus must be true of, across files.
//!
//! Anything a single file can be wrong about on its own is the build script's, which fails naming
//! the file — see `build.rs`. What is here needs two files or more to notice.

use std::collections::BTreeSet;

use mixengine_docs::{Locale, pages};
use sha2::{Digest as _, Sha256};

/// The one page that is deliberately not a translation.
///
/// `cli` is generated from the binary's own English help strings, and a hand-translated copy would
/// be a second source of truth for twenty commands — see `docs/guide/vi/cli.md`, which says so to a
/// reader rather than only to this list.
const NOT_TRANSLATED: &[&str] = &["cli"];

#[test]
fn every_page_opens_with_its_own_title() {
    for locale in Locale::ALL {
        for page in pages(locale) {
            let first = page.body().lines().next().unwrap_or_default();
            assert_eq!(
                first,
                format!("# {}", page.title),
                "{} does not open with its front matter's title",
                page.path()
            );
        }
    }
}

#[test]
fn the_body_is_the_file_without_its_front_matter() {
    for locale in Locale::ALL {
        for page in pages(locale) {
            assert!(
                page.source().ends_with(page.body()),
                "{}'s body is not a suffix of its source",
                page.path()
            );
            assert!(
                !page.body().contains("+++"),
                "{}'s body still carries a front matter delimiter",
                page.path()
            );
        }
    }
}

#[test]
fn both_locales_hold_the_same_slugs() {
    let english: BTreeSet<&str> = pages(Locale::En).iter().map(|page| page.slug).collect();
    let vietnamese: BTreeSet<&str> = pages(Locale::Vi).iter().map(|page| page.slug).collect();
    assert_eq!(
        english, vietnamese,
        "a page that exists in one language only makes \"available in Vietnamese\" false for \
         whoever needs the missing one"
    );
}

#[test]
fn the_reading_order_is_unique_and_ascending() {
    for locale in Locale::ALL {
        let orders: Vec<u32> = pages(locale).iter().map(|page| page.order).collect();
        let unique: BTreeSet<u32> = orders.iter().copied().collect();
        assert_eq!(
            orders.len(),
            unique.len(),
            "{} gives two pages the same order",
            locale.code()
        );
        assert!(
            orders.windows(2).all(|pair| pair[0] < pair[1]),
            "{} is not sorted by order",
            locale.code()
        );
    }
}

#[test]
fn every_internal_link_resolves() {
    let slugs: BTreeSet<&str> = pages(Locale::En).iter().map(|page| page.slug).collect();

    for locale in Locale::ALL {
        for page in pages(locale) {
            for target in links(page.body()) {
                let (path, _) = target.split_once('#').unwrap_or((target.as_str(), ""));
                let slug = path
                    .strip_prefix("./")
                    .and_then(|slug| slug.strip_suffix(".md"))
                    .unwrap_or_else(|| {
                        panic!(
                            "{} links to `{target}` — an internal link is always `./<slug>.md`, \
                             which is the one spelling that is correct in this repository, on \
                             github.com and at /{}/{}.md alike",
                            page.path(),
                            locale.code(),
                            page.slug
                        )
                    });
                assert!(
                    slugs.contains(slug),
                    "{} links to `{target}`, which is not a page of this handbook",
                    page.path()
                );
            }
        }
    }
}

#[test]
fn a_translation_names_the_version_it_was_made_from() {
    for page in pages(Locale::Vi) {
        if NOT_TRANSLATED.contains(&page.slug) {
            assert!(
                page.untranslated_reason.is_some(),
                "{} is exempt from translation and says nothing about why",
                page.path()
            );
            continue;
        }

        assert!(
            page.untranslated_reason.is_none(),
            "{} claims to be untranslated; only {NOT_TRANSLATED:?} may",
            page.path()
        );

        let source = page
            .translation_of
            .unwrap_or_else(|| panic!("{} declares no translation_of", page.path()));
        assert_eq!(source, format!("en/{}.md", page.slug));

        let english = mixengine_docs::page(Locale::En, page.slug)
            .unwrap_or_else(|| panic!("{source} is missing"));
        let expected = format!("{:x}", Sha256::digest(english.source().as_bytes()));
        assert_eq!(
            page.source_sha256,
            Some(expected.as_str()),
            "{} was written against an older {source}.\n\
             Translate the change, then run: bash packaging/docs.sh --restamp",
            page.path()
        );
    }
}

#[test]
fn no_page_carries_raw_html_or_a_reference_link() {
    for locale in Locale::ALL {
        for page in pages(locale) {
            for (number, line) in outside_fences(page.body()) {
                let prose = without_code_spans(line);
                assert!(
                    !looks_like_a_tag(&prose),
                    "{}:{number} carries raw HTML. The generator turns it into text — which is how \
                     \"the site has no JavaScript\" is held by a program rather than by a reviewer",
                    page.path()
                );
                assert!(
                    !is_reference_definition(line),
                    "{}:{number} is a reference-style link definition; links here are inline",
                    page.path()
                );
            }
        }
    }
}

#[test]
fn prose_is_wrapped_at_a_hundred_columns() {
    for locale in Locale::ALL {
        for page in pages(locale) {
            for (number, line) in outside_fences(page.body()) {
                // A table row and a line carrying a URL are the two things that cannot be wrapped.
                // An internal link is `./<slug>.md` and fits, so it is not exempt.
                if line.starts_with('|') || line.contains("http") {
                    continue;
                }
                assert!(
                    line.chars().count() <= 100,
                    "{}:{number} is {} columns wide",
                    page.path(),
                    line.chars().count()
                );
            }
        }
    }
}

/// Every inline link target in `body` that is not an external URL.
fn links(body: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find("](") {
        rest = &rest[open + 2..];
        let Some(close) = rest.find(')') else { break };
        found.push(rest[..close].to_owned());
        rest = &rest[close..];
    }
    found
        .into_iter()
        .filter(|target| !target.starts_with("http"))
        .collect()
}

/// Every line that is not inside a fenced code block, with its 1-based number.
fn outside_fences(body: &str) -> Vec<(usize, &str)> {
    let mut inside = false;
    body.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            if line.trim_start().starts_with("```") {
                inside = !inside;
                return None;
            }
            (!inside).then_some((index + 1, line))
        })
        .collect()
}

/// The line with everything between backticks removed.
///
/// `<DIR>` inside a code span is a placeholder in a usage line, not a tag — and the reference page
/// is full of them.
fn without_code_spans(line: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for character in line.chars() {
        if character == '`' {
            inside = !inside;
        } else if !inside {
            out.push(character);
        }
    }
    out
}

/// `<div>`, `<br/>`, `</p>` — but not `a < b` and not `<https://example.test>`.
///
/// The name is collected out of ASCII characters only, so slicing by its byte length is safe on a
/// line of Vietnamese prose — which is the bug the obvious version of this has.
fn looks_like_a_tag(line: &str) -> bool {
    for (index, byte) in line.bytes().enumerate() {
        if byte != b'<' {
            continue;
        }
        let rest = &line[index + 1..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '/')
            .collect();
        if name.is_empty() {
            continue;
        }
        let after = &rest[name.len()..];
        if after.starts_with('>') || after.starts_with(' ') || after.starts_with("/>") {
            return true;
        }
    }
    false
}

/// `[label]: https://…` at the start of a line.
fn is_reference_definition(line: &str) -> bool {
    line.starts_with('[') && line.contains("]: ")
}

#[test]
fn a_language_tag_answers_the_locale_it_names() {
    assert_eq!(Locale::from_tag("vi"), Some(Locale::Vi));
    assert_eq!(Locale::from_tag("vi_VN.UTF-8"), Some(Locale::Vi));
    assert_eq!(Locale::from_tag("VI-vn"), Some(Locale::Vi));
    assert_eq!(Locale::from_tag("en_GB"), Some(Locale::En));
    assert_eq!(Locale::from_tag("C"), None);
    assert_eq!(Locale::from_tag(""), None);
}
