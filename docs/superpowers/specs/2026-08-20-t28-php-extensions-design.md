# T28 — PHP extensions, and the ini set a runtime carries

*Design for roadmap task [T28](../../../.claude/roadmap/phase-2-runtimes.md). Written 2026-08-20,
before any code. What survives implementation goes into `phase-2-runtimes.md`; this file is the
argument, not the record.*

## What this task is, and what it is not

An installed PHP grows a configuration of its own: a generated `conf.d` under `etc/`, holding the
extensions that build is to load and the settings a development machine wants instead of PHP's
shipping defaults. Turning an extension on rewrites that set and reloads the pool T32 gave the
version; the same set reaches `php` on the command line, so a terminal and a browser agree about
what is loaded.

It is **not** a download. The one thing the task's own title promises and this design refuses is
"prebuilt extension artifacts" as a *fetchable* thing — see the next section, where they turn out to
be already inside the archive.

It is **not** per-project configuration. Two directories may already ask for different PHP
*versions*; they may not ask for different extensions of one version, and the reasoning is written
under "Why there is no profile" below rather than left as an omission.

It is not the pool — T32 owns the process, its workers and its socket — and it is not a site. Phase 4
renders the first per-site file, and per-site ini settings belong to it.

## Where the knowledge already is

`mixengine-packages` closed **P2** by deciding this project's extension set once, for all six cells
of every PHP version, in `tools/php_parity.py`. What that means here is that the interesting half of
"install an extension" has already happened at packing time:

- every cell of every version carries `igbinary`, `redis`, `mongodb` and `xdebug`, and from 8.1
  `yaml` and `zstd`, as loadable modules;
- forty-odd more are compiled in on Unix and are DLLs on Windows, because no Windows build exists
  with them static;
- the index says which is which per artifact — `extensions.static`, `extensions.shared`, and
  `extensions.enabled`, "which of `shared` an installer is expected to switch on, so that the cells
  of one version behave alike";
- and `extension_dir` says where the loadable ones are inside the archive.

So the daemon does not have to fetch, build or guess anything to offer `redis` on any version it
ships. `mix runtime ext enable redis` is a line in a generated file and a reload — which is exactly
why this design is about a file, a column and two consumers rather than about a downloader.

**One gap on this side:** `index::Artifact` in `mixengine-core` reads `static` and `shared` and drops
`enabled` on the floor. The index has carried it since P2. The first thing this task does is add the
field, because "what is on by default" is the whole difference between a Windows PHP that behaves
like its Unix twin and one that starts without `mbstring`.

## Part 1 — What is written down

Migration `0005_runtime_extensions.sql`, three additive columns on `runtime_installs`, each with a
default that makes an existing row mean something honest:

```sql
ALTER TABLE runtime_installs ADD COLUMN extension_dir          TEXT NOT NULL DEFAULT '';
ALTER TABLE runtime_installs ADD COLUMN extensions_json        TEXT NOT NULL DEFAULT '{}';
ALTER TABLE runtime_installs ADD COLUMN extension_choices_json TEXT NOT NULL DEFAULT '{}';
```

The first two are **the artifact's facts, copied down at install time**, which is `provides_json`'s
argument from [0002](../../../crates/mixengine-core/migrations/0002_runtime_provides.sql) applied a
second time: the index is a cache with a six-hour life and a network behind it, and whether `redis`
can be enabled for a PHP that is on this disk must not depend on either. `*_json` for the same reason
that migration gives — nothing queries into them; one runtime's whole map is read and looked up in
memory.

The third is **the user's, and it holds deviations rather than a set**:

```json
{"xdebug": true, "mongodb": false}
```

The effective set is `enabled ∪ {chosen on} − {chosen off}`, intersected with `shared`. Storing the
deviation rather than the resulting list is what makes a reinstall or a patch-version upgrade behave:
the new build's defaults arrive, and the two extensions the user deliberately touched stay touched. A
stored set would freeze 8.3.33's answer and carry it silently onto 8.3.34.

Two refusals fall out of the model, and both are typed rather than a rewritten file that quietly does
nothing:

| Asked | Answer |
| --- | --- |
| disable something in `static` | `Unsupported` — compiled into this build, naming that a different build would be needed |
| enable a name in neither list | `NotFound`, listing what this build does offer |

## Part 2 — What is generated

`etc/php/<version>/conf.d/`, through T30's `Document` and `install`: staged, diffed, renamed. The
diff is not decoration here — "identical is not a change" is what keeps a pool from reloading because
a daemon restarted.

**`etc/` and not `runtimes/php/<version>/conf.d/`**, which is what
[runtime-versions.md](../../../.claude/features/runtime-versions.md) says today and which this design
changes. Two reasons, one of them fatal: an install is a rename of a staging directory over the
destination, so a generated `conf.d` living inside it is destroyed by reinstalling the same version —
and the project's own rule is that generated configuration is disposable, lives under `etc/`, and is
rebuilt from SQLite. The cost is that `runtime.uninstall` must now remove a second directory, which
it already does for a pool's `etc/<service-id>/`.

The set is:

**`00-mixengine.ini`** — `extension_dir` first, as an **absolute** path, the install directory joined
with what the index published as a path relative to the archive root. Always written explicitly, even
where PHP would find its own, because upstream PHP for Windows bakes an absolute `C:\php\ext` into
the binary that would otherwise be consulted by accident on a machine where that path happens to
exist. Then the settings a development machine wants:

| Setting | Value | Why not PHP's default |
| --- | --- | --- |
| `memory_limit` | `512M` | 128M is a composer install that dies |
| `upload_max_filesize` | `128M` | 2M is the first wall every user hits |
| `post_max_size` | `128M` | must not be below the above, or the upload limit is a lie |
| `max_execution_time` | `120` | 30 is short for a dev request with a debugger attached. The CLI SAPI pins this to 0 whatever the ini says, so a long `artisan` command is unaffected |
| `display_errors` | `On` | a dev machine that hides errors is the wrong default |
| `error_reporting` | `E_ALL` | as above |
| `date.timezone` | `UTC` | PHP's implicit default, made explicit; reading the machine's zone is a platform call and belongs behind `mixengine-platform` if anybody wants it |
| `opcache.enable` | `1` | see below |
| `opcache.revalidate_freq` | `0` | an edited file must take effect on the next request, which is the whole difference between opcache in production and opcache on a laptop |

**`<NN>-<name>.ini`** — one per enabled shared extension, holding one line. `NN` exists because
`conf.d` is scanned in name order and order is load order:

| Prefix | What | Why |
| --- | --- | --- |
| `20` | `igbinary` | `redis` links against it when it can find it, and silently stores a serialisation nothing else reads when it cannot |
| `40` | `opcache` | an optimiser wants to be under whatever wraps it |
| `50` | everything else | |
| `90` | `xdebug` | wants to be the outermost, and is the one whose presence changes how everything else behaves |

**Two names are written `zend_extension=` rather than `extension=`: `opcache` and `xdebug`.** That is
a fact about PHP, not about the index, so it lives beside `runtimes::smoke_test` as a two-entry table
with the reason written down — the same place, and for the same reason, that "which flag prints a
version" lives.

The value is the bare name on both systems, which modern PHP resolves to `php_<name>.dll` on Windows
itself. **That is the one spelling in this design that is asserted rather than measured**, and it
fails quietly — a `zend_extension` PHP cannot load is a startup warning, not a refusal to start. The
CI suite compares loaded sets rather than exit codes precisely so this cannot pass by being ignored;
if the bare name turns out not to work there, the table gains a per-platform spelling and nothing
else changes.

**Opcache is where the two systems visibly differ**, and it is a good test of the model: it is
compiled in on the Unix cells and is a DLL on Windows, so Windows gets a `40-opcache.ini` naming a
zend extension and Unix gets no file at all — while `opcache.enable=1` is written on both, from
`00-mixengine.ini`, because a static opcache is present and idle until an ini says otherwise. One
setting, one meaning, two renderings, decided by what the index says about *this* artifact.

**There is no `php.ini`.** `PHP_INI_SCAN_DIR` alone was measured to work on all three systems during
T32, and a second file is a second place for the truth to live.

**Nothing is generated for Node, Python or Ruby.** The machinery keys off the artifact declaring an
`extension_dir`, which only PHP does. It is not a `match` on the kind.

## Part 3 — Who reads it, and why it has to be both

`PHP_INI_SCAN_DIR=<home>/etc/php/<version>/conf.d`, in two places:

1. **The pool** — one `.env(…)` in `php_fpm.rs`, on the shared part of the builder rather than in
   either arm, since Unix and Windows want it identically.
2. **The shim** — `main.rs::run` builds an environment holding `PATH` today; PHP gets this variable
   beside it. It is T25's own note coming due: *"No `PHPRC`, no `GEM_HOME` — the rest are files T28's
   `conf.d` model generates, and a variable pointing at a file nothing writes is worse than no
   variable."* Something writes them now.

**Doing only the first would be a bug, not a smaller feature.** `php -m` in a terminal and
`phpinfo()` in a browser would answer differently on every system, and on Windows the terminal answer
would be a PHP with no `curl`, no `mbstring` and no `intl`, because there those are shared modules
that only an ini switches on. The integration test therefore asserts both in one breath rather than
in two tests.

The shim already resolves the runtime it is about to become; what it lacks is the version threaded
out of that resolution to the environment it builds. The cost is a `join` and a `insert` — T29's
budget is on the resolution and this adds no I/O to it, but the benchmark job will say so rather than
be trusted.

## Part 4 — When it is rebuilt, and what reload means

Three call sites, which is `shims::refresh`'s policy rather than a new one: after `runtime.install`,
after every toggle, and on daemon start. `runtime.uninstall` removes the directory.

After a rebuild that changed something, the pool for *that version* is asked to reload:

| | Unix | Windows |
| --- | --- | --- |
| mechanism | `SIGUSR2`, which `ReloadBehaviour::Signal` already carries | none — `php-cgi.exe` reads its ini at startup and there is no signal to send |
| answer | `Reloaded` | `RestartRequired` |

**The daemon does not restart a pool nobody asked it to restart**; it reports. That is T32's own
policy for a changed override, and it is what `features/runtime-versions.md` asks for when it says
the GUI shows extensions as toggles "with the *requires restart* state made obvious". A pool that is
not running at all answers `PoolNotRunning`, which is neither a failure nor a reload.

**One assumption here must be proved and not believed:** that php-fpm's `SIGUSR2` actually picks up a
newly enabled extension. It should — a reload re-executes the master, which re-reads the ini set —
but "should" is how the last four platform findings in this workspace started. The CI suite asserts
it directly. If it turns out false, Unix answers `RestartRequired` too and the design is unchanged
apart from one arm of one match.

## Part 5 — The surface

Two methods, named the way `runtime.list_installed` and `runtime.set_default` already are:

```
runtime.list_extensions { kind, version }  -> [RuntimeExtension]
runtime.set_extension   { kind, version, name, enabled } -> ExtensionChange
```

```rust
pub struct RuntimeExtension {
    pub name: String,
    /// Whether it can be turned off at all.
    pub linkage: Linkage,          // Static | Shared
    pub enabled: bool,
    /// Why it is in that state: this build's default, or somebody's choice.
    pub source: ExtensionSource,   // BuildDefault | User
}
```

`source` is there for `runtime.resolve`'s reason: the question is asked precisely when the answer is
surprising, and "on because the build says so" and "on because you turned it on" are different
answers to "why is xdebug loaded".

`ExtensionChange` carries what happened to the pool — the table in Part 4 — so a client can print one
honest sentence rather than guess from the OS it is running on.

CLI: `mix runtime ext list|enable|disable [name] --php <version>`, defaulting to the kind's default
version. `runtime-versions.md` writes this as `mix php ext …`; that would open a per-language command
family for one language, and the deviation is a line changed in the spec rather than a new noun.

## Why there is no profile

The obvious next question — *two projects on 7.4, one wanting `mongodb` and a 64M upload, the other
wanting neither* — is answered "not here", and the reasoning is worth keeping because it will be
asked again.

PHP loads extensions at process startup and nowhere else. Per-project extensions therefore mean a
**second pool** for the same version, which means a second ini set, a second process idle on the
machine, and — the expensive part — a second axis in `resolve`: `php` in a directory would need to
answer *which profile* as well as which version, on the CLI and in the pool, or a terminal and a
browser stop agreeing again. That is a real feature with a real cost, and it lands on the two things
this phase just finished measuring and closing.

What it would buy is smaller than it looks. Settings are separable without any of that — php-fpm
takes `php_admin_value[…]` per pool, which is where a per-site upload limit belongs when Phase 4
renders per-site files. And the one extension whose cost is worth avoiding, `xdebug`, has
`xdebug.mode`, which is settable the same way: loaded everywhere, active where somebody asked.

So: version-wide here, per-site settings in Phase 4, and profiles as their own task if a real use
survives both.

## What proves it

Unit, in `mixengine-core`: the deviation merge against `enabled`; the two refusals; file ordering;
`zend_extension` for the two names that need it; opcache rendering differently on the two platforms
from the same state; and a second render writing nothing.

Integration, `#[ignore]`d in the shape T31–T33 established, against the real PHP that CI already
fetches for the T32 suite — one sentence, both consumers:

1. install a PHP; `runtime.list_extensions` reports `xdebug` present, shared, off, `BuildDefault`
2. the generated `conf.d` exists and the pool starts against it
3. `php -m` **through the shim** and a script executed **through the pool** agree on the loaded set
4. enable `xdebug`; both agree again, now including it, and the pool served a request across the
   reload rather than dying into it
5. disable it; both agree once more
6. disabling something static is refused, and enabling a name this build has never heard of is
   refused with a list
7. uninstalling the runtime takes `etc/php/<version>/` with it

Step 3 is the one that would be missing if this were written as a pool feature, and it is the one
that fails first on Windows.

## Deliberately not done

- **Extensions from anywhere but the archive** — no `mix runtime ext install`, no PECL build, no
  separate index package. `imagick` and `swoole` are the two names in `runtime-versions.md` that no
  cell carries; the task that adds them adds them in `mixengine-packages` first, and this design's
  only obligation is that the state model does not have to change when it happens: a name is a name,
  and where the file came from is not in the wire format.
- **User-editable ini settings** — `00-mixengine.ini` is MixEngine's opinion, not a store. The
  overrides machinery exists (`Setting`/`Preset`) and would fit, but nothing yet asks for it that
  Phase 4's per-site directives will not answer better.
- **Per-project or per-site extension sets** — above.
- **A restart the user did not ask for**, on either system.
- **`opcache.enable_cli`** stays at PHP's default of off. A CLI opcache helps a long-lived worker and
  nothing that a shim runs for a fraction of a second.
