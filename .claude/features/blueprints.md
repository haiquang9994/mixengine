# Project blueprints

**Goal**: capture "PHP 8.2 + MariaDB 11.4 + Redis, Laravel layout, HTTPS on" once, then create the
next project with that stack in one action — and hand the same file to a teammate.

## Manifest

A blueprint is a single TOML file, human-readable and diffable, stored in `blueprints/<name>.toml`
and exportable anywhere:

```toml
schema = 1

[blueprint]
name = "laravel-php82"
description = "Laravel + MariaDB + Redis"
created_at = "2026-08-10T09:00:00Z"
created_on = { os = "windows", version = "0.1.0" }

[runtimes]
php = "8.2.23"          # exact when captured, range allowed when hand-written
node = "22.8.0"

[site]
kind = "php-fpm"
doc_root = "public"
https = true
domain_pattern = "{project}.test"

[[services]]
name = "mariadb"
version = "11.4.3"
instance = "main"        # "main" = reuse the shared instance; "per-project" = dedicated
database = "{project}"
user = "{project}"

[[services]]
name = "redis"
instance = "main"

[php]
extensions = ["redis", "imagick", "xdebug"]

[scaffold]
# optional, only runs with explicit user consent at apply time
command = "composer create-project laravel/laravel ."
```

`{project}` is the only templating token; substitution is literal and validated (slug charset).

## Capture

`blueprint.capture { project, name }` reads the project's resolved state — runtime versions, linked
services and their versions, PHP extensions, site kind and HTTPS — and writes the manifest. It
captures *what is actually in use*, not the global defaults, and it never captures data,
credentials, or absolute paths.

**There is no `[php] ini`, and this line used to promise one.** T77 went looking for the source and
there is none: every ini value MixEngine writes is a constant in `core::runtimes::extensions`,
identical on every machine, and the only per-pool override map a php-fpm service has holds
`max_children` and its siblings — process supervision, not ini. So "non-default ini values" named
nothing that deviates, and capturing them would have captured a global default, which is the one
thing this feature is defined against. The key arrives with the task that gives a project an ini of
its own. `extensions` stays, because a runtime's extension *choices* already are deviations.

Of those choices, capture takes only the ones turned **on**. A blueprint says what a project needs
loaded; turning something off on the receiving machine would change the PHP every other project
there runs, which is harm it was never asked to do.

A project with more than one site is **refused** rather than reduced to its first: a manifest has one
`[site]`, and losing the others silently is worse than saying so. `[[sites]]` is where that widens.

## Apply

`blueprint.apply { blueprint, project_name, root_path }` runs as a job with a **plan-then-execute**
shape:

1. **Plan**: resolve every requirement against what is installed, and return the full list of actions
   (install PHP 8.2.23, create DB `blog`, add domain `blog.test`, issue cert…). The plan is returned
   before anything happens; `mix blueprint apply --dry-run` prints it.
2. **Execute**: run the actions with progress. Each action is idempotent, so a failed apply can be
   resumed rather than restarted.
3. **Rollback on failure removes what belongs to the project and keeps what belongs to the
   machine** — T78's design, D4. Undone: the site, a service instance dedicated to this project, and
   the project row. Kept, and each one *named* in the failure: **the database**, because by the time
   an apply has failed a scaffold may have migrated into it and destroying data to tidy up is the
   more expensive direction to be wrong in (`database.create` is idempotent, so running the apply
   again finds it and moves on, and there is no `database.drop` in this product — see
   [services.md](services.md)); **a runtime or package this apply installed**, which is what a
   resumed apply would otherwise download all over again; **a PHP extension it turned on**, which
   reaches every project on the machine; and **the project's directory**, on `project.delete`'s
   standing rule that the files were never ours.

   A shared instance that already existed is never removed either way. And `job.cancel` is not a
   failure: it stops where it is and leaves what was made, because running the apply again continues
   from there.

**Resuming is running it again.** Every action is an *ensure*, so a second apply plans against what
the first one left: everything already done comes back `Satisfied` and what remains is exactly what
remains. There is no ledger of half-finished applies to reconcile — the rows are the record.

Version mismatches are surfaced as choices, not silent decisions: *"PHP 8.2.23 is not installed.
Install it / use installed 8.2.29 / cancel."* **The question is asked by a client and answered in the
request**: a daemon has no keyboard, so `blueprint.apply` carries an answer per subject and refuses,
before anything happens, when one is missing. The answer decides the project's *pin* as well as the
download — without that, "install it" and "use the installed one" would leave identical machines
behind and the question would be theatre.

**An apply never raises an elevation prompt.** It queues what needs one, exactly as `site.create`
does, and the client spends the single prompt afterwards — `mix blueprint apply --grant`, or the
question it asks at the end.

## Scaffold commands

`[scaffold]` runs an arbitrary command in the new project directory, which is a real execution of
untrusted content when the blueprint came from someone else. **T78a** is what built the answer:

- **Never runs without agreement naming the exact command.** The consent travels in the
  `blueprint.apply` request — a daemon has no keyboard — and it carries *the command the person
  read*, so a blueprint re-imported between the plan and the apply cannot be run under an old yes.
  An apply carrying no consent applies everything else and reports the step as `NotRun`, because a
  blueprint must not become worthless over the one step nobody answered for.
- **Never runs on import**, only on apply, and never with an elevation: the command runs under the
  user's own account and nothing it does reaches the elevation queue.
- Runs in the project directory with `<home>/bin` in front of `PATH`, which is how the blueprint's
  own `[runtimes]` reaches it — the shims resolve a version from the project they are run in.
  Output goes to the job's log (`GET /logs/job/{id}`, `mix job logs <job> -f`), which is the log
  surface a service's output already uses and not the event stream: how much a scaffold prints is
  decided by somebody else's program.
- **No timeout.** Any number would kill a legitimate `composer install` on a slow line; the bound is
  that the job is visible and `job.cancel` stops it — killing the process *group*, so what a package
  manager forked goes with it.
- **A command that exits non-zero is a failed step, not a failed apply**, and rolls nothing back: the
  project it made works, and destroying it because a post-install script failed is the more
  expensive direction to be wrong in. `mix` exits non-zero on it.
- **Trust is decided when a blueprint arrives and is never raised.** `blueprint.import` verifies a
  detached minisign signature against the compiled-in gallery key; what verifies is trusted, and
  what does not — including a file with no signature at all — is untrusted for good. A signature
  that does not verify is not a refusal: the blueprint is still imported, and what it loses is the
  right to a quiet yes. `mix blueprint apply --run-scaffold` agrees for a signed blueprint and
  `--run-untrusted-scaffold` for one nobody vouches for; neither covers the other, so a script that
  runs somebody's unsigned command says so on the line that does it.

## Built-in gallery

Six blueprints ship **inside the binary** and are seeded into every home as `builtin` rows the first
time a daemon starts there: `django`, `laravel`, `nextjs`, `static`, `symfony`, `wordpress`. They are
trusted without a signature check, because a signature travelling in the same binary as the key it
would be checked against proves nothing the binary has not already proved — publishing them as
signed files for hand import is T79a.

Seeding **compares before it writes**, so the ordinary daemon start touches nothing, and a row whose
source is `captured` or `imported` is never overwritten: capturing over `laravel` makes that slug
this machine's own for good. There is no `blueprint.delete` in this build, so the six are in every
home for good as well.

Three of them carry a `[scaffold]` — `laravel`, `symfony` and `nextjs` — and three deliberately do
not. A gallery command has to be non-interactive (there is no timeout, so a prompt would hang a
job), spelled the same for `cmd.exe` and `sh`, and it may not write into a shared runtime: that last
rule is what removes Django's, since `pip install django` reaches every project using that Python.
The gallery sells a stack, not a scaffold.

They double as end-to-end tests of the whole system, but **not of the cross-OS criterion below** — a
hand-written manifest is byte-identical on all three systems, so what proves that one is a real
capture taken on Windows and committed as a fixture.

## Acceptance criteria

- Capture a working project, apply it under a new name, and both sites serve correctly at the same
  time with no manual steps.
- Applying a blueprint whose PHP version is missing installs it and continues without user
  babysitting beyond the initial confirmation.
- A blueprint exported on Windows applies on macOS (no absolute paths, no OS-specific service names).
- `--dry-run` output matches exactly the actions the real run performs.
