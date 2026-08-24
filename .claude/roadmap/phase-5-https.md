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
      the operating system and is **T49**'s, and a field this build could only fill with "unknown"
      is not an answer. No `mix doctor` check, because adding a `ProblemId` means deciding what
      repairing it is, which is T54's decision. And `mix cert status` is **left free** for T53's
      per-site handshake, with a test asserting it still fails.
- [ ] **T49** Trust store install/remove per OS, including Linux NSS DBs for Firefox/Chrome —
      **batched with T42 and T45 into the single first-run elevation prompt**. **(P)**
- [ ] **T50** Leaf issuance: 90 days, site SANs, `serverAuth` only, idempotent reuse.
- [ ] **T51** Web server TLS wiring; **disable Caddy's automatic ACME** explicitly.
- [ ] **T52** Renewal scheduler: daily + on-boot check, < 30 days threshold, reload without restart.
- [ ] **T53** `mix cert status` with a live handshake and SAN-mismatch detection; one-click reissue.
- [ ] **T54** `cert.ca_rotate` and complete `ca_uninstall`, verified by enumerating the stores.

**Milestone M5** — `https://blog.test` is trusted in Chrome, Firefox, Safari and Edge on their
platforms; adding a domain keeps the padlock green.

---

Previous: [Phase 4 — Sites, domains and on-demand elevation](phase-4-sites-and-elevation.md) · Next: [Phase 7 — Efficiency](phase-7-efficiency.md)
