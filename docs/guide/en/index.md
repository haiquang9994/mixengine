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

## This handbook

Every page here is written in English and in Vietnamese, and every page is also published as plain
Markdown at a predictable address so that a program can read it. The same pages are compiled into
the `mix` binary itself, so `mix docs` answers on a machine with no network and no running daemon —
which is the state in which somebody most often needs to read this.

MixEngine runs on Windows, macOS and Linux, and every page here applies to all three. Where a system
genuinely differs, the page says which one it is talking about.
