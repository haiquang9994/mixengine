+++
title = "Servers, databases and caches"
slug = "services"
order = 6
summary = "Caddy or Nginx, MariaDB, MySQL, PostgreSQL, Redis and Memcached — installed on request, configured for you, and never printing a password."
+++

# Servers, databases and caches

Two words, kept apart the way MixEngine keeps them apart.

A **package** is a program MixEngine knows how to run — Caddy, MariaDB, Redis. Installing one puts a
copy of it in MixEngine's own directory and does nothing else.

A **service** is a running instance of a package: a port, a data directory, a generated
configuration, a log, and a state. `mariadb@main` and `mariadb@legacy` are two services of one
package, with different ports, different data and possibly different versions.

## What is on offer

| Service | Default line | Default port |
| --- | --- | --- |
| Caddy | 2.x | 80 and 443 — the default front end |
| Nginx | 1.27 | 80 and 443 — the alternative, one front end at a time |
| php-fpm | one per installed PHP | a socket, or a local port on Windows |
| MariaDB | 11.4 LTS | 3306 |
| MySQL | 8.4 LTS | 3306 — a different product from MariaDB, not a version of it |
| PostgreSQL | 16 | 5432 |
| Redis | 7.x | 6379 |
| Memcached | 1.6 | 11211 |

**Nothing arrives by itself.** A fresh MixEngine has no web server until you install one, and
"default" above means *the one this project recommends when there is a choice*, not *the one that is
already there*.

## Installing and creating

```bash
mix package available
mix package install mariadb 11.4.4
mix service create mariadb@main 11.4.4
```

The part of a service id before the `@` is the package it is an instance of, which is why
`mix service create` needs no separate argument for it. The part after the `@` is yours: it is what
tells two of them apart, and MixEngine attaches no meaning to the words. Caddy runs once for a whole
MixEngine home, so its service is simply `caddy` with no `@` at all.

The id cannot be changed afterwards — it is also the generated configuration directory, the log
directory, the socket and the address the password is stored at — so renaming one means creating the
other and deleting this one, which keeps the data.

Useful flags on `mix service create`:

| Flag | What it does |
| --- | --- |
| `--port` | The port it listens on. The recipe's own default when left out |
| `--bind` | The address it binds. `127.0.0.1` when left out |
| `--data-dir` | Where its data lives. A directory under the home when left out |
| `--autostart` | Start it whenever the daemon starts |

### Who gets 3306

MariaDB and MySQL want the same port, and so do two instances of either. The rule is one rule:
**first created, first served**. The first to ask for 3306 gets it; the next gets the first free
port above. MixEngine reports the port it chose, because a port you did not pick is one you have to
be told about.

A port you name explicitly is taken at your word, with no allocation at all.

### One data directory, one service

`mix service create` refuses a `--data-dir` another service already holds, and names who holds it.
Two servers over one set of files corrupt them, and that cost lands on your data rather than on a
start that fails.

## Running them

```bash
mix service list
mix service status mariadb@main
mix service start mariadb@main
mix service stop mariadb@main
mix service logs mariadb@main --follow
```

`mix service status` requires an id where `start` and the rest take an optional one: a status with
no subject is a `list` typed wrongly, and answering it as a list would hide that.

Deleting a service takes the row and the configuration generated from it, and **never the data** —
that is somebody's databases. The answer names the directory that was left, so nobody has to go
looking:

```bash
mix service delete mariadb@legacy
```

## Databases and accounts

Making a database is one command, and it starts the server if it is not running:

```bash
mix database create mariadb@main --name blog
mix database create mariadb@main --name shop --user shop_app
```

**Nothing prints the password.** It is generated and put into your operating system's own credential
store — Credential Manager on Windows, the Keychain on macOS, the Secret Service on Linux — and what
is printed is the address it was stored at, as the store's own name and key. That is what lets a
client tell you *"stored in your credential store as …"* without anybody hardcoding MixEngine's
naming.

To open the database in a desktop client:

```bash
mix database client mariadb@main   # what is installed, and where this system looked
mix database open mariadb@main     # open it
```

`client` reads only: it starts nothing and opens nothing, and *"no client installed"* is an answer
rather than a failure — it names where MixEngine looked and where to get one.

`open` starts the instance if it is stopped, reads the password from the credential store **at that
moment**, and hands it to the client in that process's own environment. It is never printed, never
put in an argument, and so never in your shell history.

## What a service may take, and when it stops

```bash
mix service limits mariadb@main
mix service limits mariadb@main set --memory 512 --cpu 50
mix service idle mariadb@main --after 30m
```

`limits` with no subcommand reads; `set` replaces; `clear` removes. **`set` replaces every field,
not only the ones you name** — `set --cpu 50` clears a memory ceiling that was there — so it prints
all three fields of the result, and a cleared limit is on your screen rather than a surprise. What
your operating system will actually enforce differs, and the answer says which of the two you have:
a **hard** ceiling is a wall — at it, the service is killed or its next allocation fails — while an
**advisory** one is a watched line the service may cross, after which MixEngine warns and, where the
recipe permits, restarts. A control drawn as a guarantee when it is advisory would be a lie about
your data.

`idle` says when a service is stopped for being unused, and what is currently holding it open.
**Nothing idles by default**: a stopped service stays stopped until you start it, so switching this
on is a choice you make per service.

## The generated configuration

MixEngine writes the configuration for every service it runs, out of what it knows. Those files are
disposable — they are regenerated, never read back — so there is nothing there for you to edit and
nothing to keep in sync. If a setting you need has no flag, that is a gap in MixEngine rather than
an invitation to edit the file.
