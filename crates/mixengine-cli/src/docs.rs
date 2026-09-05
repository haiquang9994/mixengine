//! `mix docs` — the handbook, from the copy compiled into this binary.
//!
//! **No daemon, no home, no socket.** The command is answered before a client exists, because the
//! page somebody most needs is the one that explains why nothing starts. What it prints is the
//! page's Markdown, byte for byte: the same document the site serves at `/<locale>/<slug>.md`, so a
//! person and a program reading either receive the same thing — roadmap task T90, and ADR 0021 for
//! why this is not an API method.
//!
//! No colour and no renderer, which is [`crate::render`]'s rule and, here, also the design: a
//! rendering would be a second telling of a document this crate does not own.

use mixengine_docs::{BASE_URL, Locale, Page, VERSION, page, pages};

/// The environment variables that name a language, in the order they are consulted.
///
/// `MIXENGINE_LANG` is not among them because `clap` reads it as the flag's `env`. The three here
/// are the POSIX convention and are what a Unix user has already set; **Windows sets none of them**,
/// which is what the Vietnamese line in [`render`] exists for.
const LANGUAGE_VARIABLES: [&str; 3] = ["LC_ALL", "LC_MESSAGES", "LANG"];

/// Which language to answer in.
///
/// `explicit` is the `--lang` flag, which `clap` has already filled from `MIXENGINE_LANG` when the
/// flag was absent. An unrecognised value falls through to English rather than failing: a command
/// whose whole job is to explain things should answer.
pub(crate) fn resolve_locale(explicit: Option<&str>) -> Locale {
    if let Some(tag) = explicit
        && let Some(locale) = Locale::from_tag(tag)
    {
        return locale;
    }

    for variable in LANGUAGE_VARIABLES {
        if let Some(value) = std::env::var_os(variable)
            && let Some(locale) = value.to_str().and_then(Locale::from_tag)
        {
            return locale;
        }
    }

    Locale::En
}

/// One page, followed by where to read it — and, on an English page that has been translated, one
/// line in Vietnamese naming the command that shows the translation.
///
/// That line exists because Windows sets none of [`LANGUAGE_VARIABLES`], so a Vietnamese speaker
/// there is answered in English by default and would otherwise have nothing to read that says
/// otherwise. It is in Vietnamese for the same reason.
pub(crate) fn render(subject: &Page) -> String {
    let mut out = subject.body().to_owned();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');

    if subject.locale == Locale::En && page(Locale::Vi, subject.slug).is_some() {
        out.push_str(&format!(
            "Tiếng Việt: mix docs {} --lang vi\n",
            subject.slug
        ));
    }
    out.push_str(&format!("{}\n", subject.url()));
    out
}

/// What to print when no topic was named: the reading order, with each summary.
pub(crate) fn topics(locale: Locale) -> String {
    let width = pages(locale)
        .iter()
        .map(|subject| subject.slug.len())
        .max()
        .unwrap_or_default();

    let mut out = format!(
        "The MixEngine handbook, version {VERSION}, in {}.\n\n",
        locale.native_name()
    );
    for subject in pages(locale) {
        out.push_str(&format!(
            "  {slug:width$}  {summary}\n",
            slug = subject.slug,
            summary = subject.summary,
        ));
    }
    out.push_str(&format!(
        "\nmix docs <topic>             read one\n\
         mix docs <topic> --lang vi   read it in Vietnamese\n\n\
         {BASE_URL}{}/\n",
        locale.code()
    ));
    out
}

/// The page, or the message naming every topic there is.
///
/// # Errors
///
/// The topic does not name a page of the handbook.
pub(crate) fn look_up(locale: Locale, topic: &str) -> Result<&'static Page, String> {
    page(locale, topic).ok_or_else(|| {
        let known = pages(locale)
            .iter()
            .map(|subject| subject.slug)
            .collect::<Vec<_>>()
            .join(", ");
        format!("no handbook topic called `{topic}`. There are: {known}")
    })
}
