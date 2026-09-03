# T82a — phpMyAdmin signs itself in (design)

Roadmap task **T82a**, phase 8. T81b gave a `web-app` extension a site, T82 gave it a generated
`config.inc.php` with the server, the port and the account already in it — and stopped one field
short of a login, deliberately, because the only place left to put a password was a process shared
with every project on the machine.

This is that field. **A php-fpm pool of the extension's own**, carrying an `EnvValue::Keyring` the
supervisor resolves at spawn, so the database superuser's password is in one process's environment,
on no disk, and in no other project's.

## Goal

`mix extension install phpmyadmin` on a machine with a managed MariaDB opens
`https://phpmyadmin.mixengine.test` at the database listing rather than at a login form, and nothing
anywhere on the filesystem holds the password. `features/extensions.md`'s second acceptance
criterion is whole again.

## Measured, not assumed

Every line below was read off this workspace's own code or off the programs it runs, and each is
what a decision rests on.

- **The supervisor already resolves a keyring value at spawn.** `EnvValue::Keyring { service, key }`
  is in `mixengine-proto`, `Runner::spawn_environment`
  (`crates/mixengine-daemon/src/services/runner.rs:2127`) reads it through
  `Host::keyring().secret(...)` on a blocking task, and `Surroundings`' hand-written `Debug`
  (`crates/mixengine-supervisor/src/command.rs`) prints the names without the values. Nothing new is
  needed to *carry* a credential; what is missing is a process to carry it in.
- **A child's environment is the spec's over a short per-OS floor, and nothing else.**
  `platform::process::whole_environment` calls `env_clear()` and then applies `sys::INHERITED_ENV` —
  `PATH`, `HOME` and the locale on Unix, the eight or so names Windows needs to load a system DLL —
  before the spec's own entries. A php-fpm master started by this daemon does **not** hold the
  daemon's environment.
- **php-fpm workers get a cleared environment by default.** `clear_env` defaults to `yes`, and the
  only way to hand a worker something the master holds without writing it into the pool file is
  `clear_env = no`. It is a pool directive from PHP 5.4.27, so every PHP this repository offers
  (7.0 upwards) accepts it — which matters, because `php-fpm --test` refuses a whole file over one
  directive it does not know, as `pm.status_listen` cost T72a.
- **`env[NAME] = value` in a pool file writes a literal.** php-fpm performs no expansion of the
  master's environment there, so that route puts the password on disk and is not available.
- **`getenv()` reads the real environment; `$_ENV` does not always exist.** `variables_order` decides
  whether `$_ENV` is populated and the default is `GPCS`. A manifest reads the credential with
  `getenv()`.
- **Windows needs no pool file for this.** The pool there is `php-cgi.exe -b <addr>` with
  `PHP_FCGI_CHILDREN` in the environment (`recipes::php_fpm`'s module note); its children inherit
  the process environment, so the same spec entry arrives with no directive at all.
- **The pool's socket is already keyed by something unique per row.**
  `php_fpm::socket_path` builds `run/php-fpm-{version}.sock`, and `pools::ensure` writes
  `instance_name = version` for every pool it makes — so the id's *instance* and the version are the
  same string on every existing home, and spelling the socket from the instance changes no byte
  anywhere.
- **`services` already takes a second pool on one runtime.** `UNIQUE (runtime_install_id,
  instance_name)` (0016) is per instance, and `runtime_install_id` is `ON DELETE RESTRICT`, so a
  second pool on one PHP is a row the schema accepts and protects.
- **`pools::of` cannot survive that**, and it is the reason it changes here: it is
  `fetch_optional` over `WHERE r.kind = ? AND r.version = ?` and has three callers —
  `runtime.uninstall`, the php-extension toggle's reload, and T81b's site resolution — each of which
  wants *every* pool of a runtime rather than one arbitrary row.
- **`pools::ensure` would skip a repair.** Its predicate is `NOT EXISTS (… WHERE
  s.runtime_install_id = r.id)`, so an extension's pool on a PHP whose shared pool somebody deleted
  would stop the shared one from ever being made again.
- **An extension's site binds loopback and cannot be shared.** Caddy's `site.caddy` writes `bind`
  per site and nginx's `site.conf` writes `listen 127.0.0.1:…`; the second listener is T74's and
  `Sites::share` runs `editable`, which refuses a site an extension owns. `network = "lan"` is
  refused at parse for the `web-app` kind (T80). So the front end, not a policy, is what keeps this
  interface on this machine.
- **`start_plan` walks dependencies and so does on-demand activation.**
  `ServiceGraph::start_plan` pulls in everything the roots transitively depend on, and
  `services::activate::ensure_running` uses it — so an edge from a pool to a database is honoured
  both by `mix service start` and by a request arriving at an idle-stopped pool.
- **A database's credential exists only after its first start.** `services::first_run` provisions it
  and `services::databases::write` puts it at `<service-id>/<administrator>` — `mariadb@main/root`,
  `postgres@main/postgres`. A pool started before that would find no entry.
- **A missing keyring entry fails a start outright.** `Runner::environment` stops at the first entry
  that will not resolve, and the service does not spawn. That is right for MariaDB and is the hazard
  this task has to place somewhere survivable.

## Scope

**In:** `mixengine-core` — `extensions::manifest` grows `[web-app.database].signs_in` and one
cross-table check; `extensions::render` grows the `{db_password_env}` placeholder; `extensions::pools`
is new and owns the pool an extension's site runs on (its id, its creation, its removal, its repair,
and the credential it carries); `extensions::install` names and creates it; `extensions::uninstall`
removes it; `services::pools::of` becomes plural and `ensure`'s predicate is narrowed; `sites` refuses
a pool that belongs to an extension to anybody else's site; `generate::recipe::Context` carries a
credential and `generate` resolves it once per walk; `recipes::php_fpm` puts it in the spec, renders
`clear_env = no` for the pool that has one, spells its socket from the instance, and declares the
edge to the database.

`mixengine-proto` — `PlannedSite` says which account the site would be signed in as, and
`ExtensionRemoval` names the pool that went with the extension.

`mixengine-daemon` — the boot repair, the stop before an uninstall, and the two callers of the
plural `pools`.

`mixengine-cli` — what `plan` and `uninstall` print.

`mixengine-testkit` — the phpMyAdmin fixture becomes the manifest that shipped, again.

Documentation: [features/extensions.md](../../../.claude/features/extensions.md),
[features/client-surface.md](../../../.claude/features/client-surface.md), the roadmap.

**In, in `mixnz/mixengine-packages`:** `data/extensions/phpmyadmin.toml` declares `signs_in` and its
generated configuration switches from `auth_type = 'cookie'` to `'config'` with the password read
from the environment.

**Out:**

- **Adminer signing itself in.** D9.
- **A second server in one generated configuration.** T82's D5, unchanged.
- **Rotating a database password.** Nothing in this build rotates one; when something does, it owes
  this pool a restart, and D8 says where that is written down.
- **A credential for anything but the database an extension already administers.** There is one
  `signs_in`, it means the server `[web-app.database]` resolved, and there is no vocabulary for a
  second.
- **MixDB.** T83 and T84.

## Decisions

### D1 — Every `web-app` gets a pool of its own, and not only the ones that ask for a password

The obvious reading of the roadmap line is *give phpMyAdmin a pool because it needs somewhere to put
a password*. That would make a manifest field decide whether an extension gets a process of its own,
and three things are wrong with it.

**A registry update would silently restructure an install.** An extension that grew `signs_in` in a
later release would, on the next install, stop sharing a pool and start owning one — a change to
what runs on the machine, arriving from a document rather than from a decision.

**The isolation is the kind's, not the field's.** `features/extensions.md` says a `web-app` is an
administrative interface onto the machine's own databases, never exposed to the network, on a
runtime *we* pick rather than the user's. A process shared with every project site contradicts the
last of those already: `pm.max_children` is five, and an interface that is walking a large schema
is competing for workers with the sites somebody is actually developing. That is true whether or not
a password is involved.

**And two shapes is two shapes to test.** Install, uninstall, `runtime.uninstall`'s refusal,
`service.delete`'s refusal, the boot repair and `mix site show` would each have an extension-with-a-
pool case and an extension-without-one case, for a saving of one process that is stopped most of the
time.

So: **a `web-app` extension's site runs on `php-fpm@<extension-id>`, always.** What `signs_in`
decides is only whether that pool's environment carries a credential.

**What it costs, said plainly.** One php-fpm master per installed `web-app`, five workers each. It is
bounded by machinery that already exists: `Recipe::idle_default` stops a pool after half an hour of
nobody using it (T70's D9), and the activator started by T70 brings it back on the next request. A
machine with phpMyAdmin and Adminer installed and neither open is running neither.

**The id is derived and the row is confirmed, never formatted and trusted.**
`extensions::pools::id` composes `php-fpm@<id>` — the same rule a `service` extension's process
already follows, where the process *is* `<id>` — and `extensions::pools::of` reads the row back
before anything names it. T81b paid for that distinction once already; this is the same rule at the
next site.

**An extension id longer than 56 characters has no pool**, because `ServiceId::MAX_LEN` is 64 and
`php-fpm@` is eight of them. Refused at `site_for`, before anything is fetched, naming the id and the
limit — the shape every other pre-download refusal in this path takes.

### D2 — The name of the variable is not the manifest's to write

The first draft of this design had `[web-app.database].password_env = "MIXENGINE_DB_PASSWORD"`, and
then had to grow a shape check, a refusal for the names the pool itself sets (`PHP_INI_SCAN_DIR`,
`PHP_FCGI_CHILDREN`, `PHP_FCGI_MAX_REQUESTS`), and a policy list of names that would break a program
outright — `PATH`, `HOME`, `LD_PRELOAD`, `DYLD_INSERT_LIBRARIES`, `PHPRC`. A per-OS list, in a crate
that may not ask what OS it is on, that ages with every loader.

**T80's D2 already answered this**, for addresses: *"there is no check to forget, because there is
nothing an extension can write that would need one."* `{listen}` renders from
`permissions.network` and a manifest cannot spell a host at all. The same move works here.

```toml
[web-app.database]
engines = ["mariadb", "mysql"]
# Hand this application the server's superuser password, in its own pool's environment.
signs_in = true
```

A boolean. The variable is `MIXENGINE_DB_PASSWORD`, a constant in `extensions::pools`, and the
manifest reaches it through a placeholder rather than by spelling it:

```php
$cfg['Servers'][$i]['password'] = (string) getenv('{db_password_env}');
```

So the name exists in one place in one repository, an author cannot collide with anything, and the
day it changes the manifests in the other repository keep working.

**`{db_password_env}` is refused when `signs_in` is false**, naming both fields — the shape every
other unanswerable placeholder takes. And **`signs_in = true` is refused when the configuration text
does not use it**: a manifest that asks for a database superuser's password and then never reads it
is asking a person to agree to something for nothing, and it is the one cross-table check this
format has. Both are `Error::ExtensionField`, at parse, against the line somebody wrote.

**The placeholder renders a variable name and never a password.** That is T82's D6 unchanged: a
generated file does not carry a credential, and this one carries the *address* of one in the same
sense `EnvValue::Keyring` does.

### D3 — `clear_env = no`, and what that actually exposes

php-fpm hands its workers a cleared environment unless told otherwise, and the alternative to telling
it otherwise is writing the password into the pool file. So the pool that carries a credential
renders

```
clear_env = no
```

and every other pool in the home renders nothing, because the directive is tied to the credential
rather than to the pool being an extension's.

**What a worker then sees is short and knowable**, which is the second half of why this is safe: the
master's own environment is `env_clear()` plus `sys::INHERITED_ENV` plus the spec's entries —
`PATH`, `HOME`, the locale, `PHP_INI_SCAN_DIR`, and `MIXENGINE_DB_PASSWORD`. Not the daemon's
environment, and not the session's beyond that floor. The daemon's own secrets are not in its
environment at all; they are in the keyring, read per spawn.

**On Windows the directive is absent and the mechanism is the same**: `php-cgi.exe`'s children
inherit the process environment, so the spec entry is the whole of it. One vocabulary, two
mechanisms — the split this recipe was built around.

**The pool file is rendered on every system**, Windows included, exactly as it already is (the
recipe's `files()` note): a home whose files differ per OS is a home two colleagues cannot compare.

### D4 — The credential is resolved once per walk, by the generator, out of the link that already exists

A recipe is synchronous and is handed a `Context`. What this one needs — which extension owns this
pool, which database its site is linked to, what that recipe calls its administrator — is three
tables. So the *generator* resolves it, once, in `declarations_with`, beside the extension manifests
and the front-end fragments it already reads there for the same reason.

```rust
/// A credential this service's processes are handed at spawn — roadmap task T82a.
pub struct Credential {
    /// The variable it arrives in.
    pub env: String,
    /// The keyring service name — `mixengine_platform::KEYRING_SERVICE`.
    pub keyring_service: String,
    /// The entry within it: `<service-id>/<administrator>`.
    pub keyring_key: String,
    /// The service that credential opens, for the edge and for the log line.
    pub database: ServiceId,
}
```

`Context::credential()` answers it and `PhpFpm::spec` puts it on the builder. **No value, ever** —
this is an address, so a `Context` may be `Debug`-printed as it always could, and `mixengine-core`
still never reaches a credential store (`generate::databases`' D1).

**Off the link and never off `engines`.** The resolution walks the extension-owned `sites` rows, and
for each takes the `site_service_links` service T82 froze at install. Re-resolving the preference
order at render time would silently re-point an application at a different server than the one it
was installed against — the pool is frozen for the same reason (T81b's D5), and the configuration
file already reads only the link.

**It fails closed.** A pool named by more than one site, or by a site that is not the extension's,
resolves to no credential and says so in `daemon.log`. That state is unreachable through D5's
refusal; the check costs four lines and is the difference between a bug and a disclosure.

### D5 — A pool that belongs to an extension may not be named by another site

This is the hole D1 opens and it has to be closed in the same task. `mix site update <project site>
--pool php-fpm@phpmyadmin` would put a project's PHP in the process holding the superuser password.

The refusal lives in `mixengine-core::sites::create` and `sites::update`, not in the daemon:
`blueprint.apply` and `domain.add` reach `update` without going through a CLI, and a refusal they
could cross is no refusal — which is T81b's D6 arriving at a second field. One query,
`SELECT 1 FROM extensions WHERE id = ?` against the pool id's instance half, and a site whose owner
is not that extension is refused by name.

**It cannot be crossed by ordering, either.** The pool row exists only from the moment the extension
is installed, and a site must name a service that exists; when the extension is uninstalled the row
goes and `sites.php_service_id` is `ON DELETE SET NULL`, so no dangling name survives to be adopted
by a reinstall.

### D6 — `pools::of` becomes plural, because a runtime now has more than one pool

`services::pools::of` answers one `ServiceId` for a `(kind, version)` pair. With a second pool on one
runtime install it answers an arbitrary one, and all three of its callers want all of them:

- `runtime.uninstall` refuses to remove a PHP whose pool is running and deletes a stopped one. Seeing
  one of two would delete the shared pool, leave the extension's, and then fail on
  `runtime_installs`' `ON DELETE RESTRICT` with a message about a foreign key.
- the php-extension toggle reloads the pool that reads the ini set it just rewrote. Both pools read
  it — `PHP_INI_SCAN_DIR` is per runtime version — so both are told.
- T81b's site resolution wanted the shared pool, and after D1 does not exist: a `web-app` names its
  own.

So there is one function, `pools::of_runtime`, answering `Vec<ServiceId>` in id order, and no second
function for "the shared one" — a pair of near-identical lookups is how a caller eventually asks the
wrong one. `PoolOutcome` on the wire is unchanged and aggregated: a restart required by any pool is
the answer, then a reload by any, then nothing running.

**And `pools::ensure`'s predicate is narrowed** to `NOT EXISTS (… AND s.instance_name = r.version)`,
so an extension's pool cannot stand in for the shared one the boot repair exists to create.

### D7 — The pool depends on the database, and the daemon writes that edge

A pool whose spec names `mariadb@main/root` and starts before MariaDB has ever run finds no entry,
and `Runner::environment` refuses to spawn. That is not a rare state: `mix extension install
phpmyadmin` on a machine whose MariaDB was declared and never started is the ordinary first
experience.

`ServiceSpec::depends_on` already answers it. `start_plan` pulls in everything the roots transitively
depend on, and `activate::ensure_running` — the request that wakes an idle pool — uses the same plan,
so both doors start the database first and the credential exists by the time the pool spawns.

**The manifest does not declare this and must not.** T80's D9 refuses `depends_on` in an
`extension.toml` because it is *"an edge into a graph the extension cannot see"*. This edge is not
the extension's: it is derived from a link this home resolved, by the generator that holds the whole
graph, and it disappears with the link.

**And it disappears together with the credential, which is what keeps the graph buildable.**
`ServiceGraph::new` fails the whole render on an unknown dependency — so an edge outliving its target
would be a daemon that renders nothing. It cannot: both the edge and the environment entry come from
the same resolved link, and `site_service_links.service_id` cascades with the service row, so
`mix service delete <db> --force` removes them in one statement.

**What it also buys, said out loud**: `mix service stop mariadb@main` now stops the interface onto it,
because `stop_plan` walks dependents. A phpMyAdmin left running against a database that has gone is
worse than one that is told the database is down — which is the sentence `stop_plan`'s own note
already carries about sites.

### D8 — A keyring that will not answer costs this one site, and that is the point of the dedicated pool

The failure this design cannot remove is a credential store that is locked or absent. `Runner` treats
a named credential it cannot read as a refusal to start, and rightly: a MariaDB started with no root
password is worse than one that did not start.

What changes is the blast radius, and it is the second reason D1 is not a conditional. Putting this
entry on the shared `[www]` pool would mean a locked keyring takes **every project site on the
machine** down. On a pool of the extension's own it takes phpMyAdmin down, the reason is in
`daemon.log` naming the entry, and every project keeps serving.

**And on a machine with no credential store there is nothing to administer anyway**: the same store
is where a managed MariaDB's root password is written at first run, so a home that cannot keep a
secret has no managed database for this interface to open. The two failures arrive together, which
is why neither needs a second explanation.

**A rotated password needs a restart**, because the environment is resolved once per spawn. Nothing
in this build rotates one; the task that adds it owes this pool a restart, and this paragraph is
where that is written down.

### D9 — Adminer keeps its login form, and the reason is upstream's

Adminer is a `web-app` with `[web-app.database]`, so D1 gives it a pool. It does not get `signs_in`.

phpMyAdmin publishes a supported way to be configured signed in: `$cfg['Servers'][$i]['auth_type'] =
'config'` with `user` and `password`, which is a documented deployment mode. Adminer has no
equivalent — its `credentials()` hook supplies a connection *once a session is authenticated*, and
skipping the authentication means forging the session state its login flow writes. T82 measured what
guessing at Adminer's hooks costs (`function_exists('adminer_object')` is a global name called from
inside `namespace Adminer`, and both obvious wrappers fail differently); doing it again against an
unsupported seam, for a credential this consequential, is not a trade this task takes.

So Adminer's manifest is unchanged apart from the pool it now runs on, its `credentials()` keeps
calling `\Adminer\get_password()`, and a person types the password `mix database` tells them where to
find. If upstream grows a supported mode, adding `signs_in = true` to that manifest is the whole of
the change — which is what putting the mechanism in the format rather than in a per-extension branch
buys.

### D10 — The pool is repaired at boot, not migrated

T82 shipped hours before this task, so a home may hold a phpMyAdmin whose site names the shared pool.
There is no migration.

`extensions::pools::ensure` is `services::pools::ensure`'s shape at a second site, and its module note
is the same one: **idempotent, run at boot as well as after an install**, so a home installed before
this task is repaired without a data migration and a home whose row somebody deleted by hand is
repaired too. For every extension-owned site whose pool is not the extension's own, it reads the
runtime install the site's current pool runs out of, creates `php-fpm@<id>` on it, and repoints the
row. A site naming no pool at all — a `--force`d `runtime.uninstall` — is skipped with a line naming
what would fix it, because there is nothing to read a runtime out of.

**Before `Extensions::configure`, and before the first render**, so the configuration written at boot
belongs to the pool the site is actually served on.

### D11 — Uninstall stops the pool before it removes the row

`extensions::uninstall`'s module note says supervision belongs to the daemon and the order it walks
is stop-then-this. A `web-app` had nothing to stop until now.

So `Extensions::uninstall` stops `php-fpm@<id>` before core is called, through the registry it
already reaches for; core then deletes the row, `etc/<service-id>/` and the service's logs after the
site is gone, on the same tolerate-a-half-done-uninstall rule as everything else in that function.
The site goes first because `sites.php_service_id` is `ON DELETE SET NULL` — deleting the pool first
would leave, for one statement, a site pointing at nothing, and an interruption there would leave it
for good.

`ExtensionRemoval` grows `pool`, so `mix extension uninstall` can say the process went with it rather
than leaving a `mix service list` entry to be discovered.

## Data flow

```
mix extension install phpmyadmin
  daemon: registry → manifest (signed)
  core:   install::plan / site_for
            resolve::runtime      → PHP 8.x            (T81b)
            database::resolve     → mariadb@main, root (T82)
            pools::id             → php-fpm@phpmyadmin
            signs_in              → signed in as root
  cli:    prints the plan, including "signs in as root on mariadb@main"; asks
  core:   install::install
            download, verify, unpack, rename
            [lock]  allocate [ports], write extensions row
            [unlock] pools::create  → services row, Origin::Runtime, php-fpm@phpmyadmin
                     sites::create  → site, pool = php-fpm@phpmyadmin, link = mariadb@main
  daemon: Extensions::configure_one → config.inc.php, getenv('MIXENGINE_DB_PASSWORD')
          Sites::now_declares       → hosts, certificate, regenerate

next render
  core:   generate::declarations_with
            extension sites → { php-fpm@phpmyadmin: Credential {
                                  env: MIXENGINE_DB_PASSWORD,
                                  keyring: mixengine / mariadb@main/root,
                                  database: mariadb@main } }
          php_fpm::spec
            env  MIXENGINE_DB_PASSWORD = Keyring{…}
            depends_on mariadb@main
            php-fpm.conf: clear_env = no
                          listen = run/php-fpm-phpmyadmin.sock

first request to https://phpmyadmin.mixengine.test
  daemon: activator → start_plan[php-fpm@phpmyadmin]
            mariadb@main first (first run provisions mariadb@main/root)
            php-fpm@phpmyadmin
              Runner::environment → keyring read → master's env
              workers inherit (clear_env = no)
  php:    getenv('MIXENGINE_DB_PASSWORD') → connected, signed in
```

## Testing

Where the rule lives, per `.claude/standards/testing.md`.

**Unit, `mixengine-core`.**

- `manifest`: `signs_in` parses; `signs_in = true` with no `{db_password_env}` in the configuration
  text is refused naming both fields; `{db_password_env}` with `signs_in = false` is refused by the
  renderer naming the field; `signs_in` outside `[web-app.database]` is an unknown field.
- `render`: `{db_password_env}` renders the constant, and the PHP around it survives (T82's D8 shape).
- `extensions::pools`: the id is `php-fpm@<id>`; an id too long is refused naming the limit; the
  credential map is built off the link and is empty for a site with no `signs_in`, for a database
  whose recipe names no administrator, and for a pool a second site also names.
- `sites`: a project site naming an extension's pool is refused; the extension's own site is not.
- `services::pools`: `of_runtime` answers both pools in id order; `ensure` still makes a shared pool
  on a runtime that has only an extension's.
- `recipes::php_fpm`: a context with a credential puts `EnvValue::Keyring` on the spec with the
  address `Context::secret_address` composes and a `depends_on` naming the database; a context
  without one puts neither; the pool file carries `clear_env = no` only in the first case; the socket
  is spelled from the instance and a pool whose instance is its version renders the byte-identical
  path it rendered before.

**Component, `crates/mixengine-core/tests/extension_install.rs`.**

- installing a `web-app` writes `php-fpm@<id>` on the same runtime install as the shared pool and
  points the site at it;
- uninstalling removes the row, the `etc/` directory and the site, in that order, and a second run
  after an interruption finishes;
- `pools::ensure` moves a site written the T82 way onto a pool of its own and is a no-op the second
  time.

**Integration, `crates/mixengine-daemon/tests/`.**

- the generated `etc/php-fpm@<id>/php-fpm.conf` carries `clear_env = no` and `etc/php-fpm@<v>/`
  does not — the one assertion that would catch the credential reaching every project's PHP;
- `runtime.uninstall` on a PHP both pools run out of refuses, naming the extension;
- a spec built for the extension pool holds no password: `spec.env()` carries a `Keyring` variant and
  the `Debug` of the resolved `Surroundings` prints the name without the value (the supervisor's own
  test's shape, at this address).

**CLI.** `mix extension plan` prints the sign-in line, and `mix extension uninstall` names the pool.

**The real run**, which is the only thing that proves D3: install phpMyAdmin against a managed
MariaDB on this machine, open the site, and land on the database listing. A unit test that agrees
with the renderer would prove nothing about whether php-fpm passes an environment — which is exactly
what T81c's fragment lesson says about checking a language with the thing that speaks it.

## Risks, and where each is answered

| Risk | Answer |
| --- | --- |
| A project's PHP reads the password | D5 refuses the write; D4 fails closed if a row ever gets there anyway; the integration test asserts `clear_env` on both files |
| The pool will not start because the keyring is locked | D8 — one site, not every site, and the same store is why there is no database to open |
| The pool starts before the database is provisioned | D7 — the edge, honoured by `start_plan` and by on-demand activation |
| `ServiceGraph` refuses to build over a stale edge | D7 — edge and credential come from one link, and the link cascades with the service row |
| An extra php-fpm master per `web-app` | D1 — idle-stopped after half an hour, woken by the request that needs it |
| A home installed under T82 keeps the shared pool | D10 — repaired at boot, no migration |
| `auth_type = 'config'` means anyone reaching the site is root | The site binds loopback in both front ends and cannot be shared — measured above, and enforced by the parse for the whole kind |
| A manifest names a variable that breaks the pool | D2 — there is no name for a manifest to write |

## What this leaves

`features/extensions.md`'s second acceptance criterion is whole: phpMyAdmin reaches the managed
MariaDB on an internal domain with a valid certificate, its server, port and account already filled
in, **and signs itself in** — with the password in one process's environment, on no disk, and in no
other project's.

What is still open is T83's handoff, which needs the same address from the same place —
`Recipe::administrator` and `Context::secret_address` — and now has a second caller proving that pair
is the right shape.
