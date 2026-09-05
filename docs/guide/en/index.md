+++
title = "MixEngine"
slug = "index"
order = 1
summary = "Run PHP, Node, Python and Ruby locally on any version, with real domains and HTTPS, without Docker."
+++

# MixEngine

MixEngine is a local web development environment. It runs several versions of PHP, Node.js, Python
and Ruby side by side and lets a directory choose which one it uses; it runs the web server,
databases and caches your projects need; and it gives every site a real name like
`https://blog.test` with a certificate your browser trusts. There is no Docker, no virtual machine
and no configuration file to write by hand — the generated configuration is MixEngine's business,
and nothing it runs stays behind as a root process.

It is one daemon and one command. `mixengined` owns everything MixEngine knows and supervises
everything it runs; `mix` is what you type. The few operations that need an administrator — a line
in the hosts file, a certificate in the system store, permission to listen on port 80 — are asked
for once, together, and by a helper that exits as soon as it is done.

## Start here

- [Installing MixEngine](./install.md) — the file for your system, and what it does and does not
  touch.
- [Your first site](./getting-started.md) — from a fresh install to a green padlock, in about five
  minutes.

## The handbook

- [Projects and sites](./projects-and-sites.md) — the two nouns, and how a checkout carries its own
  setup.
- [PHP, Node, Python and Ruby versions](./runtimes.md) — several at once, chosen per directory.
- [Servers, databases and caches](./services.md) — what your project runs against.
- [Names and the padlock](./domains-and-https.md) — why `blog.test` resolves, and what signed it.
- [Showing a site to your phone](./sharing.md) — one site on the local network, and back off again.
- [Blueprints](./blueprints.md) — write down what a project is made of, and set it up again
  elsewhere.
- [Extensions](./extensions.md) — phpMyAdmin, Mailpit and the rest, from a signed registry.
- [What MixEngine asks permission for](./permissions.md) — every prompt, and what each one changes.
- [Keeping MixEngine current](./updating.md) — updates are opt-in, signed, and rehearsed.
- [Removing MixEngine](./uninstalling.md) — and how to check nothing was left behind.
- [When something is wrong](./troubleshooting.md) — `mix doctor` first.
- [Command reference](./cli.md) — every command and flag, generated from the program itself.

## For programs

- [Reading this as a program](./for-agents.md) — every page as plain Markdown, a manifest, and the
  same bytes offline in `mix docs`.

Every page here is written in English and in Vietnamese, and every page is also published as plain
Markdown at a predictable address. The same pages are compiled into the `mix` binary itself, so
`mix docs` answers on a machine with no network and no running daemon — which is the state in which
somebody most often needs to read this.

MixEngine runs on Windows, macOS and Linux, and every page here applies to all three. Where a system
genuinely differs, the page says which one it is talking about.
