# T81a — publishing the extension registry (design)

Roadmap task **T81a**, phase 8. T81 built everything that *reads* `extensions.json`: the signed
fetch, the cache, the rollback refusal, the per-entry skip, and the install that follows. It verifies
against a key its own tests mint — which is what proves the verification path rather than switching
it off — and it leaves one thing missing. **Nothing produces the document.** Until this task lands,
`mix extension available` asks a URL that answers 404, and there is nothing anybody can install.

This is the other half, on T79a's shape: a workflow in the packaging repository that renders every
manifest through the reader an installed MixEngine uses, proves the key it is about to sign with is
the key that build checks against, signs, publishes to the moved `index` tag, and then downloads
what it published to prove it verifies.

The sentence with teeth is D3, and it is T79a's D3 one notch better: over there the check scrapes a
constant out of a source file with a regex, and has to carry a branch for *"did it move?"*. Here the
generator is **compiled from the checkout being published**, so it holds the constant instead of
looking for it.

## Goal

`extensions.json` and `extensions.json.minisig` at
`https://github.com/mixnz/mixengine-packages/releases/download/index/extensions.json` — the URL
`registry::DEFAULT_URL` already names — signed with the index key, regenerated from
`data/extensions/*.toml` in the packaging repository.

The success sentence is that `mix extension available` stops being a 404 and starts being an answer,
including on the day the answer is *"no extensions yet"*. T82 then adds a file to a directory rather
than building a pipeline under deadline.

## Scope

**In, this repository:** `extensions::registry::assemble` — the whole of the generation, so it can
be tested by ordinary tests; `crates/mixengine-core/examples/extensions_json.rs`, a thin command-line
shell over it; `impl FromStr for Timestamp` in `index::format`, because this is the first thing in
the workspace that *makes* a timestamp instead of reading one; the tests below; and the documentation
that stops being true the moment the document exists — the Registry section of
[features/extensions.md](../../../.claude/features/extensions.md) and the roadmap tick.

**In, `mixengine-packages`:** `data/extensions/` with its `README.md`;
`.github/workflows/publish-extensions.yml`; `.github/workflows/check-extensions.yml`;
`release/publish-extensions.sh`; a runbook entry in `release/README.md` and a numbered task in
`docs/roadmap.md`.

**Out:**

- **The first extension manifest.** Mailpit, phpMyAdmin and Adminer are T82's, and one of them needs
  T81b before it can be served at all. This task publishes an empty document (D7) and T82 fills it.
- **A key of its own.** T81's D1 settled it: an extension has the package index's blast radius
  exactly, so the index key signs this and there is no third secret, no third constant, no third
  half-finished rotation.
- **Anything that fetches an artifact.** The document carries `url` and `sha256` per target; checking
  them is the installer's, on the machine that downloads.

## Decisions

### D1 — The manifests live in the packaging repository

`data/extensions/<id>.toml`, beside `data/eol.json`, and not in this repository.

T79a read its manifests out of a `mixengine` checkout because the gallery **is** compiled into that
binary — publishing a copy would have made two galleries. Nothing of the sort holds here: no
extension manifest is compiled into MixEngine, and `manifest::read` is a *format*, not a roster. What
an extension manifest describes is a third-party artifact at a URL with a SHA-256 — which is what
that repository already exists to describe — and a Mailpit version bump has no business being an
application release.

So this repository owns the format and the reader; that one owns the roster and the key.

### D2 — The generator is an `example`, and the logic is not in it

`cargo run -p mixengine-core --example extensions_json` is what the workflow calls. Three surfaces
were possible and two were refused.

A subcommand of `mix` is not available at any price: `mix` depends on `mixengine-proto` and
`mixengine-platform` and **not** on `mixengine-core`, deliberately, and
[`workspace_layering.rs`](../../../crates/mixengine-proto/tests/workspace_layering.rs) is the test
that keeps it that way. A CI-only crate in the workspace would be a fourth member of the layout list
that ships nothing. An example is neither: it is already built by
`cargo clippy --workspace --all-targets` and by `cargo test --workspace --all-targets`, so it cannot
rot quietly, and it adds no member to anything.

**But an example is not a library**, so nothing outside it can call it, and a generator nothing can
call is a generator nothing can test. The work therefore lives in `registry::assemble`, and the
example is argument parsing, two file reads and one file write. That also keeps the document's shape
where the document's type is.

### D3 — The key chain, held rather than scraped

Before a manifest is read, `assemble` compares the `minisign.pub` it was handed against
`index::PUBLIC_KEY`. Disagreement is an error and nothing is generated.

`tools/blueprints.py` does the equivalent with a regex over `trust.rs`, and pays for it with a
failure mode — `no pub const PUBLIC_KEY to compare against — did it move?` — that exists only because
a Python script cannot hold a Rust constant. This one is compiled out of the same checkout the
workflow is publishing from, so the constant it compares against *is* the constant that build
compiles in. There is nothing to scrape and no branch for the scrape failing.

What this buys is the same thing it buys over there: a half-finished key rotation is a red run rather
than a document at a stable URL that no installed MixEngine will accept. A signature made with a key
nobody checks against is worse than no signature, because it looks published.

### D4 — Not a second reader, and not a second renderer

Every file goes through `manifest::read` — the same function a `--path` install calls — and every
entry is written with `manifest::to_value`, the same rendering the `manifest_json` column stores.
The generator has no opinion of its own about what a manifest may say.

It adds exactly one rule the reader cannot have, and it is about the *directory* rather than the
file: a file's stem must equal its `[extension] id`. The reader sees one file at a time and cannot
know that.

**One rule and not two.** "Two files may not claim one id" was the obvious second, and it is already
implied: a filesystem holds one `mailpit.toml`, so once every file is named after the id it declares,
a repeated id has nowhere to live. Writing the check anyway would be a branch no input can reach.

This is `blueprints.py`'s "not a second renderer" argued from the other side. Over there the reader
was out of reach and the check had to be reimplemented narrowly in Python; here it is in reach, so
nothing is reimplemented at all.

### D5 — The document is sorted, and read back before it is written

Entries are sorted by id, so two runs over the same directory differ in `generated_at` and nowhere
else. `index::format` says the order is the generator's and nothing depends on it — which is exactly
why the generator should pick a stable one rather than the filesystem's.

Then `assemble` calls `Registry::listing()` on what it just built and requires `unreadable == 0`.
D4 of T81 makes an unreadable entry survivable on a user's machine on purpose; it must not be
survivable here. A document published with an entry this build cannot read is a document generated by
a build older than its own inputs, and the honest place to fail is before the signature.

### D6 — `generated_at` comes from the shell

`Timestamp` parses `YYYY-MM-DDTHH:MM:SSZ` and nothing else, and this workspace has no date library —
`index::format` records why, and buying a civil calendar to produce a format we ourselves emit would
be a poor trade. So the workflow passes `date -u +%Y-%m-%dT%H:%M:%SZ` and Rust reads it.

`Timestamp::parse` is private today and reachable only through `Deserialize`; it gains
`impl FromStr`, because reaching a parser through `serde_json::from_value(json!(text))` is a
workaround written down in the one place it would be read as a pattern.

### D7 — An empty document is published now

`data/extensions/` holds a `README.md` and no manifest until T82, and the run publishes
`{ "schema": 1, "generated_at": …, "extensions": [] }`.

The alternative was a complete workflow nobody dispatches until there is something to lose. It was
refused for the reason dry runs are never quite enough: the parts that fail on a real system are the
secret, the tag, the asset URL and the download-and-verify, and none of them is exercised by a run
that stops before them. Publishing the empty document exercises all four while the cost of getting
them wrong is nothing.

The consequence for callers is small and good: `mix extension available` answers *"no extensions"*
rather than an index error, from the day this merges.

An empty directory is therefore legitimate, which is where `assemble` parts company with
`blueprints.py` — that one treats an empty gallery as evidence it was pointed at the wrong directory.

### D8 — Its own workflow, not a job in `publish-index.yml`

Both documents go to the moved `index` tag, which is the argument for one workflow. It is refused for
the reason T81's D1 wrote two documents in the first place: failure isolation.

`publish-index.yml` downloads every asset of every release and runs the cross-platform parity
comparison before it generates anything. Cutting the registry again would mean doing all of that, and
a red parity step would mean the registry cannot be published either — two independent things wired
together at the one point they were separated to avoid. Two `gh release upload --clobber` calls to
one tag do not collide: they name different files.

### D9 — Staleness is caught on push, not only on a clock

`check-blueprints.yml` runs weekly because its input lives in a repository its own `push` trigger
cannot see. This input is local, so `push: paths: data/extensions/**` catches an edited manifest
immediately — before it is merged, on the branch that edited it.

The clock still earns its place for the two things a push cannot see: `index::PUBLIC_KEY` rotating in
this repository, and the published document drifting from the tree because a run was never dispatched.
Wednesday, a day off `check-eol`'s Monday and `check-blueprints`' Tuesday, so three unrelated
failures never arrive together and get read as one. The comparison is over the `extensions` array;
`generated_at` differs on every run and comparing it would report staleness that is not there.

The check therefore builds the generator too, against `mixengine` `master` — which is the point, for
the key half: the question it answers on Wednesday is whether the key this repository signs with is
still the key the current application checks against.

## Delivery

Two chains, in two repositories, on the branch `feat/t81a-registry-publication` in each.

Here: the branch, CI, PR against `master`, squash. There: the same branch and a PR, unlike T79a's
straight-to-`master` — this change adds a workflow that reads a directory that does not exist yet, and
the packaging repository has no gate that would catch it, so the review is the gate.

This repository lands first. The workflow cannot be dispatched against a `mixengine` ref whose
`--example extensions_json` does not exist.

## Testing

- **Here:** `registry::assemble` against a temporary directory — the empty case (D7), a fixture
  manifest read back through `Registry::listing`, a stem that disagrees with its id, two files
  claiming one id, and a `minisign.pub` that disagrees with `index::PUBLIC_KEY` (D3). Then the
  ordinary workspace gates: `clippy --all-targets`, `fmt`, `cargo test`, `cargo doc`.
- **There:** `release/publish-extensions.sh --dry`, the full rehearsal — it builds the generator
  against a real `mixengine` ref, checks the key chain, generates the document, signs nothing and
  publishes nothing, and uploads what a real run would have signed.
- **The real run** is the acceptance below and not a test, for T79a's reason: a signature is worth
  what the published file proves and nothing less.

## Risks

- **Build time in the packaging repository.** Nothing there compiles Rust today, and both workflows
  need the generator — D9's check as much as the publish. `mixengine-core` pulls `rcgen`,
  `minijinja`, `zip` and `tar`, so a cold run is minutes; `Swatinem/rust-cache` makes the rest short,
  and the check's cache is warm because it runs on a schedule rather than once a quarter. If that
  ever stops being true the check is the one to reconsider, not the publish.
- **Line endings.** What is signed is what git gives the ubuntu runner, which is LF, and no step
  rewrites a byte. `core.autocrlf` on a Windows machine cannot reach the published bytes.
- **The `index` tag not existing.** `publish-index.yml` creates it; this workflow creates it the same
  way if it is absent, so the order the two are first run in does not matter.
- **A rollback by regeneration.** `generated_at` is what makes one detectable, and a re-run always
  moves it forward, so a client never sees the document walk backwards. What a re-run *can* do is
  republish a roster an older packaging commit held — a manifest since removed, back at a stable URL
  under a fresh timestamp. Nothing in the client can see that, so the answer is the same one
  `publish-blueprints.yml` uses and `publish-index.yml` does not: the release notes name the commit
  each document was cut from, so a run is reproducible after the fact instead of being `master` at an
  unrecorded moment.

## Acceptance

- `release/publish-extensions.sh --dry` is green and its artifact holds `extensions.json` with an
  empty `extensions` array and a `generated_at` from that run.
- After a real run, `mix extension available` on a clean home answers with no extensions and no
  error — the signature verified, the document parsed, nothing listed.
- Editing a byte of the downloaded `extensions.json` and pointing a home at it reports the signature
  failure, not a parse failure: verify happens before parse.
- A run whose `minisign.pub` disagrees with `index::PUBLIC_KEY` fails before it signs (D3).
- A `data/extensions/` holding a manifest whose stem is not its id fails the check workflow on the
  push that added it (D4, D9).
- T81a is ticked in phase 8, and the packaging repository's roadmap carries the task under its own
  number.
