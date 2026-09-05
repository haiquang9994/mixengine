# T90 — The documentation site (design)

Roadmap task **T90**, phase 9: *"User documentation site + in-app help; English and Vietnamese.
Hosted at `mixnz.github.io/mixengine`. Content must be structured so an AI agent can easily fetch
and understand it (e.g. plain Markdown pages, predictable URLs/paths, no JS-only rendering of the
actual text) — not just human-readable HTML."*

Everything written down in this repository so far is written for the people who build MixEngine.
`.claude/` is architecture, features, standards, decisions and a roadmap; `docs/superpowers/specs/`
is one design per task. There is no page anywhere that tells somebody who has just installed
MixEngine how to serve a site out of a directory, and `mix --help` is a list of twenty nouns rather
than an answer to that question.

The sentence has a second half that is unusual and is the reason this task is shaped the way it is:
the documentation must be **fetchable and understandable by a program**, not only readable by a
person. That is not a rendering preference. It decides where the content lives, what a URL means,
and — as it turns out — what `mix` prints when it is asked for help.

## Goal

Three readers, one corpus.

1. **A person** opens `https://mixnz.github.io/mixengine/` and finds a handbook in English or
   Vietnamese: how to install MixEngine, how to get a first site answering on `https://blog.test`,
   what each subsystem is for, and what to do when something is wrong.
2. **A program** — an agent helping that person — fetches `…/llms.txt`, discovers every page as an
   absolute URL, and reads each one as plain Markdown at a path it could have guessed. Nothing it
   needs is behind JavaScript, and nothing it reads is a summary of the real text.
3. **The same person, offline, with a daemon that will not start** types `mix docs install` and gets
   the same bytes the site serves.

Nothing about the daemon changes. This task adds no RPC method, no field and no behaviour to
`mixengined`.

## Measured, not assumed

Read on 2026-09-05 out of this tree. Every one of these decided something below.

1. **`mix` has 20 top-level commands and 18 subcommand groups.** `awk` over `enum Command`, `grep`
   over `^enum .*Command`. A command reference of that size is generated or it is wrong within a
   week — D10.
2. **No Markdown crate and no YAML crate is in this workspace.** `grep` for `pulldown`, `comrak`,
   `markdown`, `serde_yaml` over every `Cargo.toml` returns nothing. So the corpus format and the
   site generator each cost a dependency decision rather than a `use` line — D5 and D11.
3. **`toml`, `minijinja` and `serde_json` are already core dependencies**
   ([../../../.claude/standards/rust.md](../../../.claude/standards/rust.md)). Front matter and the
   page template therefore need no new crate at all.
4. **`deny.toml` line 58 sets `multiple-versions = "deny"`.** A Markdown crate that drags a second
   copy of something already here fails `lint`. Verified against the real lock file before the
   dependency is added, not after — D11.
5. **`.claude/features/client-surface.md`, under *Left to the client*, says localisation belongs to
   whoever builds the client.** That is this repository's own answer to the question this task would
   otherwise have to invent: help does not become an RPC — D2.
6. **`crates/mixengine-cli/src/render.rs` opens with "No colour, and no dependency for one."** The
   reason it gives is that nearly every line `mix` prints ends up pasted into a bug report. So there
   is no terminal renderer here either — and the thing that replaces it is better than what it
   replaces, which is D4.
7. **`docs/` holds exactly one directory, `superpowers/`, and this repository has no root
   `README.md`.** So `docs/guide/` is unoccupied, and the front door a person and a crawler both hit
   first is empty — D14.
8. **`.gitignore` ignores exactly one thing under `docs/`: `docs/superpowers/plans/`.** A new
   directory there is tracked by default, with no edit needed.
9. **T89's rule: a suite that needs nothing downloaded and no privilege runs in `test` on all three
   runners with no edit to the workflow.** Every corpus invariant in this design is a `cargo test`,
   so the `docs` job exists for the one thing that is not — D15.
10. **T56's rule: generated, committed, and checked by regenerating into a temporary directory and
    diffing.** The generated command reference is that pattern again, and reuses the shape of
    `packaging/bindings.sh` deliberately.

## Scope

**In.** A Markdown corpus at `docs/guide/{en,vi}/`, 16 pages per locale. A `mixengine-docs` crate
that embeds it at compile time. A `mix docs` command. A generated command reference. A static site
generator producing HTML, raw Markdown, `llms.txt`, `llms-full.txt`, `index.json`, `sitemap.xml` and
`robots.txt`. `packaging/docs.sh`. A `docs` job in `ci.yml` and a `pages.yml` workflow that
publishes to GitHub Pages. A root `README.md`. The documents that describe all of it, and an ADR.

**Out.** Search (it needs JavaScript, and the corpus is 16 pages). Screenshots (the product is a
command line). Versioned documentation paths — one product version exists. Checking external links
(it needs the network in a job that otherwise does not). Any third language. Automatic translation
of anything. Detecting the operating system's language — the reason is in *What it leaves*.

## The shape of the answer

```
docs/guide/en/*.md   +   docs/guide/vi/*.md          ← one source, plain Markdown, 16 pages each
        │
        ├── crates/mixengine-docs/build.rs
        │      walks the two directories, writes include_str! for each into OUT_DIR
        │            ▼
        │      mixengine-docs (Corpus, Page, Locale)  ← no dependencies at all
        │            ▼
        │      mix docs [<topic>] [--lang vi] [--json] [--reference]   ← needs no daemon, no home
        │
        └── cargo run -p mixengine-docs --example build-site -- <out>
               pulldown-cmark + minijinja, both DEV-dependencies: no renderer reaches `mix`
                     ▼
               target/site/
                 index.html            the language chooser
                 en/index.html         …and one directory per page
                 en/<slug>/index.html  the page, for a person
                 en/<slug>.md          the page, byte-identical to docs/guide/en/<slug>.md
                 en/llms-full.txt      every English page in one file
                 vi/…                  the same again
                 llms.txt              every page of both locales, absolute URLs
                 index.json            the machine manifest
                 sitemap.xml  robots.txt  style.css  .nojekyll
                     │
                     ├─ ci.yml job `docs`:  bash packaging/docs.sh --check
                     └─ pages.yml on master: build → actions/deploy-pages
```

## Decisions

### D1 — One corpus, three publications, and none of them is a second telling

The Markdown file in `docs/guide/` is the document. The HTML page is a rendering of it; the `.md`
the site serves is **byte-identical** to it; the copy compiled into `mix` is the same bytes again.

This is the invariant the whole task hangs on, and it is what makes the second half of the roadmap
sentence true rather than aspirational: an agent asking `mix docs sites` and an agent fetching
`https://mixnz.github.io/mixengine/en/sites.md` receive the same document, so neither has to be told
which one is authoritative.

The alternative — a site built from one source and a `--help` text written separately — is two
tellings of one thing, which
[../../../.claude/standards/rust.md](../../../.claude/standards/rust.md) already calls out as two
places for a decision to drift.

### D2 — Help is not an RPC method, and `mix` answers it with no daemon

`.claude/features/client-surface.md` says localisation is left to whoever builds the client. A
`help.get` method carrying Vietnamese prose would be `mixengined` deciding a client's localisation
policy, which is the line that document draws.

The second reason is stronger and is about failure. Help is most wanted when something is broken,
and *"the daemon will not start"* is exactly that case. A `mix docs install` that first has to reach
a daemon is a command that stops working at the moment it is needed. So `Command::Docs` is answered
before any client is constructed, before a home is resolved, and it never touches the socket.

**This is not business logic in a client.** The rule in `CLAUDE.md` exists so a client does not
*decide* anything the daemon should decide. Selecting a compiled-in string by key is not a decision;
there is no state, no policy and no second possible answer.

**What a graphical client does instead** is fetch the published `.md`, which is the reason those
URLs exist and are stable. `client-surface.md` gains a line saying so, so the next person to ask
this question finds the answer rather than the argument.

### D3 — The command is `mix docs`, not `mix help`

`clap` generates a `help` subcommand by default (`disable_help_subcommand` exists precisely to turn
it off). Defining our own would take `mix help site` away from the person who typed it expecting
`mix site --help`.

`mix docs` is a noun of its own, is declared beside `daemon` in the command list, and leaves clap's
help exactly as it is. A claim about another crate's behaviour is a test rather than a sentence —
`crates/mixengine-cli/tests/docs.rs` asserts `mix help site` still prints clap's help for `site`.

### D4 — `mix docs` prints Markdown, verbatim

`render.rs` forbids colour and forbids a dependency for one. That removes a terminal renderer from
the options, and what is left turns out to be the better answer rather than the resigned one:

- the bytes are identical to the published `.md`, which is D1's invariant reaching the terminal;
- Markdown is a format designed to be read as plain text, and the corpus style rules below keep it
  that way (hard-wrapped prose, no HTML, no reference links, no nested tables);
- there is no rendering code, so there is nothing that can render differently from the site.

`mix docs <topic>` prints the page body verbatim — the `+++` front matter is stripped, because it is
metadata about the file and not part of the document, and the body already opens with the title as
an H1. A footer follows: the page's URL, and on an English page the one Vietnamese line of D17.
`mix docs --json` answers `{ topic, locale, title, url, body }`, the body being that same string.

**The body opening with `# <title>` is a corpus rule and not an accident.** It is what makes the
published `.md` a complete document on github.com and in a terminal, rather than one whose title
lives only in metadata — and it is what lets this command print the body and nothing else.

### D5 — Front matter is TOML between `+++`

YAML front matter would need `serde_yaml`, a crate its own author has archived and which
`deny.toml`'s `[advisories]` section is there to catch. `toml` is already the workspace's format for
every file a person writes by hand.

The cost is cosmetic and worth naming: github.com renders a `---` YAML block as a table and shows a
`+++` block as text. Readers of the corpus on GitHub therefore see four lines of metadata at the top
of each page. A deprecated dependency is the worse of the two.

```toml
+++
title = "Your first site"
slug = "getting-started"
order = 3
summary = "From a fresh install to https://blog.test, in about five minutes."
+++
```

and, in every Vietnamese page, two more:

```toml
translation_of = "en/getting-started.md"
source_sha256 = "…64 hex…"
```

### D6 — Internal links are always `./<slug>.md`, and only HTML rewrites them

One link syntax has to be correct in four places at once: the file in the repository, the same file
rendered by github.com, the `.md` the site serves at `/en/<slug>.md`, and the HTML page at
`/en/<slug>/`.

`./other.md` is correct in the first three by construction. It is wrong in the fourth only because
the HTML page lives one directory deeper, so the generator — and nothing else — rewrites
`./other.md` to `../other/` and `./other.md#anchor` to `../other/#anchor`.

A corpus test resolves every relative link and fails on a target that does not exist, which is what
makes a renamed page a red test rather than a 404 somebody finds later.

### D7 — A Vietnamese page carries the hash of the English page it was translated from

The certain failure mode of every bilingual documentation set is that one language quietly stops
describing the product. `source_sha256` is the whole of the mechanism: a test hashes
`docs/guide/en/<slug>.md` and compares. Editing the English page without revisiting the Vietnamese
one is a red test naming both files and the command that clears it.

**Say plainly what it does not do.** It checks that the Vietnamese page was *looked at* after the
English one changed. It cannot check that the translation is correct, and no machine in this
repository can. A typo fix in English costs a look at the Vietnamese page; that is the price, and it
is deliberate — a threshold ("only if more than N lines changed") would be a rule about diffs
pretending to be a rule about meaning.

`bash packaging/docs.sh --restamp` rewrites the hashes after the translation is done. It is named
for what it does rather than `--bless`, because a person running it without translating should feel
that they are stamping something.

### D8 — The generated site is not committed

`bindings/` is committed because it is *source code that another repository compiles*. HTML is not
that: it is what a browser receives at a URL, it is thousands of generated lines, and no reader of
this repository ever wants it in a diff.

So what is committed is the Markdown corpus and one generated file, `docs/guide/en/cli.md` (D10).
`packaging/docs.sh --check` builds the site into a temporary directory and asserts its structure
rather than diffing it against a committed copy; the file it *does* diff is the command reference.

### D9 — No JavaScript, and the generator enforces it rather than trusting the author

The roadmap sentence asks for no JS-only rendering of the text. This design goes further: the site
contains no JavaScript at all, and raw HTML passthrough is **disabled** in the Markdown parser — so
a `<script>` that ever appears in a page is escaped into visible text rather than executed.

Consequences, all accepted: no search box, no copy-to-clipboard button, no theme toggle beyond
`prefers-color-scheme`. The stylesheet is one self-contained file with a system font stack and no
third-party request of any kind, which is also the answer to a privacy question nobody should have
to ask of a documentation site.

### D10 — The command reference is generated by `mix` itself

Twenty commands and eighteen subcommand groups are not documented by hand. `mix docs --reference`
walks its own `clap::Command` tree and prints the reference as Markdown, front matter included, so
the file is complete rather than assembled by a script.

```bash
cargo run -q -p mixengine-cli -- docs --reference > docs/guide/en/cli.md
```

`packaging/docs.sh --check` regenerates it into a temporary file and diffs, exactly as
`bindings.sh --check` does for the contract. Forgetting to regenerate is a red job and not a
reference that describes last month's flags.

**The circularity is real and is benign.** `cli.md` is generated by a binary that embeds `cli.md`.
It converges in one pass because the reference generator reads only the `clap` tree and never the
corpus — and a test asserts exactly that, because the day somebody makes `mix docs`' help list the
topics dynamically is the day this stops converging.

**The reference is English only.** It is generated from `clap` help strings that are English in the
source; a hand-translated copy would be a second source of truth for the same twenty commands, drifting
silently, and `mix --help` itself would still answer in English. `docs/guide/vi/cli.md` is a real
Vietnamese page that says this and links across — which keeps the parity rule (D12) exceptionless.

### D11 — `pulldown-cmark` is a dev-dependency, and the site generator is an example

The renderer must not reach a shipped binary. Making the generator
`crates/mixengine-docs/examples/build-site.rs` puts `pulldown-cmark` and `minijinja` in
`[dev-dependencies]`, where `cargo` keeps them out of `mix` by construction rather than by review.

`mixengine-docs` itself has **no runtime dependencies at all**. Its `build.rs` walks the two
directories, parses each file's front matter with `toml` — a *build*-dependency, so nothing of it is
linked into anything — and writes into `OUT_DIR` one `include_str!` per file together with the
metadata as plain constants. The library at run time therefore parses nothing: `Page::source()` is
the whole file, `Page::body()` is the same string from a byte offset the build script computed, and
the title, slug, order and summary are already `&'static str`.

Two consequences worth naming. **The site generator reads the embedded corpus rather than the
directory**, which is what makes `/en/<slug>.md` byte-identical to the repository file by
construction instead of by a copy that could be filtered on the way. And **a malformed page is a
build error naming the file**, not a test failure — front matter that does not parse is a syntax
error in a document, and it should behave like one.

A `cargo:rerun-if-changed` on each directory means a page added, renamed or deleted is picked up with
nothing to remember — which is why this is a build script rather than a hand-maintained list or the
`include_dir` crate.

Before the dependency lands, `cargo deny check bans` must be clean with it in the lock file
(measured fact 4). If `pulldown-cmark` duplicates a crate already in the tree, the entry goes in
`deny.toml` naming the edge, per `rust.md` — or the dependency is reconsidered.

### D12 — Every page exists in both locales, and the tests say so

`en/` and `vi/` hold the same 16 slugs. No exceptions, including `cli` (D10). A page that exists in
one language only would make "the site is available in Vietnamese" true in a way that is false for
whoever needs the missing page.

### D13 — The generator's output is a pure function of the corpus and the workspace version

No timestamps, no git SHA, no build host. Two runs over the same tree produce identical bytes.
Without that, `--check` means nothing and a publish cannot be compared with anything.

### D14 — URLs, and what each one is for

Base: `https://mixnz.github.io/mixengine/`.

| Path | What it is |
| --- | --- |
| `/` | The language chooser. Short, real content, links to both locales and to `llms.txt` |
| `/en/` · `/vi/` | The locale index: every page with its summary |
| `/en/<slug>/` | The page, as HTML, for a person |
| `/en/<slug>.md` | The page, as Markdown, byte-identical to the repository file |
| `/en/llms-full.txt` | Every English page concatenated, for one fetch instead of sixteen |
| `/llms.txt` | The index a program reads first: both locales, absolute URLs, one summary each |
| `/index.json` | The manifest: version, locales, and per page the title, summary, both URLs and the SHA-256 of the Markdown |
| `/sitemap.xml` · `/robots.txt` | For crawlers. `robots.txt` names the sitemap and `llms.txt` |
| `/.nojekyll` | So GitHub Pages serves what was uploaded, including files starting with `_` |

`/` is a page rather than a redirect: a redirect costs a hop and gives English two URLs. Every HTML
page carries `<link rel="alternate" type="text/markdown">` pointing at its own `.md`, a
`<link rel="canonical">`, the right `<html lang>`, a `<meta name="description">` from the summary,
and a visible link to the Markdown — so a program that landed on HTML finds the source without
guessing, and so does a person who wants to copy it.

**One thing here is not measurable from this tree**: the `Content-Type` GitHub Pages serves a `.md`
file with. Whether it is `text/markdown` or `text/plain` changes nothing for a program, and the
human path is the HTML URL either way; it is written down as an assumption rather than as a fact,
and the first publish confirms it.

### D15 — Corpus invariants are `cargo test`; the `docs` job exists for the site

T89's rule. Parity, front-matter validity, link resolution, translation freshness, slug/filename
agreement and unique ordering all need nothing downloaded and no privilege, so they run in `test` on
all three runners with no workflow edit — and they run on a developer's machine in milliseconds,
which is where they are most useful.

What is left over needs a job: building the site (which compiles `pulldown-cmark`), and diffing the
committed command reference against what `mix` prints. That is `docs`, one ubuntu job running
`bash packaging/docs.sh --check`, alongside `bindings` and for the same reason — a red job that names
what broke without anybody opening a log. The table in
[../../../.claude/operations/build-and-release.md](../../../.claude/operations/build-and-release.md)
becomes seven jobs.

### D16 — Publishing is a workflow of its own, on every push to `master`

Deployment needs `pages: write` and `id-token: write` and a `github-pages` environment, which is not
what `ci.yml`'s `permissions: contents: read` is or should become. So `pages.yml`: checkout, build,
`actions/upload-pages-artifact`, `actions/deploy-pages`.

**No `paths:` filter.** Filtering to `docs/guide/**` would leave the site claiming the previous
version after a release bumped `Cargo.toml`, and the failure would be silent. Two minutes on every
master push buys a site that is never stale.

`concurrency: pages` with `cancel-in-progress: false` — a cancelled deploy is the one state a
publishing pipeline has no answer for, which is the argument `ci.yml` already makes about a release
run.

### D17 — Language selection, and the one line that carries it on Windows

`--lang` › `MIXENGINE_LANG` › `LC_ALL` › `LC_MESSAGES` › `LANG` › English. A value like `vi_VN.UTF-8`
matches on its first two letters.

Windows sets none of those. Rather than add an OS call — which would have to be a
`mixengine-platform` trait method, and is *What it leaves* below — every English page printed by
`mix docs` ends with one line **in Vietnamese** naming the command that shows the Vietnamese one. A
Vietnamese speaker on any operating system sees an instruction they can read on the first page they
open, and it costs one line rather than a trait, three implementations and a round of platform
verification.

## The corpus

Sixteen pages per locale, in this order:

| # | Slug | What it answers |
| --- | --- | --- |
| 1 | `index` | What MixEngine is, and what is on this site |
| 2 | `install` | Installing it, per operating system, including the unsigned-binary warnings |
| 3 | `getting-started` | A first project, a first site, a green padlock |
| 4 | `projects-and-sites` | The two nouns, what each owns, and `mixengine.toml` |
| 5 | `runtimes` | Several PHP/Node/Python/Ruby versions, and how a directory chooses one |
| 6 | `services` | Web server, databases and caches; making a database and opening it |
| 7 | `domains-and-https` | `.test` names, the hosts file, the internal DNS, the certificate authority |
| 8 | `sharing` | Reaching a site from a phone on the same Wi-Fi |
| 9 | `blueprints` | Capturing a setup and reproducing it elsewhere |
| 10 | `extensions` | What an extension may do, and what it must ask for |
| 11 | `permissions` | Every administrator prompt MixEngine raises, and why |
| 12 | `updating` | `mix self-update`, and what it deliberately does not update |
| 13 | `uninstalling` | Taking it off the machine and verifying nothing was left |
| 14 | `troubleshooting` | `mix doctor`, logs, the diagnostics bundle |
| 15 | `cli` | The generated command reference (D10) |
| 16 | `for-agents` | Reading this site as a program, and `mix --json` |

**Style rules, held by tests where they can be.** Prose hard-wrapped at 100 columns. No raw HTML. No
reference-style links. Every command shown in a fenced block tagged `bash`. Every page opens with one
paragraph that could stand alone as the summary. Cross-references as `./<slug>.md`.

**Where the content comes from.** Each page is written against the feature document that owns its
subject in `.claude/features/`, and against the behaviour of the commands themselves. Where the two
disagree, the commands win and the disagreement is worth a line in the roadmap.

## `packaging/docs.sh`

Four modes, shaped after `packaging/bindings.sh` so that a reader of one already knows the other.

```bash
bash packaging/docs.sh              # build the site into target/site/
bash packaging/docs.sh --reference  # regenerate docs/guide/en/cli.md from `mix`
bash packaging/docs.sh --restamp    # rewrite every Vietnamese page's source_sha256 (D7)
bash packaging/docs.sh --check      # build into a temp dir, validate it, diff the reference; writes nothing
```

`--check` writes nothing anywhere for the reason `bindings.sh` gives: a red job should be a message
and not also a dirty checkout, and it has to answer the same way on a machine that unpacked a
tarball rather than cloned one.

## Testing

| Suite | Holds |
| --- | --- |
| `crates/mixengine-docs/tests/corpus.rs` | `slug` equals the filename; `order` is unique within a locale; both locales hold the same 16 slugs; every `./x.md` link resolves; every Vietnamese page's `source_sha256` matches its English source; no page contains raw HTML or a reference-style link; no line outside a fenced block or a table exceeds 100 columns. (Front matter parsing is D11's build script, so a malformed page never reaches a test.) |
| `crates/mixengine-docs/tests/site.rs` | The generator, run into a `TempDir`: every expected path exists, `/en/<slug>.md` is byte-identical to the corpus file, `llms.txt` lists every page, `index.json` parses and its hashes match, no output file contains `<script`, and two runs produce identical bytes (D13) |
| `crates/mixengine-cli/tests/docs.rs` | `mix docs` with no daemon and no home; a known topic; an unknown topic naming the ones that exist; `--lang vi`; `--json`; `MIXENGINE_LANG` and `LANG`; `mix help site` still prints clap's help (D3); `--reference` output does not vary with the corpus (D10) |
| `packaging/docs.sh --check` | The site builds, and the committed `cli.md` is what `mix` prints |

## Documents this changes

- **`README.md`** — new, and this repository's first. What MixEngine is in a paragraph, where the
  handbook is, where `llms.txt` and `bindings/` are, and where a contributor goes instead
  (`.claude/`). It is the page a person and a crawler both meet first, and it has been empty.
- **`.claude/decisions/0021-…`** — the ADR for D1 and D2 together: the handbook is one Markdown
  corpus published three ways, and help is not an API method. Both are cross-cutting, and D2 in
  particular is a question the next client author will ask.
- **`.claude/operations/build-and-release.md`** — the job table becomes seven, and gains `docs`; a
  paragraph on `pages.yml` and on the one setting a person owns.
- **`.claude/standards/rust.md`** — one row in the core-dependency table: Markdown rendering,
  `pulldown-cmark`, dev-dependency only.
- **`.claude/features/client-surface.md`** — one paragraph under *Left to the client*: where a
  graphical client gets help text, and why it is a URL rather than a method.
- **`.claude/README.md`** — `docs/guide/` named in the map, so the two documentation trees are told
  apart on the page whose job is telling folders apart.
- **`.claude/roadmap/phase-9-ship.md`** — T90 ticked, with what the task changed about its own
  sentence and what it left.

## Human steps this task cannot take

1. **GitHub Pages must be enabled for `mixnz/mixengine` with the source set to GitHub Actions.** The
   workflow passes `enablement: true` to `actions/configure-pages`, which turns it on through the API
   where the token allows; where it does not, the first run on `master` fails with a message saying
   exactly which setting to change. It fails loudly on purpose — a deploy that skipped itself quietly
   would leave a green tick over a site nobody published.
2. **The first publish confirms D14's one assumption**, the `Content-Type` of a `.md` file.

## What it leaves

- **Nothing detects the operating system's language.** Doing it properly means a
  `mixengine-platform` trait method — Windows sets no `LANG`, and `GetUserDefaultLocaleName` is an OS
  call, which `CLAUDE.md` allows in exactly one crate. That is a platform change with three
  implementations and a verification round on three operating systems, for a default; D17's one
  Vietnamese line delivers the same reachability now. The trait method belongs in the roadmap beside
  the other platform work, not smuggled into a documentation task.
- **No external link is checked.** It needs the network in a job that otherwise has none, and a
  documentation build that fails because somebody else's site was down is a build that gets ignored.
- **Nothing verifies a translation is correct** — only that it was revisited (D7).
- **The site documents one version.** There is one, and a version switcher built before the second
  release is a guess about how the second release will differ.
- **`mix docs` has no pager.** Sixteen pages of prose through `less` is somebody's shell doing its
  job — `mix docs sites | less` — and a pager built into a command that must work when everything
  else is broken is a second thing that can be broken.
