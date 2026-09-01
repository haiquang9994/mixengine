# T79 — the built-in blueprint gallery (design)

Roadmap task **T79**, phase 8. T77 wrote a blueprint down, T78 carried one out, and T78a decided
whether the command inside it may run. Every one of those has only ever been exercised against
blueprints this repository's own tests wrote. This task ships six the product vouches for, and they
are the first blueprints anybody outside this repository will ever apply.

The sentence with teeth is D1: **the gallery is a table compiled into the binary**, which is what
makes `BlueprintSource::Builtin` — a word that has existed since T77 and that nothing has ever
written — mean *this build's own*.

## Goal

Six maintained blueprints — `laravel`, `wordpress`, `symfony`, `static`, `nextjs`, `django` — present
in every home without anybody asking for them, trusted, and each one applying to a project that
works. They double as the end-to-end exercise of the whole feature: capture, plan, apply, scaffold.

## Scope

In: the six manifests and the compiled-in table that holds them; `blueprints::gallery::seed` and its
call site in the daemon's start; the two existing tests that stop being true when a fresh home is no
longer empty; a captured fixture and the cross-OS test built on it; and **T79a** added to the
roadmap for the half that leaves this task.

Out: publishing the gallery as signed files with a `.minisig` beside each one. T78a's design said
this task would do it; D3 is why it moved and where it went. `blueprint.delete`, still — and D12 is
what that costs here. No new API method and no new CLI command: a gallery blueprint is reached
through the same `filed_of` every other blueprint is, and a task that needed a special case would
have chosen its storage wrong.

## Decisions

**D1 — The gallery is a table compiled into the binary, seeded into rows.** Six `.toml` files under
`crates/mixengine-core/src/blueprints/gallery/`, `include_str!`d into a `&'static [Entry]` in
`blueprints/gallery.rs` — [`shims::COMMANDS`](../../../crates/mixengine-core/src/shims.rs)' shape,
and for its reason: a set this build ships is a constant of this build, not a document it fetches.
`gallery::seed` puts them in the `blueprints` table as ordinary rows.

**Rows rather than a second source read at lookup time**, which was the alternative. `blueprint.plan`
and `blueprint.apply` reach a blueprint through `store::filed_of`; a gallery that lived outside the
table would mean every one of those paths asking two places and agreeing about which answer wins.
The rows are also what makes the rendered file appear at `blueprints/<slug>.toml`, where a person
can read and copy it, exactly as a captured one does.

**Not fetched from the packaging repository**, the other alternative. A gallery that arrives over the
network is a gallery that is absent on a fresh machine with no connection, needs a cache, and needs
an answer for what a stale cache means — and `builtin` would stop meaning *this build's own*, which
is the only thing that makes D3 sound.

**D2 — The files are canonical renderings, and a test holds them to it.**
[`manifest::render`](../../../crates/mixengine-core/src/blueprints/manifest.rs) writes one fixed
order and keeps no comments, so a hand-written file with commentary would produce three different
texts for one blueprint: the source in this repository, the `manifest_toml` column, and the file in
the home. Each gallery file is written in exactly the renderer's output shape, and
`render(read(bytes)) == bytes` is asserted for all six.

**The cost is real and taken deliberately**: a gallery blueprint is a thing people read to learn the
format, and it cannot carry a single explanatory comment. What carries the explanation instead is
`[blueprint] description`, which survives the round trip because it is a field. What is bought is
that the file in this repository and the file in a user's home are the same bytes, so a `diff`
between them means something — and so the byte-for-byte property T78a's D16 already asserts for a
gallery-shaped manifest keeps holding for the actual gallery.

**D3 — A builtin row is trusted without a signature check, and the publishing half becomes T79a.**
`seed` writes `trusted = true` and never calls `blueprints::trust::verify`. A signature travelling
inside the same binary as the key it would be checked against proves nothing that the binary has not
already proved: an attacker who can change the compiled-in bytes can change the compiled-in key in
the same edit.

This is a **departure from T78a's design**, which listed the gallery's signed files and their
`.minisig` generation as T79's. Compiling the gallery in (D1) removes the channel those signatures
were for — nothing downloads a gallery file, so nothing has bytes to check. The need returns the day
gallery blueprints are published for hand import, and that day is **T79a — publishing the gallery as
signed files**, added to `.claude/roadmap/phase-8-differentiators.md` immediately after this task
rather than left as a promise nobody wrote down. `trust::PUBLIC_KEY` stays exactly where it is; it
is what `blueprint.import` already uses.

**D4 — Seeding compares before it writes.** One query reads `id`, `source` and `manifest_toml` for
the six slugs; a row whose manifest already equals the compiled-in bytes is left alone, and the
rendered file is rewritten only when what is on disk differs. Steady state — which is every daemon
start after the first on a given build — is one `SELECT` and no writes at all.

**This is not premature tuning.** `serve` explains, in as many words, that nineteen file copies
before the endpoint bind made an ordinary parallel test run fail, and every CLI test in this
workspace starts a daemon. Six file writes and six upserts on every one of those starts, on the
Windows leg that is already CI's slowest, is a cost with nothing on the other side of it: the bytes
are identical every time.

**D5 — It runs at start, beside the shims, and never fails one.** The call sits next to
`shims.refresh()` in `serve`, before the bind, and a failure is a `tracing::warn!` rather than a
returned error. Same argument, one object along: `bin/` is a projection of a compiled-in table,
`etc/` is a projection of the database, and the gallery is a projection of a compiled-in table into
the database. A home whose gallery was deleted is repaired by starting the daemon; a gallery that
could not be written leaves a daemon that works and a missing blueprint somebody can see, where
refusing to start would leave them with no daemon at all.

**D6 — A row somebody else owns is never touched.** `seed` skips any row whose `source` is
`captured` or `imported`, whatever its slug. A person who captures over `laravel` — which takes
`--overwrite`, because `store::save` refuses a collision without it — owns that slug from then on,
and no upgrade takes it back. This needs no new rule: `save` already writes `source = captured` on
the conflict path, so the skip on the next start follows from the row.

**D7 — Version constraints are series, never pins.** `php = "8.3"`, `node = "22"`,
`python = "3.12"`. `VersionConstraint` accepts a prefix and
[`apply`](../../../crates/mixengine-daemon/src/api/apply.rs) resolves it with `newest_satisfying`, so
a series names whatever the index publishes now. A gallery that pinned `8.3.14` would be stale on
the next patch release and would ask every user a version question about a difference nobody cares
about — the exact question T78's D7 built the answer machinery for, spent on noise.

**Which series** is the current upstream release line at the time the file is written, checked
against what the package index actually publishes rather than taken from memory — the numbers in the
table below are this document's, and the implementation confirms each one against the index before
committing it. Moving a series later is ordinary gallery maintenance, and it is a one-line edit to
one file.

**D8 — Three of the six carry a `[scaffold]`, and three deliberately do not.** `laravel`, `symfony`
and `nextjs` have one; `wordpress`, `django` and `static` have none.

The gallery sells a **stack**, not a scaffold. T78a already made a blueprint with no command a
first-class thing, and where no command satisfies D9 the honest answer is to ship none and say so in
the description, rather than to ship one that half works.

**D9 — What a gallery command may be.** Four rules, each from a constraint that already exists:

- **Non-interactive.** T78a's D10 refuses a timeout on purpose, so a command that prints a prompt is
  a job that hangs until somebody cancels it. Hence `--no-interaction` and `--yes`.
- **OS-neutral.** It runs through `cmd.exe /C` and `sh -c` (T78a's D9), so `&&` is fine and
  `.venv/bin/…` is not, because Windows spells that `Scripts`.
- **It does not bootstrap a package manager this product does not ship.** `composer` has no shim —
  [`shims::COMMANDS`](../../../crates/mixengine-core/src/shims.rs) holds php, node, python and ruby
  tools and nothing else — so `composer create-project` needs one on `PATH`. A machine without it
  gets a **failed step naming the command** and a project that still exists, which is T78a's D7 and
  D8 together: a step that ran and failed is its own outcome, and it unwinds nothing. The
  alternative was embedding a `composer-setup.php` download in the manifest, which is an unverified
  fetch inside a document this product vouches for.
- **It does not write into a shared runtime.** This is what removes Django's command:
  `python -m pip install django` installs into the managed Python that every other project resolving
  that version also uses. `[php] extensions` is the one shared-runtime change a blueprint makes, and
  it is one the feature argued for explicitly; quietly adding a package to somebody's Python is not.

**D10 — Provenance says `os = "any"`.** `[blueprint.created_on]` is required by the schema and a
gallery blueprint was captured on no machine. `any` is the true answer; naming one of the three
systems would be a fabricated fact in a field people read. `version` is the release the file was
written for and is not maintained afterwards — "written by MixEngine 0.1.0" stays true forever,
where a value re-stamped each release would be a lie about when the file last changed.

**D11 — The cross-OS property belongs to capture, so its test starts from a captured fixture.** The
acceptance criterion is *"a blueprint exported on Windows applies on macOS (no absolute paths, no
OS-specific service names)"*. A gallery manifest is hand-written and byte-identical on all three
systems, so applying one on macOS says nothing whatever about what a Windows machine writes. The
test therefore uses a **manifest captured on Windows and committed under `tests/`**, applied by every
system in the ordinary suite.

Chosen over passing an artifact between a Windows and a macOS CI job: that route proves it on bytes
just produced, but it runs only in CI, can fail for reasons about artifact plumbing, and cannot be
reproduced on a laptop. The fixture's weakness — it freezes, and a schema change means capturing it
again by hand — is visible and cheap; a green CI job that stopped actually transferring anything is
neither.

**D12 — Six rows are permanent, and that is the price of D1.** This build has no
`blueprint.delete`. Once seeded, the gallery is in every home for good, and a person who wants none
of it still sees six lines in `mix blueprint list`. Written here rather than discovered later: the
way out is `blueprint.delete`, which is a task of its own, and until it exists this is what choosing
rows over a read-time merge costs.

## The six

Every one of them uses `domain_pattern = "{project}.test"` and `https = true`.

| slug | runtimes | site | services | scaffold |
|---|---|---|---|---|
| `laravel` | php 8.3, node 22 | `php-fpm`, `public` | mariadb main, db+user `{project}`; redis main | `composer create-project laravel/laravel . --no-interaction` |
| `symfony` | php 8.3 | `php-fpm`, `public` | mariadb main, db+user `{project}` | `composer create-project symfony/skeleton . --no-interaction` |
| `nextjs` | node 22 | `node-app`, port 3000 | — | `npx --yes create-next-app@latest . --yes` |
| `wordpress` | php 8.3 | `php-fpm`, doc root at the project root | mariadb main, db+user `{project}` | — |
| `django` | python 3.12 | `reverse-proxy` → `http://127.0.0.1:8000` | postgres main, db+user `{project}` | — |
| `static` | — | `static`, doc root at the project root | — | — |

`laravel` is the only one carrying `[php] extensions`, and it carries `redis` — a shared extension
the PHP artifacts already ship enabled, so on an ordinary machine the step plans `Satisfied` and the
line is there for the machine where somebody turned it off.

`{project}` appears in `database`, `user` and `domain_pattern`, and in no scaffold command: all three
commands run *in* the project directory and address it as `.`, so there is nothing for the token to
say. That keeps T78a's D6 expansion exercised by the tests it already has rather than by the gallery.

## Data model

**No migration.** The `blueprints` table, the `source` column and the word `builtin` have existed
since T77 and T78a respectively; this task is the first thing that writes that word.

## API and CLI

**Nothing new in either.** `blueprint.list` answers six more rows, each `"source": "builtin"` and
`"trusted": true`; `mix blueprint list` prints them with the columns it already has. `blueprint.plan`
and `blueprint.apply` reach them with no branch of any kind, which is the property D1 was chosen for.

## Testing

1. **Round trip, all six** (`mixengine-core`): each file parses, and `render` gives back its own
   bytes — D2's assertion, and the thing that keeps the six files honest as they are edited.
2. **Plan, all six** (`mixengine-core`): each blueprint plans on a clean store, and the steps are
   asserted by shape — which actions, in dependency order, with which dispositions. A machine with
   nothing installed is the ordinary case for a gallery blueprint, so `Create` throughout is the
   expected answer, not a degenerate one.
3. **Seeding** (`mixengine-core`, against a real store): a fresh home gets six trusted `builtin`
   rows and six files; seeding twice writes nothing the second time (D4); a row whose source is
   `captured` survives a seed untouched (D6); a builtin row whose manifest was edited in the
   database is put back.
4. **A real apply** (`mixengine-cli`, `tests/blueprint.rs`): `mix blueprint apply static` end to end
   against a real daemon. Offline by construction — no runtime, no service, nothing reaching the
   index — which is the same property that suite's own note already claims for its fixture.

   **`https = true` puts a certificate step in this plan, and the assertion is written against what
   that step actually does.** It is not queued: `apply` runs it, and a certificate that could not be
   issued comes back `NotRun` with the reason, on `site.create`'s standing position that a site is
   worth more than a certificate. So the test asserts the project, the site and its domain, and that
   **no step `Failed`** — rather than pinning an outcome that depends on whether the machine running
   the suite has an authority.
5. **Cross-OS** (`mixengine-cli`): the Windows-captured fixture of D11, applied on whatever system
   is running, asserting a project and a site that work.

Two existing tests stop being true and change with this task: `a_fresh_home_holds_no_blueprints`
(`mixengine-daemon/tests/api.rs`) becomes *a fresh home holds the gallery and nothing else*, which is
the stronger claim; and `a_hand_written_blueprint_arrives_untrusted`
(`mixengine-cli/tests/scaffold.rs`) stops indexing `blueprints[0]` and finds its blueprint by slug.

## Dependencies

T77 (the manifest, the store, the rendering), T78 (plan and apply), T78a (the `trusted` column and
the scaffold). Nothing outside this repository — which is exactly what D3 changed.

## Risks

**A gallery command rots.** `create-next-app` changes a flag, a `composer create-project` target
moves. The blast radius is bounded by T78a's D8: the step fails, names the command, and the project
it was scaffolding still exists. The gallery is a maintained set and this is the maintenance.

**Six rows nobody can delete** — D12, written down rather than mitigated.

**A blueprint that is wrong is wrong on every machine at once**, which is the difference between
this and a captured blueprint. The plan test (2) is what stands in front of that: a manifest that
would plan into nonsense fails in CI rather than on somebody's laptop.

## Text that this task makes wrong

- `.claude/features/blueprints.md`, *Built-in gallery*: it promises the set; after this task it can
  say which six, that they are compiled in and trusted, and that three carry a command.
- `.claude/roadmap/phase-8-differentiators.md`: T79 ticked, with what was found; **T79a added
  immediately after it** (D3).
- T78a's design listed the `.minisig` generation as this task's. Design records are not edited after
  the fact — this document's D3 is where that moved, and T79a is where it lands.
