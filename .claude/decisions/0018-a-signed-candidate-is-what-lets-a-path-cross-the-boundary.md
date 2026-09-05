# 0018. A signed candidate is what lets a path cross into the elevated process

**Status**: Accepted
**Date**: 2026-09-05

## Context

[ADR 0015](0015-the-helper-installs-itself.md) gave `mixengine-elevate` a way onto a machine —
`PrivilegedOp::HelperInstall {}`, carrying no fields, copying the elevated process's own image to a
compiled-in destination — and refused `HelperInstall { source: PathBuf }` in one line: *"it is
`Exec { cmd }` with two more steps, and the closed-enum rule in the security model exists to refuse
that shape."*

That refusal was right for an operation with no way to tell one file from another. It also left the
helper **unreplaceable**, and T88a measured how completely:

1. `elevation::choose` prefers the *installed* copy, so on any machine past its first prompt the
   elevated process **is** the installed helper. `HelperInstall {}` then compares `current_exe()`
   with its destination, finds them the same file, and answers `AlreadyDone` — for ever.
2. `updates::apply::swap` deliberately keeps `mixengine-elevate`, so after `mix self-update` even the
   copy beside `mixengined` is the old one. Nothing on the machine is newer than what is installed.

MixEngine 0.1.0 ships T88a. **A 0.1.0 that went out without this is a 0.1.0 whose helper no later
release could ever fix** — and it is the only file this product runs as root.

## Decision

**A path may cross into the elevated process when, and only when, that process itself establishes
that the bytes at it carry a signature made by a key compiled into it.**

- `PrivilegedOp::HelperReplace {}` **still carries no field**. The candidate is at
  `helper_candidate(request.home)` — a compiled-in name under the directory the elevated process has
  already established belongs to whoever wrote the request, and inside which the request itself
  sits. Nothing about *where* is anything the caller said.
- The elevated process reads those bytes **once**, verifies them against
  `mixengine_elevate::candidate::PUBLIC_KEY` — the release signing key, compiled in, checked at build
  time against the committed `packaging/updates.pub` — and writes **the bytes it verified**. It never
  re-opens the file: a `verify(path)` followed by a `copy(path, …)` is a check the caller can step
  past by swapping the file in between.
- The signature's **trusted comment**, which minisign's global signature covers, carries
  `mixengine-elevate <version> <os> <arch>`. A candidate ordered before the running helper's own
  version is refused, and so is one built for another machine.
- **Only the installed copy may apply it.** A helper running out of a directory the user can write,
  checking a signature, proves nothing: whoever could replace the helper could replace the check.

So the primitive this grants is not *copy this file as root*. It is *install a `mixengine-elevate`
that MixEngine signed, for this machine, and never an older one* — which is a different thing from
`Exec { cmd }` in exactly the way that matters, and is the difference ADR 0015's one-line refusal did
not yet have available to it.

## Consequences

- **The helper can be upgraded**, which it could not be, by `mix elevation upgrade` →
  `mix elevation grant`. Nothing is installed without an elevation prompt, and the prompt's own
  screen says what the check was.
- **The check is made twice, and that is not a duplicate.** The daemon verifies before staging so a
  bad download costs a sentence rather than a prompt; the elevated process verifies again because it
  is the only one of the two that is not the attacker if the daemon has been compromised.
- **Rotating the updater key becomes a heavier one-way door.** Every installed helper pins exactly
  one key, so after a rotation no helper installed before it will accept any candidate again, and the
  only thing that can replace one is a package manager running as root.
  [../features/updates.md](../features/updates.md) already called a rotation an application release
  *and* an announcement; it is now also a helper nobody can replace remotely.
- **The first prompt on a fresh machine is unchanged, and still unchecked.** There the elevated
  binary is the copy beside the daemon — the only candidate there is — and it installs its own image.
  [../architecture/security-model.md](../architecture/security-model.md) states that residual; this
  ADR closes every replacement *after* the first and does not close the first.
- **An offline machine cannot upgrade its helper.** The candidate comes from the release, because the
  signing key exists only in the `release` job (see the alternatives below), so a person who installed
  by hand on a machine with no network keeps the helper they have — working, serving everything it
  knows, and saying so on `mix elevation status`. The `.deb`, the `.rpm` and the `.pkg` do not have
  this problem: they place the helper as root at install time.
- `mixengine-elevate` gains one dependency, `minisign-verify`. It has none of its own.

## Alternatives considered

- **Ship the signature inside the payload archive**, beside the binary. Impossible rather than
  unattractive: `UPDATE_SECRET_KEY` reaches exactly one step of one job, after all five `build` legs
  have uploaded, and an artifact cannot gain a file after `feed.sh` has hashed it. Putting the key on
  five runners across three operating systems to avoid one download is not a trade this project
  makes.
- **Repack the archives inside the `release` job**, adding the signature there. Then the bytes a
  build leg opened and checked are not the bytes that ship, and it reaches none of the five
  installers anyway.
- **No version in the signed comment**, on the grounds that only we can sign. "Only we can sign"
  bounds an attacker to *our own past releases*, which is the entire content of a downgrade attack.
- **No OS or architecture in it either.** A correctly signed `aarch64` helper installed on an
  `x86_64` machine is a machine that can no longer elevate anything, with no way back but a
  reinstall.
- **Teach `HelperInstall {}` to prefer a candidate when one is present.** One operation whose meaning
  depends on a file the untrusted caller controls is an operation whose `describe()` cannot tell the
  truth, on the one screen whose whole job is to say what is about to happen. Worse, it would turn a
  *first* install into a downgrade: plant an old signed helper, wait for first-run setup, and the
  machine's permanent helper is the one with the hole in it.
- **Let `mix self-update` replace the helper beside `mixengined`.** That is the auto-update boundary
  [../features/updates.md](../features/updates.md) calls the single most important rule on its page,
  and on a machine with nothing installed it is *the file the next prompt elevates*.
- **An `--from <path>` on `mix elevation upgrade`**, which would make the upgrade work offline. Not
  refused, and not built: it is a flag that chooses which file is a candidate for running as root,
  and a signature makes that *safe* rather than *obviously safe*. It ships the day somebody has the
  machine that needs it.
