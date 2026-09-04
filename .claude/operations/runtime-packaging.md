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
          "url": "https://github.com/mixnz/mixengine-packages/releases/download/php-8.3.33/php-8.3.33-windows-x86_64.zip",
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
  [`mixengine-packages`](https://github.com/mixnz/mixengine-packages), one release per
  runtime version, which gives a permanent URL, a CDN and no bill.
- **`provides` is per artifact, not per package, because the SAPIs differ by OS.** A Windows PHP zip
  contains `php.exe`, `php-cgi.exe`, `php-win.exe` and `phpdbg.exe` — and no `php-fpm.exe`, which
  upstream PHP has never built for Windows. Anything reading this index to decide "can this runtime
  serve a site" has to read the artifact it is about to install, not the package.

## Where binaries come from, per runtime

| Runtime | Windows | macOS | Linux |
| --- | --- | --- | --- |
| PHP | official windows.php.net builds (NTS + TS, VS-version matched), back to 7.0 in `archives/` | **`static-php-cli`**, 8.1+; 7.0–8.0 **we build** from source — both arches either way | **`static-php-cli`**, 8.1+; 7.0–8.0 **we build** from source |
| Node.js | official nodejs.org zips, 16+ (x86_64) and 20+ (aarch64) — **answered at T27: borrow, and there was nothing to weigh** | official tarballs, 16+ on both architectures | official tarballs, 16+ on both architectures |
| Python | `python-build-standalone`, 3.10+ — **answered at T27: borrow, and the row was right** | ditto | ditto |
| Ruby | official **RubyInstaller** `.7z`, 3.2+ (x64) and 3.4+ (arm64) — **answered at T27: borrow** | **we build** from ruby-lang.org source, 3.2+ — answered at [T27b](../roadmap/phase-2-runtimes.md) | **we build**, 3.2+, inside AlmaLinux 8 — T27b |
| Caddy | official releases (single static binary) | ditto | ditto |
| Nginx | official Windows zip | **we build** | **we build** |
| MariaDB | official zip, x86_64 only; ARM64 **we build** — **answered at T33a, and this row was wrong** | **we build** from upstream source, both arches: there has never been a macOS build | official bintar on x86_64; on aarch64 upstream's own `arm64` **`.deb`**, rearranged into its bintar layout |
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
here maintains anything for it. That row was written on the strength of what the project says about
itself and was **checked at T27, where it held**. **Before a cell is accepted as "we build", it has
to be checked against an existing relocatable distribution**, and the answer recorded here so the
question is not reopened every phase.

Two of the answers below came out the opposite way to what this table assumed, in both directions:
Ruby-on-Windows said "we build" and is the easiest borrow in the whole table, and Ruby-on-Unix is
the only remaining cell where nothing borrowable exists at all.

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
  means only the second half, because every extension there is already a separate DLL. The API says
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

**What it measured out at.** All six branches build natively on all five targets — no cell was
dropped, so the "not available" mechanism above stayed theoretical. The floors are `glibc 2.28` on
both Linux architectures, `macos 14.0` on Apple Silicon and `macos 15.0` on Intel, the last being
the runner's own macOS and therefore the widest an artifact built from Homebrew can be. The
extension versions differ per branch, resolved from each package's declared PHP range and then
proven by loading it: `mongodb` runs 1.9.2 on 7.0 through 1.20.1 on 7.4 and 8.0, `xdebug` 2.9.0
through 3.5.3.

One finding is worth adding to the three above, because it is the sharpest evidence for the second
of them. **macOS needed its era built, not just borrowed.** ext/intl before 7.4 does not compile
against a current ICU for two independent reasons — ICU 61 stopped putting its classes in the global
namespace, and ICU 70 changed `CharacterIterator`'s virtuals from returning `UBool` to returning
`bool`. The first has a macro. The second does not: 7.4.33 carries `#if U_ICU_VERSION_MAJOR_NUM >= 70`
around the declaration and 7.3.33 was released before ICU 70 existed, so it never could. On Linux
none of this arises, because AlmaLinux 8 simply *has* ICU 60. On macOS there is no such distribution,
so ICU 67.1 is compiled from source for those branches. A dependency that only compiles against a
range of versions is the strongest argument there is for controlling the toolchain rather than
accepting whatever a package manager installed this month.

The failures behind all of this — including four rounds lost to extensions that were never loaded
because `HAVE_LIBDL` was missing, and two `configure` probes that answered "no" because a modern
compiler rejected code written for an old one — are written up in
[`docs/building-from-source.md`](https://github.com/mixnz/mixengine-packages/blob/master/docs/building-from-source.md)
in that repository. Little of it is about PHP, and the remaining **built** cells below will hit most
of it again.

### What the whole range measured out at, once both halves were published

The signed index now carries **eleven versions on five targets each** — 7.0.33, 7.1.33, 7.2.34,
7.3.33, 7.4.33, 8.0.30, 8.1.34, 8.2.33, 8.3.33, 8.4.24, 8.5.9 — with no cell missing. The floors are
`glibc 2.28` for the compiled range and `2.35` for the borrowed one, `macos 14.0`/`15.0` for the
compiled range and **`macos 12.0`** for the borrowed one on both architectures.

That difference is worth reading twice, because it is the reverse of what the table above assumes.
The *borrowed* artifacts reach further back: `static-php-cli` compiles every dependency itself under
`MACOSX_DEPLOYMENT_TARGET`, while our own recipe links Homebrew's libraries and therefore inherits
whatever macOS the runner was — which is why 7.x asks for 14.0 on Apple Silicon and 15.0 on Intel.
Lowering the compiled range to match would mean building all of its dependencies from source too,
i.e. rewriting the tool we borrowed. It is not worth it for six frozen branches, but the number in
`requires.macos` has to be the measured one either way, so a client refuses with a reason.

Two gaps in the borrowed half were only visible once the compiled half existed beside it. Neither
was a bug in this range; both were things nothing had been compared against before:

- **The 8.1+ artifacts had no macOS Intel build**, so an Intel Mac could install PHP 7.4 and not
  PHP 8.3 — a stranger matrix than the one this whole effort set out to fix. `static-php-cli`
  supports that target; nobody had asked it to.
- **Their macOS artifacts declared no floor at all**, which reads as "runs anywhere" and meant a
  machine on macOS 12 was told nothing either way. Measured, they run from 12.0.

**The third gap was in the proof itself, and it is the one worth remembering.** Both recipes wrote
`smoke.relocated: true` into every manifest, and the index generator refuses any artifact without
it — but the two were not proving the same thing. The compiled half re-resolved every dependency
from a tree it had moved elsewhere, *called* eight bundled libraries and compared their answers, and
loaded every shared extension through a generated ini. The borrowed half ran `php -v`, then stopped
at the first extension that loaded. Same field, same value, two different claims, and nothing in the
manifest distinguished them; the weaker one covered twenty-five of the fifty-five artifacts.

The four checks now live in one module both recipes call, and all fifty-five artifacts were rebuilt
against it. The general rule this leaves behind: **a check that two producers implement separately
will drift, and the drift is invisible exactly because they agree on the field name.** If a manifest
field is a claim, one piece of code has to own what the claim means.

Even so, be precise about what is claimed. Every artifact is proven to start, to find its own
libraries after being moved, and to load its extensions. **None of this runs PHP's own test suite,
serves a request through FPM, or connects `redis` to a Redis.** Loading is not working, and the
floors are read off the binaries rather than tried on a machine that old.

**And the borrowed half needs its era pinned too, for the same reason the compiled half did.**
Building 8.1 for Intel failed where the identical source and flags had succeeded for Apple Silicon,
which looks like an architecture problem and is not one: `AC_PROG_CC` probes for the newest C
standard the compiler accepts and writes it into `CC`, so the standard a build gets is decided by
the age of the runner's clang. The newer image chose C23, C23 removed old-style function
definitions, and that is how all of libbcmath is written in 8.1. The fix is to answer the probe
(`ac_cv_prog_cc_c23=no`) rather than to override the result afterwards, and it is set for every
branch — a version that builds today would otherwise break the first time a runner image ships a
newer compiler, and it would break in the same place. The general form of this is the finding the
compiled range already recorded: **a build is only reproducible if its toolchain is pinned, and
"whatever the runner has" is not pinned.**

### Node.js — answered at T27: **borrow, and the evaluation was over in a sentence**

Upstream publishes exactly what this repository wants: an archive that unpacks into a directory of
its own and runs from wherever it is put, which is what every Node version manager already relies
on. One recipe covers all six targets, because the whole per-target difference is a file name.
Four things came out of doing it that the sentence above does not contain.

- **The wrapper directory is the one thing rearranged.** Every upstream archive contains a single
  `node-v22.23.2-linux-x64/`, and the daemon unpacks straight into `runtimes/node/<version>/`, so
  keeping it would put the runtime one level below every path in `provides`. It is stripped by the
  recipe rather than by the installer, which stays ignorant of who packed what.
- **On Windows, `npm` is `npm.cmd`, and that turned out to cost nothing.** Upstream ships `npm` as a
  shell script for Git Bash and `npm.cmd` as the thing a Windows process can start; a batch file is
  not a PE image and `CreateProcess` refuses one outright. What makes the shim work anyway is that
  `std::process::Command` recognises the extension, goes through `cmd.exe`, returns the batch file's
  own exit code, and escapes arguments against `&`-style injection. That was **measured before the
  index named a `.cmd`**, and it is pinned by a test in `crates/mixengine-shim/tests/shim.rs`,
  because it is a property of the standard library rather than of any code here.
- **The range starts at 16, which is where every architecture has a native build.** Upstream's
  first `darwin-arm64` is 16.0.0 and its first `win-arm64` is 20.0.0. Below either, the recipe
  refuses rather than handing an Apple Silicon or ARM64 Windows machine an x86_64 build to emulate —
  the same rule the PHP range keeps. A target with no build is an **empty cell and not a failure**:
  the workflow skips that leg, so Node 18 still publishes its five artifacts.
- **The floors are upstream's, not ours**, and they are read off the binaries either way:
  `glibc 2.28` on Linux and `macos 11.0` on both macOS architectures. Nothing here chose them, which
  is precisely why they are measured rather than written down.

What the smoke test proves is four things, and `node --version` is the weakest of them: that
`process.execPath` is inside the moved tree, that `npm` reports the version packed beside it rather
than the runner's own Node, that `Intl` formats a German locale (a `small-icu` build fails nothing
while quietly formatting every locale as English), and that the bundled OpenSSL hashes a string. It
runs with a `PATH` holding the artifact's own directory and the system ones and nothing else, since
every runner in the matrix has a Node.js installed and a check that let it answer would prove
nothing at all.

### Python — answered at T27: **borrow, and the row this table already had was right**

`python-build-standalone` does exactly what the table has claimed since before there was a pipeline:
one archive per target that unpacks into a directory of its own, computes its `prefix` from `argv[0]`
at every start, and runs from wherever it is put. Six targets, one recipe, no evaluation to hold.
Eight things came out of doing it that the sentence does not contain.

- **The stripped variant, and the saving is not marginal.** Every `install_only` asset has an
  `install_only_stripped` counterpart with the debug symbols removed — 22.0 MB against 46.1 MB on
  Windows and 34.1 MB against 109.2 MB on Linux, measured on 3.12.14. macOS is identical either way,
  having no separate symbols to strip. Nothing a local development environment does needs a CPython
  built with symbols, and the storage screen shows the difference to the user.
- **`gnu` and never `musl`, and plain `x86_64` rather than `v2`/`v3`/`v4`.** The first is the same
  rule PHP's row already records — a static musl build cannot `dlopen`, and a Python that cannot load
  a compiled wheel is not one anybody can develop against. The second is a floor nobody asked for:
  the micro-architecture variants buy a benchmark and cost the machines that cannot run them.
- **The Unix console scripts are already relocatable, by a trick worth stealing.** `bin/pip3` is not
  a Python script with a `#!` naming an interpreter that will not be there; it is a `/bin/sh` script
  whose second line re-executes `$(dirname "$(realpath "$0")")/python3.12` on itself. The whole tree
  moves and every entry point keeps working, with nothing for the recipe to repair.
- **On Windows there is no `pip` to run at all — and that answers T27's open question about the
  post-install hook.** `Scripts/` contains one file called `.empty`; the pip *package* is in
  `Lib/site-packages` and only `python -m pip` reaches it.
  [runtime-versions.md](../features/runtime-versions.md) reserved a per-runtime post-install hook and
  named *ensure pip* as Python's, so this is exactly the cell it existed for. **It is not needed, and
  the reason generalises.** The obvious repair — run `ensurepip` and let it generate `pip.exe` —
  produces a launcher with *the absolute path of the interpreter that generated it* written inside,
  which is precisely what every artifact here is built to survive: generated at build time it names
  the runner, generated by a hook on the user's machine it names the install directory and breaks the
  first time `~/.mixengine` is moved or restored onto another machine. What the recipe writes instead
  is a two-line `Scripts/pip.cmd` that computes the interpreter from its own location, which cannot
  go stale and needs nothing to run it — `std::process::Command` starts a `.cmd` through `cmd.exe`,
  which T27's Node half already measured and pinned for `npm.cmd`. **The general rule: a path
  computed at run time beats a path written at install time, and an install-time hook is a build-time
  mistake postponed.**
- **3.10 has no Windows-on-ARM build**, which is the second real use of the empty-cell mechanism
  after Node 18's: the leg says so, exits 75 and skips its upload, and the other five targets are
  released. So the range is 3.10 upwards on five targets and 3.11 upwards on ARM64 Windows.
- **The range is upstream's rather than the policy's, and here that binds tighter.** MixEngine's
  generic rule is upstream support plus a year of grace, which would still admit Python 3.9 until
  October 2026 — but `python-build-standalone`'s current releases stop at 3.10, so 3.9 is not on
  offer. `tools/python.py --release <date>` pins an older dated release for anyone who has to
  reproduce a specific build, and that is the only way back to a line upstream has stopped
  publishing.
- **`ldd` on a plugin asks a question the loader never asks**, and that rejected two working
  archives. CPython's compiled modules carry no search path of their own: `_tkinter` needs
  `libtcl9.0.so`, the library is in the tree's own `lib/`, and nothing inside that file points at it.
  It loads anyway, because the *interpreter* that `dlopen`s it carries `DT_RPATH $ORIGIN/../lib` and
  glibc searches the whole chain that led to a load rather than only the object being resolved. So
  `relocate.verify` now measures the executables' own `DT_RPATH` and resolves through it — measured,
  so a tree that arranges itself some other way says so in its own binaries. **This is a property of
  borrowing rather than of Python**: everything `relocate.py` had bundled itself wore `$ORIGIN` on
  every file it touched, so the gap could not appear until an archive laid out by somebody else came
  through.
- **One module in the whole tree genuinely does reach outside it, and it is deleted rather than
  allowed.** Linux CPython before 3.13 links `_crypt` against `libcrypt.so.1` — libxcrypt, which is
  not the C runtime and is not covered by the glibc floor — and upstream ships no copy. Debian and
  Ubuntu install it as a base package; Fedora and RHEL offer it as `libxcrypt-compat` and do not. An
  artifact that works on some glibc distributions and not others is the one thing the relocation
  check exists to prevent, so the recipe removes the module and records it under `upstream.removed`,
  the mirror of the `upstream.added` that Windows's `pip.cmd` needed. The alternative — an allowance
  in `verify` — was refused on principle: **the rule that an artifact never reaches outside itself is
  either absolute or it is a habit, and the first exception is what teaches the second one to be
  written.** It costs nothing here: `crypt` was deprecated in 3.11 and removed outright by CPython in
  3.13, so this makes 3.10–3.12 behave like the versions after them on the one point where they would
  otherwise be non-portable, and the failure is now the same `ModuleNotFoundError` on every machine
  instead of a working import on Ubuntu and that error on Fedora.

What the smoke test proves is five things, and `python --version` is the weakest of them: that
`sys.executable` is inside the moved tree, that `sys.prefix` is too — the whole relocation claim for
an interpreter that recomputes it at every start — that thirteen standard-library modules import,
that OpenSSL and SQLite are *called* rather than reported, and that `pip` reports the version whose
`dist-info` is in the same archive. One check has no counterpart in the other recipes: the packed
interpreter has to **verify a real certificate chain over a real connection**, because a Python that
starts, imports `ssl` and then fails every `pip install` with a handshake error is the failure
furthest from its cause in this whole table.

That check was first written as "the default context has loaded at least one certificate authority",
and **counting answers a different question** — wrongly, on exactly the platform where the trust
store is a directory. A Unix `capath` is a hash directory OpenSSL reads one certificate at a time at
verification, so the same interpreter, verifying the same live chain, reports **409 authorities on
Windows, 128 on macOS and 0 on Linux** — where the store is `/etc/ssl/certs/` and nothing is loaded
until something needs verifying. Two good archives were refused before the check was made to do the
thing instead of measure a proxy for it. **A smoke test that counts is a smoke test that has not run the feature**,
which is the same finding as `php -v` passing with a broken `extension_dir`, arrived at from the
other side.

### Ruby — answered at T27: **Windows borrows; macOS and Linux are the last unborrowable cell**

The table said "we build" three times and one of those was wrong in the cheap direction.

**Windows: borrow RubyInstaller.** `rubyinstaller-<version>-<arch>.7z` is relocatable by
construction — upstream configures Ruby with `--enable-load-relative`, so the standard library, the
gem home *and the CA bundle* are computed from `ruby.exe`'s own location. All four were checked from
a directory the archive had been moved to. Four things came with it:

- **It publishes ARM64**, from the 3.4 line onwards, so a Windows-on-ARM machine gets a native Ruby.
  Against `windows.php.net`, which offers none in any branch, and Node.js, which starts at 20.
- **The CA bundle travels inside the archive.** `OpenSSL::X509::DEFAULT_CERT_FILE` resolves to
  `lib/ruby/<abi>/etc/ssl/cert.pem` in the moved tree, which is a stronger position than Python's,
  where the trust store is the machine's. The smoke test asserts both that it exists and that it is
  *inside the tree*, because a Ruby whose CA bundle points at the packaging machine works perfectly
  on the packaging machine.
- **Reading a 7-Zip archive and *decoding* it are two different capabilities**, and only the first
  is where it looks. `bsdtar` ships with Windows and understands the container, but decompresses
  LZMA only when its libarchive was built with liblzma — Windows 11's was, Windows Server 2022's was
  not. So the recipe worked on the development machine, worked on `windows-11-arm`, and failed on
  `windows-2022` with a bare exit code after a 20 MB download. It now tries 7-Zip first, falls back
  to bsdtar, requires neither, and quotes both refusals when neither works. The lesson is not about
  tar: **a tool being present is not the same as the feature being present, and the build of it that
  answers on a runner is not the one on the machine the recipe was written on.** Which is also why
  `tar` is still called by absolute path — a runner with Git ahead of it in `PATH` answers with a
  GNU tar that cannot read a 7z at all.
- **The extension of the wrapper scripts moves between lines** — `bin/bundle.bat` in the lines
  MixEngine offers, `bin/bundle.cmd` in the 2.x ones, `gem.cmd` throughout — so the recipe takes
  whichever exists rather than hard-coding one. Both run identically through `cmd.exe`.

What the Windows archive does **not** contain is the MSYS2 devkit, which is a 146 MB self-extracting
installer upstream ships separately. So a gem with a native extension and no precompiled Windows
binary cannot be built on this Ruby. Most of the ones people reach for — `nokogiri`, `sqlite3`,
`puma` — publish `x64-mingw-ucrt` gems and are unaffected; the rest need a devkit artifact, which is
a task nobody has opened and which should be opened the first time somebody hits it rather than
speculatively.

**macOS and Linux: nothing borrowable exists, and all three candidates were checked.**

- **Homebrew's `portable-ruby`** is relocatable by construction — Homebrew bootstraps itself with it
  — and publishes **exactly one version**, whichever one Homebrew currently needs (3.4.6, on four
  bottles). A version manager cannot be built on a source that offers one version, and its four
  bottles do not include Windows either.
- **`ruby/ruby-builder`**, whose artifacts are what `ruby/setup-ruby` installs, is refused by its own
  documentation: setup-ruby's README says the builds "embed the install path when built and cannot be
  moved around", and asks that the machine already have `libssl`, `libyaml` and `libgmp` of matching
  versions. They are also published per Ubuntu release rather than per architecture.
- **RVM's binary rubies** are prefix-bound to `/usr/local/rvm`, indexed per distribution release, and
  the newest directories on `rvm.io/binaries` are from 2020–2023.

So this is the last cell in the table where an owned pipeline is genuinely the only answer, and it
was [T27b](../roadmap/phase-2-runtimes.md) rather than part of T27 for the same reason T27a was
carved out of T20a: it costs a build pipeline, and a recipe written blind against two operating
systems this project has no machine for is a recipe that discovers itself in CI.

### Ruby — answered at T27b: **`tools/ruby_unix.py`, and the trust store is answered in OpenSSL**

`--enable-load-relative` did everything RubyInstaller's archive proves it does, on the first
attempt and on all four targets: the standard library, the architecture directory and the gem home
all resolve inside a tree that has been moved, and `rbinstall` writes `bin/gem` and its siblings as
a `/bin/sh` preamble that re-executes `$bindir/ruby -x` on itself rather than as a script with an
absolute `#!`. Nothing about the *language* needed discovering — the recipe checks the second half
rather than trusting it, and has yet to find one it had to fix.

**The CA store was the open question and the answer is one library down.** `OPENSSLDIR` is fixed
when OpenSSL is compiled, so `OpenSSL::X509::DEFAULT_CERT_FILE` is a statement about the *build*
machine — and shipping AlmaLinux's answer to a Debian user breaks every handshake, `gem install`
first, with an error that names nothing. Setting `SSL_CERT_FILE` from Ruby would cover only the
programs that read the environment and would leave the constant lying. So OpenSSL 3.5.7 is compiled
here with its four default-path functions taught to resolve against **the loaded `libcrypto`'s own
location** — `dladdr`, two directories up, `ssl/cert.pem` — falling back to the compiled-in path
when that file is absent, and leaving `SSL_CERT_FILE` to win over both so a corporate CA still
works. That is `--enable-load-relative` applied to a library instead of an interpreter, and it is
what RubyInstaller gets from MSYS2 on Windows. It is proven twice, because neither half implies the
other: the constant has to name a file **inside the moved tree**, and a real chain has to **verify
over the network** from there.

**What is published**: 3.2.11, 3.3.12, 3.4.10 and 4.0.6, from ruby-lang.org's own source, checked
against the SHA-256 in `cache.ruby-lang.org/pub/ruby/index.txt` — four Unix targets under each, so
twenty-two Ruby artifacts against the six the borrow alone gave. The only cells missing anywhere are
Windows on ARM for 3.2 and 3.3, which is upstream's: RubyInstaller's first ARM64 archive is in the
3.4 line. The signed index now carries twenty-five packages and one hundred and thirty-four
artifacts. Linux builds inside AlmaLinux 8 — here purely
for the glibc 2.28 floor, since nothing in Ruby 3.2+ wants an old toolchain and everything it *is*
version-sensitive about (OpenSSL, libyaml, libffi) is compiled by the recipe on every target alike.
**YJIT is on**, which is a decision rather than a default: `--enable-yjit` without a Rust compiler
*warns* and produces an interpreter with no JIT, so the recipe installs a toolchain where the image
has none and the smoke test asks the finished artifact whether `RubyVM::YJIT.enabled?` is true.
GNU readline is refused in favour of libedit — the reason is the licence, not the API, and since
3.3 `irb` uses the pure-Ruby `reline` regardless.

**The two Ruby recipes share what they claim, not how they work.** `ruby_smoke.py` is the claim, and
it exists because a daemon installing one of these cannot tell which recipe produced it — the
general form of that rule is in [`borrow.py`](https://github.com/mixnz/mixengine-packages/blob/master/tools/borrow.py)'s
own docstring and it is why the borrowed Windows archive now verifies a live certificate chain too.

**Four rounds of CI, and not one of them was Ruby.** Every failure was in the shared packing code or
in this repository's idea of what a check should ask, which is the strongest argument yet for the
"borrow before you build" rule cutting the other way: a *second* build pipeline is where the first
one's assumptions get audited. They are written up in
[`docs/building-from-source.md`](https://github.com/mixnz/mixengine-packages/blob/master/docs/building-from-source.md)
and the two that generalise beyond packaging are: **a file can carry the right magic number and
never be loaded by anything** (`debug.o`, a `.dSYM` companion — each refusing the very tool that
would have rewritten it), and **a check that asks the artifact a question must strip the machine's
environment, while a check that asks what the artifact can do on a user's machine must not** —
compiling a native gem with a cut-down `PATH` produced "you have to install development tools
first" on an image whose compiler was simply somewhere else.

One limitation is upstream's and is recorded rather than worked around: macOS `mkmf` writes
`-bundle_loader <bindir>/ruby` unquoted, so **a native gem cannot be compiled against a Ruby whose
path contains a space** — a user whose home directory has one included. Everything else about such
an installation works, and the recipe compiles its proof gem from a second moved copy without one.

### MariaDB: the first row in this table that was simply wrong (T33a)

This row said "official zip / official tarball / official tarball" and it had never been checked. The
catalogue was then read rather than assumed — `downloads.mariadb.org`'s REST API and
`archive.mariadb.org`, across every release from 10.2 to 13.1 — and it answers:

* **x86_64 Windows and x86_64 Linux**: a zip and a bintar, as the row said.
* **macOS**: nothing. Not on Apple Silicon, not on Intel, not in any release ever published. Homebrew
  and MacPorts have one; both are prefix-bound package-manager installs rather than relocatable
  artifacts, and both were refused for the reason `ruby_unix.py` refused `ruby-builder` and RVM.
* **ARM64, on either system**: no tarball and no zip. Linux ARM64 exists only as `.deb` packages in
  upstream's own apt repository, and Windows ARM64 does not exist at all.

So the cell that was expected to cost one evaluation costs three recipes, and the honest reading is
that **the borrow/build table is a set of hypotheses until each cell is opened.** Four of the six
MariaDB cells are not what this document claimed. PostgreSQL's "EDB binaries, which exist for all
three" is the same kind of sentence, written the same way, and should be treated as unverified until
somebody downloads one.

Three findings worth having beyond MariaDB itself:

**A borrowed artifact is not automatically a self-contained one.** Every borrow before this was
either a single static binary or a distribution built to be relocated. A MariaDB bintar is neither:
it names `libssl.so.3`, `libaio`, `libnuma` and `libsystemd` by soname with no search path, so
`relocate.bundle` — written for the *build* cells — now runs over a borrowed tree, and the manifest's
`upstream.added` says which libraries were put in. "Borrowed" describes where the payload came from,
not what it can do after it moves.

**A first-run job can be a different program under the same name.** `mariadb-install-db` is a shell
script on Unix and a C++ program on Windows, sharing almost none of their options — passing the Unix
spelling of "create root with a password rather than `unix_socket` authentication" makes the Windows
build exit 7 with `unknown variable`. That matters to T33 directly: the random root password in the
OS keyring is created by a different mechanism per platform, and the platform difference is not in
any documentation either half links to.

**Seven rounds of CI, and the smoke test found more than the build did.** Two cells went green on
the first run — Windows on x86_64, borrowed, and Windows on ARM64, *compiled from source*, which was
the cell expected to be hardest. Everything else failed four to six times, and almost none of it was
about compiling: the build succeeded and then the artifact could not be made to *be a database*.
What that cost is the argument for packing a service by running it rather than by checking that its
binary starts.

The findings that outlive MariaDB:

*A first-run script is a program with an environment, and it will find the machine's.* Without
`--no-defaults`, `mariadb-install-db` reads `/etc/mysql/my.cnf` — present on a GitHub Linux runner,
because those images ship a MySQL — and fails while blaming the data directory. **T33 needs this
more than CI does:** a user with their own MariaDB installed has a `my.cnf` naming a datadir, a
socket and a port, and an instance that silently inherited any of them would be writing into
somebody else's database.

*Two limits are the operating system's rather than the artifact's.* A Unix socket path is capped at
103 characters by `sockaddr_un`, which a temporary directory on a macOS runner nearly exhausts by
itself — and mariadbd reports it *after* InnoDB has started, so it reads like a storage failure.
And `chown` lives in `/usr/sbin` on macOS and `/usr/bin` on Linux, which is the difference between
a working bootstrap and `chown: command not found`.

*`$basedir` and `$datadir` are both unquoted in that script*, so neither may contain a space. It is
upstream's escaping rather than anything about relocation, and it will fail identically for a user
whose installation path has one — the same shape as the `mkmf -bundle_loader` limitation recorded
above for Ruby. T33 either keeps these paths space-free or bootstraps without the script.

*The script wants to give the data directory to a user called `mysql`* — the account a distribution
package would have created — and stops when it cannot. MixEngine runs services as whoever installed
them, so `--user` has to be stated.

*A borrowed plugin directory is not all shippable.* A bintar is built where everything is installed,
so it carries plugins linked against libraries a user will not have: `cracklib_password_check.so`
needs `libcrack`, `ha_oqgraph.so` needs `libJudy`. Each cost a round of CI until the recipe started
asking *every* plugin what it needs and dropping the ones that cannot resolve — which is the honest
answer, because such a plugin would fail `INSTALL SONAME` on a user's machine too.

**A distribution package is versioned and named in its own vocabulary, not upstream's.** Two traps,
both found by running the recipe against every series rather than the newest: these packages carry a
Debian *epoch* — `1:11.8.8+maria~ubu2204` — so a prefix match on the MariaDB version matches nothing
and reports a suite as empty when it is not; and on older lines the core packages carry the series in
their name (`mariadb-server-core-10.6`), which upstream dropped when it stopped co-installing two
servers. Anything reading a distribution's index has to normalise both.

**A service can write its diagnostics somewhere the supervisor is not looking.** Windows mariadbd
sends nothing to stdout and writes `<datadir>/<hostname>.err` instead, so capturing the child's
output yields an empty file and the appearance of a server with nothing to say. T33 should render
`log_error` explicitly rather than inheriting a default that differs per platform — which is what the
packaging smoke test now does.

#### What the whole catalogue then taught, which one series had not

Thirty cells — five series across six targets — found four more, and every one of them had been
green on 11.8 alone. A recipe proven on the newest version is a recipe proven on one column.

**A publisher's download URL need not be the publisher.** MariaDB's REST API states each file with
all four digests beside it, and its `file_download_url` is a *redirector* that answers 302 to
whichever third-party mirror it picks that minute. One served a 10.6.28 tarball 1,846 bytes short of
upstream's own copy. The checksum caught it — but which mirror answers changes per run, so the same
recipe passed and failed at random, and a green build was luck rather than evidence. The rule that
falls out is general: **a stated checksum and a redirected download are two different trust
decisions.** Take the digest from the catalogue and the bytes from a host the publisher runs.

**One release missing a checksum is not the API changing shape.** `mariadb-11.4.0-winx64.zip` is
listed with an empty checksum object while every 11.4.x after it states one, and treating that as a
format change killed the whole Windows cell over a version the recipe would never have chosen. Skip
the entry and name it; fail only when *nothing* left is verifiable.

**A vendor renames its own directories mid-catalogue, and the first-run script follows.** 11.8's
`mariadb-install-db` reads `$basedir/share/mariadb/mariadb_system_tables.sql`; 10.11's reads
`$basedir/share/mysql/mysql_system_tables.sql`. A recipe that normalises the layout has to satisfy
every spelling still in support, not the one it was written against.

**Licence text is payload, and nothing but diffing finished artifacts finds it missing.** Three
separate holes, none of which any smoke test could have shown: the `.deb` cell shipped GPL binaries
with no licence at all, because the recipe pruned `share/doc` along with the manual pages; the macOS
recipe collected none, because its walk up the Homebrew keg stopped one directory above the files and
reported success; and both Linux recipes bundle eighteen to twenty-two system libraries apiece and
never looked. `relocate.bundle` answers with *where each library came from* precisely so its caller
can do this. **Whatever a packager copies into an archive, it also redistributes.**

#### What the six cells of one version still do not share

Parity was measured rather than assumed — every plugin of 12.3 compared across all six cells — and
21 of 36 are in all six. T33 should read the rest as capability that varies by cell rather than by
version, because a blueprint naming one of them works on one machine and not another:

* **Correct and permanent.** `auth_named_pipe` and `authentication_windows_client` exist only where
  Windows IPC does; `disks` and `handlersocket` need Unix syscalls. Windows x86_64 bundles ten MSVC
  runtime DLLs and Windows ARM64 bundles none — the compiled cell links that runtime statically, so
  both stand alone by different means.
* **Upstream's own asymmetry, which borrowing inherits.** The x86_64 bintar carries `zstd` and
  `type_mysql_timestamp` that the `arm64` packages do not; the packages carry `auth_parsec`,
  `auth_mysql_sha2` and `sha256_password` that the bintar does not. Closing this means compiling
  Linux x86_64 instead of borrowing it, which is a pipeline maintained for every security release.
* **Ours, and closed.** macOS carried four of the five compression providers because
  `cmake/FindSnappy.cmake` is a bare `find_path` and Homebrew's prefix is on the default search path
  on Intel but not on Apple Silicon — so the same recipe produced a different artifact per
  architecture with no error on either. Named explicitly now, and missing is fatal.
* **10.6 has no compression providers at all**, on any cell: upstream did not build them until 10.11.
  The manifest's `variant` states which packages a cell was actually assembled from, so this is
  readable from the artifact rather than inferred from its size.

Still open — each is a cell nobody has checked yet, and MariaDB is the reason to read that literally:

| Cell | Look at first | What to check |
| --- | --- | --- |
| PostgreSQL | EDB binaries, which exist for all three — **claimed here, never verified** | whether the archive can be used without the installer, and whether ARM64 exists at all: it is where the MariaDB row turned out to be wrong |
| Redis, Windows | the hardest cell in the table — Redis has no upstream Windows support, Microsoft's fork is long dead, and WSL/Docker are excluded by [ADR 0003](../decisions/0003-no-container-isolation.md) | Memurai, or Valkey, or declaring Redis-on-Windows unsupported and saying so plainly rather than shipping a fork nobody maintains |
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
| `python.exe`, `python312.dll` (3.12.14) | python-build-standalone | **NotSigned** |
| `ruby.exe`, `x64-ucrt-ruby340.dll` (3.4.10) | RubyInstaller | **NotSigned** |

**Node is the only one**, and T27 measuring the other two borrowed runtimes only widened the gap:
the artifacts python.org itself signs are its installers, and what
`python-build-standalone` publishes is a rebuild of CPython that nobody signs. So of the four
borrowed runtimes, one is signed. So for PHP, nginx and Caddy, borrowing buys nothing at all against SAC: a
borrowed unsigned binary and one we compiled are the same unsigned binary to it, and the risk is
identical whichever side of the table the cell falls on. Borrowing still wins those cells — on the
maintenance cost that "borrow before you build" was actually about — but the signing argument must
not be used to decide any of them, because it is only true of Node.

The consequence is that whether a certificate repairs this is not a question about *our* build
pipeline at all: SAC would refuse the same artifacts even if MixEngine shipped none of its own.

**Answered by [T94](../roadmap/phase-9-ship.md) on 2026-09-04, and this table is what answered it.**
A certificate covers the four images this project builds and none of the ones above, and Smart App
Control judges each image load on its own file — so it repairs the first load and the product dies at
the second. The remedy that would work is the one this table exists to refuse: building all of them.
[ADR 0017](../decisions/0017-smart-app-control-is-an-unsupported-configuration.md) records the
decision — a machine with SAC enforcing is a configuration MixEngine does not support, names, and
does not work around. Nothing here changes: the signing argument still decides no cell, and now it
does not decide the product's distribution either.

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

The generic rule is **upstream-supported plus one year of EOL grace**, marked with a warning but
kept installable. Security releases reach the index promptly and raise an update badge
per runtime; only stable channels are offered, RC and beta behind a setting.

**PHP is deliberately outside that rule.** MixEngine offers **7.0 through the newest stable**, which
puts PHP 7.0 — EOL since January 2019 — seven years past the grace period. That is not an oversight.
The people who reach for a local development environment rather than a container are very often the
people maintaining something old, and a tool that cannot open their project is not a tool they can
use; ServBay and Laragon both carry these versions for the same reason. The grace rule stays for
every other runtime, where nobody has asked.

For the three borrowed runtimes the grace rule has turned out never to be the binding constraint —
**the publisher's own range is**, and it is the narrower one in all three cases:

| Runtime | Offered | What decides the floor |
| --- | --- | --- |
| Node.js | 16 – newest (20 – newest on ARM64 Windows) | the oldest line with a *native* build for every architecture |
| Python | 3.10 – newest (3.11 – newest on ARM64 Windows) | the oldest line `python-build-standalone` still publishes; 3.9 is inside the grace period and cannot be had |
| Ruby | 3.2 – newest (3.4 – newest on ARM64 Windows) | the grace rule, for once: RubyInstaller still publishes 3.1, which is past it |

What is offered is bounded by what can be produced, and for PHP that differs per OS:

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
  This holds across the whole range and not only the compiled part of it: the 8.1+ artifacts were
  arm64-only until T27a and gained their Intel build at the same time, because a range offered on
  both architectures at one end and one at the other is not a range anyone can plan against.
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

Artifacts are compressed (zstd where we control the build). The download size is available before
installing and the cost of each installed version afterwards, so any client can offer "remove
unused versions" — which respects project pins.

## Offline and mirrors

- `MIXENGINE_INDEX_URL` and `MIXENGINE_MIRROR_URL` let a team host their own mirror; the signature
  requirement stays.
- `mix runtime install --from ./php-8.3.12.zip --sha256 …` for air-gapped machines.
