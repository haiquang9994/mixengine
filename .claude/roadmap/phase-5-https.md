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
      thing here a unit test could not have found. And the macOS *removal* does not return with no
      console, while the macOS install is complete — certificate in the keychain, trusted as a root
      for every use. That half is **T49c**, and it blocks T54 rather than anything shipped.
      **A helper behind an elevation prompt has no console and must never wait for one.** Every
      `security` call now has `/dev/null` for standard input and a thirty-second deadline, and every
      `Failed` this helper produces carries the OS's own words through `mixengine_proto::flatten`
      instead of only the verb it was attempting — four sites, one mistake, found because a failure
      that named the action and dropped the cause could not be acted on.
      **What it deliberately did not do**: no producer for the removal — built, validated and tested
      with none, on T42's D12 and T45's D13, because T54 and T87 are the producers. No `certutil`
      fallback on Windows. And `tls.md`'s claim that a removal deletes by fingerprint is corrected
      rather than implemented.
- [ ] **T49b** The Linux NSS databases for Firefox and Chrome — `~/.pki/nssdb` and every profile
      under `~/.mozilla/firefox/*/`. Unprivileged, in the daemon, and **no part of any elevation
      batch**.
      **Starts from a measurement rather than from the specification**: `tls.md` names `certutil` as
      the mechanism, and on a stock Ubuntu 24.04 it is not installed — it ships in `libnss3-tools`,
      and `~/.pki/nssdb` does not exist either. A machine without the tool is a state to report, not
      a failure. It must also answer, by measuring rather than by assuming, whether Firefox on
      Windows and macOS needs this treatment too: `tls.md` names NSS on Linux alone, and whether the
      other two read the system store depends on `security.enterprise_roots`.
- [ ] **T49c** The macOS removal, which T49a measured and could not make work. **(P)**
      `security remove-trusted-cert -d <file>` **does not return** when run as root without a
      console: on two separate CI runs it sat until `mixengine-platform`'s own thirty-second
      deadline killed it. Everything around it was measured on the same runs and works —
      `add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain` installs, a second
      install answers `AlreadyDone`, and `security dump-trust-settings -d` reads the admin domain
      under `sudo` without pausing for anything. The install is complete rather than partial:
      `dump-trust-settings` lists the certificate with an empty settings array, which is macOS for
      *trusted as a root for every use*. So one verb is the fault, and the two explanations left
      have not been told apart:
      **(a)** `remove-trusted-cert` takes no keychain argument, so it searches the default list —
      which under CI's `sudo -E` is the *invoking* user's, whose login keychain is locked. If that
      is it, the hang is an artefact of `sudo -E` and not of the product, since a helper raised
      through the OS elevation prompt runs with root's own home.
      **(b)** `add-trusted-cert -k <keychain>` names a keychain and `remove-trusted-cert -d` goes
      through the admin-domain API, which asks `AuthorizationCopyRights` for a right that cannot be
      granted with no agent to display. If that is it, the mechanism has to change and the T49a
      design's D6 changes with it.
      **`delete-certificate` is not the answer on its own** and the reason is worth writing down:
      it takes an explicit keychain and would not hang, but macOS evaluates admin trust settings by
      certificate hash, so deleting the certificate from the keychain leaves the machine *still
      trusting* it. A `ca-uninstall` built on that would report a removal it had not performed,
      which is worse than one that fails.
      **What this does not block.** T49a shipped no producer for the removal — T54 and T87 are the
      producers — so nothing in the product calls this today. What it blocks is T54.
      `mixengine-elevate/tests/system.rs` measures the removal on macOS and does not require it:
      it must answer either a change or this known failure, so a `Refused` or a different message
      is still a test failure, and a machine that starts answering properly makes the note here
      wrong in a way somebody will notice.
- [ ] **T50** Leaf issuance: 90 days, site SANs, `serverAuth` only, idempotent reuse.
- [ ] **T51** Web server TLS wiring; **disable Caddy's automatic ACME** explicitly.
- [ ] **T52** Renewal scheduler: daily + on-boot check, < 30 days threshold, reload without restart.
- [ ] **T53** `mix cert status` with a live handshake and SAN-mismatch detection; one-click reissue.
- [ ] **T54** `cert.ca_rotate` and complete `ca_uninstall`, verified by enumerating the stores.

**Milestone M5** — `https://blog.test` is trusted in Chrome, Firefox, Safari and Edge on their
platforms; adding a domain keeps the padlock green.

---

Previous: [Phase 4 — Sites, domains and on-demand elevation](phase-4-sites-and-elevation.md) · Next: [Phase 7 — Efficiency](phase-7-efficiency.md)
