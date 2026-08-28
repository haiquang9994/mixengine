# Technical review notes

Every full review of the repository is one `YYYY-MM-DD.md` file in this folder. The folder records
**the state of the code at a point in time** — which the roadmap does not: the roadmap says *what
to build*, this says *how good what was built turned out to be*.

The scope is purely technical: correctness, safety, testing, structure, style. Anything that
belongs to releasing or to end users (installers, a public README, documentation, market) is
**not recorded here** — it has its own place in [phase-9-ship.md](../roadmap/phase-9-ship.md).

## Conventions

- Every finding carries an id `R<n>` that is **global and never reused**, like the roadmap's
  `T<n>` task numbers: a later review citing `R7` means that finding, whether or not it has been
  fixed since.
- Three groups, in this order: **Bugs** (wrong against what the code or its documentation claims),
  **Improvements** (correct, but expensive, fragile, or off the standard the rest of the workspace
  keeps), **Done well** (what to preserve when refactoring).
- Status: `[ ]` open · `[x]` fixed (name the commit/PR) · `[-]` deliberately not fixed (say why).
- A later review **updates the status in the older file first**, then writes a new file holding
  only *new* or *recurring* findings; open findings are not copied forward.
- Every finding cites `file:line` as of the review; line numbers drift, the function or constant
  named beside them is what to search for.

## Reviews

| Date | Scope | Open / total |
| --- | --- | --- |
| [2026-08-27](2026-08-27.md) | The whole workspace at `d29e6dc` (T69), 129 commits | 5 / 16 (R10 and R16 deliberately not fixed) |
