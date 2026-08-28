# Phase 5 — HTTPS

*Goal: green padlock, automatically, forever.*

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [x] **T48** Internal CA generation (`rcgen`), key permissions, fingerprint, `cert.ca_status`.
      **T3b's note is followed and then made unnecessary.** The order it asks for already held —
      `Paths::bootstrap` strips `certs/` before anything is written into it — but the other half of
      its advice is what landed: `mixengine_platform::write_private` restricts the key file in its
      own right. Relying on the directory alone would make the key's protection a property of
      something `mix doctor` already has a name for losing (`HomePermissionsLost`), and the two are
      not the same claim. It is a **new platform primitive** rather than `restrict_to_owner` with a
      different argument: that method grants `(OI)(CI)F`, and Object Inherit and Container Inherit
      are directory-only flags `icacls` refuses on a file. Both systems apply the permission *as the
      file is created* — `open(2)`'s mode on Unix, an empty file restricted before it is written on
      Windows — because applying it afterwards leaves an instant in which the key is readable.
      **The subject `security-model.md` asked for cannot exist.** It named the common name
      `MixEngine Local CA <short-fingerprint>`; a fingerprint is a hash of the certificate and the
      subject is inside the bytes being hashed. The eight characters come from the **public key**
      instead — computable before anything is signed, and stable across re-signing the same key,
      which makes two certificates for one authority recognisable as one. `cert.ca_status` still
      reports the certificate's hash as the fingerprint, because that is the number a browser shows.
      Both are in the answer and the document now says which is which.
      **The two documents disagreed about when it appears, and T45 had already settled it.**
      `security-model.md` said "on first use" while `tls.md` put generation at step 1 of first-run
      setup with the trust-store install batched into one prompt; the first breaks the second, and
      the single-prompt promise is four lines above the sentence breaking it. Generation is at daemon
      start, in the block that already asks for ports and the resolver, under the same rule they
      state — a failure warns and never refuses the start.
      **A damaged authority is reported and never replaced.** Replacing one would invalidate every
      leaf already issued and every trust store holding it, in answer to nobody; the five ways it can
      be damaged are a closed enum on the wire, and `KeyAndCertificateDisagree` is a real check
      because a backup that caught one file and not the other is how the two come apart.
      **What it deliberately did not do**: no trust-store field on `cert.ca_status` — that is about
      the operating system and is **T49a**'s, and a field this build could only fill with "unknown"
      is not an answer. No `mix doctor` check, because adding a `ProblemId` means deciding what
      repairing it is, which is T54's decision. And `mix cert status` is **left free** for T53's
      per-site handshake, with a test asserting it still fails.
- [x] **T49a** Trust store install/remove per OS — Windows `LocalMachine\Root` through CryptoAPI,
      the macOS System keychain through `security`, and whichever anchors directory this Linux has —
      **batched with T42 and T45 into the single first-run elevation prompt**. **(P)**
      **Split from T49 at the privilege boundary**, which is the only line the two halves differ on:
      the system store needs root and goes through `mixengine-elevate`, while the NSS databases below
      belong to the user and cannot be batched into a prompt at all, there being no prompt to batch
      them into. Design:
      [T49a spec](../../docs/superpowers/specs/2026-08-24-t49a-system-trust-store-design.md).
      **The removal is the direction that can do damage, so it carries no fingerprint.** An install
      is close to harmless — a daemon compromised badly enough to forge one already holds the CA key
      and can sign anything — but a removal naming a certificate by hash could take the root that
      validates Windows Update out of the machine, through the audited binary and under the user's
      own Allow click. What travels is T48's eight-character key-id, which cannot describe a
      corporate root; the helper checks it before opening a store and checks the whole shape again
      against every certificate the store hands back. `ResolverPlan`'s argument one capability along:
      the value an attacker would abuse is not validated, it is absent.
      **Two dependencies were costed and refused, and costing one of them corrected the design.**
      `x509-parser` is 29 crates with 7 already present; `sha2` is 8 with none. Both would have gone
      into a binary that runs as root and whose closure CI diffs. Costing `sha2` found that the check
      it was for — recomputing the key-id from the public key — refuses nothing, since whoever
      generates a certificate sets its name to their own key's identifier. So the checks are
      hand-written over a DER reader that only knows how to say no, the name is checked as a *shape*,
      and the key-id earns its keep naming an authority for removal instead. `pem` was measured at
      two crates and **taken** — the rule that file states is that a line has to be argued for, not
      that the number may never go up.
      **The install check is not a security boundary and the code says so in its first line.** It
      exists so `ca-uninstall` (T54) and uninstall (T87) can enumerate everything an install could
      ever have created; an unconstrained one could leave a root called anything at all behind.
      **`CaNotTrusted` is a `mix doctor` condition where T48 declined to add one**, because the
      repair here is *ask again* — what `ResolverNotWired` and `PortAccessMissing` already do —
      rather than regenerate, which was T48's condition and is destructive.
      **Whether the machine trusts it is read, never recorded**, against `tls.md` step 4: a stored
      flag is a claim an OS update or another account can falsify silently, and the read costs no
      privilege on any of the three systems — which `mixengine-platform/tests/trust.rs` measures in
      CI's ordinary job rather than asserting in a comment.
      **Thirteen existing tests had to change**, all of them because a started daemon now queues what
      first-run setup needs and they had written the queue's length, or the doctor's check count, as
      a constant. None was filtered green: `nothing_was_granted` now compares against a measured
      count, which is the stronger claim it always meant; the empty-queue test empties the queue,
      because filtering would have let it reach a real prompt.
      **What CI answered that no machine here could.** Both unprivileged reads succeed, so the
      producer and the doctor check keep their shape. The Windows store reports "already there" as
      `CRYPT_E_EXISTS` and not `ERROR_ALREADY_EXISTS` — the crypto layer's `HRESULT` through
      `SetLastError`, invisible on a first install and a failure on every one after it, and the only
      thing here a unit test could not have found.
      **And the macOS removal is not the command the specification named.** Measured one command at
      a time under an alarm, on a runner with no window server: `security remove-trusted-cert -d`
      never returns — not under plain `sudo`, not under `sudo -H`, not with `HOME` unset, not
      against a root-owned path, and **not even when there is nothing left to remove**, which is
      what makes it a call that could never have answered rather than one defeated by its input.
      Without `-d` it fails in a millisecond, so it is the admin domain specifically.
      `trust-settings-import -d` hangs the same way, unchanged or edited, while `export` reads that
      domain and `add-trusted-cert` writes it — so on a machine with no agent to display a prompt
      the admin trust domain can be **read and added to, and neither removed from nor replaced**.
      `security delete-certificate` answers at once and takes the trust setting out **with** the
      certificate, because the admin domain *is* `/Library/Keychains/System.keychain` rather than a
      store beside it — which corrects a claim this entry briefly carried in the other direction and
      is the reason the probe measured it instead of reasoning about it.
      **Proved targeted rather than wholesale by installing two certificates and deleting one**: the
      other was still there and still trusted. One certificate could never have told those apart,
      and taking somebody's corporate root out along with ours is the worst thing this task could
      have shipped. What names the certificate is the SHA-1 `security` itself printed for it, so the
      DER is still what every check runs against, nothing in the command comes from the request, and
      no hashing dependency joined the binary that runs as root.
      **A helper behind an elevation prompt has no console and must never wait for one.** Every
      `security` call now has `/dev/null` for standard input and a thirty-second deadline, and every
      `Failed` this helper produces carries the OS's own words through `mixengine_proto::flatten`
      instead of only the verb it was attempting — four sites, one mistake, found because a failure
      that named the action and dropped the cause could not be acted on.
      **What it deliberately did not do**: no producer for the removal — built, validated and tested
      with none, on T42's D12 and T45's D13, because T54 and T87 are the producers. No `certutil`
      fallback on Windows. And `tls.md`'s claim that a removal deletes by fingerprint is corrected
      rather than implemented.
- [x] **T49b** The Linux NSS databases for Firefox and Chrome — `~/.pki/nssdb` and every profile
      under `~/.mozilla/firefox/*/`. Unprivileged, in the daemon, and **no part of any elevation
      batch**.
      **Starts from a measurement rather than from the specification**: `tls.md` names `certutil` as
      the mechanism, and on a stock Ubuntu 24.04 it is not installed — it ships in `libnss3-tools`,
      and `~/.pki/nssdb` does not exist either. A machine without the tool is a state to report, not
      a failure. It must also answer, by measuring rather than by assuming, whether Firefox on
      Windows and macOS needs this treatment too: `tls.md` names NSS on Linux alone, and whether the
      other two read the system store depends on `security.enterprise_roots`.
      **What it found.** `certutil` is absent on a stock Ubuntu 24.04 and ships in `libnss3-tools`,
      which is a `NoTool` state naming the package rather than a failure. The `firefox` deb on
      Ubuntu 22.04+ is a transitional package to the snap, so the specification's two search roots
      became **six** — a root that matches nothing costs a `readdir`, a root that is missing costs a
      red padlock with no diagnostic. The nickname carries T48's key id, because `tls.md`'s bare
      `MixEngine` would have two homes on one machine overwriting each other's entry with no error.
      And `certutil` will not read a certificate from a pipe: `-i /dev/stdin` is
      `SEC_ERROR_INVALID_ARGS`, while the PEM on stdin with no `-i` **exits 0 and installs nothing**
      — a silent success, found because the round trip is measured against a real tool rather than
      asserted. The certificate goes in through a file written beside the database and unlinked
      after, in the user's own directory rather than world-writable `/tmp`.
      **And it corrected `repair.rs`.** `InHome` said "everything it touches is under
      `MIXENGINE_HOME`"; these databases are under the *user's* home. The invariant that holds is
      **no privilege**, and the path clause was a description of the three repairs that happened to
      exist.
      **What it deliberately did not do**: no database is created — a profile with no `cert9.db` has
      never been opened; no legacy `dbm:` support; no producer for the removal, on T42's D12 and
      T45's D13, because T54 and T87 are the producers. Design:
      [T49b spec](../../docs/superpowers/specs/2026-08-25-t49b-nss-databases-design.md).
      **The Windows half of D14 was answered on 2026-08-25, by handshake.** Firefox 154 on Windows
      **does** read the operating system's trust store: a throwaway authority placed only in
      `Cert:\CurrentUser\Root` produced an ordinary padlock, so `Browsers::NotSearched` is the
      measured answer there rather than an unexamined one, and T49b needs no Windows counterpart.
      **Three indirect measurements pointed the opposite way first**, and all three were wrong for
      one reason: **Firefox's Certificate Manager does not list enterprise roots at all** — its
      Authorities tab shows Mozilla's built-in set and nothing more. Looking at a certificate list is
      not a way to answer "does this browser trust this authority"; only a handshake is, and the
      browser must be fully restarted first because those roots are read at start-up. A .NET client,
      which certainly reads the Windows store, was the control that caught the error.
      **And it found `CurrentUser\Root` is enough for every browser on Windows** — Firefox 154,
      Chrome 151 and Edge 151 all completed the handshake against an authority placed only there, so
      Chrome's own root store still accepts a locally installed anchor. **Moving T49a to it was
      considered and rejected**, on a second measurement: writing to the user's root store raises
      CryptoAPI's own "Security Warning" dialog, and **so does removing from it**. That is not an
      elevation, but it is a click, and it cannot be batched — where one `mixengine-elevate`
      invocation covers the hosts file, the port grant and the trust store together, and covers a
      rotation's remove-then-add in a single grant. Per-user would therefore mean **two** prompts at
      first run instead of one, and **two** clicks for `ca_rotate` (T54) instead of one. It wins in
      exactly one situation — a machine whose user has no administrator token, where the current
      design yields no HTTPS at all. Recorded rather than built: a fallback is two code paths for one
      job, and nobody has reported that machine yet. The measurement is here so that whoever does
      report it does not have to make it again.
      **macOS was measured the same way on 2026-08-25, and agrees**: Safari, Edge **and Firefox**
      all completed the handshake against an authority placed only in the user's login keychain. So
      D14 is closed on all three systems, and **Linux is the exception rather than the rule** — it is
      the only one where a browser keeps a trust store of its own, which is why T49b exists there and
      nowhere else. macOS also asks for the account password **twice**, once to add and once to
      remove, which is heavier than Windows' click and is the strongest evidence against ever moving
      the trust store per-user: one `ca_rotate` would cost two password prompts where a single
      elevation grant covers remove-and-add together.
      **The method, because the conclusion has a shelf life** — a browser can change its default, and
      whoever re-measures should not have to rediscover how. Never read a certificate list: Firefox's
      Certificate Manager does not show enterprise roots at all, and three separate list-based
      measurements pointed the wrong way before a handshake corrected them. Always carry a control
      that certainly reads the store under test — `security verify-cert` on macOS, a .NET client on
      Windows — because "the browser refuses" and "the probe is built wrong" are the same red padlock
      without one. And fully quit the browser first, Cmd-Q rather than closing the window: trust
      anchors are read at start-up, and skipping it yields a false negative indistinguishable from a
      true one.
      **What this did not measure is MixEngine's own macOS code.** The probe used an authority
      `openssl` generated and `security` installed; `mixengine-platform`'s macOS trust store has
      still never run on a Mac. That is a separate question from the one D14 asked, and it is still
      open.
- [x] **T50** Leaf issuance: 90 days, site SANs, `serverAuth` only, idempotent reuse.
      **What it decided.** Issuance is a **precondition of configuration generation, never part of
      it**: the generator's output is disposable and rebuilt from SQLite, a certificate is state that
      cannot be rebuilt from a row, and a generator that sometimes produced state would make that
      rule unreadable. So the start orders it — authority, trust stores, browsers, **certificates**,
      then the generators — and `site.create` and `site.update` issue before their own walk. T51
      inherits a guarantee rather than a mechanism.
      **What it found.** `.claude/features/tls.md` specified `cert.issue { domains }`, which puts the
      decision of what a certificate covers in the client; the method names a site. Its `localhost`
      alias clause was not implementable and its wildcard sentence had been wrong since T44. `rcgen`
      leaves `use_authority_key_identifier_extension` **off** by default, so a leaf carried no
      `authorityKeyIdentifier` at all until it was set — measured by the test asserting that the
      cheap issuer-name comparison agrees with the extension, which had nothing to compare against
      otherwise. And Windows' reserved device names were **measured and dismissed**: `nul.test.crt`
      is an ordinary file, because the rule applies to the stem before the final extension, so no
      domain-validation rule was added on a premise that turned out to be false.
      **And it corrected the plan's own shape.** `Certificates::issue` takes a **site record** and
      never a `SiteRef`: resolving a reference lives on `sites::Sites`, which T50 gives a
      `Certificates` of its own, and a `Certificates` that resolved references would close the loop.
      The two callers holding a reference already hold the row it names.
      **What it deliberately did not do**: no web-server wiring (T51), no renewal schedule (T52), no
      handshake and no `mix cert status` (T53), no `force`, no rotation, no removal (T53, T54), and
      **no orphan sweep** — a renamed or deleted site leaves a leaf behind, and removal is the
      direction that can do damage, on T42's D12 and T45's D13 for the third time. Design:
      [T50 spec](../../docs/superpowers/specs/2026-08-25-t50-leaf-issuance-design.md).
- [x] **T51** Web server TLS wiring; **disable Caddy's automatic ACME** explicitly.
      **Half of what it was asked for was already done.** `auto_https off` was **measured** against
      Caddy 2.11.4 to serve an explicitly configured `tls` perfectly well, so T43 had already
      discharged the ACME half by making it a setting with `off` as its preset; T51 owes that line a
      test rather than an edit, and `tests/caddy.rs` now asserts the global block still says it while
      sites are being served over TLS.
      **What it found, by running the programs rather than reasoning about them.** A Caddy site block
      naming both schemes with a `tls` inside is refused outright — `server listening on [:80] is
      HTTP, but attempts to configure TLS connection policies` — so an HTTPS site renders **two**
      blocks and repeats its handler. nginx was then measured rather than assumed to behave the same
      way, and does not: `ssl` attaches to a `listen` line there, so one `server` carries both. The
      asymmetry is a property of the two programs and is written into both templates.
      **And the finding that cost the most:** from T51 a front end **binds a TLS port for the first
      time**, because until now no site had a certificate to serve. Both servers reject the *whole*
      configuration when a single listener will not bind, so on a machine without the port grant the
      symptom is not "no HTTPS" — it is a reload refused and the previous configuration left running.
      The first-run grant covers `[80, 443]` together, so a machine that can bind one can bind the
      other; but the TLS port had been written as a **constant** in the nginx recipe, which no test
      could move, and the real-nginx suite cannot bind 443. It is a setting on both recipes now.
      **What it decided.** `generate` reads the certificate directory — one call, in
      `generate::served`, through `certs::leaf::read` rather than an existence check, so the question
      "is there a `tls` line" is answered by the same code that answers "is this pair usable". A site
      with no usable certificate renders HTTP alone rather than a `tls` at a path that is not there:
      validation judges a whole rendering, so that one site would otherwise cost every other site its
      configuration. And each generated site file carries the certificate's fingerprint in its header
      — not a note, a mechanism: a certificate is reissued to the same path, so without it the
      installer's diff finds no change and the running server never re-reads the new certificate.
      **What it deliberately did not do**: no redirect (a site has two real addresses), no renewal
      schedule (T52), no live handshake and no `mix cert status` (T53), no HSTS, no cipher list, no
      TLS-version pinning, and nothing deleted — a site that stops declaring HTTPS loses its TLS
      block and keeps its certificate, on T42's D12 and T45's D13 for the fourth time. Design:
      [T51 spec](../../docs/superpowers/specs/2026-08-25-t51-web-server-tls-design.md).
- [x] **T52** Renewal scheduler: daily + on-boot check, < 30 days threshold, reload without restart.
      **Two of the three things this line names already existed.** The on-boot check has run since
      T50 — every start calls `issue(None)`, `leaf::ensure` refuses to reuse a leaf with 30 days or
      fewer left, and the generator blocks run after it — and the threshold was already *reported*,
      because `mix doctor`'s `SiteCertificateMissing` counts a certificate inside the window as a
      site without one. What was missing was a tick, a reload for a renewal no call asked for, and
      `CertExpiring`. So the task is small, and the half that already worked turned out to be the
      half **nothing asserted**: every suite that exercises issuance creates its site through the
      API, which issues on its own path, so a start that stopped issuing would have gone unnoticed
      until a padlock went red. That test exists now.
      **Hourly, and the reason is not caution.** `tls.md` said *daily*, which taken literally is a
      24-hour `tokio::time::interval` — and Tokio measures from `std::time::Instant`, which counts
      no time on Linux or macOS while the machine is suspended, so a laptop closed over a weekend
      turns a day into four. Rather than make the alarm accurate, the check is made cheap enough
      that its accuracy stops mattering: **the 30-day threshold is the tolerance**, and a tick four
      days late still renews with 26 days to spare. That also disposes of resume-from-suspend and of
      `MissedTickBehavior` — a late tick has nothing to catch up on, because a pass that finds
      nothing due does nothing. Windows counts suspended time differently again, and this design
      deliberately does not depend on knowing which way, because a scheduler whose correctness rests
      on a per-OS clock is a scheduler with three behaviours.
      **The period is a setting and not a constant**, which is T51's lesson applied one task later:
      the nginx TLS port was a constant no test could move, and the missing test was what nearly let
      a whole configuration be refused on a real machine. `[certs] renew_check_seconds` refuses zero
      — a loop with no pause in it is not a schedule — and takes no ceiling, because a period long
      enough to matter answers a different question than the key asks.
      **And T52 was the first caller that had to tell two of T50's answers apart.** `IssueOutcome`
      documented `Refused` as covering *"no usable authority, HTTPS not declared, no domains"*; the
      middle one is not a refusal, and a renewal loop announcing every refusal would have announced
      one per plaintext site, once an hour, forever. The conflation was **already producing a wrong
      line**: `Sites::now_has_a_certificate` warns on every refusal, so creating a site with HTTPS
      off logged `the site has no certificate yet` about a site that never wanted one. `NotWanted`
      is the fourth outcome, and the log line's absence is asserted — after a clean shutdown, since
      an absence read from a running daemon's log would pass whether or not the line was produced.
      **What it decided.** A pass reports and the loop acts, so that `once` needs only a
      `Certificates` and can be tested over a temporary directory rather than a whole `Registry`.
      `Pass` is an **enum** with `Skipped` in it rather than a struct with a count of zero: a pass
      that stopped because there is no authority and a pass that found nothing due would otherwise
      be one value, and the test for the gate would pass whether or not the gate had been written.
      The reload is T51's fingerprint and nothing new — renewal calls the generator, and the one log
      line covers both halves so that it cannot be written by a renewal that never got that far.
      **And the test's own guard caught the fixture.** Backdating a certificate by calling
      `leaf::ensure` with a past `now` writes nothing: `ensure` asks whether what is there is
      reusable *as of the `now` it is given*, and a certificate issued today has 160 days left as of
      seventy days ago. The pair has to be removed first — found because the fixture asserted that
      what it wrote differed from what was there, rather than assuming it.
      **What it deliberately did not do**: the authority is not renewed (ten years, and replacing it
      is `ca_rotate`, T54 — a destructive operation with a person on the other end of it, not
      something a timer does at three in the morning); trust stores and browser databases are not
      re-checked, because an hourly loop would spawn `certutil` per profile on Linux forever to
      answer a question that changes when a person changes it; nothing is deleted, on T42's D12 and
      T45's D13 for the fifth time; no handshake (T53); and no `cert.renew`, because `cert.issue`
      already reissues anything inside the window and a second name for one operation is two things
      to keep in step. Design:
      [T52 spec](../../docs/superpowers/specs/2026-08-25-t52-renewal-scheduler-design.md).
- [x] **T53** `mix cert status` with a live handshake and SAN-mismatch detection; one-click reissue.
      **The first measurement in this repository that answers whether the padlock is green.**
      Everything phase 5 built before it reads a file: T48 reads an authority, T50 writes a leaf and
      reads it back, T51 renders a `tls` line naming it, T52 replaces it before it expires — and not
      one of them establishes that the running server presents that file to anything. The report
      `tls.md` calls the most common of all is invisible to every one of them.
      **Three of the four things it was asked for already existed** — days left is `leaf::read`,
      the name comparison is `mix doctor`'s, the trust stores are `cert.ca_status`' — and so did the
      fourth: *"offers one-click reissue"* is `mix cert issue --site` and `mix doctor --repair`, both
      shipped. So the new capability is exactly one: the handshake. The rest is an assembly, and it
      is worth building because the three answers live in three commands while the question a person
      has ("why is my padlock red") is answered by their conjunction.
      **What it decided.** The connection goes to loopback with the site's name as SNI and never to
      a resolved address: whether a name resolves is `DomainUnreachable`'s question, and a handshake
      that resolved would report a TLS fault on a machine whose only problem is a resolver nobody
      wired. **One connection answers both questions** — the verifier captures the chain *and* judges
      it against this home's authority, then returns `Ok` so the handshake completes and a failing
      server's certificate is reported rather than replaced by an error about it. Comparing issuer
      names was rejected and the test that proves why is in the suite: a leaf from a second authority
      is `Rejected`, and a name comparison would have to call the same name over a different key
      trusted. Two connections were rejected too — T52's loop can reload between them, so the two
      answers could describe two different servers.
      **The comparison is by fingerprint, and that is a correction to this task's own design.** The
      spec first said the presented certificate's SANs were compared against the site's domains;
      writing the plan found that a hash is the stronger rule, because it differs whenever anything
      differs, where names would call a server holding last month's certificate correct as long as
      the names had not changed. The spec was changed to match the code rather than the other way
      round.
      **And the daemon names the condition while the client names the command.** `CertProblem` is a
      closed set in the order a person would act, first match only — which is `ProblemId`'s own
      decision applied again, and it is what lets a graphical client render a button where `mix`
      renders `mix cert issue --site blog.test`. It is deliberately *not* `ProblemId`: those are
      conditions of the machine that `mix doctor` repairs, and `ServedCertificateDiffers` is repaired
      by reloading a front end rather than by touching a certificate at all.
      **What writing the plan found before any code was written.** Reading the front end's TLS port
      through `Generator::generate` would have made a read-only diagnostic **write**: `generate`
      goes through `declared`, which installs, so `mix cert status` would have rewritten this home's
      configuration and possibly reloaded a running server as a side effect of being asked a
      question — and would have destroyed the very state its most important test reproduces. The
      read-only `Generator::settings` exists for that, on `drift`'s precedent, and the `SpecSource`
      port gained the same question.
      **And it took a new edge rather than a new package**: `rustls` and `tokio-rustls` already
      reach the daemon through `mixengine-core` → `reqwest`, measured with `cargo tree` and then
      measured again — taking them added **zero** packages to `Cargo.lock`. The provider in this tree
      is `aws-lc-rs` and every config here names it, so a tree that ever enables a second provider
      fails to compile instead of panicking inside a running daemon.
      **What it deliberately did not do**: no name resolution; no `--fix`, because `cert.issue` and
      `doctor --repair` already reissue and a diagnostic that repaired what it found could not report
      it; no change to the front end's `ServiceSpec::ports`, which T51 made stale and which T38's
      diagnosis reads — separate work; no `mix doctor` check, because adding a `ProblemId` means
      deciding what repairing it is and the answer here is "reload the front end", which is not this
      task's to decide; and nothing written at all. Design:
      [T53 spec](../../docs/superpowers/specs/2026-08-25-t53-cert-status-design.md).
- [x] **T54** `cert.ca_rotate` and complete `ca_uninstall`, verified by enumerating the stores.
      **The only two operations in phase 5 that take something away**, and both were half-built
      before this task started: `PrivilegedOp::TrustCaRemove` has been implemented and tested in
      `mixengine-elevate` since T49a and `BrowserTrust::remove` since T49b, each documented as having
      no caller and naming T54 as its producer. So this is wiring and ordering, and the ordering is
      where the whole risk sits.
      **A rotation changes nothing until a fresh reading of the trust store agrees.** The candidate
      is generated into `certs/pending/`, one grant covers remove-old and install-new together, and
      the store is read *again* before anything is promoted — a declined prompt discards the
      candidate and leaves the home byte-identical. Deciding by the prompt's own report was rejected
      for T49b's reason: the helper describes finished work, and a probe reads the thing itself.
      **The staging root is a certificates root of its own, and that one choice paid for the task.**
      Every function in `ca.rs` takes the certificates root first, so `ensure` *generates* the
      candidate and `read` *describes* it with no second code path — and therefore no way for a
      candidate to be made differently from the authority it replaces. Promoting is two moves,
      discarding is one `remove_dir_all`, and `certs/pending/` collides with nothing because `read`
      reads two exact paths and leaves live under `certs/sites/`.
      **The commit condition is four clauses, not one**, and "is the new authority installed" is the
      wrong one: a machine with no store MixEngine can write would never pass it, and that machine is
      supported (T49a's D7). It commits when the store holds the new one, when there is no store,
      or when the store never held ours — and refuses when the old one was trusted and the new one is
      not, **and when either reading failed**. That last is the opposite of what every other probe in
      this daemon does; `require_trust_store` and `require_port_access` treat a failed read as "ask
      for nothing and carry on" because what they do next is harmless, and this is the one
      destructive operation in the phase.
      **And the "was it trusting the old one" reading has to be taken before the removal runs.**
      Asked afterwards it always answers no — the removal is what made it so — and a rotation that
      read it late would commit every time, leaving the clause in the source and out of the
      behaviour. Found in the plan's self-review, before any code.
      **No reissue code was written**, which is T50's fourth reuse question doing the job it was
      added for: the moment the authority differs, every leaf is stale by the existing rule and
      `issue(None)` replaces all of them. If this task had needed reissue logic, that would have been
      evidence T50's question was wrong.
      **`Elevation::grant_within` is the one change to the elevation machinery.** `grant` starts a
      job of its own, and a caller with work to do *after* permission is given has no hook between
      one job ending and another beginning. `grant` and `grant_within` now share a `preflight` that
      makes the same checks in the same order, and `flush`'s existing `Drop` guard on the single
      grant slot is what makes the split safe — a rotation that panics mid-way does not wedge every
      later grant for the life of the daemon.
      **`ca_uninstall` takes trust and never a file**, and it is allowed partial progress where a
      rotation is not: each store is independent, so cleaning Firefox is complete whatever the system
      store did, while a home with a new authority and half the machine trusting the old one serves
      leaves nobody accepts.
      **A test raised a real UAC prompt and installed a certificate authority into
      `LocalMachine\Root`.** The spec and the plan both argued that no machine running `cargo test`
      can raise an elevation prompt, so a rotation would always refuse and the test could assert
      "nothing changed". Measured 2026-08-26 on Windows: false. No arrangement of the *home* prevents
      it, because the store a rotation reaches belongs to the machine. Both end-to-end rotations are
      now `#[ignore]`d **and** gated on `MIXENGINE_SYSTEM_TESTS=1` — the second gate matters because
      `.github/workflows/ci.yml`'s `caddy` step runs that suite with `--ignored` on macOS and
      Windows — and what they assert is the *invariant*: a rotation either replaces the authority or
      leaves it alone, and never leaves a candidate private key on disk. Asserting one outcome would
      have made the test a statement about whoever answered the prompt.
      **And the gate is opened somewhere, which it was not until the 2026-08-27 review's R3.**
      `MIXENGINE_SYSTEM_TESTS=1` was named in four documents and set in none, so for as long as T52,
      T53 and T54 have been ticked, neither rotation had run anywhere. CI's `system` job sets it now
      and runs both suites — the `cert` invariant on all three systems, and the end-to-end `caddy`
      rotation on Windows and macOS, which are the two that can grant one. A Linux runner has no
      polkit agent, so there a rotation is refused and the invariant is the whole of what is left.
      **What runs on an ordinary machine**: the commit decision (six unit tests over a pure
      function), the discard (`ca::discard` leaves the live pair byte-identical, asserted on *both*
      halves so a discard that deleted everything could not pass), the store enumeration against
      `mock::Host` with a control, and the two refusals that reach no store at all. The wiring
      between the decision and the discard is covered only by the gated test, and saying so is the
      honest accounting.
      **Two documentation corrections.** `daemon-and-ipc.md` listed `cert.list`, `cert.renew` and
      `cert.ca_install`; none exists, and each was refused for a recorded reason rather than
      forgotten. And `tls.md`'s `ca-uninstall` criterion now says what the code does — it leaves the
      files. Design:
      [T54 spec](../../docs/superpowers/specs/2026-08-26-t54-ca-rotate-and-uninstall-design.md).

**Milestone M5** — `https://blog.test` is trusted in Chrome, Firefox, Safari and Edge on their
platforms; adding a domain keeps the padlock green.

---

Previous: [Phase 4 — Sites, domains and on-demand elevation](phase-4-sites-and-elevation.md) · Next: [Phase 7 — Efficiency](phase-7-efficiency.md)
