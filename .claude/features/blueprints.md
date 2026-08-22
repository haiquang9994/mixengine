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
ini = { memory_limit = "512M", upload_max_filesize = "64M" }

[scaffold]
# optional, only runs with explicit user consent at apply time
command = "composer create-project laravel/laravel ."
```

`{project}` is the only templating token; substitution is literal and validated (slug charset).

## Capture

`blueprint.capture { project_id, name }` reads the project's resolved state — runtime versions,
linked services and their versions, PHP extensions and non-default ini values, site kind and HTTPS —
and writes the manifest. It captures *what is actually in use*, not the global defaults, and it never
captures data, credentials, or absolute paths.

## Apply

`blueprint.apply { blueprint, project_name, root_path }` runs as a job with a **plan-then-execute**
shape:

1. **Plan**: resolve every requirement against what is installed, and return the full list of actions
   (install PHP 8.2.23, create DB `blog`, add domain `blog.test`, issue cert…). The plan is returned
   before anything happens; `mix blueprint apply --dry-run` prints it.
2. **Execute**: run the actions with progress. Each action is idempotent, so a failed apply can be
   resumed rather than restarted.
3. **Rollback on failure** is limited to what this apply created — a shared MariaDB instance that
   already existed is never removed.

Version mismatches are surfaced as choices, not silent decisions: *"PHP 8.2.23 is not installed.
Install it (120 MB) / use installed 8.2.29 / cancel."*

## Scaffold commands

`[scaffold]` runs an arbitrary command in the new project directory, which is a real execution of
untrusted content when the blueprint came from someone else. Therefore:

- Never runs without an explicit confirmation showing the exact command.
- Never runs on import, only on apply.
- Runs with the project's resolved runtime on PATH, in the project dir, with output streamed to the
  job log.
- Blueprints from the built-in gallery are signed; hand-imported ones are marked untrusted forever.

## Built-in gallery

Ship a small, maintained set: Laravel, WordPress, Symfony, static site, Node/Next.js reverse-proxy,
Python/Django. They double as end-to-end tests of the whole system.

## Acceptance criteria

- Capture a working project, apply it under a new name, and both sites serve correctly at the same
  time with no manual steps.
- Applying a blueprint whose PHP version is missing installs it and continues without user
  babysitting beyond the initial confirmation.
- A blueprint exported on Windows applies on macOS (no absolute paths, no OS-specific service names).
- `--dry-run` output matches exactly the actions the real run performs.
