# Git and code review

## Commit messages

Conventional-commit prefix, always, in English:

```
<type>(<scope>): <imperative message>
```

Types: `feat`, `fix`, `refactor`, `perf`, `docs`, `style`, `test`, `chore`, `ci`, `build`.

Scopes in this repo: `core`, `proto`, `platform`, `supervisor`, `daemon`, `elevate`, `cli`,
`runtime`, `services`, `dns`, `tls`, `blueprints`, `extensions`, `deps`, `ci`.

```
feat(dns): serve wildcard records for managed TLDs
fix(platform): preserve unmanaged lines when rewriting hosts file
refactor(supervisor): extract ready-check polling
chore(deps): bump hickory-dns to 0.24
```

Rules: never commit without a prefix; omit the scope rather than invent one; keep the subject under
72 characters; **never add a `Co-Authored-By` trailer**.

## Branches

- `main` is always releasable; every change lands via a branch.
- Branch names: `feat/dns-wildcards`, `fix/hosts-rollback`.
- Rebase to keep history linear; squash trivia, keep meaningful commits separate.

## Pull requests

A PR describes: what changed, why, which platforms it was tested on, and what it does *not* cover.
If it touches `mixengine-platform` or `mixengine-elevate`, it states explicitly how it was verified
on Windows, macOS and Linux.

## Reviews

**When asked to review, always ask which branch to compare against** (`main`, `dev`, or another) —
never assume a base branch.

Review checklist, in priority order:

1. **Damage potential** — can this leave the machine in a broken state (hosts, trust store, ports,
   firewall)? Is every mutation reversible and marker-scoped?
2. **Privilege** — does anything new cross into `mixengine-elevate`? Is it a typed, validated,
   allowlisted, one-shot op? Does it re-validate rather than trust the daemon? Does any code path
   elevate more than once for a single user action?
3. **Cross-platform** — does it compile and behave on all three? Are unsupported paths typed errors,
   not panics?
4. **Layering** — no logic in clients, no OS calls outside `mixengine-platform`, no `core` → `daemon`
   dependency.
5. **Failure handling** — timeouts, rollback, idempotency, `Degraded` vs `Failed` accuracy.
6. **Tests** — does the failure mode being fixed have a test that fails without the change?
7. Then style, naming and simplification.

## Definition of done

A roadmap task is done when: code + tests pass on all three OSes in CI, the CLI covers any new API
surface (and where a graphical client would need more than the CLI exposes, a follow-up task exists
and is listed in the roadmap in the right position), the relevant spec in `.claude/features/` matches reality, and
the task is ticked in its phase file under [`../roadmap/`](../roadmap/todo.md).
