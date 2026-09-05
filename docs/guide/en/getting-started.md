+++
title = "Your first site"
slug = "getting-started"
order = 3
summary = "From a fresh install to https://blog.test with a green padlock, in about five minutes."
+++

# Your first site

This walks the whole path once: a PHP version, a web server, a project, a site, and a certificate
your browser accepts. It assumes MixEngine is installed — [Installing MixEngine](./install.md) if it
is not — and it assumes nothing else.

## 1. Check the daemon

```bash
mix status
```

The first `mix` command starts the daemon if it is not already running, so this is also how you find
out that the install worked. What comes back is the daemon's version, where its home directory is,
and what it is supervising, which at this point is nothing.

## 2. Install a PHP

MixEngine ships no runtimes: it downloads the versions you ask for, and only those. See what is on
offer, then take one:

```bash
mix runtime available --kind php
mix runtime install php 8.3.33
```

The version is exact rather than a range, deliberately — `8.3` would be asking MixEngine to choose
between versions none of which are on the machine yet. `mix runtime list` shows what you now have.

## 3. Install and create a web server

A **package** is a program MixEngine knows how to run; a **service** is a running instance of one
with its own configuration. Caddy is the default front end:

```bash
mix package available
mix package install caddy 2.10.0
mix service create caddy 2.10.0
mix service list
```

Versions move: take one from what `mix package available` actually lists rather than from this page.
Caddy runs once for the whole home rather than once per site, which is why its service id has no
`@name` on it — `mariadb@main` names an instance, `caddy` names the only one there is.

## 4. Register a project

A **project** is a directory MixEngine knows about. Go to the one you want to serve — make an empty
one if you are just trying this out — and register it:

```bash
mkdir -p ~/code/blog && cd ~/code/blog
echo '<?php phpinfo();' > index.php
mix project create
```

With no arguments it takes the current directory and names the project after it, so this one is
called `blog`.

## 5. Declare a site

```bash
mix site create --domain blog.test --kind php-fpm --https true
```

**This is the step that asks for permission**, and on a fresh machine it is the only one that does.
MixEngine needs the name `blog.test` to reach your own machine, and it needs your browser to trust
the certificate it is about to issue. It collects both — and the grant to listen on port 80 and 443,
where that is privileged — and raises **one** prompt for all of them. If you want to see exactly
what is being asked for before you agree, `mix elevation status` prints it; [What MixEngine asks
permission for](./permissions.md) explains each one.

Declining is a supported answer. The site is still created and still served over `http://`.

## 6. Open it

```bash
mix site list
```

Then open `https://blog.test` in a browser. You should get `phpinfo()` and a padlock with no warning
behind it. If the padlock is not green, ask the server rather than guessing:

```bash
mix cert status
```

That opens a real TLS connection to your own front end for every site and reports the certificate it
actually presented — which is the only thing a browser ever sees.

## 7. Add a database, if the project needs one

```bash
mix package install mariadb 11.4.4
mix service create mariadb@main 11.4.4
mix database create mariadb@main --name blog
```

The last command makes the database and an account that reaches it. **Nothing prints the password**:
it goes into your operating system's own credential store, and what is printed is the address it was
stored at. `mix database open` hands it to a desktop database client without it ever appearing in a
shell history or an argument list.

## What just happened

- MixEngine downloaded one PHP and one web server into its own directory. Nothing was installed
  system-wide, and no other version of anything on your machine was touched.
- It generated a certificate authority, asked once to have it trusted, and issued a 90-day
  certificate for `blog.test` — and it will reissue that certificate before it expires without being
  asked.
- It wrote the web server's configuration itself. That configuration is disposable: MixEngine
  regenerates it from what it knows, so there is no file for you to keep in sync.

## Where to go next

- [What MixEngine asks permission for](./permissions.md) — every prompt, and what it changes.
