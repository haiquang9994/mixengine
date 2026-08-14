# Runtime and package sourcing

MixEngine's hardest logistics problem is not code — it is having a trustworthy PHP 8.3 binary for six
OS/arch combinations, and keeping it current.

## The package index

A single signed `index.json`, published in its own repository and CDN-cached:

```json
{
  "schema": 1,
  "generated_at": "2026-08-10T00:00:00Z",
  "packages": [
    {
      "kind": "php", "version": "8.3.12", "channel": "stable",
      "artifacts": [
        { "os": "windows", "arch": "x86_64",
          "url": "https://github.com/haiquang9994/mixengine-packages/releases/download/php-8.3.33/php-8.3.33-windows-x86_64.zip",
          "sha256": "…", "size": 33871183,
          "provides": ["php", "php-cgi"] }
      ],
      "requires": { "vcredist": "2019" },
      "eol": "2027-12-31"
    }
  ]
}
```

- Signed with Ed25519 (minisign); the public key is compiled into the binary and rotated only via an
  app update.
- Every artifact is verified by SHA-256 *after* download; a mismatch deletes the file and fails loudly.
- The client caches the index for 6 hours and works offline against the cache.
- Old versions are never removed from the index — a blueprint pinning PHP 8.1.29 must keep working.
  This is why the index points at **our own mirror and never at an upstream URL**: upstreams prune.
  Artifacts are GitHub release assets of
  [`mixengine-packages`](https://github.com/haiquang9994/mixengine-packages), one release per
  runtime version, which gives a permanent URL, a CDN and no bill.
- **`provides` is per artifact, not per package, because the SAPIs differ by OS.** A Windows PHP zip
  contains `php.exe`, `php-cgi.exe`, `php-win.exe` and `phpdbg.exe` — and no `php-fpm.exe`, which
  upstream PHP has never built for Windows. Anything reading this index to decide "can this runtime
  serve a site" has to read the artifact it is about to install, not the package.

## Where binaries come from, per runtime

| Runtime | Windows | macOS | Linux |
| --- | --- | --- | --- |
| PHP | official windows.php.net builds (NTS + TS, VS-version matched), back to 7.0 in `archives/` | **`static-php-cli`**, 8.1+; 7.0–8.0 **we build** from source, both arches | **`static-php-cli`**, 8.1+; 7.0–8.0 **we build** from source |
| Node.js | official nodejs.org tarballs/zips — usable as-is on all three | ditto | ditto |
| Python | `python-build-standalone` (relocatable, all platforms) | ditto | ditto |
| Ruby | **we build** | **we build** | **we build** |
| Caddy | official releases (single static binary) | ditto | ditto |
| Nginx | official Windows zip | **we build** | **we build** |
| MariaDB | official zip | official tarball | official tarball |
| PostgreSQL | EDB binaries zip | **we build** or EDB | EDB / **we build** |
| Redis | Microsoft's fork is dead → **we build** with MSVC, or ship Valkey | official source build | official source build |
| Memcached | **we build** | source build | source build |

"We build" means a reproducible build pipeline in the packaging repo, producing **relocatable**
artifacts. Relocatability is the requirement that breaks most upstream builds: hardcoded prefixes in
`php-config`, `pg_config`, RPATHs, and `.dylib` install names must be patched at build time or fixed
at install time.

## Borrow before you build

Every cell reading "we build" is a build pipeline maintained for as long as MixEngine offers that
version — not once, but for every security release, on six targets. The Python row is the shape to
aim for instead: `python-build-standalone` already solved relocatability for that runtime, so nobody
here maintains anything for it. **Before a cell is accepted as "we build", it has to be checked
against an existing relocatable distribution**, and the answer recorded here so the question is not
reopened every phase.

### PHP, macOS + Linux — answered at T20a: **borrow `static-php-cli`**

MIT, actively released, 115 extensions in its `config/ext.json`, SAPIs `cli`/`fpm`/`micro`/`embed`,
and both extensions MixEngine was told it must have — `redis` and `mongodb` — are supported on Linux
and macOS. It builds **PHP 8.1 through 8.5 and nothing older**, which is the boundary everything
below is drawn around: not the boundary of what is offered, but of what is borrowed. Older branches
are compiled by T27a's own recipe instead.

Two conditions come with it, and neither is optional:

- **glibc, never musl — and it costs a floor.** A statically linked musl has no `dlopen`, so the
  tool's default Linux output cannot load a dynamic extension at all and refuses the build the
  moment one is asked for. The glibc and macOS outputs can, and `--build-shared=<ext>` produces the
  loadable `.so`. That is the only shape in which [T28](../roadmap/phase-2-runtimes.md)'s prebuilt
  extension artifacts exist on these two systems, so the musl mode — the tool's own headline
  feature — is the one MixEngine must not use.
  What is given up is that a musl build runs on any Linux and a glibc build does not: it needs one
  at least as new as the machine that produced it. So the requirement is **measured off the finished
  binary** — the highest `GLIBC_x.y` symbol version it imports — and carried in the index as
  `requires.glibc`, rather than assumed from whatever the runner happened to have. A client can then
  refuse the install and say why, instead of handing the user a loader error.
- **Compiled-in is not toggleable.** Whatever is linked into the binary is present forever. So the
  common set (including `redis`, `mongodb`, `opcache`) is compiled in and *always on*, and only the
  optional and heavy ones (`xdebug`) ship as separate `.so` artifacts. "Enable an extension" on
  macOS and Linux therefore means "install the artifact and write the `conf.d` line"; on Windows it
  means only the second half, because every extension there is already a separate DLL. The GUI says
  the same sentence on all three, and one of them has a download behind it.

`shivammathur/php-builder` was the other candidate and is the better *recipe* — MIT, PHP 5.6 to 8.6,
amd64 and arm64, redis and mongodb included. Its artifacts install under prefix `/usr`, so they
cannot be borrowed; the recipe can, which is where T27a started.

### PHP 7.0 – 8.0, macOS + Linux — answered at T27a: **we build, and macOS is in scope after all**

Nothing relocatable exists for this range at any prefix, so this is the one PHP cell that costs a
pipeline. What makes it affordable is that **the six branches are final**: 7.0.33, 7.1.33, 7.2.34,
7.3.33, 7.4.33 and 8.0.30 will never have another release, so the recipe runs a handful of times
and is then done — it is not the standing per-security-release commitment "we build" usually means.

Three findings came out of it, each of which contradicts something written above it:

- **macOS is not out of scope, and 7.x is native on Apple Silicon.** T20a excluded it on the
  grounds that upstream PHP had no Apple Silicon support before 8.0, which is true of upstream and
  not of reality: `shivammathur/homebrew-php` publishes `arm64_sonoma`/`sequoia`/`tahoe` bottles for
  php@7.0 through php@7.4, built with a small `acinclude.m4` patch. So the range is offered on both
  macOS architectures, each compiled on a runner of its own. **Nothing is cross-compiled and nothing
  runs under Rosetta**: a branch that will not build natively for an architecture is a cell the index
  does without, which is a truthful "not available" rather than an artifact that silently emulates.
- **Build on an old distribution, not a new one.** The Linux legs run inside AlmaLinux 8
  (`manylinux_2_28`). The glibc floor that falls out — 2.28, against the 2.35 the 8.1+ artifacts
  carry — is the smaller half of the reason. The larger one is that the image's OpenSSL is 1.1.1,
  its ICU is 60 and its autoconf is 2.69, which is the toolchain PHP 7 was written against; a current
  distribution is wrong on all three at once (ICU 68 removed the `TRUE`/`FALSE` macros `ext/intl`
  uses, autoconf 2.70 broke `phpize` for these branches, and PHP 7 predates OpenSSL 3).
- **Bundled, not static.** These builds link the distribution's or Homebrew's libraries, so every
  non-system library is copied into the archive's `lib/` and every reference rewritten to
  `$ORIGIN`/`@loader_path` — then verified from a directory the tree has never seen. On macOS each
  rewritten Mach-O is re-signed ad-hoc, without which arm64 refuses to load it at all. The floor this
  produces is measured off the finished archive, `requires.glibc` on Linux and `requires.macos` on
  macOS, rather than assumed from the runner.

The consequence for extensions is that this range inverts the 8.1+ arrangement: `redis`, `mongodb`,
`igbinary` and `xdebug` are **shared** here, because compiling an extension in needs `buildconf` and
these branches cannot be reconfigured with a current autoconf. The daemon already carries both
shapes, and shared is the one T28's enable/disable model wants anyway.

Still open — each is a cell nobody has checked yet:

| Cell | Look at first | What to check |
| --- | --- | --- |
| Ruby, all three | Homebrew's `portable-ruby` (Homebrew bootstraps itself with it, so it is relocatable by construction); RVM's binary rubies | licence, currency, and whether gems with native extensions build against it |
| PostgreSQL | EDB binaries, which exist for all three | whether the archive can be used without the installer |
| Redis, Windows | the hardest cell in the table — Redis has no upstream Windows support, Microsoft's fork is long dead, and WSL/Docker are excluded by [ADR 0003](../decisions/0003-no-container-isolation.md) | Memurai, or Valkey, or declaring Redis-on-Windows unsupported and saying so in the GUI rather than shipping a fork nobody maintains |
| Nginx, macOS + Linux | source build is genuinely small here | whether it is worth it before T37, which is the alternative front end and not the default |

The rule the table follows: **a borrowed artifact costs one evaluation, an owned one costs a
pipeline.** Where the answer is "we build" anyway, that is a finding worth writing down next to the
cell, not a default to fall back on.

### Signing was expected to weigh on the borrow side. Measured, it does not

The reasoning was that Smart App Control judges every image load, so an artifact its own publisher
signs might execute where one we built is refused — and MixEngine does not merely install these
binaries, it starts them. T20a therefore ran `Get-AuthenticodeSignature` over every upstream Windows
artifact this project intends to redistribute:

| Artifact | Publisher | Authenticode |
| --- | --- | --- |
| `php.exe`, `php-cgi.exe`, `php-win.exe`, `phpdbg.exe` (8.3.33 NTS x64) | windows.php.net | **NotSigned** |
| the DLLs shipped beside them (`brotlicommon`, `glib-2`, …) | windows.php.net | **NotSigned** |
| `nginx.exe` (1.30.4) | nginx.org | **NotSigned** |
| `caddy.exe` (2.11.4) | GitHub releases | **NotSigned** |
| `node.exe` (24.19.0 LTS) | nodejs.org | **Valid** — `CN=OpenJS Foundation` |

**Node is the only one.** So for PHP, nginx and Caddy, borrowing buys nothing at all against SAC: a
borrowed unsigned binary and one we compiled are the same unsigned binary to it, and the risk is
identical whichever side of the table the cell falls on. Borrowing still wins those cells — on the
maintenance cost that "borrow before you build" was actually about — but the signing argument must
not be used to decide any of them, because it is only true of Node.

The consequence is that whether a certificate repairs this is not a question about *our* build
pipeline at all: SAC would refuse the same artifacts even if MixEngine shipped none of its own.
[T41a](../roadmap/phase-4-sites-and-elevation.md) still owns the question; what changed is that its
answer now governs the whole table rather than only the "we build" half of it.

## Relocation rules

- No absolute path baked into a binary or config. Paths come from arguments and generated config.
- macOS: `install_name_tool`/`@loader_path` for every bundled dylib; sign the result (Ventura+
  rejects modified signed binaries).
- Linux: `$ORIGIN` RPATH; bundle only what the distro will not reliably have.
- Windows: ship the required VC++ runtime check as a `requires` entry; prompt the user rather than
  installing it silently.
- After install, a **smoke test** runs the binary (`php -v`, `mariadbd --version`) and fails the
  install if it does not execute. Never register an artifact that has not been proven to run.

**"No absolute path baked in" is a rule upstream PHP already breaks, and harmlessly.** T20a extracted
the official Windows zip, moved it to an unrelated directory whose name contains a space, and ran
`php -v` and `php-cgi -v` from there. Both work, and `php --ini` reports *no* configuration path at
all — the Windows build looks beside its own executable, so `PHPRC` and `-c` are enough to place a
generated `php.ini`. But `extension_dir` **is** compiled in, as `C:\php\ext`, a directory that does
not exist on the machine that ran the test. Nothing failed, because the 27 extensions in a default
build are static; the moment a dynamic one is wanted, `extension_dir` has to be overridden. So the
rule for a borrowed artifact is not "no baked path" — we do not control that — it is:

> **Every path the generated config can set, it sets, whether or not the binary bakes one.** A baked
> path that is never consulted is not a bug; one consulted by accident on a machine where it happens
> to exist is, and it would be undebuggable.

That is also why the smoke test cannot be `php -v` alone: `php -v` passes with a wrong
`extension_dir`. It has to load something.

### Every value in a generated `php.ini` is quoted, or Windows breaks on some machines and not others

T20a's smoke test passed on a developer machine and failed on the Windows runner, with
`PHP: syntax error, unexpected '~'`. **PHP's ini parser rejects `~` in an unquoted value**, and
Windows puts one in every 8.3 short path — `RUNNER~1` on the runner, `PROGRA~1`, and the profile
directory of any user whose name is not plain ASCII, which on this project's own machine it is not.

What makes it worth a heading rather than a footnote is the failure mode. The parse error kills the
file from that line onwards, so **every extension silently stops loading** — and `php -v` keeps
answering perfectly, because the built-ins are static and never consult `extension_dir`. A user
whose Windows username has a diacritic would get a PHP that starts, reports the right version, and
cannot open a database, on a machine where the developer's identical config works.

```ini
extension_dir="C:\Users\NGUYỄ~1\.mixengine\runtimes\php\8.3.33\ext"   ; loads
extension_dir=C:\Users\NGUYỄ~1\.mixengine\runtimes\php\8.3.33\ext     ; syntax error, then nothing
```

So the rule is not "quote paths that need it" — nothing can tell which those are, since Windows
generates the short name behind the long one. **Every value the generator writes is quoted.**

## Version policy

The generic rule is **upstream-supported plus one year of EOL grace**, marked in the GUI with a
warning but kept installable. Security releases reach the index promptly and raise an update badge
per runtime; only stable channels are offered, RC and beta behind a setting.

**PHP is deliberately outside that rule.** MixEngine offers **7.0 through the newest stable**, which
puts PHP 7.0 — EOL since January 2019 — seven years past the grace period. That is not an oversight.
The people who reach for a local development environment rather than a container are very often the
people maintaining something old, and a tool that cannot open their project is not a tool they can
use; ServBay and Laragon both carry these versions for the same reason. The grace rule stays for
every other runtime, where nobody has asked.

What is offered is bounded by what can be produced, and that differs per OS:

| OS / arch | PHP range | Source |
| --- | --- | --- |
| Windows x86_64 | **7.0 – newest** | official builds; `releases/archives/` keeps every branch back to 7.0 |
| macOS aarch64, x86_64 | **7.0 – newest** | `static-php-cli` from 8.1; 7.0–8.0 compiled from source |
| Linux x86_64, aarch64 | **7.0 – newest** | `static-php-cli` from 8.1; 7.0–8.0 compiled from source in AlmaLinux 8 |

Three consequences worth stating, because each of them is a thing a client will otherwise discover
at install time:

- **macOS is offered on both architectures, and every artifact is native.** The rule is not "arm64
  only" but "never emulated": each architecture is compiled on a runner of its own, so an Apple
  Silicon machine is never handed an x86_64 build to run under Rosetta, and an architecture with no
  native build for a branch simply has no artifact for it. The cell the original table feared most —
  `install_name_tool` over every bundled dylib, followed by a re-sign — is entered after all for
  7.0–8.0, and it is survivable because the re-sign is ad-hoc and done at build time, not at install
  time. Each artifact carries the macOS floor its bundled libraries actually impose, as
  `requires.macos`, so a machine too old to load it is told rather than shown a loader error.
- **There is no ARM64 Windows PHP, in any branch.** `releases.json` offers `x64` and `x86` and
  nothing else for 8.3, 8.4 and 8.5 alike. MixEngine itself targets `aarch64-pc-windows-msvc`, so a
  Windows-on-ARM machine runs the daemon natively and PHP under emulation. That is a fact about
  upstream, not something a build pipeline of ours could fix.
- **The VC++ toolset moves mid-range**, so `requires.vcredist` is per branch and not per runtime:
  7.0–7.1 are VC14, 7.2–7.3 VC15, 7.4–8.3 VS16, and 8.4 onwards VS17. An index entry that named one
  redistributable for "PHP" would be wrong for most of the table.

**Extensions follow the version, not the runtime.** `redis` and `mongodb` are required across the
whole range. On Windows they come from the official PECL DLL archive at
`downloads.php.net/~windows/pecl/releases/`, which is indexed by extension version and carries a
separate DLL per PHP branch × NTS/TS × VC toolset — 68 published `redis` versions at the time of
writing, enough to pair every branch MixEngine offers. On macOS and Linux they are compiled into the
`static-php-cli` binary from 8.1 up, and shipped as loadable `.so` files beside it on 7.0–8.0, where
compiling an extension in would mean regenerating a 2016 build system. Same extension, same name in
`mixengine.toml`, three entirely different delivery mechanisms underneath — which is exactly the sort
of thing the daemon exists to hide.

## Size

Artifacts are compressed (zstd where we control the build). The GUI always shows the download size
before installing, and the storage screen shows what each installed version costs, with a one-click
"remove unused versions" that respects project pins.

## Offline and mirrors

- `MIXENGINE_INDEX_URL` and `MIXENGINE_MIRROR_URL` let a team host their own mirror; the signature
  requirement stays.
- `mix runtime install --from ./php-8.3.12.zip --sha256 …` for air-gapped machines.
