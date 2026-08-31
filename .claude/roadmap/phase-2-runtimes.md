# Phase 2 — Runtimes

*Goal: multiple PHP/Node/Python/Ruby versions installed and selectable.*

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [x] **T20a** One real artifact and one real index, before a client is written against either. **(P)**
      **Ordered before T20 although it is lettered after it**, on T19c's precedent: T20–T24 are a
      client for a package index nobody had produced, and a client written against a sketch is a
      client written against a guess. What exists now is not a sketch — PHP 8.3.33 for four targets
      and a signed `index.json` describing exactly those, in
      [`mixengine-packages`](https://github.com/mixnz/mixengine-packages). The evaluations
      are written up in [../operations/runtime-packaging.md](../operations/runtime-packaging.md) and
      **this file does not repeat them**; what follows is only what that page cannot carry.
      **Its first output was a decision, and the decision was borrow.** PHP is repacked from
      windows.php.net on Windows and built with `static-php-cli` (MIT, 115 extensions, `cli` and
      `fpm`) on macOS and Linux. The macOS cell the plan feared most — `install_name_tool` over every
      bundled dylib followed by a re-sign that Ventura would reject — was never entered, because
      arm64-only puts the floor at 8.1, which is exactly where `static-php-cli` starts. Two limits
      that were chosen for unrelated reasons agreed, and a whole class of work disappeared.
      **One premise of the task was false, and finding that out was worth more than the artifacts.**
      It reasoned that an artifact its publisher already signs might run under Smart App Control
      where one we built is refused, and said to weigh that on the borrow side. Measured:
      `php.exe`, `nginx.exe` and `caddy.exe` are **unsigned**, `node.exe` is the only signed binary
      in the table. So the SAC risk is identical on both sides of every cell but one, borrowing wins
      on maintenance cost alone, and [T41a](phase-4-sites-and-elevation.md)'s answer now governs the
      whole table rather than the half we build.
      **The one thing this task was told to do and did not** is run that smoke test on a machine with
      SAC **enforced**. Nobody has one — SAC cannot be re-enabled once turned off, so it needs a
      fresh install — and the measurement above is why it is not worth blocking on: it would tell us
      about *every* artifact MixEngine ships rather than about a choice this task makes, which is
      T41a's question and not this one's. It moves there rather than staying open here.
      **Four bugs, and only one of them was found on a developer machine.** The interesting one is
      recorded as a rule in the packaging doc rather than here, because it outlives this task: PHP's
      ini parser rejects `~` in an unquoted value, Windows puts one in every 8.3 short path, and the
      failure is silent — every extension stops loading while `php -v` keeps answering. It passed
      locally and failed on the runner, which is the whole argument for the runners being the build
      machines. The other three: `php-win.exe` answers `-v` with nothing at all; `--build-shared`
      links what `download` already fetched and refuses at the *end* of a ten-minute build if it did
      not; and `static-php-cli` resolves two dozen libraries through an unauthenticated GitHub API
      that allows 60 requests an hour per IP, shared across everything Azure is running.
      **`SPC_LIBC=glibc` is forced, and it costs a floor.** Static musl has no `dlopen`, so the
      tool's default Linux output cannot load an extension at all — which would make T28 impossible
      on Linux. Choosing glibc means the binary needs a glibc at least as new as the machine that
      built it, so Linux builds on the oldest image GitHub still offers rather than the newest, and
      the requirement is *measured* off the finished binary and carried in the index. A floor read
      from the build machine would have been a guess that stayed conservative until it did not.
      Left for the tasks that need them: the index has **one runtime and one version in it**. The
      range the version policy promises — 7.0 upwards on Windows, 8.1 upwards elsewhere — is a
      matter of running the same workflow with a different argument, except for the Linux 7.x cell,
      which is **T27a** because it is the only part that costs a pipeline. Nothing here is a client:
      T20 fetches and verifies this index, T21 downloads from it.
- [x] **T20** Package index client: fetch, Ed25519 signature verification, 6-hour cache, offline mode.
      Written against what T20a produced, not against the schema that preceded it — and the two
      differ in exactly the places a client trips over, which is what ordering T20a first bought.
      `provides` is a map from executable name to its path inside the archive rather than a list of
      names, because a borrowed archive keeps its publisher's layout and the daemon has to know
      *where* a binary is; and `requires` hangs off the artifact rather than the package, because a
      Windows PHP needs a VC++ redistributable and the same version on Linux needs a glibc.
      **This is the first outbound request in the workspace, and therefore the first TLS.** `hyper`
      was already here for the local IPC socket, and `mixengine-supervisor` refuses an `https://`
      health check rather than pull a certificate store in for `127.0.0.1` — a reason that stops
      applying the moment something has to reach a CDN. The root store decision is written up in
      [../standards/rust.md](../standards/rust.md); the short version is that `reqwest`'s default
      already uses the OS verifier, so being right here cost nothing but knowing.
      **One verification path, two sources.** An index read from the cache goes through the same
      check as one read from the network. Re-verifying a file this process wrote a minute ago looks
      redundant and is not: the cache is an ordinary file in the user's home, and a client that
      trusts it because it trusted the network once has moved the boundary from "we signed this" to
      "nothing on this machine touched it". A test rewrites the cache and watches the client go back
      to the network rather than serve it.
      **Signature first, parse second**, which is an ordering and not a style: a JSON parser is a far
      larger attack surface than an Ed25519 check, and running it on unverified bytes hands that
      surface to whoever answered the URL.
      **Two decisions about what an old index means**, neither of which the one-line description
      above contains. A cache past its six hours with no network is **served, with a warning** —
      the document is still one we signed, and a tool that can list nothing because the wifi is down
      is worse than a version list two days old. And an index whose `generated_at` is *older* than
      the cached one is **refused, and the cache kept**: every index we ever published verifies
      against the same key, so the signature cannot tell a replayed copy from before a security
      release apart from the current one, and one comparison can. Done now because adding it later
      means deciding what to do about caches already holding a newer document than the server has.
      Those two converge on one rule the code states once: **every way of failing to obtain a new
      index falls back to the last verified one and says why.** Unreachable server, bad signature,
      unreadable document, rolled-back timestamp, read-only cache directory — they differ in what a
      person should do and not at all in what the call can do next.
      **`generated_at` is parsed rather than string-compared**, because it decides whether an index
      is a rollback and lexicographic order is silently wrong for `+00:00`, for a fractional second
      and for an unpadded month — all valid RFC 3339. The accepted shape is narrowed to the one the
      generator emits and everything else is refused; this workspace still has no date library, for
      the reason T22 recorded, and thirty lines beat a civil-calendar dependency bought to parse back
      what we ourselves wrote.
      **`cache/` is a new directory and not `run/`**: `run/` is scratch belonging to the daemon
      currently running, while the entire value of a cached index is surviving a reboot.
      `MockRegistry` finally exists too — T11 deferred it here on purpose. It generates **its own**
      keypair, which is what forced the product's public key to be injectable rather than hard-wired,
      and signs with the real `minisign` crate so the client is proven against what minisign
      produces rather than what we believed it produces.
      Left for the tasks that need them: **no CLI and no artifact download.** `mix runtime` is
      T23's, on the same split T19a/T19b and T22 all used. One test does reach the internet and is
      `#[ignore]`d — the only one in the workspace that does — because it checks the one thing no
      mock can: that the compiled-in key still verifies the index actually published.
- [x] **T21** Download pipeline: resumable download, SHA-256 verification, staging dir, atomic rename,
      rollback on failure, post-install smoke test.
      **The six items above are one transaction, and putting them in one function is the design.**
      The rename is its commit; everything that can still refuse the install happens while the tree
      is in a staging directory nothing looks in. That is what decided where the smoke test goes:
      run after the rename it would be a check on something already installed, and its failure
      would have nothing to undo except a directory a client may already have been told about.
      **The two halves keep opposite promises about surviving.** A `.part` file is named after the
      hash the index publishes, lives in `cache/downloads/` and is kept through a failure, a
      cancellation and a daemon restart — resuming is the whole point, and somebody who cancels at
      sixty percent has not asked for those bytes to be thrown away. The staging directory belongs
      to one attempt and is removed by anything that goes wrong, *including the next attempt finding
      one*, which is what a killed daemon leaves. Two cases exist only to keep the resume from
      becoming a loop: a `.part` that cannot verify is deleted, or every retry would arrive at the
      same wrong answer forever, and a `416` means what is on disk is longer than what the server
      has and cannot be a prefix of it.
      **A `200` in reply to a `Range` request is the case worth knowing**: the server ignored the
      header — a CDN edge, a proxy that strips it — and the body is then the whole file, so
      appending it to what is on disk builds something that is neither. The checksum would catch
      that, after the entire download.
      **The smoke test is what a checksum cannot do.** A hash proves the bytes are the ones we
      published and says nothing about whether this machine can execute them; every failure the
      index's `requires` field *describes* — a missing VC++ redistributable, a glibc below the
      measured floor, an architecture the loader refuses — is invisible until something tries. What
      to run arrives as a `SmokeTest` rather than being decided here, because "which flag prints the
      version" is a fact about PHP and Node.
      **Unpacking is the one step where the archive would otherwise choose where its bytes land.**
      The entry paths are checked before `tar` or `zip` is asked to write, although both already
      refuse to escape — so the refusal is ours and has a test, rather than being a property of
      whichever crate version resolved this week. What is deliberately *not* reimplemented is
      everything below the path: mode bits, symlinks, hardlinks are the OS's business, and
      delegating them to the crate whose job it is *is* the platform abstraction. `tar`'s
      `preserve_permissions` is off by default, which would have produced a `php` that could not be
      executed — a failure that surfaces somewhere else entirely.
      **Five decompressors and a hash, and two of the six are chosen against the obvious version.**
      `sha2` is pinned to the 0.10 `sqlx` already brings rather than the current 0.11, which would
      have put a second copy of it and of `digest` in the tree for four calls. And zstd is `ruzstd`
      in the product — decode-only, pure Rust, no C toolchain for the aarch64 Windows target — while
      the fixture writes with the C `zstd`, so what ships is proved against what a *different*
      implementation produced. That is `minisign`/`minisign-verify`'s split, one layer down.
      Left for the task that needs it: **no job and no method.** `runtime.install` is T23's, so this
      shipped without becoming the job system's first producer after all — a producer wired up
      before any client could reach it would be one nothing could test end to end. `Watcher` is
      shaped exactly like the daemon's `JobHandle`, so T23's wiring is an impl and not an adapter.
- [x] **T22** Job system: `jobs` table, `JobProgress`/`JobFinished` events, `job.wait`, cancellation.
      **Done before T20a and T21, out of the order above**, because it is the one task in this phase
      that needs nothing from outside the repo: T20a waits on a signing key and a place to publish
      artifacts, and this waits on nothing. What it costs is that the registry ships with **no
      producer** — the first is T21's download, and T23's `runtime.install` is the first method to
      return a job. That is T19's position exactly, which built the runner before anything could
      declare a service, and for the same reason: the alternative is writing the loop once inside the
      first producer and once properly afterwards.
      **`jobs.state` closes a promise `0001_initial.sql` made in its own header.** That file said the
      column was deliberately un-`CHECK`ed because the state machine "belongs to T22 and does not
      exist yet"; it exists now, four states, closed in Rust for the reason `ServiceState` is, with
      the column carrying the same list. `jobs.kind` stays free text on `packages.name`'s precedent —
      the set grows with every phase that has something long to do, and from T80 with every extension
      — and what keeps it honest is the rule that **a kind is the method that produced it**, so there
      is one vocabulary rather than a second one to keep in step with the method names.
      **T22 is the first task that had to write an `_at` column at runtime**, and it found that this
      workspace still has no date library: every other one is a literal in a fixture. So
      `jobs.started_at` and `finished_at` are epoch milliseconds, joining `services.last_started_at`
      — the same argument T15 made, reaching the same answer, and both are moments the daemon does
      arithmetic on rather than shows. The alternative was a civil-calendar dependency bought to
      parse back what we had just formatted.
      **A job does not survive the process running it, which makes recovery a different problem from
      T18's.** A service is a process of its own and can outlive the daemon that spawned it, so
      recovery there asks the OS what survived and adopts it. The work behind a job is a task
      *inside* the daemon: a row still saying `running` at boot means one thing only, and there is
      nothing to adopt and nothing to signal. `core::jobs::abandon` closes those before the first
      client is served, as **failed** rather than cancelled — nobody asked for the work to stop.
      **Cancellation is cooperative and there is no `Cancelling` state.** Nothing kills the work: a
      download half way through a file has a staging directory to remove, and a task dropped
      mid-`await` does not remove it. So `job.cancel` cancels a token the work watches and answers
      with the job *as it stands* — which may still say `running`, because claiming an outcome this
      daemon has not seen would be the same mistake T19a fixed in the service walk. A state between
      the asking and the ending would have to be written by every producer, and there is no producer
      yet to say whether it is wanted.
      **Work that finished while being cancelled has finished**, which is T15a's stop-command reading
      one layer up: the outcome is judged by what the work produced, and only work that gave up is
      recorded as cancelled — otherwise a download that completed in the instant somebody clicked
      cancel would be recorded as though it had not.
      **`job.wait` is the one method in this API that blocks on purpose**, and the timeout is what
      keeps that inside the rule rather than outside it. A wait that runs out is an **answer** and
      not an error, on `ServiceWalk::complete`'s precedent; the daemon caps what it grants, so a
      client asking for an hour does not hold a connection for one. The row is read *after* the wait
      on both paths, because a job that ended while the caller was being polled has ended.
      **The ordering `wait` rests on is written once, in `Jobs::ended`**: persist, announce, then let
      go of the entry. A waiter is released by a token the registry cancels only after the row is
      written, so it reads the ending rather than racing it — and "is there an entry" is never true
      for a job whose ending is not yet readable, which is what makes the no-entry path able to
      answer from the row alone.
      **Two smaller decisions, each where it is paid for.** `Api::new` reached eight arguments, which
      is the growth the `Shutdown` type's own note predicted, so the two registries became one
      `Supervision` argument rather than a lint being silenced. And jobs are waited for **beside**
      the services at shutdown rather than after them: they hold different things — a port and a data
      directory against a staging directory — and sequencing the two waits would add one budget to
      the other, which is the arithmetic T9a's single budget exists to prevent. A job that will not
      stop inside it is left, and the next daemon's `abandon` closes its row.
      Left for the task that needs it: **no CLI**. `mix job list|status|wait|cancel` is T23's to add
      beside `runtime.install`, on T19a/T19b's split — the wire surface and the client are separate
      tasks, and a client for a namespace nothing can produce a row in would be untestable end to
      end. `core::Error::NotFound { kind: "job" }` therefore names a namespace whose CLI does not
      exist yet, which is the one thing here that is true early rather than wrong.
- [x] **T23** `runtime.install|uninstall|list_installed|list_available|set_default` — **PHP first**.
      **The task that made the three before it reachable.** T20 verified an index nobody asked for,
      T21 installed an artifact nobody named and T22 ran jobs nobody started; what this adds is a
      method in front of each, so the wiring is an `impl` rather than an adapter — `Watcher` was
      shaped after `JobHandle` at T21 precisely so that it would be.
      **One of the five returns a job and four answer inline**, and the split is the download and
      nothing else. Removing a directory, reading a table and moving a default are none of them long,
      and making them jobs would make every client learn a second protocol to hear an answer that was
      ready before it asked.
      **An install already running is answered with the job running it**, rather than started twice
      or refused. Two calls for one version is what two terminals or a double-clicked button produce
      and the second is asking for the same outcome — but both would append to one `.part` file named
      after the artifact's hash, which is a download that can only fail its checksum. The check and
      the start are one decision under one `tokio` mutex, because two callers arriving together
      otherwise both find nothing.
      **A version is a validated path component and not a string.** It names
      `runtimes/<kind>/<version>/`, so `RuntimeVersion::parse` refuses anything that could be
      somewhere else — and the rule that does most of the work is *it begins with a digit*, which
      excludes `.`, `..`, `-rf` and every name Windows reserves at once, while accepting every version
      these four upstreams have ever published. `ServiceId`'s reasoning, arriving from the other side
      of the wire.
      **The first version of a kind becomes its default, and no later one does.** A home whose only
      PHP is not the default is a home where `php` resolves to nothing; an install that silently moved
      what `php` means would break a project the user was not thinking about. The other half of that:
      uninstalling the default **promotes nothing** and says so. Choosing a successor means deciding
      which remaining version is *newest*, which needs T24's grammar — and one chosen by row order and
      described as the newest would be a guess wearing a fact's clothes.
      **Two directions of ordering, and neither is a transaction**, because there is no such thing:
      SQLite cannot roll back a rename. Install into place then write the row; remove the directory
      then delete the row. What that buys is that the surviving failure is always the harmless one — a
      directory with no row is invisible and costs disk, a row with no directory is a runtime that
      fails when somebody uses it. The single exception is the install whose row will not write, which
      removes what it just installed: it is the one moment we *know* an orphan exists, and leaving it
      would make the retry that fixes everything else fail with `already installed`.
      **The daemon grew two flags, and they only make sense together.** `--index-url` without
      `--index-key` is a setting that can only ever fail, since nobody else can sign with our key —
      `clap`'s `requires` says so. Overriding the pair is trusting a different publisher, which only
      somebody who already controls how the daemon starts can do, and a daemon started that way says
      so in its log. They also made the end-to-end tests possible at all: `MockRegistry` over loopback
      is what stands in for a network the test suite forbids.
      **Every index and install error was `internal` until now**, because nothing could reach one.
      Classifying them is most of what landed in `error.rs`, and the split that decides each is *whose
      fault it is*: a document or an archive that verified and is then unusable is one **we** published
      (`internal`), a signature or a checksum that does not match is somebody between us and them
      (`precondition_failed`), and a runtime that will not start on this machine is
      `dependency_missing` — which is exactly the shape of a missing VC++ redistributable and of a
      glibc below the floor.
      `Timestamp::to_rfc3339` is here for the reason T22's epoch milliseconds were there: this is the
      first task that had to *write* an ISO-8601 `_at` column at runtime rather than as a fixture
      literal, so the civil-calendar arithmetic is thirty lines in `mixengine-proto` rather than a
      date dependency in the crate every client links.
      Left for the tasks that need them: **no uninstall refusal and no `--force`.**
      [runtime-versions.md](../features/runtime-versions.md) says an uninstall refuses when a project
      pins the version or a site uses its php-fpm pool, and neither exists yet — projects are Phase 4
      and pools are [T32](phase-3-services.md). A refusal nothing could trigger, and a flag with
      nothing to force past, would both be guesses about a shape those tasks have to live with.
      *(T32 landed the pool half: an uninstall now refuses while the pool is running and removes its
      row when it is not. `--force` is still unwritten, and now has something to force past.)* **No `runtime.resolve`** either: it
      is listed in the architecture's namespace table and it is T24's, because resolution is the
      constraint grammar and not a lookup.
- [x] **T24** Version resolution (`core::resolve`): flag → `mixengine.toml` → project record → default;
      exact/minor/caret constraints.
      **The grammar went to `mixengine-proto` and the order stayed in `core`**, which is the one
      structural decision here and is not where the task name points. A constraint travels on the
      wire — `runtime.resolve` takes one — and that crate validates everything it carries, on
      `RuntimeVersion`'s own reasoning: `^8.3` arriving as a `String` is refused somewhere further
      in, with the request already half honoured. What stayed in `core::resolve` is which of four
      sources a constraint came from, because that reads a file and a table.
      **Two orders on one type, and the derived one is not the version order.** `RuntimeVersion` is a
      `BTreeMap` key in the daemon, so `Ord` stays the string's; `cmp_precedence` is the answer to
      *which is newer*, and the two disagree on `8.10.0` against `8.9.0`. Overriding the derive would
      have silently changed what that map keys on.
      **The surprise a version grammar always has is pre-releases, and the rule is one sentence**: a
      constraint with no pre-release in it never selects one. `8.5` is not `8.5.0RC1` and neither is
      `^8.5` — somebody who wants a release candidate names it. Without that, one machine holding an
      RC resolves differently from every other machine in the team.
      **`8.3` is a prefix rather than a third form.** One rule covers `8`, `8.3` and `8.3.12`: as
      many segments as were written have to agree, and a segment nobody wrote is a zero. The caret is
      the only range, and it stops at the leftmost *non-zero* segment — `^0.12` ends at `0.13`, which
      is Node's own 0.x line and not a hypothetical.
      **A manifest that says nothing about a language is not an answer about it.** The walk continues
      past it, so a repository whose root pins PHP and whose subdirectory pins only Node keeps both.
      And both walks run to the top in turn — the whole manifest walk before the first project row is
      considered — because a file checked into the repository outranks a registration on this machine
      even when the registration is nearer.
      **Step 3 is implemented against a table that is empty on every machine.** There are no
      `project.*` methods until Phase 4, so nothing writes a `projects` row; it is written now anyway
      because the order is the contract, and a step left out would have its behaviour decided later
      by whichever task first needed it — against a shim that had already shipped. Its own test
      inserts the row by hand. What is deliberately *not* done there is canonicalising `root_path`:
      normalising on the way in is what makes one directory one project, and doing it on the way out
      would leave two spellings able to register twice with only one of them findable. That belongs
      to `project.create`.
      **`mix` reads `MIXENGINE_PHP`, not the daemon** — the one place this client reads the
      environment below `main`, and the exception states itself: the variable's *name* depends on the
      kind the user just named, so nothing above the parse knows which to look at. The name is
      `RuntimeKind::override_env` in `mixengine-proto` all the same, so the shim (T25) and the GUI
      read the same one. A daemon consulting its own environment would answer with whatever started
      it, for everybody at once. An empty value is "not set", every other value that is not a version
      is refused rather than skipped past — a variable that quietly does nothing is the exact failure
      this command exists to explain.
      **What a failed resolution says is the feature.** `dependency_missing`, the message naming both
      the constraint and the file that asked for it, and a hint that is the command to type: an exact
      pin becomes `mix runtime install php 8.1.30`, a range becomes `mix runtime available`, because
      inventing the version that would satisfy a range would be inventing a release. Constraints are
      matched against **installed** versions and never against the index — the alternative is a `cd`
      starting an eighty-megabyte download.
      Left alone deliberately: **uninstalling the default still promotes nothing**, and the note in
      `runtimes::forget` now says so on its own terms rather than by pointing here. The grammar was
      never the whole reason — an uninstall that silently moved what `php` means would break a
      project nobody was thinking about, which is the same argument that stops an *install* from
      moving it. **`runtime.list_installed` is still ordered by the version string**, for a listing's
      own reason: it is a table somebody scans, and the order that makes a row findable is the one
      the eye can predict.
- [x] **T25** Shim binary: name-based dispatch, in-process resolution without IPC, `exec` on Unix /
      Job-Object child on Windows, exit-code and signal passthrough. **(P)**
      **A crate of its own, and the one client that links `mixengine-core`.** `mix` refuses that
      edge on the argument that it can ask a daemon and a bundled SQLite is a poor trade for
      learning where a socket is; here the whole promise is the opposite — a version resolves with
      the daemon stopped, still starting, or never installed — so there is nothing to ask, and a
      connection would spend the 15 ms budget before the query started. The layering test carries
      the exception rather than a comment.
      **`provides` became a column, because T25 is the first reader that is not the installer.**
      The index has always said which file inside an archive is `php`, and until now it was
      consulted during the install and thrown away; a shim that guessed the layout would be
      guessing at the one thing a borrowed archive keeps from its publisher. Migration 0002 adds
      `provides_json` with `DEFAULT '{}'`, and a row from before it answers "publishes nothing
      recorded" rather than crashing — with the same path check `install::archive` already applies
      to an entry name, shared rather than restated, because what an archive was allowed to unpack
      and what a database is allowed to run have to be one rule.
      **The shim has no arguments and can have none.** Every one of them belongs to the program
      being fronted, so `--home` is impossible and so is `--explain`: anything printed on its own
      account is a line in the middle of somebody's `php -r`. The only inputs are `argv[0]` —
      **not `current_exe`**, which follows a symlink back to `mixengine-shim` and would make every
      command in `bin/` the same unknown one — and the environment.
      **Windows is the whole platform half.** There is no `exec`, so the program is a child in a
      Job Object with `KILL_ON_JOB_CLOSE` (a killed shim must not leave a `php -S` holding a port)
      and Ctrl-C is *swallowed by the shim*: a console event reaches every process attached to the
      console, so the child already has its own copy, and the default handling would end the shim,
      close the job, and kill the child in the moment it was deciding what to do. What is
      deliberately **not** treated as a failure is the assignment to the job: Windows refuses to
      assign a process that has already exited and reports it as `ERROR_ACCESS_DENIED`,
      indistinguishable from a real refusal — and for a shim in front of `php -v` that is the
      ordinary case, not an exotic one.
      **`Store::open_read_only` is a door and not a shortcut**: it does not create the file and does
      not migrate it, and SQLite enforces both rather than our remembering to. Its test is the one
      that had to exist — a clean close checkpoints the WAL and removes the `-shm`, which a
      read-only connection cannot recreate, so a shim that only worked while a daemon was running
      would pass on every developer machine and fail on the first `php -v` after a reboot.
      Left for the tasks that own them: **nothing fills `bin/`.** The shim is the binary; copying it
      per command name is T26's, beside putting that directory on `PATH`, and the table it will copy
      from is `core::shims::COMMANDS`. **No `PHPRC`, no `GEM_HOME`** — only the resolved program's
      own directory, prepended to `PATH` so a runtime's tools reach each other; the rest are files
      T28's `conf.d` model generates, and a variable pointing at a file nothing writes is worse than
      no variable. **No `composer`** in the table, for the same reason: it is not inside any
      artifact.
      Not measured properly, which is **T29**'s: with both binaries warm, thirty runs of the shim
      against a real home and thirty of the same binary exiting immediately were within a
      millisecond or two of each other on this machine — process creation dominates, and the
      resolution itself did not stand out of the noise. That is a reason to believe the design
      fits the budget, not a benchmark.
- [x] **T26** PATH integration for `<root>/bin`, reversible — **and filling it**: one copy of the
      T25 binary per name in `core::shims::COMMANDS`, which is what turns that binary into commands
      a person can type. **(P)**
      **The two halves have opposite policies about being done without being asked, and that split
      is the whole design.** Filling `bin/` is inside the root: it is a projection of a table
      compiled into the binary, exactly as `etc/` is a projection of the database, so the daemon does
      it on **every start** and a home whose `bin/` was emptied is repaired by starting the daemon.
      Putting that directory on the PATH writes a file in the user's home or a value in their
      registry hive — outside the root, and outside what
      [../architecture/overview.md](../architecture/overview.md) lists as MixEngine's to write on its
      own account — so it happens only when `path.install` asks. A daemon that edited `~/.zprofile`
      because it started at login would be a program that changed the shell of somebody who had only
      installed it.
      **`PathIntegrationApply` came *off* the privileged-operation list rather than being
      implemented.** All three systems keep the current user's PATH somewhere that user can already
      write. The one exception was `/etc/paths.d`, which the platform document named for macOS and
      which is root's precisely because it is machine-wide — the opposite of what a per-user tool
      wants. So this is an ordinary API method and nobody types a password to add a line to their own
      profile. The list is closed against *additions*; a removal is a promise kept more cheaply.
      **The lie a user-level PATH cannot avoid is precedence, and it is written down rather than
      papered over.** Windows composes a process's `PATH` as the machine value followed by the user
      one, so a PHP installed for the whole machine stays ahead of `<root>/bin` whatever we write;
      prepending inside the value we own is as far as either system reaches without touching
      something that is not ours. "Something else is answering `php`" is T47's to report.
      **`setx` is not used, and the reason is a data-loss bug rather than a preference.** It
      truncates the value at 1024 characters — a limit of the tool and not of the registry — and a
      developer's user `Path` past a kilobyte is ordinary. `RegSetValueExW` has none, and two other
      things are preserved with it: the value's *type*, since writing a `REG_EXPAND_SZ` back as
      `REG_SZ` turns every `%USERPROFILE%\go\bin` into a directory that does not exist, and every
      entry that is not ours, joined back verbatim down to the empty segments.
      **On Unix the block is written to every profile that exists, not to one.** Which file a login
      shell reads is decided by a shell this process is not, and a home with both a `.bash_profile`
      and a `.zprofile` belongs to somebody who uses both — so `PathState::complete` is *every*
      location and not any of them, which is what names the half-state where one terminal finds `php`
      and the next does not. The block carries a POSIX `case` guard because a login shell inside a
      login shell is an ordinary thing and an unguarded prepend grows `PATH` every time; the quoting
      inside that pattern is what lets a directory containing `*` or `[` match itself. **`fish` and
      `nushell` are not covered** — neither reads a POSIX profile — and `path.status` names the files
      it did write, so the absence is visible rather than silent.
      **`bin/` is swept as well as filled, which is only defensible because the directory is
      entirely ours.** A name no command answers to is a command that was renamed or dropped between
      releases — a program that exists, runs, and refuses to be anything — and it would otherwise sit
      on the user's PATH forever. Replacing a copy something is *running* is the Windows case: the
      file cannot be overwritten and can be renamed away, so it is, and the moved copy is rubbish the
      next sweep collects. A refresh compares length and modification time rather than bytes, so the
      ordinary start stats nineteen files and writes none.
      **The refresh happens before the endpoint is bound**, unlike T18's and T22's recovery passes,
      and the reason is the endpoint rather than the work: a bound named pipe that is not yet in
      `accept` has exactly one pending connection on Windows, so every moment between the two is a
      moment a second client meets `ERROR_PIPE_BUSY`. Recovery is a database read; nineteen file
      copies are not, and putting them after the bind made an ordinary parallel test run fail.
      **They are nineteen *names* now rather than nineteen copies, and that was a CI failure before
      it was a saving.** The reasoning above bounds when the fill happens and said nothing about what
      it costs; a `mixengine-shim` carrying its debug info is tens of megabytes, so a first start
      moved most of a gigabyte before it bound anything — and a suite that gives every test a home of
      its own pays that per test. Four daemons on one Linux runner took thirty seconds each to
      answer and the fourth was still copying when the client waiting for it gave up. So a shim is
      placed as a hard link to the shim binary wherever the filesystem gives one file a second name,
      and as a copy of its bytes only where it does not. Everything the refresh rests on survives it:
      a link shares the length and modification time it is compared on, so the next start still
      writes nothing, and a build that *replaces* the shim binary leaves the links on the older file
      they were made from, so every one of them is replaced. **Never on Windows, and the shim's own
      behaviour is what decides that** — it stays alive as the parent of a Job Object child instead
      of `exec`ing away, so a link would let a `php -S` somebody left running hold the shim binary
      itself open against the next upgrade, which is the one case the move-aside above cannot answer
      because there would be nothing to move aside.
      Left for the tasks that own them: **`path.install` has no end-to-end test and will not get
      one.** It writes the PATH of the account running it, so a suite that called it would be a
      `cargo test` that edits the environment of whoever ran it. The two real implementations are
      proved in `mixengine-platform` against a registry key and a home directory each test creates
      and deletes; the dispatch is proved against `mock::Host`; and `crates/mixengine-cli/tests/path.rs`
      covers what is left — that a daemon which has started has filled `bin/`, and that
      `mix path status` reads the real machine. **No `mix doctor`**: "your PATH says `<root>/bin` but
      something else is answering `php`" is T47's, and so is repairing a `bin/` entry that could not
      be removed. **Nothing calls it on the user's behalf yet** — a first run that offers to install
      the PATH entry is T47's, and it calls the same method.
- [x] **T27** Node.js, Python, Ruby support in the same pipeline. Taken one language at a time on
      purpose: the three are borrowed from three different publishers with three different
      relocation stories, and doing them together would have made one recipe's surprise look like a
      property of the task. Ruby on macOS and Linux is the one part that cannot be borrowed from
      anybody, and it was **T27b** below for the reason T27a was carved out of T20a — it costs a
      build pipeline.
      **The pipeline needed nothing.** `RuntimeKind` already had four variants, `core::shims::COMMANDS`
      already listed nineteen names across all of them, `runtimes::smoke_test` already knew that
      three of the four answer `--version`, and `resolve` never mentioned PHP. So the whole of the
      Node half is in [`mixengine-packages`](https://github.com/mixnz/mixengine-packages) —
      `tools/node.py` and `build-node.yml` — and what landed *here* is two tests and the
      documentation. That is the payment for T23's and T24's refusal to special-case a language, and
      it is the strongest evidence so far that those two were right.
      **What is published**: 16.20.2, 18.20.8, 20.20.2, 22.23.2 and 24.19.0, six targets each except
      the two oldest, which have five. The index now carries **sixteen packages and eighty-three
      artifacts**; the evaluation and its four findings are in
      [../operations/runtime-packaging.md](../operations/runtime-packaging.md) and **this file does
      not repeat them**.
      **Windows on ARM is offered for Node and not for PHP**, which makes MixEngine's own platform a
      first-class runtime target for the first time: upstream builds `win-arm64` from Node 20, where
      `windows.php.net` publishes `x64` and `x86` and nothing else in any branch. A version that has
      no build for a target is an **empty cell** rather than a failed run — the leg says so and
      skips, so Node 18 still publishes the five artifacts that do exist. That mechanism was a bug
      before it was a design: the recipe's exit code was being swallowed by the `-e` GitHub sets on
      `shell: bash`, so the leg failed exactly where it was meant to be skipped and took the release
      of five good artifacts with it.
      **Two tests, and the second is about the standard library rather than about us.** A home with
      two languages in it proves a shim dispatches on the *command* — everything before it was one
      runtime kind, which cannot tell that apart from a shim hard-wired to PHP. And a shim fronting a
      `.cmd` is what `npm` **is** on Windows: `CreateProcess` refuses a batch file, and what makes it
      work is `std::process::Command` recognising the extension, going through `cmd.exe`, returning
      the batch file's own status and escaping the arguments. Nothing here would notice the day that
      changes, and `npm` would break on every Windows machine.
      Proven end to end on a real machine rather than only in CI: Node 22 and 20 installed from the
      signed index, `node`, `npm` and `npx` run out of `bin/`, a `mixengine.toml` pinning `node = "20"`
      switching all three by directory, `npm 10.8.2` against Node 20 and `10.9.8` against Node 22 —
      **and the same after `mix daemon stop`**, which is the claim the shim exists to make.
      **Python was the row this table had already written down, and it held.** One recipe, six
      targets, borrowed from `python-build-standalone`; `tools/python.py` and `build-python.yml` are
      the whole of it and nothing in this repository changed to accept a third language. The six
      findings are in [../operations/runtime-packaging.md](../operations/runtime-packaging.md) and
      **this file does not repeat them** — except the one that was this task's own open question.
      **The post-install hook does not need to exist.**
      [runtime-versions.md](../features/runtime-versions.md)'s install flow reserved one per runtime
      and named *ensure `pip`* as Python's, and Python is exactly the cell it was reserved for: on
      Windows upstream ships `Scripts/` empty, so `pip` is importable and not runnable. Letting
      `ensurepip` generate `pip.exe` is what a hook would do, and a `pip.exe` has the absolute path
      of the interpreter that generated it written inside — so the hook would produce, on the user's
      own machine, exactly the baked path every artifact here exists to avoid, and it would break the
      first time `~/.mixengine` moved. The recipe writes a two-line `Scripts/pip.cmd` that computes
      the interpreter from its own location instead. **A path computed at run time beats a path
      written at install time**, which is the same sentence the shim itself is an instance of.
      **Ruby split into a borrow and a build, and the borrow is the half nobody expected.** The table
      said "we build" in all three columns; RubyInstaller publishes relocatable `.7z` archives for
      Windows on x64 **and arm64**, with Ruby's standard library, gem home and CA bundle all computed
      from `ruby.exe`'s own location. So `tools/ruby.py` and `build-ruby.yml` cover Windows, and
      macOS and Linux were **T27b**: `portable-ruby` publishes one version, `ruby/ruby-builder`'s own
      README says its artifacts "cannot be moved around", and RVM's are prefix-bound and years stale.
      **One test, and it is the property Python and Ruby are the first to reach.** Everything before
      them named its executables exactly as its commands are typed, so nothing could tell
      `Command::name` from `Command::executable` — a shim that looked the invoked name up in
      `provides` would have passed the whole suite. Python publishes one interpreter that `python`
      and `python3` both run, Ruby does the same with `bundle` and `bundler`, and the test installs a
      runtime whose file is named like neither command and runs it under both.
      Proven end to end on this machine rather than only in CI, which is what the local Windows
      x86_64 leg is worth: Python 3.12.14 and Ruby 3.4.10 and 3.2.11 packed, moved to a directory
      with a space in its name and run from there — `pip` through the generated `.cmd`, `gem`,
      `bundle`, `rake` and `irb` each reporting the version packed beside them, and Ruby verifying
      certificates against the bundle inside its own tree.
      **What is published**: Python 3.10.21, 3.11.16, 3.12.14, 3.13.15 and 3.14.7 on six targets
      each except 3.10, which has five; Ruby 3.2.11, 3.3.12, 3.4.10 and 4.0.6 on Windows, the first
      two on x64 alone. The signed index now carries **twenty-five packages and one hundred and
      eighteen artifacts** across four languages.
      **The five targets this machine is not found three real defects, and each was a check being
      wrong rather than an archive being wrong.** All three are written up in
      [../operations/runtime-packaging.md](../operations/runtime-packaging.md); what belongs here is
      the shape they share. `bsdtar` reads a 7-Zip container on every Windows and *decodes* its LZMA
      only where libarchive was built with liblzma, so the Ruby recipe passed on Windows 11 and
      failed on Windows Server 2022. `ldd` on a CPython extension module answers "not found" about a
      library that is in the tree, because the module carries no search path and the interpreter that
      `dlopen`s it does. And counting the certificate authorities a default context has loaded says
      `0` on a Linux that verifies perfectly, because a `capath` is a hash directory read one
      certificate at a time. **Each check was measuring a proxy for the property it claimed**, and
      each survived a local Windows run and four green targets before the fifth disagreed — which is
      the argument for the matrix, stated in defects rather than in principle.
      Proven end to end on this machine after publication, with **`mixengined` stopped**: Python
      3.12.14 and 3.13.15 and Ruby 3.4.10 installed from the signed index, `python`, `python3`, `pip`
      and `pip3` run out of `bin/`, a `mixengine.toml` pinning `python = "3.13"` switching all four
      by directory, and `bundle` and `bundler` both reaching the one executable — the alias claim the
      test above makes, made again by the real binary against a real install.
- [x] **T27a** PHP 7.0–8.0 on macOS **and** Linux — the one cell of the version policy nothing can
      be borrowed for. T20a settled the rest: Windows reaches 7.0 from the official archive for
      free, and `static-php-cli` covers 8.1 upwards on both other systems. What is left is six EOL
      branches, and they are their own task because they are the only part of the range that costs
      a build pipeline — against sources that predate OpenSSL 3 and the libxml2 API removals, with
      no upstream security releases behind them.
      **macOS is in scope, contrary to what this entry used to say.** The exclusion rested on
      upstream PHP having no Apple Silicon support before 8.0, which is true of upstream and not of
      reality: `shivammathur/homebrew-php` ships arm64 bottles for php@7.0 through php@7.4 built
      with a small `acinclude.m4` patch. Both macOS architectures are built, each on a runner of its
      own — **nothing cross-compiled, nothing under Rosetta**, and a branch that will not build
      natively for an architecture is a cell the index does without.
      **Borrow the recipe, not the artifact.** `shivammathur/php-builder` and the Homebrew tap are
      MIT and already build 5.6 upwards with `redis` and `mongodb`; what they produce installs under
      `/usr` or `/opt/homebrew`, which is exactly why T20a could not take their output. What landed
      in `mixengine-packages`: `tools/php_legacy_unix.py` (configure table per branch, PECL versions
      resolved from each package's own declared PHP range, never `buildconf` — so those four
      extensions are shared here rather than compiled in) and `tools/relocate.py`, which copies every
      non-system library into the archive and rewrites the tree to load it from `$ORIGIN` /
      `@loader_path`, re-signing each Mach-O ad-hoc because arm64 will not load an unsigned one.
      The Linux legs build inside AlmaLinux 8, chosen for its era's toolchain (OpenSSL 1.1.1, ICU 60,
      autoconf 2.69) rather than for the glibc 2.28 floor it also happens to give. macOS has no such
      distribution, so it builds its own era: OpenSSL 1.1.1w, libxml2 2.9.14 and — for the branches
      before 7.4 — **ICU 67.1**, because ext/intl on those does not compile against a current ICU and
      one half of why has no macro to fix it.
      **All thirty artifacts build**, six branches across five targets, each with `redis`, `mongodb`,
      `igbinary`, `xdebug` and `opcache` loaded from a directory the tree had been moved to. No cell
      was dropped, so the "an architecture that will not build natively is simply not offered"
      mechanism above stayed theoretical. Measured floors: `glibc 2.28` on Linux, `macos 14.0` on
      Apple Silicon, `macos 15.0` on Intel.
      **Published.** The signed index carries eleven versions and fifty-five artifacts — 7.0 through
      8.5, five targets each, no cell missing. Finishing this range exposed two gaps on the borrowed
      side that had nothing to do with it and are fixed here: the 8.1+ artifacts had no macOS Intel
      build at all, so an Intel Mac could have installed 7.4 and not 8.3, and their macOS artifacts
      declared no floor while in fact running from 12.0. Both are measured and offered now.
- [x] **T27b** Ruby on macOS **and** Linux — the last cell in the runtime table that nothing can be
      borrowed for, and the counterpart of T27a on the other side of the table. T27 settled the rest:
      Windows takes RubyInstaller's relocatable `.7z` on both architectures for free. What is left is
      four targets, and they are their own task for the same reason T27a was: they cost a build
      pipeline, and unlike a borrow that is a standing commitment renewed at every security release.
      **All three candidates were checked and each failed differently** — recorded in
      [../operations/runtime-packaging.md](../operations/runtime-packaging.md) so nobody reopens
      them: Homebrew's `portable-ruby` is relocatable by construction and publishes exactly one
      version, `ruby/ruby-builder`'s README says its artifacts "embed the install path when built and
      cannot be moved around", and RVM's binaries are prefix-bound with nothing newer than 2023.
      **`--enable-load-relative` did everything RubyInstaller proves it does**, first time and on all
      four targets: library path, architecture directory and gem home all resolve inside a moved
      tree, and `rbinstall` writes `bin/gem` and its siblings as a `/bin/sh` preamble that
      re-executes `$bindir/ruby -x` on itself instead of as a script with an absolute `#!`. The
      recipe checks that second half rather than trusting it — one flag in a build system nobody
      here controls stands between it and a command that fails on a user's machine with "no such
      file or directory" — and has not yet had to fix one.
      **The trust store was the open question and the answer is one library down.** `OPENSSLDIR` is
      fixed when OpenSSL is compiled, so `OpenSSL::X509::DEFAULT_CERT_FILE` is a statement about the
      *build* machine: ship AlmaLinux's `/etc/pki/tls/cert.pem` to a Debian user and every handshake
      fails, `gem install` first, with an error that names neither the file nor the reason. Setting
      `SSL_CERT_FILE` from Ruby would cover only the programs that read the environment and leave
      the constant lying. So OpenSSL 3.5.7 is compiled here with its four default-path functions
      taught to resolve against **the loaded `libcrypto`'s own location** — `dladdr`, two
      directories up, `ssl/cert.pem` — falling back to the compiled-in path when that file is absent
      and still letting `SSL_CERT_FILE` win, which is how a corporate CA keeps working. That is
      `--enable-load-relative` applied to a library instead of an interpreter, and it is what
      RubyInstaller gets from MSYS2 on Windows. **Proven twice, because neither half implies the
      other**: the constant has to name a file inside the moved tree, and a real chain has to verify
      over the network from there.
      **The three decisions this entry asked to make before building, and what they turned out to
      be worth.** *AlmaLinux 8* — taken, and for the floor alone: nothing in Ruby 3.2+ wants an old
      toolchain, so the image buys glibc 2.28 instead of the runner's 2.39 and costs nothing,
      because everything Ruby *is* version-sensitive about is compiled by the recipe on every target
      alike. *YJIT* — offered, and it is a decision rather than a default, because `--enable-yjit`
      without a Rust compiler **warns** and produces an interpreter with no JIT: the recipe installs
      a toolchain where the image has none, and the smoke test asks the finished artifact whether
      `RubyVM::YJIT.enabled?` is true. *Which lines* — 3.2 upwards, the same floor the Windows half
      offers.
      **Four rounds of CI, and not one of them was Ruby.** Every failure was in the shared packing
      code or in this repository's own idea of what a check should ask, which is the strongest
      argument yet that a second build pipeline is where the first one's assumptions get audited.
      Two generalise beyond packaging and are in
      [`docs/building-from-source.md`](https://github.com/mixnz/mixengine-packages/blob/master/docs/building-from-source.md):
      **a file can carry the right magic number and never be loaded by anything** — `debug.o` left in
      a gem's build directory and the `.dSYM` companion beside every macOS extension, each refusing
      the very tool that would have rewritten it — and **a check that asks the artifact a question
      must strip the machine's environment, while a check that asks what the artifact can do on a
      user's machine must not**, which is how compiling a native gem with a cut-down `PATH` produced
      "you have to install development tools first" on an image whose compiler was somewhere else.
      **What the two Ruby recipes share is the claim, not the mechanics.** `ruby_smoke.py` is what a
      borrowed Windows archive and a compiled Unix one both have to satisfy, because a daemon
      installing one of them cannot tell which produced it — and the Windows half now verifies a
      live certificate chain too, which it did not before.
      **One limitation is upstream's and is recorded rather than worked around**: macOS `mkmf` writes
      `-bundle_loader <bindir>/ruby` unquoted, so a native gem cannot be compiled against a Ruby
      whose path contains a space — a user whose home directory has one included. Everything else
      about such an installation works; the recipe compiles its proof gem from a second moved copy
      without one.
      **Published.** The signed index carries Ruby 3.2.11, 3.3.12, 3.4.10 and 4.0.6 — the four lines
      the Windows half already offered, now with all four Unix targets under each, so **twenty-two
      Ruby artifacts** where there were six. Windows on ARM is the only cell still missing anywhere,
      on 3.2 and 3.3, and it is upstream's: RubyInstaller's first ARM64 archive is in the 3.4 line.
      4.0 had never been compiled by this recipe before the release build and needed nothing. The
      index now holds **twenty-five packages and one hundred and thirty-four artifacts** across four
      languages.
      **One flake and one silence, both from the release builds.** A dependency download timed out
      on one leg of four, twenty minutes into a build, on a version that had already gone green
      twice — `borrow.fetch` retries the network now and still refuses to retry an HTTP status,
      because a 404 is an answer. And reading four logs side by side showed one version published as
      `.tar.zst` on macOS and `.tar.gz` on Linux, for a reason nothing had printed: `tar --zstd` is
      refused on the manylinux image and installing the compressor does not change it. Both suffixes
      are named in the index and either installs; what was not acceptable was the build machine
      deciding in silence, so `pack` quotes the refusal now.
      **Measured, per artifact rather than assumed**: `glibc 2.28` on Linux, `macos 14.0` on Apple
      Silicon and `macos 15.0` on Intel; ten bundled libraries on Linux 3.2 and seven on 3.4,
      because from 3.3 upstream replaced the C `readline` extension with a shim over the pure-Ruby
      `reline` — `require "readline"` works on every line either way. Two differences nobody chose
      were caught the same way and closed: one runner had GMP installed and its sibling did not, and
      `/opt/homebrew` is not a compiler search path where `/usr/local` is, so the second attempt at
      evenness was still half wrong until Homebrew was asked where it had put the thing.
- [x] **T28** PHP extensions: `conf.d` model, enable/disable, prebuilt extension artifacts, per-pool
      reload.
      **Both of the things this was waiting for had landed with
      [T32](phase-3-services.md).** There is a pool per installed PHP, so "per-pool reload" names
      something — and on the two systems that have a signal it is `SIGUSR2` to that pool's master
      rather than a restart. `PHP_INI_SCAN_DIR` was measured to load `conf.d/*.ini` over a `php.ini`
      on all three systems, which is the road the `conf.d` model takes; T32 deliberately rendered
      neither file, because what a *pool* configures and what a *runtime's* ini set contains have
      different owners, and this task owns the second.
      **"Prebuilt extension artifacts" turned out to be already inside the archive.** The Windows
      build ships 31 loadable modules and the Unix one compiles its set in, and the index has
      published `extensions.{static,shared,enabled}` per artifact since `mixengine-packages` P2 — so
      what this task owed was not a second download path but a *switch*. An extension from anywhere
      else is a `mixengine-packages` task first, and the state model does not change when one
      arrives: it becomes another name in that artifact's `shared` list.
      **What is stored is a deviation and not a set.** `extension_choices_json` holds
      `{"xdebug": true}`, so a reinstall or a patch upgrade brings the new build's defaults with it
      and keeps only what somebody deliberately turned round — a stored list would freeze 8.3.33's
      answer and carry it silently onto 8.3.34. A choice that agrees with the build is *forgotten*
      rather than written, for the same reason.
      Two things the design asserted and this task measured, on the Windows cell against a real PHP:
      `zend_extension = xdebug` spelled as a bare name **is** the spelling modern PHP accepts there —
      it resolves `php_xdebug.dll` itself — and a pool whose recipe has no `ReloadBehaviour` answers
      `restart_required`, which the suite obeys rather than guesses. **The `SIGUSR2` half is measured
      by the Linux leg**, where `crates/mixengine-cli/tests/php_extensions.rs` runs inside the network
      namespace: whether a reload picks up a *newly enabled* extension is a question only a system
      with signals can answer.
      Two deviations from what [runtime-versions.md](../features/runtime-versions.md) said, both
      recorded there: the set lives at `etc/<kind>/<version>/conf.d/` rather than inside the install —
      an install is a rename over the destination, and generated configuration is disposable — and
      the command is `mix runtime ext …` rather than `mix php ext …`, because a per-language family
      for one language is a noun this CLI would then owe every other runtime.
      Left for the tasks that own them: **no user-editable ini settings.** MixEngine writes one
      dev-tuned block — `memory_limit`, `display_errors`, `opcache.revalidate_freq = 0` and five more
      — and nothing reads a generated file back into state. A settable `php.ini` is a feature of its
      own, and per-site sets are impossible by construction while one pool serves every site on a
      version.
- [x] **T29** Shim overhead benchmark in CI (< 15 ms budget), and the `bench` job to run it in.
      **The first thing this task had to decide is what the budget is a budget on**, because the
      one-line description above does not say and the two readings differ by an order of magnitude.
      [runtime-versions.md](../features/runtime-versions.md) attaches the number to *step 2* of the
      shim — "calls `resolve` (in-process, reading SQLite read-only + walking for `mixengine.toml`)
      — **no IPC** … Target: < 15 ms" — and to nothing else. So the gate is on the resolution, and it
      is the resolution that misses if somebody ever makes a shim ask the daemon, parse
      `config.toml`, or reach an index.
      **Measured on all three runners, in release**, against a home with five runtimes in it and from
      both shapes a directory can have — nothing pinned, which walks to the root and then reads the
      kind's default, and a manifest in the directory, which stops at the first try and parses a TOML
      instead. At p50:

      | | program alone | through the shim | difference | **resolution** |
      | --- | --- | --- | --- | --- |
      | ubuntu | 1.06 ms | 3.26 ms | 2.19 ms | **0.74 ms** |
      | macos | 3.11 ms | 7.66 ms | 4.52 ms | **0.58 ms** |
      | windows | 8.63 ms | 23.54 ms | 15.03 ms | **1.71 ms** |

      **The gated column is the last one, and it is nine to twenty-five times inside its budget.**
      **The wall clock is measured, printed and gated on nothing**, which is the second decision and
      the more contestable one — and the Windows row is the argument for it. Unix pays for the shim's
      own image and then `exec`s; Windows pays for that *and an entire second process*, and lands
      **on** the 15 ms line, where a gate would flap from run to run while saying nothing whatever
      about the resolution beside it. A budget there is a budget on the runner's process model, and a
      pessimistic one: `fakeservice` is a one-megabyte binary where the `php.exe` a real shim fronts
      is sixty, so the fixture overstates the shim image's share by a wide margin. Where the Windows
      time goes was taken apart on a developer machine rather than in CI — a shim that only loads and
      refuses to dispatch cost 16 ms of the 30 ms a full run took there — so it is the two images and
      the two creations and nothing between them. What the number is for is the log, and the fact
      that nothing in this workspace had ever timed `php -v` at all.
      **T25's guess was right and is now a measurement.** It said process creation dominates and the
      resolution did not stand out of the noise; both hold. What it could not see from thirty hand
      runs is the shape *behind* that — that on Windows the hand-over is a second process rather
      than a hand-over, which is the whole of the difference between the two systems here.
      **Three findings, and two of them are about how a benchmark lies.** A stale
      `target/release/fakeservice` — built before that program learned `--version` — was found by the
      benchmark's own check that the fronted program really ran, and it is exactly the failure a
      performance test hides: a shim that resolves nothing is *faster*, so every way of breaking the
      fixture improves the number. Selecting one test target does not rebuild a dev-dependency's
      binary, so the CI job builds it explicitly. And `--test-threads=1` is load-bearing rather than
      tidiness: the two benchmarks spend their whole time creating processes, so run in parallel each
      measures the other — 34 ms against 21 ms for the same difference, same machine, same minute.
      **The `bench` job runs on all three systems**, where
      [../operations/build-and-release.md](../operations/build-and-release.md)'s table had sketched
      ubuntu alone. The gate is the same everywhere; what it stands in front of is not, and a wall
      clock measured only where process creation is cheapest is not measured. A job of its own rather
      than a step in `test`, because these are `#[ignore]`d and need a release build — a second
      compilation of the workspace that no correctness answer should wait behind.
      The fixture moved to `crates/mixengine-shim/tests/harness/mod.rs` on the CLI suite's precedent,
      so the home this measures and the home `shim.rs` asserts against are one definition. A budget
      taken against a home built slightly differently from the one under test would be a number about
      the fixture.
      Left for the tasks that own them: **the other two budgets in
      [../standards/testing.md](../standards/testing.md) are still only written down** — idle
      footprint and cold path belong to [phase 7](phase-7-efficiency.md), and each needs the thing it
      measures to exist first. The third, GUI cold start, went with the phase
      [ADR 0011](../decisions/0011-no-gui-in-this-repository.md) withdrew. The job they will run in is now there.

**Milestone M2** — two PHP versions installed; `php -v` differs between two directories with no shell
hook installed.

---

Previous: [Phase 1 — Process supervision](phase-1-process-supervision.md) · Next: [Phase 3 — Services](phase-3-services.md)
