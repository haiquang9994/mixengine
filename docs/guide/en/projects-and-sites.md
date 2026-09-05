+++
title = "Projects and sites"
slug = "projects-and-sites"
order = 4
summary = "The two nouns MixEngine is built on, what each one owns, and how a checkout carries its own setup."
+++

# Projects and sites

MixEngine has two nouns and they are worth keeping apart.

A **project** is a directory on your disk that MixEngine knows about. It owns the path, a name, and
which language versions that directory uses.

A **site** is something served, under a project. It owns one or more domains, what is served out of
which folder, and what serves it. A project with no site is perfectly normal — it is a directory
whose PHP version MixEngine knows. A project can have several sites.

## Registering a project

```bash
cd ~/code/blog
mix project create
mix project list
mix project show blog
```

With no arguments, `mix project create` takes the current directory and names the project after it.
`--name` overrides that, and `--pin` fixes a language version for everything under that directory:

```bash
mix project create --name blog --pin php=^8.3 --pin node=22
```

`mix project update` changes any of it afterwards. One thing to know: `--pin` **replaces** every pin
rather than adding to the set, and `--clear-pins` with no `--pin` removes them all. Deleting a
project forgets it; your files are left exactly as they are.

## Declaring a site

```bash
mix site create --domain blog.test --kind php-fpm --https true
mix site list
mix site show blog.test
```

`--doc-root` is the folder that is served, relative to the project root — `public` for most modern
PHP frameworks, and the project root itself when it is left out. `--domain` may be given more than
once; the first is the **primary**, and the rest are aliases. The primary matters: it is what the
site's canonical URL and its certificate are named after.

`mix site update` changes a site. Like `--pin` above, `--domain` and `--service` replace what the
site had rather than adding to it — giving neither changes neither.

Starting and stopping a site is a flag and a re-render, not a process:

```bash
mix site stop blog.test
mix site start blog.test
```

Nothing is started or killed by those. A site is a declaration; the services it uses have states of
their own.

## The four kinds of site

| `--kind` | What it is |
| --- | --- |
| `php-fpm` | PHP, through a pool of the version this directory resolves to |
| `static` | Files, and nothing running |
| `reverse-proxy` | Everything forwarded to an address you already have listening — `--upstream` |
| `node-app` | A Node process you run yourself, on a port — `--port` |

`reverse-proxy` and `node-app` are the two that matter when you are already running something.
MixEngine gives it a real name and a certificate without taking over how it is started.

## `mixengine.toml`, and adopting a colleague's checkout

A project can describe itself, in a file checked into the repository:

```toml
[project]
name = "blog"

[runtimes]
php = "8.3"
node = "22"

[site]
domain = "blog.test"
aliases = ["api.blog.test"]
doc_root = "public"
kind = "php-fpm"
https = true

[[services]]
name = "mariadb"
version = "11.4"
database = "blog"
```

With that file present, `mix project create` and then `mix site create` with no arguments at all do
what the file says. That is what adopting somebody else's checkout looks like: clone, two commands,
and the same PHP version and the same domain as the person who wrote it.

Going the other way, `mix project export` writes the current project into `<root>/mixengine.toml`,
keeping everything else already in the file.

## Which version does this directory use?

Four things can decide, and they are consulted in this order:

1. An explicit flag or environment variable for the command you are running.
2. The nearest `mixengine.toml` that names **this language**, walking up from where you are. A
   manifest that says nothing about PHP is not an answer about PHP, so an outer pin still applies.
3. The registered project whose root is that directory or one above it.
4. The global default.

Rather than work that out yourself, ask:

```bash
mix runtime resolve php
```

That answers which installed version this directory gets **and which of the four decided it**, which
is the half people actually want when a version surprises them. Nothing is run to find out.

## Keeping a project warm

Services can be stopped automatically when nothing has used them for a while. While you are working
on a project, that is a pause you do not want:

```bash
mix project keep-warm blog
mix project keep-warm blog --off
```

This is a verb of its own rather than a setting on the project, because it is something you do for
an afternoon and not part of what the project *is*. It reaches the PHP pool the project's sites
name; it does not yet reach a database they query, because nothing in MixEngine records which
database a project uses.
