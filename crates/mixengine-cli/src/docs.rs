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

/// The whole command tree as one Markdown document, front matter included.
///
/// **Generated rather than written.** `mix` has twenty top-level commands and eighteen groups under
/// them, and a hand-written reference for that is wrong within a week — roadmap task T90, D10.
///
/// It reads the `clap` tree and **nothing else**. In particular it never reads the corpus, which is
/// what keeps the generation non-circular: this output becomes `docs/guide/en/cli.md`, which is
/// compiled into the binary that produces it.
pub(crate) fn reference(command: &clap::Command) -> String {
    let mut out = String::new();
    out.push_str("+++\n");
    out.push_str("title = \"Command reference\"\n");
    out.push_str("slug = \"cli\"\n");
    out.push_str("order = 15\n");
    out.push_str(
        "summary = \"Every mix command and every flag, generated from the binary's own \
         definitions.\"\n",
    );
    out.push_str("+++\n\n");

    out.push_str("# Command reference\n\n");
    out.push_str(&format!(
        "Every command `mix` accepts, in version {VERSION}. This page is **generated** from the\n\
         binary's own definitions, so it cannot describe a flag that is not there — and it is the\n\
         one page of this handbook that exists in English only, because those definitions are.\n\
         `mix docs cli --lang vi` says why, in Vietnamese.\n\n\
         The same text is `mix <command> --help` on the machine in front of you, and\n\
         `mix docs --reference` prints this whole page.\n\n\
         Three flags are accepted by every command below and are not repeated in each table:\n\
         `--home <DIR>` chooses which installation to talk to, `--json` asks for the answer as\n\
         JSON, and `--no-autostart` refuses to start a daemon that is not running.\n\n",
    ));

    // Depth 1 is the root, which prints nothing: `# Command reference` is already the H1, and `mix`
    // on its own is not a command anybody looks up. Its children are therefore `##`.
    section(&mut out, command, &["mix".to_owned()], 1);
    out
}

/// One command and then each of its subcommands, depth-first, in declaration order.
fn section(out: &mut String, command: &clap::Command, path: &[String], depth: usize) {
    if path.len() > 1 {
        out.push_str(&format!("{} {}\n\n", "#".repeat(depth), path.join(" ")));

        if let Some(about) = command.get_long_about().or_else(|| command.get_about()) {
            // `wrap` already ends each paragraph with a blank line, so nothing is added here.
            out.push_str(&wrap(&about.to_string()));
        }

        out.push_str("```\n");
        out.push_str(&usage(command, path));
        out.push_str("```\n\n");
        out.push_str(&arguments(command));
    }

    for sub in command.get_subcommands().filter(|sub| !sub.is_hide_set()) {
        let mut next = path.to_vec();
        next.push(sub.get_name().to_owned());
        // `min(6)` because Markdown has no `#######`. Nothing in this tree is that deep today; the
        // clamp is there so that adding a level is a flatter reference rather than broken syntax.
        section(out, sub, &next, (depth + 1).min(6));
    }
}

/// Prose wrapped at a hundred columns, paragraph by paragraph.
///
/// The corpus is hard-wrapped and a test holds it there, so a generated page has to wrap itself:
/// `clap`'s long help is one long line per paragraph, and a page nobody can read in a terminal is
/// not a page this handbook wants. A word longer than the limit is left alone rather than broken —
/// it is a URL or a path, and breaking either makes it wrong.
fn wrap(text: &str) -> String {
    let mut out = String::new();
    for paragraph in text.split("\n\n") {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > 100 {
                out.push_str(&line);
                out.push('\n');
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if !line.is_empty() {
            out.push_str(&line);
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// The one-line usage, rooted at the real command path rather than at the binary name.
fn usage(command: &clap::Command, path: &[String]) -> String {
    let mut line = path.join(" ");

    for argument in command
        .get_arguments()
        .filter(|argument| argument.is_positional() && !argument.is_hide_set())
    {
        let name = positional(argument);
        line.push_str(&if argument.is_required_set() {
            format!(" <{name}>")
        } else {
            format!(" [{name}]")
        });
    }

    if command.get_subcommands().next().is_some() {
        line.push_str(" <COMMAND>");
    }
    if command.get_arguments().any(|argument| {
        !argument.is_positional() && !argument.is_hide_set() && !is_global(argument)
    }) {
        line.push_str(" [OPTIONS]");
    }

    line.push('\n');
    line
}

/// How a positional argument is spelled, in one place so the usage line and the table agree.
///
/// `get_value_names` answers `Option<&[clap::builder::Str]>`, which is neither `Copy` nor joinable —
/// take the first by reference and borrow it as a `&str`. Where a command names none, the field's
/// own identifier is what `clap` would have shown.
fn positional(argument: &clap::Arg) -> &str {
    argument
        .get_value_names()
        .and_then(<[clap::builder::Str]>::first)
        .map_or_else(|| argument.get_id().as_str(), clap::builder::Str::as_str)
}

/// One table row per flag: how it is spelled, and what it is for.
///
/// The three global flags are left out of every table — they are stated once, in the page's opening
/// paragraph, and repeating them sixty times would bury the flags that differ.
fn arguments(command: &clap::Command) -> String {
    let rows: Vec<String> = command
        .get_arguments()
        .filter(|argument| {
            !argument.is_hide_set() && argument.get_id() != "help" && !is_global(argument)
        })
        .map(|argument| {
            let mut spelling = String::new();
            if let Some(short) = argument.get_short() {
                spelling.push_str(&format!("`-{short}`, "));
            }
            if let Some(long) = argument.get_long() {
                spelling.push_str(&format!("`--{long}`"));
            } else if argument.is_positional() {
                // The same spelling the usage line above uses. `get_id` and the value name can
                // differ — `runtime install`'s first positional is `kind` and is shown `<RUNTIME>` —
                // and a table disagreeing with the line above it describes a command nobody typed.
                spelling.push_str(&format!("`<{}>`", positional(argument)));
            }
            // A switch carries a value name too — `clap` derives one from the field — and printing
            // it would describe `--no-wait` as taking an argument it refuses. The **action** is what
            // tells the two apart; `get_num_args` answers `None` for both.
            if let Some(names) = argument.get_value_names()
                && !argument.is_positional()
                && argument.get_action().takes_values()
            {
                let names: Vec<&str> = names.iter().map(clap::builder::Str::as_str).collect();
                spelling.push_str(&format!(" `<{}>`", names.join("> <")));
            }

            // One line per row, because a Markdown table cell cannot hold a newline. The long help
            // is preferred over the short one: it carries the *why*, which is the half of a flag's
            // description that a reference is for.
            let about = argument
                .get_long_help()
                .or_else(|| argument.get_help())
                .map(|help| {
                    help.to_string()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            format!("| {spelling} | {about} |")
        })
        .collect();

    if rows.is_empty() {
        return String::new();
    }

    let mut table = String::from("| Flag | What it does |\n| --- | --- |\n");
    for row in rows {
        table.push_str(&row);
        table.push('\n');
    }
    table.push('\n');
    table
}

/// The three flags declared `global = true` on the root command.
///
/// `clap` copies a global argument into every subcommand, so without this every table below would
/// open with the same three rows.
fn is_global(argument: &clap::Arg) -> bool {
    matches!(argument.get_id().as_str(), "home" | "json" | "no_autostart")
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
