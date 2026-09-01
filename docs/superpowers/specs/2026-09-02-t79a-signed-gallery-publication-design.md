# T79a — publishing the gallery as signed files (design)

Roadmap task **T79a**, phase 8. T78a taught `blueprint.import` to check a detached minisign
signature against a compiled-in key, and minted the key that would sign. T79 then compiled the
gallery into the binary and trusted it without a signature check — which was right, and which
removed the only channel those signatures were for. This task restores the channel: the same six
manifests, published as files from the packaging repository with a `.minisig` beside each one, so a
blueprint somebody downloads and imports by hand lands **trusted** rather than untrusted for good.

The sentence with teeth is D3: the workflow proves that the key it signs with is the key the
application checks against. Everything else here is publication mechanics; without D3 the whole
chain is a signature nobody has shown MixEngine will accept.

## Goal

Six pairs — `<slug>.toml` and `<slug>.toml.minisig` — at a stable URL under a moved `blueprints`
tag on `mixnz/mixengine-packages`, signed by the gallery key. The success sentence is one command:

```
mix blueprint import ~/Downloads/laravel.toml --overwrite    # trusted, no --name, no --signature
```

(`--overwrite` only because every home already holds the builtin `laravel` — D11 is what the
published files are for, and the case they exist for needs no flag.)

`.minisig` beside the file is not decoration: it is the name
[`blueprint.import`](../../../crates/mixengine-daemon/src/blueprints.rs) already looks for when the
request names no signature.

**That command does not work today**, and finding out why is what grew this task past a workflow.
`[blueprint] name` is a *display* name — the gallery's six say `Laravel`, `Next.js`, `Static site` —
and import, given no `--name`, files a blueprint under exactly that string. `validated_slug` then
refuses all six for a capital letter, a dot, a space. The signature would never be reached. **D10**
is the answer, and it is the one behaviour change this task makes.

## Scope

**In, `mixengine-packages`:** `.github/workflows/publish-blueprints.yml`; `tools/blueprints.py`;
`release/publish-blueprints.sh`; `.github/workflows/check-blueprints.yml`; and the documentation
that stops being true the moment any of them exists — the second-key section of `docs/the-archive.md`
(“Nothing signs with it yet”), a runbook entry in `release/README.md`, and a numbered task in
`docs/roadmap.md`.

**In, this repository:** D10 — the slug a file arrives under — which touches
[`blueprints.rs`](../../../crates/mixengine-daemon/src/blueprints.rs)' `import`, the doc comment on
[`BlueprintImport::name`](../../../crates/mixengine-proto/src/blueprint_api.rs) and the `--name` help
in [`main.rs`](../../../crates/mixengine-cli/src/main.rs); the two tests D9 names; the download
channel written into
[`.claude/features/blueprints.md`](../../../.claude/features/blueprints.md); T79a ticked in
[phase 8](../../../.claude/roadmap/phase-8-differentiators.md).

**Out:** anything that fetches. The daemon does not learn to download a blueprint, now or here —
T79's D1 refused a gallery that arrives over the network and this does not reopen it; what is
published is for a person with a browser. No bundle archive and no index file listing the six: a
bundle needs a signature story of its own and `blueprint.import` takes one file, and nothing in this
product reads a gallery index — the release page is the listing. No signing on a developer's
machine, ever: the secret is an Actions secret and the runner is the only thing that holds it.

## Decisions

**D1 — The bytes are read from a `mixengine` checkout, never copied into the packaging repository.**
`publish-blueprints.yml` takes a `ref` input (default `master`), checks out `mixnz/mixengine` at it
into a subdirectory — the repository is public, so `github.token` reaches it and no PAT is needed —
and signs the files under `crates/mixengine-core/src/blueprints/gallery/` exactly as they are in
git.

The two alternatives both create a second source of truth for one manifest. **Vendoring** the six
into `data/blueprints/` contradicts that repository's own first rule (“this repository holds no
MixEngine source code”) and would drift silently, since nothing there can tell that a manifest
changed here. **Publishing a release asset from this repository** for the packaging workflow to
download is the cleanest boundary and the wrong task: it makes T79a wait on a MixEngine release,
and there is not one yet.

Because `master` moves, the run is otherwise unreproducible after the fact: the release notes name
the mixengine commit SHA the assets were cut from, and the job summary prints it.

**D2 — The tag is moved, not added to.** One tag, `blueprints`, always the newest set, so the URL a
person reads in the documentation never changes — the `index` tag's arrangement and its reason. The
archive's cumulative promise does not apply here: nothing pins a blueprint by version, no installer
reads one, and a superseded manifest has nobody to keep working for.

**D3 — The workflow proves the key chain, not merely the signature.** `publish-index.yml` verifies
what it signed against the committed `minisign.pub`, which answers *did the secret and the public
half match*. That is one link short here. The link that matters is
[`blueprints::trust::PUBLIC_KEY`](../../../crates/mixengine-core/src/blueprints/trust.rs) — the
constant every installed MixEngine checks against — and the workflow already has that file on disk,
because D1 checked the repository out. So it reads the constant out of `trust.rs`, compares it with
the second line of `blueprints.pub`, and fails the run before signing anything when they differ.

Without this step a publish means “signed by whatever secret this repository holds”. With it, a
publish means “MixEngine will accept this”, and a key rotation that has reached only one of the two
places is a red run rather than twelve files no installed copy will take.

**D4 — Assets that are no longer the gallery's are deleted.** `--clobber` overwrites what is
uploaded and touches nothing else, so on a moved tag a slug the gallery drops keeps a valid
signature at a stable URL for good. That is worse here than it would be for the index, because
**trust is decided when a blueprint arrives and never re-examined** (T78a's D1): a file downloaded
after the gallery disowned it is trusted on the strength of a signature nobody would make again.
After the upload the job lists the release's assets and removes every one not in the set it just
published.

**D5 — What is verified is the published bytes, twice over.** Locally before the upload, so a bad
signature fails the run instead of reaching the tag; and then by downloading the whole published set back
from the tag and running `minisign -V` against every pair in it. `check-archive.yml` is in that repository
because *created* and *published* are different claims, and an upload that clobbered the wrong asset
is exactly the accident the second check catches.

**D6 — A weekly check says whether what is published is still master's gallery.**
`check-blueprints.yml`, on a cron with `workflow_dispatch`, downloads the published set and compares
it with the gallery in `mixnz/mixengine@master`, failing when they differ. This is `check-eol.yml`'s
shape and its reason: after this task, a manifest edited in this repository leaves the published file
quietly stale and nothing anywhere is obliged to notice. The check is the obligation. It reports
drift; it does not publish, because deciding to re-cut the gallery is a person's call.

**D7 — The roster is read from the directory, not written down in the packaging repository.**
`tools/blueprints.py` takes whatever `gallery/` holds and asserts three things about it: every file
parses as TOML, every file's stem is a slug MixEngine will accept — lower-case ASCII, digits and
hyphens, no leading or trailing hyphen — and the directory is not empty. The stem is what is checked
because the stem is what the blueprint gets filed under (D10); `[blueprint] name` is display text and
is deliberately not a slug. It does **not** hardcode “six” or the six names. How many blueprints the gallery has is this
repository's decision, asserted by
[`the_gallery_is_the_six_the_roadmap_names`](../../../crates/mixengine-core/tests/blueprint_gallery.rs);
a number repeated over there would be a roster to keep in step by hand, and the drift check in D6 is
what makes a deliberate addition or removal visible anyway.

The same script carries D6's comparison as a second mode (`--published <base url>`), so the publish
workflow and the weekly check share one reader of the gallery rather than growing two.

It also does not re-check that a file is canonical. `manifest::render` is the only definition of
that shape, T79's D2 asserts it here, and a second renderer written in Python would be a second
opinion about the format — the one thing the canonical-rendering decision exists to prevent.

**D8 — `release/publish-blueprints.sh` is how it is run.** `publish-blueprints.sh [--ref REF]
[--dry]`, sourcing `_dispatch.sh`, `publish` defaulting to `true` in the script and `false` in the
workflow — `publish.sh`'s arrangement exactly, including the reason: dispatching by hand from the
GitHub UI and forgetting the switch must publish nothing. It prints the download URL when it
finishes.

**D9 — This repository's share is D10, its tests, and a paragraph.** Two tests, at the level each
belongs to. A daemon-level one walks the whole gallery: for every entry it writes the rendering to
`<slug>.toml` and imports it with no `--name` and no signature, asserting the row lands under that
slug, `imported` and untrusted. That is the published pair's shape minus the one thing a test cannot
hold, and walking all six is what puts `Next.js` and `Static site` — the two names that break every
other rule — inside the assertion. A core-level one asserts that those display names are
**deliberately not slugs**: `validated_slug` refuses them. Without it, somebody renaming the gallery's
display names into slugs would leave the daemon test passing for a reason that is no longer true.

**A real import cannot be tested with a test key, by design.** The daemon verifies against the
compiled-in constant and takes no key from anywhere else — a key a test could substitute is a key an
attacker could substitute, which is T78a's whole argument. So the naming half is the ceiling of what
a test here can prove about the published file, and the signature half is D3's job on the runner.

**D10 — A file is filed under its own name.** `blueprint.import` with no `name` takes the **file's
stem** rather than `[blueprint] name`, still through `validated_slug`.

The row has carried both columns since T77 — `slug` is the key, `name` is display text — and the
gallery is what makes the difference visible: `Static site` and `Next.js` are good names for a
human and cannot be slugs at all. The stem is also the only fallback that round-trips this
product's own output: everything MixEngine renders is written as `<slug>.toml` by
[`store::file`](../../../crates/mixengine-core/src/blueprints/store.rs), so a blueprint exported
from one machine and imported on another keeps its name. The old fallback did that only when the
two happened to be the same string, which is why T78a's test — a hand-written `borrowed.toml` named
`borrowed` — never noticed.

**The boundary does not move.** The stem goes through `validated_slug` exactly as before, so a
downloaded `My Stack.toml` is still refused by name with the same error; what changed is which
string is offered to it. `--name` remains for saying otherwise, and renaming a downloaded file now
changes where it is filed — which is the rule stated plainly, rather than a surprise.

**Rejected: slugifying the display name.** `Static site` would become `static-site`, which is not
what the gallery calls that blueprint, so a downloaded copy would sit beside the built-in one under
a different name — the exact confusion this publication exists to remove. **Rejected: publishing
under display names.** `Static site.toml` is not a filename to put in documentation, and it leaves
the slug question unanswered anyway.

**D11 — What the published files are actually for, now that every home already has the six.** T79
seeds the whole gallery into every home, so the published pair is not how a person gets `laravel`
onto a machine that has MixEngine. It is three other things, and the documentation says so rather
than implying the first:

1. **A blueprint a build does not carry.** The gallery moves on the packaging repository's clock;
   a seventh blueprint, or a corrected one, reaches an installed MixEngine this way and no other.
   There is no collision, no flag, and this is the case the channel exists for.
2. **Replacing one of the six**, which needs `mix blueprint import <file> --overwrite`, because a
   slug already filed is refused by design. **It costs that slug its builtin refresh**: T79's D6
   leaves a row whose source is not `builtin` alone for good, so the imported copy is that machine's
   `laravel` from then on, even when the bytes were identical. Accepted rather than special-cased —
   a rule that made an exception for “identical bytes from the gallery key” would be a second way a
   row can change hands, and the first one is already the interesting one.
3. **A file to read and to fork.** The canonical rendering is the format's worked example (T79's
   D2); a copy edited and re-imported is untrusted, correctly, because the signature was over the
   bytes somebody vouched for.

## Delivery

Two chains, because the work lands in two repositories. Here: the branch `t79a-signing-the-gallery`,
CI, PR, squash. There: commits straight onto `master`, as the other tooling changes in that
repository are made — it has no lint or test workflow that a change of this kind could be gated on,
and its publishing workflows are dispatch-only.

## Testing

- **Here:** the two tests D9 names, written before D10 changes anything — the daemon-level one fails
  on today's code with `InvalidBlueprintName`, which is the bug being reported before it is fixed;
  then the ordinary workspace gates (`clippy`, `fmt`, `cargo test`, `cargo doc`).
- **There:** `python tools/blueprints.py --gallery <path>` against a local checkout of this
  repository, which is the same code path the workflow runs; then
  `release/publish-blueprints.sh --dry`, which is the full rehearsal — it checks the key chain,
  signs nothing, publishes nothing, and uploads the run's artifacts for inspection.
- **The real run** is the acceptance below, not a test: a signature is worth what the published file
  proves and nothing less.

## Risks

- **Line endings.** What is signed is what git gives the ubuntu runner, which is LF, and no step
  rewrites a byte. A checkout on Windows never signs anything, so `core.autocrlf` on somebody's
  machine cannot reach the published bytes.
- **Key rotation.** Rotating the gallery key is an application release (T78a), and D3 is what turns a
  half-finished rotation into a failed run rather than a tag full of files no installed MixEngine
  accepts.
- **A gallery blueprint changing after publication.** Handled as staleness, not as an error: D6
  reports it weekly and a person re-runs D8's script.

## Acceptance

- `release/publish-blueprints.sh --dry` is green and its artifacts hold two files per gallery
  blueprint — twelve today.
- After a real run, downloading `laravel.toml` and `laravel.toml.minisig` and running
  `mix blueprint import laravel.toml --overwrite` reports **trusted**, filed under `laravel`, with no
  `--name` and no `--signature` — the whole of D10 and D3 in one line. `--overwrite` because the home
  already holds the builtin one (D11); the same command on a slug the build does not carry needs no
  flag at all.
- Editing a byte of a downloaded manifest and importing it again reports untrusted — the signature is
  over the bytes, and this is the check being worth something.
- A publish run whose `blueprints.pub` disagrees with `trust.rs` fails before it signs.
- An asset on the tag that the gallery no longer contains is gone after the next run.
- T79a is ticked in phase 8, and the packaging repository's roadmap carries the task under its own
  number.
