+++
title = "PHP, Node, Python and Ruby versions"
slug = "runtimes"
order = 5
summary = "Install as many versions as you need, and let each directory choose its own — with no shell hook and nothing to remember."
+++

# PHP, Node, Python and Ruby versions

MixEngine installs language runtimes into its own directory, one immutable folder per version, and
never touches whatever your operating system already has. Installing a version never modifies a
version already installed, so nothing you have working can be broken by adding something new.

Four languages are managed: **PHP**, **Node.js**, **Python** and **Ruby**.

## Installing a version

```bash
mix runtime available --kind php
mix runtime install php 8.3.33
mix runtime list
```

The version is exact, and that is deliberate rather than an omission. `8.3` asks *"choose one for
me"*, and there is nothing to choose from until something is installed — choosing between versions
is what resolution does, and resolution answers with what is on the machine. `mix runtime available`
is where a range belongs.

An install is a job, and `mix` waits for it by default: `mix runtime install php 8.3.33 && …` is a
sentence about PHP being there. `--no-wait` returns as soon as the daemon has accepted the work and
hands you a job id, which `mix job wait` can be pointed at later.

**Installing a PHP also creates its php-fpm pool** — `php-fpm@8.3.33`, a service like any other, in
`mix service list`. Node, Python and Ruby are invoked per command and have nothing supervised.

## Choosing which one a directory uses

Nothing here changes a shell, patches a profile, or asks you to type an activate command. A
directory resolves to a version, and the shims do the rest.

```bash
mix runtime default php 8.3.33      # the machine-wide fallback
mix project update blog --pin php=^8.1
mix runtime resolve php             # what does *this* directory get, and why?
```

`mix runtime resolve` is the command to remember. It answers what `php -v` would answer, without
running anything, **and** it names which of four sources decided it:

1. An explicit flag or environment variable on the command being run.
2. The nearest `mixengine.toml` that names this language, walking up from where you are.
3. The registered project covering this directory.
4. The global default.

A `mixengine.toml` that says nothing about PHP is not an answer about PHP, so a pin further up still
applies.

### Writing a constraint

Pins and `--version` accept three shapes, all resolved against **installed** versions and never
silently against downloadable ones:

| Written | Means |
| --- | --- |
| `8.3.33` | Exactly that |
| `8.3` or `8` | As many segments as are written have to agree; one nobody wrote is a zero |
| `^8.3` | Up to the leftmost non-zero segment — `^0.12` stops before `0.13` |

A constraint with no pre-release in it never selects one. `8.5` and `^8.5` both pass over
`8.5.0RC1`; naming it exactly is how you ask for it.

## The shims

`mix path install` fills `<root>/bin` and puts that one directory on your `PATH`. It holds a small
program per command — `php`, `php-config`, `pecl`, `composer`, `node`, `npm`, `npx`, `python`,
`pip`, `ruby`, `gem`, `bundle` — and each one works out which version this directory wants and hands
over to the real binary.

Two things follow that are worth knowing:

- **It works with the daemon stopped.** A shim reads what it needs directly rather than asking over
  a socket, which is why `php -v` in a project still answers when MixEngine is not running.
- **There is nothing to refresh after an install.** The list of commands is fixed, so `<root>/bin`
  does not depend on what you have installed. A `node` shim on a machine with no Node.js resolves
  nothing and tells you which command to type.

Only `<root>/bin` goes on your `PATH` — one entry, never a directory per version.

```bash
mix path status
mix path uninstall
```

`mix path uninstall` takes the directory back off your `PATH` and leaves the commands where they
are: they live inside MixEngine's own home, and removing the home is what removes them.

## PHP extensions

Extensions are per installed version, because that is what they are compiled against:

```bash
mix runtime ext list --php 8.3.33
mix runtime ext enable redis --php 8.3.33
mix runtime ext disable xdebug --php 8.3.33
```

`list` says which extensions the build has and **why each one is on or off**, which is usually the
question. Leaving `--php` out means the version this directory resolves to.

Enabling loads the extension on every PHP process of that version, the pool included.

## Removing a version

```bash
mix runtime uninstall php 8.1.31
```

This is refused while a registered project pins that version — the projects are named — and while
the php-fpm pool running out of it is running. `--force` crosses the first of those and never the
second.
