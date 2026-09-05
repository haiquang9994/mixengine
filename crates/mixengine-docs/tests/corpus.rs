//! What the corpus must be true of, across files.
//!
//! Anything a single file can be wrong about on its own is the build script's, which fails naming
//! the file — see `build.rs`. What is here needs two files or more to notice.

use mixengine_docs::{Locale, pages};

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
fn a_language_tag_answers_the_locale_it_names() {
    assert_eq!(Locale::from_tag("vi"), Some(Locale::Vi));
    assert_eq!(Locale::from_tag("vi_VN.UTF-8"), Some(Locale::Vi));
    assert_eq!(Locale::from_tag("VI-vn"), Some(Locale::Vi));
    assert_eq!(Locale::from_tag("en_GB"), Some(Locale::En));
    assert_eq!(Locale::from_tag("C"), None);
    assert_eq!(Locale::from_tag(""), None);
}
