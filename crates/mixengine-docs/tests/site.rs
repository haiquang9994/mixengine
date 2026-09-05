//! The generator, run into a `TempDir`.
//!
//! It lives in `examples/support/generate.rs` — an example and not a library function, which is
//! what keeps the Markdown renderer out of `mix` — and this file **includes** it rather than
//! running the built example as a subprocess.
//!
//! That was found the hard way. `cargo test --all-targets` compiles an example but does not leave
//! it at `target/debug/examples/<name>`, so the earlier version of this file passed on a machine
//! where somebody had run `cargo build` and failed on all three CI runners. Including the module is
//! also faster and gives a real stack trace. The example's own five-line `main` is exercised end to
//! end by `packaging/docs.sh --check`, in the job that exists for it.

use std::path::{Path, PathBuf};

use mixengine_docs::{Locale, pages};

#[path = "../examples/support/generate.rs"]
mod generate;

/// Build the site into a fresh directory and hand it back.
fn build() -> tempfile::TempDir {
    let out = tempfile::tempdir().expect("a temporary directory");
    generate::build(out.path());
    out
}

fn read(root: &Path, relative: &str) -> String {
    std::fs::read_to_string(root.join(relative))
        .unwrap_or_else(|error| panic!("{relative}: {error}"))
}

#[test]
fn the_published_markdown_is_the_corpus_byte_for_byte() {
    let site = build();
    for locale in Locale::ALL {
        for page in pages(locale) {
            let published = read(site.path(), &page.path());
            assert_eq!(
                published,
                page.source(),
                "{} was altered on the way out",
                page.path()
            );
        }
    }
}

#[test]
fn every_page_has_an_html_rendering_that_links_back_to_its_markdown() {
    let site = build();
    for locale in Locale::ALL {
        for page in pages(locale) {
            let html = read(
                site.path(),
                &format!("{}/{}/index.html", locale.code(), page.slug),
            );
            assert!(
                html.contains(&format!("<html lang=\"{}\">", locale.code())),
                "{}",
                page.path()
            );
            assert!(
                html.contains("rel=\"alternate\" type=\"text/markdown\""),
                "{}",
                page.path()
            );
            assert!(
                html.contains(&format!("href=\"{}\"", page.markdown_url())),
                "{}",
                page.path()
            );
            assert!(html.contains("<h1>"), "{}", page.path());
            // The template escapes HTML, and its escaper turns `/` into `&#x2f;`. An address that
            // came out that way still works and is unreadable in the source, which is a
            // documentation site failing at the one thing it is for. Prose is a different matter
            // and stays escaped — a summary quoting `https://blog.test` is the common case, and it
            // is why this asks about the addresses rather than about the whole file.
            assert!(
                html.contains(&format!("href=\"{}\"", page.url())),
                "{} does not carry an unescaped canonical",
                page.path()
            );
        }

        // `/en/` is the index page, not a 404 and not a redirect.
        let index = read(site.path(), &format!("{}/index.html", locale.code()));
        assert!(
            index.contains("<h1>"),
            "{} has no locale index",
            locale.code()
        );
    }
}

#[test]
fn a_link_between_pages_is_rewritten_for_the_html_tree_and_left_alone_in_the_markdown() {
    let site = build();
    // `vi/cli.md` links `./index.md`, which is correct as it stands for the Markdown and has to
    // become `../index/` one directory deeper.
    let markdown = read(site.path(), "vi/cli.md");
    assert!(markdown.contains("](./index.md)"), "{markdown}");

    let html = read(site.path(), "vi/cli/index.html");
    assert!(html.contains("href=\"../index/\""), "{html}");
    assert!(!html.contains("./index.md"), "{html}");
}

#[test]
fn nothing_published_contains_a_script() {
    let site = build();
    let mut checked = 0;
    for entry in walk(site.path()) {
        let text = std::fs::read_to_string(&entry).unwrap_or_default();
        assert!(
            !text.contains("<script"),
            "{} carries a script",
            entry.display()
        );
        checked += 1;
    }
    assert!(
        checked > 10,
        "the walk found almost nothing: {checked} files"
    );
}

#[test]
fn the_machine_readable_index_lists_every_page() {
    let site = build();
    let manifest: serde_json::Value =
        serde_json::from_str(&read(site.path(), "index.json")).expect("valid JSON");
    let listed = manifest["pages"].as_array().expect("an array").len();
    assert_eq!(listed, pages(Locale::En).len() + pages(Locale::Vi).len());
    assert_eq!(manifest["version"], mixengine_docs::VERSION);
    assert_eq!(manifest["base_url"], mixengine_docs::BASE_URL);

    let llms = read(site.path(), "llms.txt");
    for locale in Locale::ALL {
        for page in pages(locale) {
            assert!(
                llms.contains(&page.markdown_url()),
                "llms.txt omits {}",
                page.path()
            );
        }
    }

    for locale in Locale::ALL {
        let full = read(site.path(), &format!("{}/llms-full.txt", locale.code()));
        for page in pages(locale) {
            assert!(
                full.contains(page.source()),
                "{}/llms-full.txt omits {}",
                locale.code(),
                page.slug
            );
        }
    }
}

#[test]
fn crawlers_are_pointed_at_something_that_exists() {
    let site = build();
    let robots = read(site.path(), "robots.txt");
    assert!(robots.contains("Sitemap: "), "{robots}");

    let sitemap = read(site.path(), "sitemap.xml");
    for locale in Locale::ALL {
        for page in pages(locale) {
            assert!(
                sitemap.contains(&page.url()),
                "the sitemap omits {}",
                page.path()
            );
        }
    }
}

#[test]
fn two_runs_produce_the_same_bytes() {
    let first = build();
    let second = build();
    for entry in walk(first.path()) {
        let relative = entry.strip_prefix(first.path()).expect("under the root");
        let a = std::fs::read(&entry).expect("readable");
        let b = std::fs::read(second.path().join(relative)).expect("readable");
        assert_eq!(
            a,
            b,
            "{} differs between two runs — the generator is not a pure function of the corpus",
            relative.display()
        );
    }
}

/// Every file under `root`, recursively.
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(&directory).expect("readable") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}
