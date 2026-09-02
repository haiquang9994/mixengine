# Phase 8 — Differentiators

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [x] **T74** LAN sharing: per-site opt-in **and its manual reverse**, a second listener on the
      shared site's block and nothing else rebound, firewall rule (one elevation prompt), the LAN
      URL, and the site certificate reissued with the LAN IP among its SANs so HTTPS does not break
      the moment it leaves loopback. Where the firewall cannot be managed, say so and give the
      manual command rather than reporting success. A QR code is a rendering of that URL and not a
      screen this repo owns: the daemon answers the URL, `mix` prints the code in the terminal, a
      graphical client draws its own. **(P)**
- [x] **T75** mDNS advertisement (`<slug>-mixengine.local` — **one label**, because a multi-label
      name under `.local` does not resolve; measured, see the T75 design's D1, which is where this
      line's own earlier spelling was overturned), that name added to the certificate SANs beside
      the LAN IP, and the CA download endpoint for phones — served only while sharing is on, only
      the public certificate, out of a directory that holds nothing else. Also fixes a defect T74
      shipped: `mix cert status` reported `NamesDiffer` for every shared site, because the
      comparison read the bare domain list while the certificate carried the LAN address. **(P)**
- [x] **T76** Revoking *by itself*, the manual path having landed with T74: a network change
      disables sharing and says why, taking the same road `site.unshare` takes — and **a finding has
      to survive two consecutive checks**, because one enumeration during a DHCP renewal or a wake
      from sleep would otherwise unshare every site on the machine (the design's D2, which is the
      correction the task turned on). Optional `--for 2h` expiry, measured against the `shared_since`
      T74 stores, and a length shorter than the share has already lasted is *refused* rather than
      honoured: a URL that is dead when it is printed is worse than a sentence saying so. Sharing
      reported on the event stream as one `SiteSharingChanged` carrying why. Two enforcement tests:
      the "web ports only" scan — which proves what is *listening* and says so, since it never
      crosses a firewall — and no firewall rule left behind, enumerated by label, a Windows test
      because `ufw` has no comment field to name a rule of ours with. **And the rule MixEngine never
      made**, answered: the responder now binds UDP 5353 only while something is shared, so Windows'
      dialog arrives in the second after somebody typed `mix site share` rather than at every daemon
      start; MixEngine refuses to pre-empt it with a rule of its own, which would cost T75's D8 and a
      prompt at start; and `mix doctor` reports the rule as a **note** with the command to remove it,
      never as a `Problem` — a `ProblemId` is what `doctor_repair` matches on, and deleting a rule
      somebody personally clicked Allow on is not a repair. **(P)**
- [x] **T77** Blueprint manifest, `blueprint.capture` and the plan `mix blueprint apply --dry-run`
      prints. The manifest is **its own type** overlapping `mixengine.toml` rather than sharing its
      struct, with one hand-built writer so that capturing a project twice gives two byte-identical
      files. Capture reads `sites` → `site_service_links`, tokenises the project's own name to
      `{project}` **by substitution and never by invention** — a domain that does not carry the name
      keeps its literal spelling and the plan reports the conflict — and reads an instance named
      after the project as `per-project`, which is what stops a second project plugging into the
      first one's database. A project with two sites is refused by name. The promise "never data,
      credentials or absolute paths" became a test that reads the **rendered TOML** and refuses to
      find them. The plan reads this home's tables and **never the index**, decides every blocker
      itself — a taken domain, a directory that is already a project, a name too long for a database
      account — and marks the steps that will ask for elevation, said once at the end.
      **Two pieces of text this task found wrong**: `[php] ini`, which the feature doc promised and
      nothing on any machine deviates from, so it is gone rather than filled with a global default;
      and `kept_warm`'s note claiming its missing join waited on T77, when `site_service_links` has
      held that edge since `0006`. And one defect older than the task: `mix project update --name`
      panicked in a debug build, two clap arguments sharing an id — now caught by a test that builds
      every command this binary offers.
- [x] **T77a** Creating a database and the account that reaches it, which `PlanAction::CreateDatabase`
      named and nothing in this workspace could do: a `Recipe` hook the three database packages
      implement, and a daemon runner on T33's division — the recipe says what statements, the daemon
      says with which credential. **A keyring entry is the deed of ownership**: an account MixEngine
      holds no credential for is refused rather than seized by an `ALTER USER`, which costs one
      read-only probe and is the whole difference between "ensure" and "take over". The rule came out
      as a pure function of what the probe found and what the store holds, so the thing the task rests
      on is a four-row table test with no server and no keyring in it. On PostgreSQL the account
      *owns* the database, because `GRANT ALL ON DATABASE` has not carried `CREATE` on `public` since
      15 — a Django blueprint would otherwise apply cleanly and die on its first migration — so the
      role is created before the database, and that difference is why the step order belongs to the
      recipe rather than to a sequence shared with the MySQL family, which took the other order and
      one shared statement builder. No table and no migration: the server is the record, the keyring
      is the deed, and the address joining them is `<service-id>/<user>`, which
      `Context::secret_address` already composed.
      **The last step logs in as the account just made** and creates a table with it — a real one in
      `public`, since a temporary table lives in a schema of its own and would prove nothing about the
      ownership above. It is a postcondition rather than an assertion in a suite because
      `tests/mariadb.rs` had already found out what the alternative costs: on macOS a keychain item
      carries an ACL naming the application that created it, so a *test process* reading the daemon's
      credential raises a dialog nobody can answer, measured once at twenty-seven minutes before the
      job timed out. Moving the proof inside the method left the three real-server suites with nothing
      to read.
      `create` only, and that is what keeps a drop out of T78: a rollback leaves a database it made
      and says it left it, because by then a scaffold may have migrated into it.
      **Two things found on the way.** `Step` and its runner moved out of `first_run` into
      `generate::step` and `services::step`, because a bootstrap and a provisioning are two kinds of
      work with one shape and the module they lived in is named after only one of them. And T77 left
      four blueprint error variants falling into `ToWire`'s `_ => internal` arm, so a mistyped
      blueprint name reached a client as an internal error; they are classified now, beside the three
      this task added.
- [x] **T78** `blueprint.apply` execution with resumable idempotent actions and rollback scoped to
      what this apply created; a version mismatch is answered as a choice (install / use the
      installed one / cancel), never decided quietly. **Resuming is running it again** and there is
      no ledger: every action is an *ensure*, so a second apply plans against what the first one
      left and reports `already true` down the list — which cost three honesty fixes in T77's plan,
      each one narrower than the block it replaced, and the third of them (a project that already
      holds its site) was found by the test rather than by reading. Rollback undoes what belongs to
      the *project* and keeps what belongs to the machine — the database, an installed
      runtime or package, an extension it turned on, the directory — naming each; the ledger records
      **intent** rather than success, because `sites::create` deliberately keeps the row it wrote
      when the rendering fails. `job.cancel` stops and does **not** roll back: running it again
      continues, and a cancellation is not a request to delete anything.
      **Two things T77's plan could not do, found by trying to execute it.** `RegisterProject`
      carried no `pins`, so an applied project ran whatever PHP the machine defaults to and a capture
      of it came back with no `[runtimes]` at all — the pin is also what makes the version question
      mean something, since without it both answers leave identical machines. And there was no
      `InstallPackage`: a blueprint from somebody else's machine planned `EnsureService` on a MariaDB
      that is not on disk, and `service.create` refuses — which is a plan discovering the impossible
      five actions into a project directory, the one thing a plan exists to prevent.
      An apply **queues** elevation and never raises a prompt, on T40b's standing rule; the client
      spends the single prompt at the end. A certificate or an extension that fails is reported as a
      step that did not run rather than undoing a working project, `site.create`'s own position. The
      job's bar is sliced per step, so a nested install reporting 0–100 of itself no longer drags it
      backwards.
      Scaffold execution stays T78a's: everything else is applied and the exact command is printed
      for somebody to run.
- [x] **T78a** Scaffold trust: `[scaffold]` is arbitrary code from whoever wrote the blueprint.
      It never runs on import, only on apply, only after a confirmation showing the exact command,
      with output streamed to the job log; gallery blueprints are signed and a hand-imported one is
      marked untrusted for good. **Trust is a column decided when a blueprint arrives** — the
      signature is checked once, over the bytes handed in, and nothing raises the flag afterwards;
      that is the departure from `index.rs`, which keeps the signed bytes and re-verifies them,
      and it is forced by the row being the truth while the file beside it is a rendering.
      **The consent names the command** rather than saying yes, and carries whether the person was
      told the blueprint was unsigned: a blueprint re-imported between the plan and the apply is
      the case both halves exist for. `blueprint.import` arrived with this task, since without it
      nothing could produce an untrusted blueprint at all.
      **Three things found on the way.** T77's plan never expanded `{project}` into the scaffold
      command, so a blueprint naming the project in its own command planned the token — fixed where
      every other expansion happens, because what is shown has to be what runs. A step that *ran and
      failed* needed a fourth `StepResult`: making it the job's failure would have thrown away the
      report of the nine steps that worked. And the log surface grew a second kind of subject —
      `GET /logs/service/{id}` and `GET /logs/job/{id}`, plus `mix job logs` — because a command's
      output is exactly the volume ADR 0009 keeps off the event stream.
      **`--run-scaffold` and `--run-untrusted-scaffold` are different flags**, neither implied by
      the other; where there is nobody to ask, the command is left unrun with a line saying so
      rather than the apply being refused, because there is no flag for *no* and a script must be
      able to apply a blueprint without its command.
- [x] **T79** Built-in blueprint gallery — six blueprints compiled into the binary and seeded as
      `builtin` rows at daemon start, which is the first thing in this product to write that word.
      **Trusted without a signature check**, and that is the departure from what T78a expected of
      this task: a signature carried inside the same binary as the key that checks it proves nothing
      the binary has not proved already, so the signing half moved to T79a rather than being
      performed for the look of it. Seeding **compares before it writes**, on `bin/`'s rule — every
      CLI test in this workspace starts a daemon, and six file writes on each of those buy nothing.
      **Three of the six carry a command.** The other three ship none rather than one that half
      works: no cross-platform, non-interactive command exists for WordPress, and Django's would
      install into a Python every other project shares. A gallery command may not be interactive
      either, because T78a gave a scaffold no timeout on purpose.
      **The cross-OS criterion is capture's, not the gallery's** — a hand-written manifest is
      byte-identical on all three systems, so applying one says nothing about what a Windows machine
      writes. What proves it is a real capture taken on Windows, committed as a fixture, and applied
      by every system in the ordinary suite.
      Found on the way: the six files must be written in the renderer's own canonical form, since a
      hand-written one with comments would make the file here, the `manifest_toml` column and the
      file in a user's home three different texts for one blueprint.
- [x] **T79a** The gallery published as signed files — `<slug>.toml` and a `.minisig` beside it,
      under a moved `blueprints` tag in the packaging repository, signed with the key T78a minted.
      T78a's design placed this in T79; T79 compiled the gallery in instead, which removed the
      channel these signatures are for and left this as the task that restores it. Design:
      [docs/superpowers/specs/2026-09-02-t79a-signed-gallery-publication-design.md](../../docs/superpowers/specs/2026-09-02-t79a-signed-gallery-publication-design.md).
      **The manifests are never copied into that repository** — its workflow checks this one out at
      a ref and reads them there, so there is one gallery and not two.
      **What the task found, and the one behaviour change here.** `[blueprint] name` is *display*
      text: the six say `Laravel`, `Next.js`, `Static site`. Import with no `--name` filed a
      blueprint under exactly that string, so `validated_slug` refused every gallery file before the
      signature was ever reached — the headline of this task was broken for all six. A file is now
      filed under **its own stem**, which is also the only fallback that round-trips this product's
      own output, since everything it renders is written as `<slug>.toml`. T78a's test never saw it:
      its fixture is `borrowed.toml` named `borrowed`.
      **The step the index's publish does not need** is what the whole chain rests on: the run reads
      `blueprints::trust::PUBLIC_KEY` out of the checkout and fails before signing when it disagrees
      with the committed `blueprints.pub`. Verifying against the public half alone only proves the
      secret matched it; what decides whether a signature is worth anything is the constant the
      application compiles in. A half-finished key rotation is a red run instead of a published tag
      nobody can use.
      **Two things the moved tag forced.** `--clobber` deletes nothing, so a slug the gallery drops
      would keep a valid signature at a stable URL for good — and trust is decided when a blueprint
      arrives and never re-examined, so the orphan is pruned after every upload. And *created* is not
      *published*: the run downloads what it just uploaded and verifies that. `check-blueprints.yml`
      says weekly whether the published set is still master's.
      **What the six are for now that every home has them**: a blueprint an installed build does not
      carry, a correction between releases, and a file to read and fork. Replacing one of the six
      needs `--overwrite` and costs that slug its builtin refresh, which is T79's D6 doing what it
      was written to do.
- [x] **T79b** Say *why* a blueprint is untrusted — design in
      [docs/superpowers/specs/2026-09-02-t79b-why-a-blueprint-is-untrusted-design.md](../../docs/superpowers/specs/2026-09-02-t79b-why-a-blueprint-is-untrusted-design.md).
      A file whose signature did not verify and a file that arrived with no signature at all used to
      produce one line — `untrusted: nothing vouches for it, and nothing will` — and they are not
      the same event: the first is a manifest edited after somebody signed it, which is what the
      gallery key exists to catch. `SignatureCheck` (`verified` / `missing` / `rejected`) now rides
      beside `trusted` on `BlueprintSummary` and `BlueprintPlan`, out of a `signature` column added
      by `0015`, and `mix` says it at import, in the `TRUST` column (`signed` / `unsigned` /
      `mismatched`) and in the question asked before a `[scaffold]` command runs. **Trust is still
      decided once**: this is a reason beside the answer, never a re-check.
      **The reason had to be a column, not a field on a response** — `blueprint.list` reads rows,
      so a test asserting only what `import` answered would stay green with the migration broken;
      the daemon test reads the listing back for that reason. **`ON CONFLICT DO UPDATE` is where a
      stale reason would have come from**, and its test fails when that one line is removed:
      without it, re-importing an unsigned file over a verified row leaves `trusted = 0` beside
      `signature = 'verified'`. The migration backfills only the knowable half — an `imported` row
      that is trusted can only have come from a signature that verified; an untrusted one is either
      of the other two, and stays NULL rather than guessed. **A fourth variant for "signed by
      another key" was refused**: the only thing that could tell it from "signed by the gallery and
      then edited" is the key id inside the `.minisig`, which is not authenticated — whoever edits
      the file edits the key id with it, so the sentence says "it is not the gallery's", which is
      true of all three failures the verifier folds together. Pinning the reason into
      `ScaffoldConsent` was considered and declined, with the reason and the case that would reopen
      it written into the design's D9.

- [x] **T80** Extension model: `extension.toml` read through the `ServiceSpec` vocabulary in
      `mixengine-proto`, the four kinds, and permission enforcement — design in
      [docs/superpowers/specs/2026-09-02-t80-extension-model-design.md](../../docs/superpowers/specs/2026-09-02-t80-extension-model-design.md).
      Nothing is installed and nothing is stored: what this leaves T81 is a format already proved to
      make sense, and one read-only way to see it — `extension.inspect`, and `mix extension inspect`,
      which renders the manifest into the `ServiceSpec` that *would* run rather than describing one.
      That is `apply --dry-run`'s position: a plan is worth having because it was computed.
      **`network = "loopback"` is enforced, and by having nothing to enforce**: a manifest may not
      write an address at all. `{listen}` renders from `permissions.network` and from nothing else,
      and a host spelled out anywhere in the file — `127.0.0.1` included, which is the one an author
      would write in good faith — is refused at parse. The alternative was a column consulted
      wherever exposure could happen, which is a rule to remember at every future site that could
      expose something, and T76 is the task that measured what one forgotten check of that shape
      costs. `filesystem = ["own-data"]` is enforced the same way: it *is* the placeholder
      vocabulary, because every path must grow from `{install_dir}` or `{data_dir}` and a manifest
      naming an absolute path is refused before anything reads it.
      **The scoped token this line used to promise was refused** — [ADR
      0014](../decisions/0014-an-extension-is-not-an-api-client.md). An extension runs as the user's
      own account, and the access control on the endpoint *is* the account, so a token it held is
      one it could put down and open its own connection instead; making it a boundary means a token
      on every connection, `mix` included, which is the second access-control story T8 already
      refused for a case nobody has. No extension in the plan calls the daemon API. `[permissions]
      services` stays as a **declaration shown before an extension is installed**, `[scaffold]`
      consent's shape, and every surface that prints it says so.
      **Three documents this task found wrong.** `features/extensions.md` and
      `architecture/process-supervision.md` (twice) said a `ServiceSpec` deserialises out of an
      `extension.toml`; it cannot — sixteen fields against four, no `ServiceId`, and every path and
      address a template — so the manifest is its own type over the shared *vocabulary* and the spec
      is built through the builder, which is T77's finding arriving a second time.
      `security-model.md`'s bullet was a promise and is now the decision, which is that document's
      own opening sentence applied to one of its own lines.
      **`[recipe]` accompanies any kind**, because T82 asks for Mailpit *with* a `sendmail_path`
      recipe and two extensions for one product would be two things to install and uninstall in step;
      `kind = "recipe"` means an extension that is only that. And **an extension id a compiled-in
      recipe already claims is refused** here rather than discovered when T81 writes the row.
      Two smaller things found by running it: a rendered path used to mix separators on Windows
      (`…\mailpit/mailpit`), so the path that begins at a placeholder is now spelled the way this
      system spells one, up to the next whitespace — an *argument* is left exactly as it will be
      passed; and `mixengine-daemon/src/extensions.rs` was already taken by PHP extensions, so it is
      `php_extensions.rs`, which is what it was always about.
      **What T81 is handed**: a `services` row has `Origin::Package` or `Origin::RuntimeInstall`
      with a `CHECK` that exactly one is set, and an installed `service` extension is neither — the
      third origin arrives with the task that writes rows, not with this one.
- [x] **T81** Extension registry client + install/uninstall/start/stop — design in
      [docs/superpowers/specs/2026-09-02-t81-extension-registry-and-lifecycle-design.md](../../docs/superpowers/specs/2026-09-02-t81-extension-registry-and-lifecycle-design.md).
      `extensions.json` is a second signed document beside `index.json`, under the same tag and the
      **same key**: an extension has the package index's blast radius exactly — a binary downloaded
      and supervised — so a key of its own would separate nothing. Two documents rather than one
      array, for failure isolation: an entry a newer build published has to be skippable, and
      skipping it inside the document that also lists every runtime means `mix runtime list` can die
      of an extension. `index::Client` is generic over its document rather than copied, because two
      copies of a verify-then-parse path is one copy that eventually skips a step.
      **An entry *is* a manifest**, which is what lets the permissions question be asked before a
      byte of artifact is fetched — asking afterwards is asking after doing the thing somebody was
      about to refuse. The `Error::Index*` family stays one family and gains `document`: a test
      caught a registry served by the wrong key being refused with *"the package index … is not
      signed"*, which sends the reader somewhere they can do nothing about.
      **Four things the task found.** `0001` had reserved an `extensions` table whose every column
      was wrong for what T80 turned out to need, and nothing had ever written to it — dropped on
      0006's reasoning rather than migrated. `{data_dir}` had to move out of `{install_dir}`,
      because *"an uninstall keeps your captured mail"* is not a promise a nested layout can keep.
      A port kept anywhere SQL cannot reach is a port handed out twice — so `extension_ports` is a
      table, and both allocators now ask one query. And the allocation lock is not reentrant:
      holding it across `services::create` is a daemon that stops answering, which the tests found
      by hanging rather than failing.
      **The migration is the riskiest thing here**: `services` is rebuilt for the third origin, and
      the two tables pointing at it would be damaged differently by a drop with foreign keys on —
      `sites.php_service_id` is SET NULL, `site_service_links.service_id` is CASCADE and deletes
      rows leaving nothing about a site to look wrong. `PRAGMA foreign_keys` is a no-op inside a
      transaction, so 0016 is a `-- no-transaction` migration that opens its own.
- [x] **T81a** Publish `extensions.json` from the packaging repository, on T79a's shape: the
      workflow checks this repository out at a ref, renders each `data/extensions/<id>.toml` through
      the reader that verifies it, signs with the index key, and proves the committed `minisign.pub`
      is the one this build compiles in before it signs anything. T81 verifies with a key its own
      tests mint, which is what proves the verification path rather than switching it off — but
      until this lands there is nothing published to install. Design:
      [docs/superpowers/specs/2026-09-02-t81a-publishing-the-extension-registry-design.md](../../docs/superpowers/specs/2026-09-02-t81a-publishing-the-extension-registry-design.md).
      **The roster lives over there, not here**, which is where this parts company with T79a: that
      task read its manifests out of a `mixengine` checkout because the gallery *is* compiled into
      the binary and a copy would have made two galleries. Nothing of the sort holds for extensions
      — no manifest is compiled in, `manifest::read` is a format rather than a roster, and what an
      entry describes is a third-party artifact at a URL with a hash, which is what that repository
      already exists to describe. A Mailpit version bump has no business being an application
      release.
      **The key chain is held rather than scraped.** `tools/blueprints.py` pulls `PUBLIC_KEY` out of
      `trust.rs` with a regex and has to carry a failure mode for the regex missing; the generator
      here is compiled *from the checkout being published*, so the constant it compares
      `minisign.pub` against is the constant that build checks with. Nothing to scrape, and no branch
      for the scrape failing — T79a's D3 with the moving part removed.
      **One rule and not two.** "Two files may not claim one id" was written into the design and then
      not implemented, because `<id>.toml` already implies it: a directory holds one `mailpit.toml`.
      Writing the check anyway would have been a branch no input can reach. The testkit's
      `sendmail.toml` declares `sendmail-to-mailpit`, so it is a ready-made case for the stem rule
      rather than a fixture the roster could take.
      **The empty document is published now** rather than waiting for T82 to have something to lose.
      A dry run rehearses everything except the four things that actually break — the secret, the
      tag, the asset URL and the download-and-verify — so they are exercised while the cost of
      getting them wrong is nothing, and `mix extension available` answers "no extensions" instead of
      an index error from the day this merges.
      Found on the way: `Timestamp::parse` was private and reachable only through `Deserialize`. A
      generator has to *make* a timestamp, and this workspace has no date library on purpose, so the
      type grew `FromStr` and the shell's `date -u` writes the text.
- [ ] **T81b** The site a `web-app` extension is served on. `sites.project_id` is `NOT NULL` and
      `served` joins `projects.root_path` for an absolute doc root; an administrative interface
      belongs to no project, so this is a schema question — a nullable parent with an
      `extension_id` beside it, against an internal project row that would show up in `mix project
      list` and cascade badly if somebody deleted it. Held back from T81 so that one PR does not
      carry two table rebuilds. **T82's phpMyAdmin and Adminer need this.**
- [ ] **T81c** Wire `[recipe] front_end` fragments. T81 refuses one by name rather than accepting a
      manifest whose stated effect does not happen: both front-end templates would have to grow an
      `import` and each rendering be revalidated against the real server, and nothing in T82 asks
      for one.
- [ ] **T82** First extensions: Mailpit (with the `sendmail_path` recipe for every managed PHP),
      phpMyAdmin, Adminer.
- [ ] **T83** **MixDB integration**: detect an installed MixDB behind the platform layer, a daemon
      method answering the connection handoff for one database service and the `mix` command that
      asks for it, credential read from the keyring at that moment and never placed in an argument
      or a URL, "not installed" answered as a state rather than an error. **(P)**
- [ ] **T84** MixDB as a `desktop-app` registry entry + a shared keyring naming convention.

**Milestone M8** — capture a project as a blueprint, apply it to a new one, open its database in
MixDB, and test it from a phone.

---

Previous: [Phase 7 — Efficiency](phase-7-efficiency.md) · Next: [Phase 9 — Ship](phase-9-ship.md)
