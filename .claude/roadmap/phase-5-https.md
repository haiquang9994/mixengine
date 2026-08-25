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
      T45's D13, because T54 and T87 are the producers. And it does **not** answer whether Firefox on
      Windows or macOS needs the same treatment: no machine here had one installed, and the method
      for finding out is written down in the design's D14 rather than guessed at. Design:
      [T49b spec](../../docs/superpowers/specs/2026-08-25-t49b-nss-databases-design.md).
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
- [ ] **T51** Web server TLS wiring; **disable Caddy's automatic ACME** explicitly.
- [ ] **T52** Renewal scheduler: daily + on-boot check, < 30 days threshold, reload without restart.
- [ ] **T53** `mix cert status` with a live handshake and SAN-mismatch detection; one-click reissue.
- [ ] **T54** `cert.ca_rotate` and complete `ca_uninstall`, verified by enumerating the stores.

**Milestone M5** — `https://blog.test` is trusted in Chrome, Firefox, Safari and Edge on their
platforms; adding a domain keeps the padlock green.

---

Previous: [Phase 4 — Sites, domains and on-demand elevation](phase-4-sites-and-elevation.md) · Next: [Phase 7 — Efficiency](phase-7-efficiency.md)
