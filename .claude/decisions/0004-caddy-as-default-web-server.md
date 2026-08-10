# 0004. Caddy as the default web server, Nginx optional

**Status**: Accepted
**Date**: 2026-08-10

## Context

We need one front-end web server owning ports 80/443, serving many local sites over HTTPS, with
config we generate. The realistic candidates are Nginx (what most users' production runs) and Caddy
(single static binary, TLS-first, clean admin API).

Constraints that matter here: we ship binaries for six OS/arch combinations, we reload config
constantly as sites come and go, and we terminate TLS for every site with our own CA.

## Decision

**Caddy is the default.** Nginx is a first-class alternative the user can switch to, with the same
feature set for site config generation; exactly one is active at a time.

## Consequences

**Easy**:

- Caddy is a single static Go binary with official builds for every target we care about — it removes
  a whole branch of the packaging problem ([../operations/runtime-packaging.md](../operations/runtime-packaging.md)).
- TLS is first-class; pointing it at our generated certs is trivial and reloads are graceful.
- The admin API gives us programmatic reloads and health data without spawning CLI processes.
- Config is simple enough that our generated files stay readable for users who want to look.

**Hard / accepted costs**:

- Most users' production is Nginx, so a Nginx-shaped mental model does not transfer directly. We
  mitigate by generating both, keeping feature parity, and making the switch one setting.
- Caddy's automatic HTTPS wants to manage certificates itself; we must explicitly disable ACME
  (`auto_https off` for our internal CA path, or `tls <cert> <key>` per site) or it will try to reach
  Let's Encrypt for a `.test` domain and fail noisily.
- Two config generators to maintain instead of one. Accepted: the alternative is losing users who
  need to reproduce a production Nginx rule locally.

**Enforcement**: any site feature must be expressible in both generators, or it is not merged. The
site-config test suite runs against both.

## Alternatives considered

- **Nginx only.** Matches production and users' knowledge, but we would have to build and relocate
  Nginx for macOS and Linux ourselves, reloads are clumsier, and TLS wiring is more manual.
- **Our own reverse proxy in Rust (hyper/pingora).** Total control, perfect integration with
  on-demand start. Rejected for v1: users need real `.htaccess`-adjacent behaviour, rewrite rules,
  FastCGI edge cases and production parity that a homegrown proxy would spend years catching up on.
  Revisit only for the on-demand activation gateway, which sits *in front of* the web server and is a
  much smaller problem.
- **Traefik.** Container-oriented; its strengths do not apply here.
