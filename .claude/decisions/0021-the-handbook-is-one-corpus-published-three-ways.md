# 0021. The handbook is one Markdown corpus published three ways, and help is not an API method

**Status**: Accepted
**Date**: 2026-09-05

## Context

Roadmap task **T90** asks for a user documentation site and in-app help, in English and Vietnamese,
hosted at `mixnz.github.io/mixengine` — and adds a requirement most documentation tasks do not carry:
*"content must be structured so an AI agent can easily fetch and understand it (e.g. plain Markdown
pages, predictable URLs/paths, no JS-only rendering of the actual text) — not just human-readable
HTML."*

That second sentence is not a rendering preference. It decides where the content lives, what a URL
means, and — as it turned out — what `mix` prints when it is asked for help.

Two forces met here. The first is this repository's standing objection to a decision told twice: two
tellings of one thing are two places for it to drift, and a documentation site plus a separately
written `--help` corpus is exactly that shape. The second is
[`.claude/features/client-surface.md`](../features/client-surface.md), which under *Left to the
client* puts localisation among the things that belong to whoever builds a client rather than to the
API — a line written long before anybody asked where a Vietnamese help page should live.

There was a third force, and it is the one that settles the argument rather than balancing it: the
page somebody most often needs is the one that explains why nothing starts.

## Decision

**One Markdown corpus, published three ways, with no second telling anywhere.**

`docs/guide/{en,vi}/*.md` is the document. From it, and only from it:

1. a static site at `https://mixnz.github.io/mixengine/`, HTML for a person;
2. the **same file, byte for byte**, served at `/<locale>/<slug>.md`, with `llms.txt`,
   `llms-full.txt` and `index.json` beside it for a program; and
3. the same bytes again compiled into `mix`, which `mix docs <topic>` prints.

**Help is not an API method.** There is no `help.get`, and `mix docs` is answered before a client is
constructed — no home is resolved and no socket is opened.

**The published Markdown is what a graphical client uses.** Those URLs are stable for that reason,
and `client-surface.md` says so rather than leaving the next client author to ask.

## Consequences

**What becomes easy.** A person, a program and an offline terminal receive the same document, so
none of them has to be told which is authoritative. A page cannot describe a `mix` that does not
exist, because the command reference is generated from the binary's own `clap` tree and CI diffs it.
`mix docs install` answers on a machine where the daemon will not start, which is when it is most
wanted.

**What we accept.** A graphical client gets no offline help from the daemon; it fetches the published
Markdown, or ships its own copy. That is the direct cost of `client-surface.md`'s rule, and it is
paid deliberately rather than discovered.

`mix docs` prints raw Markdown, with no colour and no renderer, which is `render.rs`' standing rule
and here also the point: a rendering would be a second telling of a document this crate does not own.
The corpus is written to read as plain text because of it.

Two copies of the corpus exist at run time — one in `mix`, one on the site — and they can differ by a
release. Neither is wrong: the binary's pages are that binary's version, the site's are the current
release, and `for-agents.md` says which to believe when they disagree.

**What becomes harder.** Every English page edit obliges a look at its Vietnamese counterpart, held
by a SHA-256 in the translation's front matter. That check knows only that somebody looked; no
machine here can check that a translation is right.

## Alternatives considered

**`help.list` / `help.get` over the JSON-RPC API.** Rejected twice over. It would make `mixengined`
the owner of a client's localisation policy, which `client-surface.md` had already assigned
elsewhere; and it would make the command fail in precisely the situation it exists for.

**A site built from one source and `--help` written separately.** The default shape, and the one this
ADR exists to refuse. Two tellings drift, and the drift is invisible until a user reads the wrong
one.

**A static-site generator (mdBook, Zola, Hugo).** Each renders HTML well and none of them makes the
raw Markdown a first-class published artifact at a guessable address; the second half of T90's
sentence is the whole reason this task is shaped as it is. A generator of our own is ~300 lines in
an example whose Markdown renderer is a dev-dependency, so nothing of it reaches `mix`.

**A terminal renderer for `mix docs`.** Rejected by `render.rs`' existing rule — no colour, and no
dependency for one — and, once written down, better refused than granted: printing the source is what
makes the terminal and the site the same document.

**Committing the generated HTML, the way `bindings/` is committed.** `bindings/` is source code
another repository compiles; HTML is what a browser receives at a URL. Committing thousands of
generated lines nobody reads in a diff buys nothing that `packaging/docs.sh --check` does not.

**Detecting the operating system's language.** Deferred rather than refused. Doing it properly means
a `mixengine-platform` trait method — Windows sets no `LANG` — with three implementations and a
verification round on three operating systems, for a default. What ships instead is `--lang`,
`MIXENGINE_LANG`, the POSIX variables, and one line of Vietnamese at the foot of every English page
naming the command that shows the Vietnamese one.
