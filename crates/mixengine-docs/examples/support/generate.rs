//! Builds the published documentation site out of the embedded corpus — roadmap task T90.
//!
//! **Not a `[[bin]]`, and not compiled into anything shipped.** It is reached from
//! `examples/build-site.rs` and from `tests/site.rs`, both of which include this file with
//! `#[path]`, so `pulldown-cmark` and `minijinja` stay in `[dev-dependencies]` and no Markdown
//! renderer is linked into `mix` — which prints the Markdown itself.
//!
//! **It is included rather than run as a subprocess**, and that was found the hard way:
//! `cargo test --all-targets` compiles an example but does not leave it at
//! `target/debug/examples/<name>`, so a test that looked for the file there passed on a machine
//! where somebody had run `cargo build` and failed on every CI runner. The example's own `main` is
//! five lines and is exercised end to end by `packaging/docs.sh --check` in the `docs` job.
//!
//! Three properties the tests hold and this file is written to keep:
//!
//! 1. **`/<locale>/<slug>.md` is the repository file, byte for byte.** It is written from
//!    [`Page::source`] and never from disk, so there is no route by which it could be filtered on
//!    the way out.
//! 2. **Nothing published contains a script.** Raw HTML passthrough is off, so a `<script>` in a
//!    page becomes visible text rather than an executed one.
//! 3. **The output is a pure function of the corpus and the version.** No timestamp, no git SHA, no
//!    host name — otherwise `packaging/docs.sh --check` would compare two things that always differ.

use std::collections::BTreeMap;
use std::path::Path;

use mixengine_docs::{BASE_URL, Locale, Page, VERSION, pages};
use sha2::{Digest as _, Sha256};

/// Write the whole site into `out`, replacing whatever was there.
///
/// `pub(crate)` rather than `pub`: this file is a module of two crates that each have exactly one
/// caller, and `unreachable_pub` is denied workspace-wide.
pub(crate) fn build(out: &Path) {
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clearing the output directory");
    }
    std::fs::create_dir_all(out).expect("creating the output directory");

    let mut environment = minijinja::Environment::new();
    environment
        .add_template("page.html", include_str!("../../templates/page.html"))
        .expect("the page template compiles");

    for locale in Locale::ALL {
        for page in pages(locale) {
            // The published Markdown, from the embedded string and not from the file system.
            write(&out.join(page.path()), page.source());

            let html = render(&environment, *page, Depth::Page);
            write(
                &out.join(locale.code()).join(page.slug).join("index.html"),
                &html,
            );

            // `/en/` and `/en/index/` are the same document. The index page's `order = 1` is what
            // makes that true rather than a coincidence, and a locale root that 404s would be the
            // first thing a person reaching this site would meet.
            if page.slug == "index" {
                let html = render(&environment, *page, Depth::Locale);
                write(&out.join(locale.code()).join("index.html"), &html);
            }
        }

        write(
            &out.join(locale.code()).join("llms-full.txt"),
            &full(locale),
        );
    }

    write(
        &out.join("style.css"),
        include_str!("../../templates/style.css"),
    );
    write(&out.join("index.html"), &chooser());
    write(&out.join("llms.txt"), &llms());
    write(&out.join("index.json"), &manifest());
    write(&out.join("sitemap.xml"), &sitemap());
    write(
        &out.join("robots.txt"),
        &format!("User-agent: *\nAllow: /\n\nSitemap: {BASE_URL}sitemap.xml\n"),
    );
    // Without this, GitHub Pages runs the upload through Jekyll, which drops files and directories
    // whose names begin with an underscore and rewrites others. What is uploaded here is finished.
    write(&out.join(".nojekyll"), "");
}

/// How deep in the tree a rendering sits, which is the only thing that changes in its links.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Depth {
    /// `/<locale>/index.html`.
    Locale,
    /// `/<locale>/<slug>/index.html`.
    Page,
}

impl Depth {
    /// The relative path from this page back to the site root.
    fn root(self) -> &'static str {
        match self {
            Depth::Locale => "../",
            Depth::Page => "../../",
        }
    }

    /// The relative path from this page to a sibling page's directory.
    fn sibling(self, slug: &str) -> String {
        match self {
            Depth::Locale => format!("{slug}/"),
            Depth::Page => format!("../{slug}/"),
        }
    }
}

/// One HTML page.
fn render(environment: &minijinja::Environment<'_>, page: Page, depth: Depth) -> String {
    let locale = page.locale;

    let contents: Vec<BTreeMap<&str, minijinja::Value>> = pages(locale)
        .iter()
        .map(|item| {
            BTreeMap::from([
                ("title", minijinja::Value::from(item.title)),
                ("href", url(depth.sibling(item.slug))),
                ("current", minijinja::Value::from(item.slug == page.slug)),
            ])
        })
        .collect();

    // A language switch that lands on the same page where there is one, and on that language's
    // index where there is not — never on a 404, which is what a switch built from the slug alone
    // would give the first time the two corpora differ.
    let languages: Vec<BTreeMap<&str, minijinja::Value>> = Locale::ALL
        .into_iter()
        .map(|other| {
            let href = if other == locale {
                depth.sibling(page.slug)
            } else if mixengine_docs::page(other, page.slug).is_some() {
                format!("{}{}/{}/", depth.root(), other.code(), page.slug)
            } else {
                format!("{}{}/", depth.root(), other.code())
            };
            BTreeMap::from([
                ("name", minijinja::Value::from(other.native_name())),
                ("href", url(href)),
                ("current", minijinja::Value::from(other == locale)),
            ])
        })
        .collect();

    let canonical = match depth {
        Depth::Locale => format!("{BASE_URL}{}/", locale.code()),
        Depth::Page => page.url(),
    };

    // A map rather than `minijinja::context!`: that macro lives behind the crate's `macros`
    // feature, and this workspace takes `minijinja` with default features off.
    let context = BTreeMap::from([
        ("locale", minijinja::Value::from(locale.code())),
        ("title", minijinja::Value::from(page.title)),
        ("summary", minijinja::Value::from(page.summary)),
        ("version", minijinja::Value::from(VERSION)),
        ("canonical", url(canonical)),
        ("markdown", url(page.markdown_url())),
        (
            "markdown_label",
            minijinja::Value::from(match locale {
                Locale::En => "This page as Markdown:",
                Locale::Vi => "Trang này ở dạng Markdown:",
            }),
        ),
        ("root", url(depth.root().to_owned())),
        ("body", minijinja::Value::from(to_html(page.body()))),
        ("contents", minijinja::Value::from(contents)),
        ("languages", minijinja::Value::from(languages)),
    ]);

    environment
        .get_template("page.html")
        .expect("the template was added above")
        .render(context)
        .expect("the template renders")
}

/// A URL, exempted from the template's HTML escaping.
///
/// `minijinja`'s HTML escaper turns `/` into `&#x2f;`, which is valid and which makes every address
/// on the page unreadable in the source — including the `rel="alternate"` link a program follows to
/// the Markdown. Every URL here is built from [`BASE_URL`], a locale code and a slug, none of which
/// can contain a character HTML would need escaped; titles and summaries, which are prose somebody
/// wrote, stay escaped.
fn url(value: String) -> minijinja::Value {
    minijinja::Value::from_safe_string(value)
}

/// Markdown to HTML, with the transformations the published HTML needs and nothing else.
///
/// **Raw HTML passthrough is off** — every `Html` and `InlineHtml` event becomes text — so a
/// `<script>` that ever reaches a page is escaped into something visible rather than executed. That
/// is the no-JavaScript promise held by a program rather than by a reviewer.
///
/// Links are rewritten here and only here: a page's HTML lives one directory deeper than its
/// Markdown, so `./other.md` — which is correct in the repository, on github.com and at
/// `/en/other.md` — becomes `../other/`.
fn to_html(body: &str) -> String {
    use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, html};

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_FOOTNOTES);

    let events = Parser::new_ext(body, options).map(|event| match event {
        Event::Html(text) | Event::InlineHtml(text) => Event::Text(text),
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Link {
            link_type,
            dest_url: CowStr::from(rewrite(&dest_url)),
            title,
            id,
        }),
        other => other,
    });

    let mut out = String::new();
    html::push_html(&mut out, events);
    out
}

/// `./other.md#anchor` becomes `../other/#anchor`. Everything else is left exactly as it is.
fn rewrite(target: &str) -> String {
    let Some(rest) = target.strip_prefix("./") else {
        return target.to_owned();
    };
    let (path, anchor) = match rest.split_once('#') {
        Some((path, anchor)) => (path, format!("#{anchor}")),
        None => (rest, String::new()),
    };
    match path.strip_suffix(".md") {
        Some(slug) => format!("../{slug}/{anchor}"),
        None => target.to_owned(),
    }
}

/// Every page of one locale in one file, for a reader that would rather make one request than
/// sixteen. Each is introduced by the URL it can be fetched at on its own.
fn full(locale: Locale) -> String {
    let mut out = format!(
        "# MixEngine {VERSION} — the whole handbook in {}\n\n\
         Every page of {BASE_URL}{}/ concatenated, in reading order. Each section names the URL it\n\
         is also published at on its own.\n",
        locale.native_name(),
        locale.code()
    );
    for page in pages(locale) {
        out.push_str(&format!(
            "\n\n---\n\n> Source: {}\n\n{}",
            page.markdown_url(),
            page.source()
        ));
    }
    out
}

/// The index a program reads first, in the `llms.txt` convention.
fn llms() -> String {
    let mut out = format!(
        "# MixEngine\n\n\
         > A local web development environment: several PHP, Node, Python and Ruby versions at\n\
         > once, with the web server, databases and caches a project needs, real `.test` domains\n\
         > and automatic HTTPS — no Docker and no configuration written by hand.\n\n\
         Documentation for MixEngine {VERSION}. Every page below is plain Markdown at the URL\n\
         shown, and is the same file this project's repository holds. The HTML rendering of a page\n\
         is the same address without the `.md`.\n"
    );

    for locale in Locale::ALL {
        out.push_str(&format!("\n## {}\n\n", locale.native_name()));
        for page in pages(locale) {
            out.push_str(&format!(
                "- [{}]({}): {}\n",
                page.title,
                page.markdown_url(),
                page.summary
            ));
        }
    }

    out.push_str(&format!(
        "\n## Machine-readable\n\n\
         - [Page manifest]({BASE_URL}index.json): every page with its title, summary, both URLs and\n\
         the SHA-256 of its Markdown.\n\
         - [Every English page in one file]({BASE_URL}en/llms-full.txt)\n\
         - [Every Vietnamese page in one file]({BASE_URL}vi/llms-full.txt)\n\
         - [The daemon's API contract](https://github.com/mixnz/mixengine/tree/master/bindings):\n\
         TypeScript types for every request, response, event and error `mixengined` speaks.\n\n\
         The same pages are compiled into the `mix` binary: `mix docs <topic>` prints one with no\n\
         network and no running daemon, and `mix docs <topic> --json` wraps it in an object.\n"
    ));
    out
}

/// `index.json` — the manifest a program reads instead of scraping the index.
fn manifest() -> String {
    let entries: Vec<serde_json::Value> = Locale::ALL
        .into_iter()
        .flat_map(|locale| pages(locale).iter())
        .map(|page| {
            serde_json::json!({
                "locale": page.locale.code(),
                "slug": page.slug,
                "order": page.order,
                "title": page.title,
                "summary": page.summary,
                "html": page.url(),
                "markdown": page.markdown_url(),
                "sha256": format!("{:x}", Sha256::digest(page.source().as_bytes())),
                "translation_of": page.translation_of,
            })
        })
        .collect();

    let document = serde_json::json!({
        "product": "MixEngine",
        "version": VERSION,
        "base_url": BASE_URL,
        "locales": Locale::ALL.map(Locale::code),
        "pages": entries,
    });

    format!(
        "{}\n",
        serde_json::to_string_pretty(&document).expect("a JSON document serialises")
    )
}

/// Every address a crawler should know about. `<loc>` only: a `<lastmod>` would be a clock in a
/// file that has to be reproducible.
fn sitemap() -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    out.push_str(&format!("  <url><loc>{BASE_URL}</loc></url>\n"));
    for locale in Locale::ALL {
        out.push_str(&format!(
            "  <url><loc>{BASE_URL}{}/</loc></url>\n",
            locale.code()
        ));
        for page in pages(locale) {
            out.push_str(&format!("  <url><loc>{}</loc></url>\n", page.url()));
        }
    }
    out.push_str("</urlset>\n");
    out
}

/// The site root: a language chooser that is a real page rather than a redirect.
///
/// A redirect costs a hop and gives English two addresses; this gives each language exactly one,
/// and gives a program a first page that already names `llms.txt`.
fn chooser() -> String {
    let mut list = String::new();
    for locale in Locale::ALL {
        let index = mixengine_docs::page(locale, "index").expect("every locale has an index page");
        list.push_str(&format!(
            "<li><a href=\"{}/\">{}</a><br><span>{}</span></li>\n",
            locale.code(),
            locale.native_name(),
            escape(index.summary)
        ));
    }

    format!(
        "<!DOCTYPE html>\n\
         <html lang=\"en\">\n\
         <head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>MixEngine documentation</title>\n\
         <meta name=\"description\" content=\"Documentation for MixEngine, in English and Vietnamese.\">\n\
         <link rel=\"canonical\" href=\"{BASE_URL}\">\n\
         <link rel=\"stylesheet\" href=\"style.css\">\n\
         </head>\n\
         <body>\n\
         <div class=\"chooser\">\n\
         <h1>MixEngine {VERSION}</h1>\n\
         <p>Documentation, in two languages.</p>\n\
         <ul>\n{list}</ul>\n\
         <hr>\n\
         <p class=\"source\">Reading this as a program? Start at <a href=\"llms.txt\">llms.txt</a>, \
         or fetch <a href=\"index.json\">index.json</a>. Every page is also published as plain \
         Markdown at the address of its HTML page with <code>.md</code> in place of the trailing \
         slash.</p>\n\
         </div>\n\
         </body>\n\
         </html>\n"
    )
}

/// The four characters that cannot appear literally in HTML text.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Write `contents` to `path`, creating whatever directories it needs.
fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("creating a directory");
    }
    std::fs::write(path, contents)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}
