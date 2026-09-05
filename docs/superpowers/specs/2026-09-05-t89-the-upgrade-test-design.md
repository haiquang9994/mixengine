# T89 — The upgrade test (design)

Roadmap task **T89**, phase 9: *"Upgrade test: an old `mixengine.db` migrated by a new binary, in
CI."*

`mixengine.db` is the one thing MixEngine owns that cannot be regenerated —
[data-model.md](../../../.claude/architecture/data-model.md) opens by saying so. Everything under
`etc/` is rendered from it, `runtimes/` can be downloaded again, `logs/` is history; this file is
the one whose loss costs a person their sites. An upgrade is the only routine event that rewrites
it, and until this task nothing in this repository has ever run a migration against a database
**this build did not write itself**.

## Goal

A person who installs a later MixEngine over an earlier one starts the daemon, and their projects,
sites, domains and settings are all still there — and a change that would have broken that fails CI
before it is merged rather than after it is released.

## Measured, not assumed

Read on 2026-09-05 out of this tree rather than reasoned about.

1. **Three suites already exercise migrations, and all three build "the old database" out of
   today's migration files.**
   - `crates/mixengine-core/src/store.rs`' `#[cfg(test)]` module writes `0001_first.sql` and
     `0002_second.sql` **at run time** and drives `Store::open_with` over them. What it proves is
     the machinery — when a backup is taken, which failure is the user's and which is ours — and it
     is deliberately not about this product's schema at all.
   - `crates/mixengine-core/tests/migration_extensions.rs` and `…/migration_extension_sites.rs`
     apply `sqlx::migrate!("./migrations")` up to but not including one version, seed, then apply
     that one. Real schema, real rows — but the "before" state is produced by replaying the same
     files the "after" state is judged against.
   - `crates/mixengine-core/tests/store.rs` asserts the schema a *first run* gets.
2. **Nothing in the repository is a `mixengine.db`.** `find . -name '*.db' -not -path './target/*'`
   is empty. Every database any test has ever opened was created inside a `TempDir` moments
   earlier, by the build running the test.
3. **The migration set is 1..=17**, `0001_initial.sql` … `0017_extension_sites.sql`, embedded by
   `static MIGRATIONS: Migrator = sqlx::migrate!("./migrations")` in
   [store.rs:27](../../../crates/mixengine-core/src/store.rs).
4. **`Store::open` already does the whole of the upgrade behaviour** — `schema_state` classifies
   `Empty` / `Current` / `Behind`, `back_up` takes a `VACUUM INTO` copy to
   `mixengine.db.bak-<version>` when and only when `Behind`, and `migration_failure` sorts the three
   kinds of failure. None of that is under test against a database written by anything but itself.
5. **`.gitattributes` sets `* text=auto eol=lf`** and lists six binary extensions; `.db` is not one
   of them.
6. **Release-checklist item 2 already asks for this test, and asks a person to run it**:
   *"verify the migration path from the previous release with a real upgrade test (old
   `mixengine.db` → new binary)"* —
   [build-and-release.md](../../../.claude/operations/build-and-release.md).
7. **`.claude/standards/testing.md` names one home for fixtures**: `crates/mixengine-testkit`, a
   dev-dependency and never anything else, enforced by
   `crates/mixengine-proto/tests/workspace_layering.rs`. It does not depend on `mixengine-core` —
   `home.rs` restates three of `Paths`' answers rather than taking the edge — so a fixture placed
   there can be read by `core` and `cli` alike without a cycle.
8. **`ci.yml` has a stated rule about new jobs**: `system` and `build` were *"a green job that
   proves nothing"* until the task that gave each something to run. The comment exists to record the
   rule rather than the list.

## Scope

**In.** A committed, frozen `mixengine.db` per captured schema generation, and the tool that
captures one; a `mixengine-core` suite that opens each with the real `Store::open` and judges what
came out; one `mixengine-cli` test that starts a real `mixengined` on one of them; the
`.gitattributes` line that keeps their bytes; the three documents that describe the rule this
creates.

**Out.** A CI job of its own — see D9. Changing anything in `store.rs`: this task tests the upgrade
path, it does not redesign it. Downgrade: a database from a *newer* build is already refused as
`IncompatibleDatabase` with a test of its own, and restoring the backup beside it is a person's
decision, not a code path. Fixing what D10 measures about `open_read_only`.

## What an "old database" is going to be

Three candidates, and the choice decides everything else.

| | What it is | What it costs |
| --- | --- | --- |
| **(a)** A committed binary `.db` | the artifact itself, `_sqlx_migrations` checksums and all | a blob a reviewer cannot read |
| **(b)** A committed SQL dump, replayed at test time | reviewable, diffable | a *reconstruction*, and the reconstruction is done by today's build |
| **(c)** A prefix of today's migrations, applied at test time | free | **this is what already exists**, and it is not what T89 asks for |

**(a).** The roadmap's own words are *"an old `mixengine.db`"*, and the difference between (a) and
(b) is not cosmetic: what makes a fixture evidence is that **this build did not produce it**. A
dump replayed at test time is produced by this build, every time, out of files this build also
carries — which is (c) wearing a different hat for every table except `_sqlx_migrations`.

And `_sqlx_migrations` is the row that cannot be reconstructed honestly. Its `checksum` column holds
a hash of the migration SQL **as it was when it ran**. Committed as bytes, it is the only thing in
this repository that can catch an edit to a migration that has already shipped — the first line of
data-model.md's compatibility rules, and today enforced only against migrations a unit test wrote
seconds earlier. Recomputed at test time it says nothing, because it would be recomputed from the
edited file.

(a)'s cost is answered rather than accepted: **the blob is committed beside the SQL that produced
it** (D3), so the diff that adds a fixture carries a readable rendering of what is inside it, and
the suite's assertions are the second rendering.

## Decisions

### D1 — The fixture is frozen on capture, and the capture tool refuses to overwrite

A fixture is evidence exactly as long as nobody regenerates it. The failure mode this design has to
survive is not malice but convenience: CI goes red, somebody re-runs the capture, CI goes green, and
the thing the test existed to catch has been erased in the same commit that hid it.

Three things stand against it, and none of them is a lock:

1. `cargo run -p mixengine-core --example capture-upgrade-fixture -- <schema>` **refuses a
   destination that exists** and says so. Overwriting takes a deliberate `rm` first.
2. A modified binary file is visible in a diff in a way a modified line is not — `git` reports
   `Binary files differ` and nothing else, which is exactly the sentence a reviewer should stop at.
3. The rule is written into
   [data-model.md](../../../.claude/architecture/data-model.md)'s compatibility list, beside *"never
   rewrite an existing migration file"*, which is the same rule about the same thing.

### D2 — The fixtures live in `mixengine-testkit`, and only `copy_into` reaches them

`.claude/standards/testing.md` names one home for fixtures and this is it. Two crates read them —
`mixengine-core`'s suite and `mixengine-cli`'s — and a path like
`../mixengine-core/tests/fixtures/…` written from the CLI crate is a dependency nothing declares
and nothing checks.

The module exposes exactly one way to get at a fixture:

```rust
mixengine_testkit::upgrade::Fixture::all()          // every fixture, oldest schema first
mixengine_testkit::upgrade::Fixture::copy_into(&self, directory) -> PathBuf
```

**There is deliberately no accessor returning the committed file's own path.** `Store::open`
migrates what it is given; a suite handed the source path would rewrite the repository's fixture on
its first run, and every run after that would be judging a database this build had written — the
whole design undone by one convenience method. `copy_into` copies and then **clears the read-only
bit on the copy**, because `std::fs::copy` on Windows carries file attributes across and a
`VACUUM INTO` against a read-only destination fails in a way that reads like a bug in `back_up`.

### D3 — Every fixture is captured with a committed seed, and the seed is committed with it

`fixtures/upgrade/` holds a pair per generation:

```
schema-0001.db     the frozen database
schema-0001.sql    the INSERTs it was seeded with
```

The `.sql` is not read at test time. It exists so that the blob's content is reviewable, and so the
next capture starts from something rather than from nothing.

**What a seed contains, and what it must never contain.** Rows in every table the schema at that
version has, chosen so that each constraint a later migration could trip over has something to trip
on: two runtimes with one default, two projects, two sites, three domains with one primary each, a
service by `package_id` and a service by `runtime_install_id`, a running job and a finished one, a
CA and a leaf that names it. **No key material, no credential, no real path, no personal data** —
`ca` and `certificates` store *paths* to key files rather than keys, which is what makes them safe
to seed at all; the paths in a seed are `/home/dev/...` literals that exist nowhere.

### D4 — What the suite asserts, per fixture

`crates/mixengine-core/tests/upgrade.rs`, over every fixture `Fixture::all()` returns:

1. **It opens.** `Store::open` returns `Ok`. A failure here is `IncompatibleDatabase` (a migration
   was edited after it shipped), `Migration` (our SQL is wrong), or `Database` (the file cannot be
   used) — three different sentences, and the assertion prints which.
2. **It ends up current.** Afterwards, the applied versions are exactly the versions this build
   carries.
3. **The copy was taken when there was something to lose, and not otherwise.**
   `mixengine.db.bak-<version>` exists iff the fixture was behind; and when it exists, it holds the
   *pre-migration* census (D5) — a backup of the post-migration state is not a backup.
4. **Nothing was lost.** The census before is contained in the census after (D5).
5. **It is a database a current build can use.** After the migration, the writes a current schema
   accepts still work — a project, a site and its primary domain, through the same statements
   `core/tests/store.rs` uses. A file that opens and then refuses every write is not a migrated
   database.
6. **Opening it again changes nothing.** A second `Store::open` finds it current and leaves no
   second backup. The daemon starts many times a day.

### D5 — The census: `quote()` over every table, compared on the columns both sides have

Naming the rows that must survive, per fixture, in Rust, would be a second copy of the seed that
drifts from the first. Instead the suite reads the *whole* database twice and compares:

```sql
SELECT quote(<col>) … FROM <table>
```

`quote` is SQLite's own faithful rendering: `NULL` for a null, `'x'` for text, `X'00ff'` for a blob,
the numeral for a number — so one comparison covers every column type without the suite knowing any
of them. Rows are collected into a sorted multiset, because a table rebuild is free to reorder.

The comparison is restricted to **the columns present on both sides**, so a migration that adds a
column is not a failure, and `_sqlx_migrations` is excluded, because it is supposed to grow.

**This is stricter than "no rows were lost", on purpose.** A migration that *changes* a value —
data-model.md's *"renaming a `ServiceId` or `kind` value requires a data migration in the same
change"* — will fail this test. That is the correct outcome: a data migration is exactly the kind of
change that should not be able to land without somebody saying out loud which rows it rewrites, and
the way to say it is an exception in this file, in the same commit.

### D6 — There must be a fixture at schema 1, and the suite fails if there is not

A fixture captured today at today's schema proves nothing today: it is `Current`, no migration runs
against it, and every assertion in D4 is trivially true. It starts carrying evidence the day a
migration lands after it.

So the suite would be perfectly green with a directory containing only such a fixture — or, once
someone deletes a file, with a directory containing none. Both are the "test that reads nothing and
passes" this repository keeps finding. The guard is one test:

- `Fixture::all()` is not empty, and
- one of them is at **schema 1**, which is the oldest state any `mixengine.db` has ever been in and
  therefore the one whose migration exercises every migration this build has.

### D7 — The checksum guard gets a sentence of its own

`Store::open` already fails on an edited migration: sqlx compares the checksum in
`_sqlx_migrations` against the migration it holds and answers `VersionMismatch`, which
`migration_failure` maps to `IncompatibleDatabase`. D4.1 therefore covers it — with a message about
a database from another build, which is the wrong sentence for what actually happened.

So the suite compares directly: for every version the fixture recorded, the checksum equals the one
`sqlx::migrate!("./migrations")` carries, and the failure names the version. *"Migration 0005 has
been edited since it shipped"* is actionable; *"IncompatibleDatabase"* sends the reader to the wrong
paragraph.

### D8 — One CLI test, because "migrated by a new binary" means the daemon

Everything above happens inside one process calling `Store::open`. What it cannot answer is whether
the **product** starts on a migrated database: whether the daemon's own readers cope with rows whose
newer columns hold defaults, and whether `mix` can list what was in the old file.

`crates/mixengine-cli/tests/upgrade.rs` puts a fixture at `Home::database_file()` **before**
`start_daemon()`, and then:

- `mix status --json` reports a healthy daemon,
- `mix site list --json` lists the site that was in the old database, by its domain,
- the home holds `mixengine.db.bak-<version>`.

One test and not a suite. What only this test can prove is that the daemon starts and reads; every
schema claim is cheaper and clearer one layer down, which is `testing.md`'s rule about which layer
owns a behaviour.

**The fixture is seeded so the daemon has nothing on disk to reconcile** — a project, a site, its
domains and some settings; no `services` row pointing at a package directory that is not there.
A red test caused by a missing directory would be a test about `TempDir`, not about a migration.

### D9 — No job of its own

`ci.yml` states the rule: a job arrives with the work that gives it something to run, and until then
it is *"a green job that proves nothing"*. This suite is `cargo test` with no fixture to download,
no privilege to acquire and no network — so it runs in the `test` job, on `ubuntu-latest`,
`windows-latest` and `macos-latest`, as part of `cargo test --workspace` and
`cargo test --workspace --all-targets`, with no edit to the workflow at all.

**All three legs earn their run.** The path under test copies a file, opens it, runs `VACUUM INTO`,
renames across a directory and re-opens — and every one of those is where Windows differs: file
attributes carried by a copy (D2), a rename over an open handle, a `-wal` sidecar that a crashed
process leaves behind. A migration test that only ran on Linux would be the third list in this
repository that was right on two systems.

### D10 — What this measures and does not fix: the shim's door on an unmigrated database

`Store::open_read_only` — *"the shim's door"* — neither creates nor migrates, deliberately: a schema
upgrade decided by whichever `php -v` ran first is the one moment `mixengine.db` can least afford a
surprise.

The consequence is a window. After a binary upgrade and before the next daemon start, the file on
disk is at the old schema while every `sqlx::query!` in the shim was compiled against the new one. A
column added by the pending migration is a column the shim asks for and does not get.

This design **measures the answer and writes it down**; it does not change it. Closing the window is
a question about start-up ordering and about what a shim should say when it finds a database older
than itself, and that is somebody's design rather than a line slipped into this one. What is
recorded is the finding and where it would go.

### D11 — `.gitattributes`

`* text=auto eol=lf` is the first line of that file, and it is what keeps `cargo fmt --check` green
on a Windows checkout. Git's own heuristic would spare a `.db` — a SQLite file starts
`SQLite format 3\0` and the NUL makes it binary — but *"would spare"* is not a property to rest a
fixture on, and a fixture silently rewritten by a line-ending filter is a database with a corrupt
page and a test failure that names nothing. `*.db binary` joins the six extensions already listed.

## What this changes about release-checklist item 2

Item 2 says today:

> Bump version, update `CHANGELOG.md`, verify the migration path from the previous release with a
> real upgrade test (old `mixengine.db` → new binary).

The verification half becomes CI's, and one thing becomes a person's that was not written down
before: **capture a fixture at the schema being released, before the version is bumped.** That is
the act that turns the next release's upgrade into something CI can check — nothing else in the
pipeline knows which schema was ever shipped, because the tree only knows which one is current.

## Testing

| Claim | Where |
| --- | --- |
| an old database opens, ends up current, keeps every row, takes a backup first | `crates/mixengine-core/tests/upgrade.rs` |
| the backup holds the state from *before* the migration | same |
| a migrated database accepts the writes a current build makes | same |
| a second open is a no-op | same |
| there is a fixture at schema 1 (D6) | same |
| no shipped migration has been edited (D7) | same |
| a fixture carries no `-wal` or `-shm` sibling | same |
| the daemon starts on a migrated database and `mix` lists what was in it | `crates/mixengine-cli/tests/upgrade.rs` |
| the capture tool refuses to overwrite a fixture | `crates/mixengine-core/examples/capture-upgrade-fixture.rs`, exercised by hand — it is a tool, and a test that ran it would have to write into the source tree |

## Risks

- **A fixture regenerated to make CI green.** D1 puts three obstacles in front of it, none of them a
  lock. This is the design's one soft spot and it is stated rather than papered over.
- **D5's strictness meets a real data migration.** Expected, and the intended cost: the change lands
  with an exception naming the rows it rewrites.
- **Fixture size in the repository.** A 17-migration schema with a few dozen rows is under 100 KB
  after the `VACUUM` the capture ends with; one per release is a rate this repository can carry for
  years.
- **The CLI test's daemon.** Bounded by seeding a database with nothing to reconcile (D8), and by
  the suite's existing `Home` drop guard.
