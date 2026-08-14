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
      [`mixengine-packages`](https://github.com/haiquang9994/mixengine-packages). The evaluations
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
      and pools are T28. A refusal nothing could trigger, and a flag with nothing to force past, would
      both be guesses about a shape those tasks have to live with. **No `runtime.resolve`** either: it
      is listed in the architecture's namespace table and it is T24's, because resolution is the
      constraint grammar and not a lookup.
- [ ] **T24** Version resolution (`core::resolve`): flag → `mixengine.toml` → project record → default;
      exact/minor/caret constraints.
- [ ] **T25** Shim binary: name-based dispatch, in-process resolution without IPC, `exec` on Unix /
      Job-Object child on Windows, exit-code and signal passthrough. **(P)**
- [ ] **T26** PATH integration for `<root>/bin`, reversible. **(P)**
- [ ] **T27** Node.js, Python, Ruby support in the same pipeline.
- [ ] **T27a** PHP 7.0–8.0 on macOS **and** Linux — the one cell of the version policy nothing can
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
      autoconf 2.69) rather than for the glibc 2.28 floor it also happens to give.
      **What is left:** dispatch `build-php.yml` for 7.0, 7.1, 7.2, 7.3, 7.4 and 8.0, then publish
      the index. This is ticked when those artifacts exist, not when the recipe does.
- [ ] **T28** PHP extensions: `conf.d` model, enable/disable, prebuilt extension artifacts, per-pool
      reload.
- [ ] **T29** Shim overhead benchmark in CI (< 15 ms budget).

**Milestone M2** — two PHP versions installed; `php -v` differs between two directories with no shell
hook installed.

---

Previous: [Phase 1 — Process supervision](phase-1-process-supervision.md) · Next: [Phase 3 — Services](phase-3-services.md)
