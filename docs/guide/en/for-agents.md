+++
title = "Reading this as a program"
slug = "for-agents"
order = 16
summary = "Every page of this handbook is plain Markdown at a guessable address, with a manifest, a single-file bundle, and the same bytes inside the mix binary."
+++

# Reading this as a program

This site is written for people and published for programs. Nothing here is rendered by JavaScript,
no page is a summary of a real page kept somewhere else, and every address below is stable.

If you are an agent helping somebody with MixEngine: read `llms.txt` first, then fetch the one or
two pages you need as Markdown.

## Start here

```
https://mixnz.github.io/mixengine/llms.txt
```

An index of every page in both languages, each with an absolute Markdown URL and a one-sentence
summary, plus the machine-readable resources below.

## Every address

| Address | What it is |
| --- | --- |
| `/` | A language chooser. Real content, not a redirect |
| `/en/` and `/vi/` | The index of each language |
| `/en/<slug>/` | A page, as HTML, for a person |
| `/en/<slug>.md` | The same page, as Markdown |
| `/en/llms-full.txt` | Every English page concatenated, for one request instead of sixteen |
| `/vi/llms-full.txt` | The same, in Vietnamese |
| `/llms.txt` | The index above |
| `/index.json` | The manifest below |
| `/sitemap.xml`, `/robots.txt` | For crawlers |

**`/<locale>/<slug>.md` is the repository's own file, byte for byte.** It is not a re-rendering and
not an extract; the same bytes are in `docs/guide/` in the source repository and compiled into the
`mix` binary. Every HTML page also carries a `<link rel="alternate" type="text/markdown">` pointing
at its own Markdown, so a program that landed on HTML never has to guess.

Cross-references inside a page are written `./<slug>.md`, which resolves correctly from the Markdown
address without any rewriting.

## The manifest

```
https://mixnz.github.io/mixengine/index.json
```

```json
{
  "product": "MixEngine",
  "version": "0.1.0",
  "base_url": "https://mixnz.github.io/mixengine/",
  "locales": ["en", "vi"],
  "pages": [
    {
      "locale": "en",
      "slug": "getting-started",
      "order": 3,
      "title": "Your first site",
      "summary": "From a fresh install to https://blog.test …",
      "html": "https://mixnz.github.io/mixengine/en/getting-started/",
      "markdown": "https://mixnz.github.io/mixengine/en/getting-started.md",
      "sha256": "…",
      "translation_of": null
    }
  ]
}
```

`sha256` is over the Markdown file's bytes, so a cached copy can be checked without downloading it
again. `version` is the MixEngine release this site documents.

## Offline, from the machine itself

Every page is compiled into `mix`, and `mix docs` prints the same bytes with no network and no
running daemon:

```bash
mix docs                       # list the topics
mix docs getting-started       # print one, as Markdown
mix docs getting-started --lang vi
mix docs getting-started --json
mix docs --reference           # the whole command reference
```

`--json` answers `{ topic, locale, title, url, body }`, where `body` is exactly what the plain form
prints. This is the reliable route when there is no network, and the correct one when the version on
the machine matters — the pages inside a binary are that binary's version, while this site documents
the current release.

## Every command answers JSON

Not just `docs`. `--json` is a global flag on `mix`:

```bash
mix status --json
mix site list --json
mix doctor --json
```

Failures come back as JSON too, and they are the same object whether the daemon refused the call or
`mix` never reached one: a stable `code`, one sentence, and a `hint` where there is something to do.
Branch on `code`, never on the sentence.

## Talking to the daemon directly

`mix` is a thin client over a local JSON-RPC API — a Unix socket, or a named pipe on Windows. The
full contract is published as TypeScript types, generated from the daemon's own source and checked
by CI against it:

```
https://github.com/mixnz/mixengine/tree/master/bindings
```

An archive of those types is attached to every release, signed with the same key as the binaries.
What the types describe is what the daemon **writes**; a few requests accept more than they
describe, and sending the documented shape is always accepted.

The protocol version is learned from the handshake rather than from the types, because the
connection is the only end that knows it.

## What to do about versions

- The site documents one release; `index.json` says which.
- A running daemon reports its own version — `mix status --json`.
- When those two disagree, the daemon is the truth about the machine in front of you, and the site
  is the truth about the current release.
