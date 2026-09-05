//! The MixEngine handbook, embedded at compile time.
//!
//! One Markdown corpus lives at `docs/guide/{en,vi}/` and is published three ways: as a static site,
//! as raw Markdown at predictable URLs, and — through this crate — compiled into `mix docs`, which
//! answers with **the same bytes** and needs no daemon. Roadmap task T90; ADR 0021 in
//! `.claude/decisions/` is why help is not an API method.
//!
//! Nothing here parses anything. The front matter is read by `build.rs`, which writes the metadata
//! out as constants beside an `include_str!` per file, so the cost of embedding the whole handbook
//! is the bytes and no more.

/// The published site's root. Every URL this crate hands out is built from it.
pub const BASE_URL: &str = "https://mixnz.github.io/mixengine/";

/// The MixEngine version this handbook describes.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A language the handbook is written in.
///
/// There are two. Adding a third is a corpus change and a variant here, and nothing else: every
/// other part of this crate iterates [`Locale::ALL`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Locale {
    /// English.
    En,
    /// Vietnamese.
    Vi,
}

impl Locale {
    /// Both of them, in the order the site lists them.
    pub const ALL: [Locale; 2] = [Locale::En, Locale::Vi];

    /// The two-letter code, which is also the directory and the URL segment.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Vi => "vi",
        }
    }

    /// What the language calls itself.
    ///
    /// A chooser shows this rather than an English name, so that somebody who cannot read the other
    /// language can still find their own.
    #[must_use]
    pub const fn native_name(self) -> &'static str {
        match self {
            Locale::En => "English",
            Locale::Vi => "Tiếng Việt",
        }
    }

    /// The exact code, and nothing else.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Locale> {
        Locale::ALL.into_iter().find(|locale| locale.code() == code)
    }

    /// A locale out of an environment variable's value: `vi`, `vi_VN` and `vi-VN.UTF-8` all answer
    /// [`Locale::Vi`].
    ///
    /// Only the leading letters are read, because everything after them names a territory or an
    /// encoding and neither picks a translation.
    #[must_use]
    pub fn from_tag(tag: &str) -> Option<Locale> {
        let head: String = tag
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .flat_map(char::to_lowercase)
            .collect();
        Locale::from_code(&head)
    }
}

/// One page of the handbook.
#[derive(Debug, Clone, Copy)]
pub struct Page {
    /// Which language this copy is in.
    pub locale: Locale,
    /// The file name without `.md`, and the URL segment.
    pub slug: &'static str,
    /// The title, which is also the body's opening `# ` heading.
    pub title: &'static str,
    /// One sentence, shown on the index, in `llms.txt` and in `index.json`.
    pub summary: &'static str,
    /// Position in the reading order, unique within a locale.
    pub order: u32,
    source: &'static str,
    body_offset: usize,
    /// For a translation, the English page it was made from.
    pub translation_of: Option<&'static str>,
    /// For a translation, the SHA-256 of that page's bytes when it was last revisited.
    pub source_sha256: Option<&'static str>,
    /// For the one page that is not a translation, why it is not.
    pub untranslated_reason: Option<&'static str>,
}

impl Page {
    /// The whole file, front matter included — what the site serves at `/<locale>/<slug>.md`.
    #[must_use]
    pub const fn source(self) -> &'static str {
        self.source
    }

    /// The document without its front matter — what `mix docs` prints, opening at its `# ` title.
    #[must_use]
    pub fn body(self) -> &'static str {
        &self.source[self.body_offset..]
    }

    /// `en/getting-started.md` — the path in this repository and on the site alike.
    #[must_use]
    pub fn path(self) -> String {
        format!("{}/{}.md", self.locale.code(), self.slug)
    }

    /// Where a person reads it.
    #[must_use]
    pub fn url(self) -> String {
        format!("{BASE_URL}{}/{}/", self.locale.code(), self.slug)
    }

    /// Where a program reads it.
    #[must_use]
    pub fn markdown_url(self) -> String {
        format!("{BASE_URL}{}", self.path())
    }
}

include!(concat!(env!("OUT_DIR"), "/pages.rs"));

/// Every page of one locale, in reading order.
#[must_use]
pub fn pages(locale: Locale) -> &'static [Page] {
    match locale {
        Locale::En => EN_PAGES,
        Locale::Vi => VI_PAGES,
    }
}

/// One page by slug, or nothing.
#[must_use]
pub fn page(locale: Locale, slug: &str) -> Option<&'static Page> {
    pages(locale).iter().find(|page| page.slug == slug)
}
