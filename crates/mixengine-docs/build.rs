//! Turns `docs/guide/{en,vi}/*.md` into a table of `&'static str` at compile time.
//!
//! The front matter is parsed **here** rather than at run time, which is what lets the library have
//! no dependencies: the title, slug, order and summary reach it as constants, and the body as a byte
//! offset into the file it already embeds. A page whose front matter does not parse is a build error
//! naming the file — a malformed document should fail like a syntax error, not like a test.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The keys every page declares, and the three a translation declares instead of prose.
#[derive(serde::Deserialize)]
struct FrontMatter {
    title: String,
    slug: String,
    order: u32,
    summary: String,
    #[serde(default)]
    translation_of: Option<String>,
    #[serde(default)]
    source_sha256: Option<String>,
    #[serde(default)]
    untranslated_reason: Option<String>,
}

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets this"));
    let guide = manifest
        .parent()
        .and_then(Path::parent)
        .expect("the crate is two directories below the workspace root")
        .join("docs")
        .join("guide");

    let mut generated = String::new();

    for locale in ["en", "vi"] {
        let directory = guide.join(locale);
        println!("cargo:rerun-if-changed={}", directory.display());

        let mut paths: Vec<PathBuf> = std::fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()))
            .map(|entry| entry.expect("a readable directory entry").path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
            .collect();
        paths.sort();

        let mut rows = Vec::new();
        for path in paths {
            println!("cargo:rerun-if-changed={}", path.display());
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
            let (front, offset) = split_front_matter(&source, &path);
            let front: FrontMatter = toml::from_str(front).unwrap_or_else(|error| {
                panic!("front matter of {} does not parse: {error}", path.display())
            });
            rows.push((front, offset, path));
        }

        rows.sort_by_key(|(front, _, _)| front.order);

        let name = locale.to_uppercase();
        writeln!(generated, "static {name}_PAGES: &[Page] = &[").expect("writing to a String");
        for (front, offset, path) in rows {
            writeln!(
                generated,
                "    Page {{ locale: Locale::{variant}, slug: {slug}, title: {title}, \
                 summary: {summary}, order: {order}, source: include_str!({path}), \
                 body_offset: {offset}, translation_of: {translation_of}, \
                 source_sha256: {source_sha256}, untranslated_reason: {untranslated_reason} }},",
                variant = if locale == "en" { "En" } else { "Vi" },
                slug = literal(&front.slug),
                title = literal(&front.title),
                summary = literal(&front.summary),
                order = front.order,
                path = literal(&path.display().to_string()),
                translation_of = option(front.translation_of.as_deref()),
                source_sha256 = option(front.source_sha256.as_deref()),
                untranslated_reason = option(front.untranslated_reason.as_deref()),
            )
            .expect("writing to a String");
        }
        generated.push_str("];\n\n");
    }

    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets this")).join("pages.rs");
    std::fs::write(&out, generated).expect("writing the generated table");
}

/// A Rust string literal for `value`, escaped by `Debug` — which is exactly the syntax a literal
/// needs, Windows path separators included.
fn literal(value: &str) -> String {
    format!("{value:?}")
}

/// `Some("x")` or `None`, as Rust source.
fn option(value: Option<&str>) -> String {
    match value {
        Some(value) => format!("Some({})", literal(value)),
        None => "None".to_owned(),
    }
}

/// The front matter between the opening and closing `+++`, and the byte offset the body starts at.
fn split_front_matter<'a>(source: &'a str, path: &Path) -> (&'a str, usize) {
    let opening = "+++\n";
    let rest = source.strip_prefix(opening).unwrap_or_else(|| {
        panic!(
            "{} does not open with a +++ front matter block",
            path.display()
        )
    });
    let end = rest
        .find("\n+++\n")
        .unwrap_or_else(|| panic!("{}'s front matter is never closed", path.display()));
    let closing = opening.len() + end + "\n+++\n".len();

    // Past the blank line every page puts between the front matter and its title. The body has to
    // begin at `# `, because that heading *is* the document's title everywhere it is read — in a
    // terminal, on github.com and at `/<locale>/<slug>.md`.
    let body_offset =
        closing + source[closing..].len() - source[closing..].trim_start_matches('\n').len();
    (&rest[..end], body_offset)
}
