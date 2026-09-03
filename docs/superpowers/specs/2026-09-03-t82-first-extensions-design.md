# T82 — the first three extensions (design)

Roadmap task **T82**, phase 8. T80 gave the manifest its format, T81 gave it a registry and a
lifecycle, T81a gave it a published document, T81b gave a `web-app` a site, and T81c made
`[recipe] front_end` take effect. Every one of those was built against four fixtures written into
`mixengine-testkit` with the note that **these are the manifests T82 will ship**.

This is that task: the three real ones — Mailpit, phpMyAdmin, Adminer — in
`mixnz/mixengine-packages`, and everything the real ones turn out to need that the fixtures did not.

## Goal

`mix extension install phpmyadmin` on a machine with a managed MariaDB opens
`https://phpmyadmin.mixengine.test` at a login form that already knows which server and which
account, with no file anybody had to edit. `mix extension install mailpit` makes `mail()` land in a
web UI. `mix extension install adminer` does the same on one PHP file. All three come out of the
signed registry, and nothing about them is compiled into MixEngine.

## What the fixtures were wrong about, said first

Four things, each found by fetching the real artifact rather than by reading the format again.

| Fixture said | The world says |
| --- | --- |
| `mailpit` 1.20.0, three targets | 1.31.0, and upstream publishes all **six** targets `Os` × `Arch` can name |
| `[web-app].root = "{install_dir}/app"` | the phpMyAdmin zip's own top level is `phpMyAdmin-5.2.3-all-languages/` |
| `template = "config.inc.php.tmpl"` — *a file inside the extension* | the artifact is upstream's zip byte for byte; there is nowhere in it to put our file |
| (no Adminer fixture) | Adminer's distribution is `adminer-6.0.1.php` — **one file, not an archive** |

The third is the one that costs a format change, and it is the reason this task is architectural
rather than three TOML files.

## Measured, not assumed

Everything below was read off the real artifact or the real tree, and each line is what a decision
rests on.

- **Mailpit `v1.31.0`** publishes `mailpit-{windows,linux}-{amd64,arm64}` and
  `mailpit-darwin-{amd64,arm64}`, as `.zip` on Windows and `.tar.gz` elsewhere — all three formats
  `install::archive` already unpacks.
- **phpMyAdmin `5.2.3`**, `composer.json` requires `^7.2.5 || ^8.0`.
  `libraries/vendor_config.php` fixes `'configFile' => ROOT_PATH . 'config.inc.php'` — **hard-coded,
  with no environment override**, so a generated configuration has exactly one legal home.
  The zip contains `libraries/cache/` and **does not contain `tmp/`**, while the same file sets
  `'tempDir' => ROOT_PATH . 'tmp' . DIRECTORY_SEPARATOR`.
- **Adminer `v6.0.1`** publishes `adminer-6.0.1.php` and a `.zip` that is the *development source
  tree*, not a release. `adminer/include/bootstrap.inc.php` reads
  `function_exists('adminer_object')` from inside `namespace Adminer`, which is the wrapper hook.
- **The published `index.json`** gives every PHP from 7.0 to 8.5. On Windows `mysqli`, `mbstring`,
  `sodium`, `openssl`, `zip`, `pdo_mysql` and `pgsql` are all in `extensions.enabled`; on Linux they
  are compiled static. Neither application needs an extension a person has to switch on.
- **`VersionConstraint`** (`mixengine-proto/src/version.rs`) accepts `^X.Y`, `X.Y` and `X.Y.Z` and
  nothing else. There is no `>=` and no upper bound short of the next major.
- **`site_service_links.service_id`** is `REFERENCES services (id) ON DELETE CASCADE`
  (`0001_initial.sql:206`).
- **Nothing in `mixengine-core` or `mixengine-daemon` verifies an install directory after the
  install** — no rehash, no manifest of files. `install::Installer` proves the *archive*, once.

## Scope

**In:** `mixengine-core` (`extensions::manifest` grows `[web-app.config]` and `[web-app.database]`
and loses `template`; `extensions::render` grows a fourth `Destination` and three placeholders;
`extensions::config` is new; `extensions::install` resolves and links the database;
`extensions::uninstall` drops the keyring entry; `install` learns a one-file artifact; `generate`
answers a database service's endpoint and renders an installed `web-app`'s configuration;
no new refusal in `services`, because writing the link is what arms the one that is there);
`mixengine-proto` (the plan says which database it would use,
and `Recipe::administrator` has a wire-visible consequence in that answer); `mixengine-daemon`
(`Extensions` writes the configurations at install and at boot, and supplies the keyring secret);
`mixengine-cli` (what `plan`, `install` and `inspect` print); `mixengine-testkit` (the four fixtures
become the manifests that were actually shipped). Documentation:
[features/extensions.md](../../../.claude/features/extensions.md),
[features/client-surface.md](../../../.claude/features/client-surface.md), and the roadmap.

**In, in `mixnz/mixengine-packages`:** `data/extensions/mailpit.toml`, `data/extensions/phpmyadmin.toml`,
`data/extensions/adminer.toml`, and `data/extensions/README.md`, which currently says there is no
manifest there yet.

**Out:**

- **Signing phpMyAdmin in with a database password.** D6.
- **A php-fpm pool of the extension's own.** What auto-login needs, and it is **T82a**.
- **More than one server in a generated configuration.** D5's honest consequence.
- **MixDB.** T83 and T84, and D8 is what this task leaves them.
- **A `[recipe] front_end` fragment.** T81c wired it; none of these three declares one, which is
  what T81c said would be true.

## Decisions

### D1 — The configuration is text the manifest carries, not a file inside the artifact

T80 wrote `[web-app].template` as *"a file inside the extension, rendered into the app's own
configuration"*, and nothing has read it since. Reading it is impossible as written: for a registry
install the extension's files **are** upstream's archive, verified against a SHA-256 that upstream
published, and there is no step between the download and the rename where a file of ours could be
added without making that hash a hash of something else.

So the text moves into the manifest:

```toml
[web-app.config]
path = "config.inc.php"          # relative to [web-app].root
text = """
<?php
$cfg['Servers'][1]['host'] = '{db_host}';
…
"""
```

`path` is `rooted`-checked against `[web-app].root` the way every other path in the format is: no
`..`, no absolute path, no climbing out. One file and not a list — phpMyAdmin needs one and Adminer
needs one, and a list is a second thing to sweep on uninstall for a need nobody has stated.

**Two alternatives, refused.**

- **Repackage phpMyAdmin and Adminer in `mixengine-packages`**, with our template inside, the way
  that repository already repackages runtimes. It would work, and it makes MixEngine the publisher
  of a modified phpMyAdmin — a supply chain to keep current, a hash that is ours, and a user who
  cannot check what they installed against what phpMyAdmin released. The roadmap already settled
  this by writing *"`[web-app].root` for the **real** phpMyAdmin zip"*.
- **Compile the template into MixEngine, keyed by extension id.** A per-extension branch in core is
  the exact thing the extension model exists to remove.

**What this changes about a registry entry, said plainly.** After this, an entry carries PHP that
will be executed by a managed runtime, where before it carried a URL and a hash. The blast radius is
unchanged and the reason is worth writing down: the entry is signed by the index key, which is the
same key that vouches for the binary the entry points at, and a binary MixEngine downloads and
supervises can do everything that PHP can and more. A `--path` install was already copying a whole
directory of the author's files. What would change the blast radius is a *second* key or an
*unsigned* registry, and this task adds neither.

### D2 — The generated file lives in the served root; what a person changes lives in `{data_dir}`

Measured above: phpMyAdmin's `configFile` is `ROOT_PATH . 'config.inc.php'`, fixed in
`vendor_config.php`, with no environment variable and no search path. There is one place the file can
go and it is inside the install directory.

That is a smaller trespass than it first reads. Nothing verifies an install directory after the
install — checked, not assumed — so no integrity claim is broken; `extension.uninstall` removes the
directory whole, so the generated file has the lifetime it should; and the file is ours, written from
state in SQLite and thrown away, which is the rule `etc/` follows rather than an exception to it.

The rendered file ends with

```php
@include '{data_dir}/config.user.php';
```

and that is the split `template` was in the format to provide: **the generated file is ours and the
settings inside it are theirs.** `{data_dir}` survives an uninstall by T81's D13, so a person's own
settings survive an upgrade, a reinstall, and a change of MixEngine's mind about what the generated
half should say.

**`$cfg['TempDir']` is pointed at `{data_dir}`** — because the zip has no `tmp/`, and because a cache
directory that survives an upgrade is worth more than one that does not.

### D3 — An artifact that is not an archive is one file, and only an extension may say so

Adminer's release is `adminer-6.0.1.php`. `install::archive::Format::of` returns `None` for it and
`Installer::install` turns that into `Error::ArtifactFormat` before it fetches a byte.

That refusal is right for the package index — `mkindex.py` produces three suffixes and a fourth in
`index.json` means an artifact this build has no decompressor for — and wrong for an extension, where
a single file is what a great many small tools ship. So the caller says which it will accept:

```rust
pub enum NotAnArchive {
    /// Refuse it. What the package index passes.
    Refuse,
    /// Install it as one file. What `extensions::install` passes.
    OneFile,
}
```

Everything else is unchanged: the same download with the same resume, the same SHA-256, the same
staging directory beside the destination and the same atomic rename. `unpack` is replaced by a copy
into staging, and by nothing else.

**The name comes from the URL, so the name is checked.** `Installer::part_file` already carries the
warning this needs — *"a name that came out of a document can never be a path"* — so the last path
segment of the URL is required to be a bare file name: not empty, no `/` or `\`, not `.` or `..`.
A URL that does not end in one is `Error::ArtifactFormat`, which is the error it already got.

**The alternative was Adminer's `.zip`**, which needs no core change at all. It is refused because
that zip is the development source tree — upstream does not publish it to be served, `adminer/` would
be a doc root full of directly-requestable `*.inc.php`, and there is nowhere in it for D1's generated
file to sit without replacing a file upstream shipped.

### D4 — A `web-app` may declare the database it needs, and the declaration becomes a row

```toml
[web-app.database]
engines = ["mariadb", "mysql"]
```

Install resolves this **exactly the way T81b resolves the PHP**: at install time, before anything is
fetched, refusing by name when nothing satisfies it and freezing the answer into a row. The row is
`site_service_links`, which T81b left empty for an extension site and named this task to decide.

The rule is written down so two machines answer the same way: **`engines` is in order of preference;
the first engine with a declared service wins; among instances of one engine the one whose instance
name is `main` wins, and failing that the first by id.** Nothing here starts anything — a stopped
MariaDB is still the server phpMyAdmin is pointed at.

**Deleting that service is already refused, and the link is what makes the refusal fire.**
`site_service_links.service_id` is `ON DELETE CASCADE`, so a cascaded delete would take the link away
in silence. It cannot happen by accident: `service.delete`'s fourth refusal reads
`sites::declaring`, whose query is `WHERE s.php_service_id = ? OR l.service_id = ?` — a *link* counts
— so writing the row is what buys the refusal. **No new refusal is needed**, which is worth saying
because the first draft of this design added one; the link was the whole mechanism.

What is left is the one path that crosses it. `mix service delete <db> --force` is a person
overruling the refusal, and the link cascades away. `Extensions::configure` then finds a `web-app`
whose `[web-app.database]` resolves to nothing and **skips it, warning with the extension's name and
the command that would put a database back** — it does not rewrite the file. Skipping is not reading
state out of a generated file; it is declining to overwrite one, and it means a forced delete costs a
warning rather than silently rewriting phpMyAdmin's configuration into something that points nowhere.

### D5 — Three placeholders, and one server

`{db_host}`, `{db_port}` and `{db_user}` render from the linked row and from nothing else. They join
`{install_dir}`, `{data_dir}`, `{listen}` and the `[ports]` names, and like those they are reserved:
`[ports]` may not claim one.

- **No `{db_socket}`.** Every database recipe binds a TCP port on all three systems — that is T34c's
  *"a pool gets its 9000 the same way `mariadb@main` gets its 3306"* — so one address form is enough,
  and a placeholder that renders to nothing on Windows is a template that cannot be written once.
- **`{db_user}` comes from the recipe, not the manifest.** `Recipe` grows
  `administrator() -> Option<&str>` with a default of `None`, answered by the three database recipes
  — `root` for MariaDB and MySQL, `postgres` for PostgreSQL — and by nothing else, so a front end, a
  cache and a pool are unchanged. A manifest that wrote `root` itself would be a manifest that is
  wrong the day a recipe changes its mind, and T83's connection handoff needs the same answer from
  the same place.

**One server, and that is a real limit.** A machine running both MariaDB and MySQL gets one of them
configured. Listing both needs a loop, a loop needs a template language, and this workspace does not
have one on purpose. The second server is three lines in `config.user.php`, which D2 makes survive
everything — and this paragraph exists so that the limit is documented rather than discovered.

### D6 — No password reaches the disk: `auth_type = 'cookie'`

The generated configuration names the host, the port and the account. It does not name the password,
and `mix` will not print one either — `main.rs:2582` already says why: *"a password on a terminal is
a password in a scrollback"*, and `generate::step::SecretFile` exists so that a credential that must
touch a disk touches it for one step and is then removed.

Writing the MariaDB root password into `config.inc.php` would overturn both, and buy something worse
than it looks: an administrative interface onto every database on the machine, reachable by anything
that can resolve a loopback name, behind no authentication whatsoever.

**What auto-login actually needs is a php-fpm pool of the extension's own**, carrying an
`EnvValue::Keyring` the supervisor resolves at spawn, so the password is in one process's environment
and on no disk. Today there is one `[www]` pool per PHP version shared by every project site, and
putting a database superuser's password into that is handing it to every project on the machine.

That is **T82a**, added to the roadmap by this task. `features/extensions.md`'s acceptance criterion
*"phpMyAdmin reaches the managed MariaDB with credentials taken from the keyring"* is edited to say
which half T82 delivers and which half T82a does — the roadmap has overturned its own lines before
(T75's D1, T81b's two findings), and it does it by writing the correction down.

### D7 — `{secret}` is a keyring entry, because the alternative is reading a generated file back

phpMyAdmin needs `blowfish_secret` to be **stable**: a value that changed on every render would log
everybody out on every regeneration, and an absent one puts a red banner under every page.

It cannot be generated at render time, and it cannot be recovered by reading the last generated file
— *"never parse a generated file back into state"* is the rule the whole `etc/` layout rests on. So
it is generated once, at install, and kept where this system keeps a secret. `{secret}` renders from
it.

`mixengine-core` has no business reaching a credential store — `generate::databases`' D1 — so the
shape is the one that already exists: the render says it needs the value, and the **daemon** creates
or reads the entry and puts it on the context, exactly as it does for a recipe's declared secrets.
`extension.uninstall` removes the entry, so a reinstall does not inherit a key from an extension that
is gone.

### D8 — A fourth `Destination`, because PHP is made of braces

`render::spelled` refuses an unknown `{…}` when the destination is `Destination::Field`, and rightly:
a `{home_dir}` surviving into an argument is a literal brace handed to a program. A `config.inc.php`
is not that document. `function adminer_object() {` is four braces before the first placeholder.

T81c already met this problem for front-end fragments and already solved it, including the part that
two unit tests missed and the real server caught: an unrecognised `{` is re-emitted and the scan
continues **one character on**, not past the next `}`, because the nearest `}` after a block's `{` is
usually the close of a real placeholder further down. `Destination::PhpSource` is that behaviour
plus forward slashes — PHP accepts `/` on Windows, and a Windows path inside a single-quoted PHP
string is a backslash the reader has to reason about.

**What takes over from the refusal** is what took over in T81c: the real thing judges it. Here that
is the PHP runtime, and the test that proves it is the real run in the plan, not a unit test that
agrees with the renderer.

### D9 — `requires = "^8.0"`, and the constraint cannot say what upstream says

phpMyAdmin declares `^7.2.5 || ^8.0`; Adminer declares `>=7.4`. `VersionConstraint` has no `||`, no
`>=` and no upper bound below the next major. `^8.0` is what it can say: every PHP 8 this repository
offers, which is every PHP 8 either application supports.

The cost is a machine holding only PHP 7.4 being refused an Adminer that would have run. That is a
refusal with a reason and a command to fix it — T81b's `RuntimeUnresolved` naming the extension —
rather than an install whose stated effect does not happen, and widening the constraint grammar for
it would be a change to how every runtime in the product is selected, argued from two manifests.

### D10 — The fixtures become the manifests that shipped

`mixengine-testkit/fixtures/extensions/*.toml` opens with *"These are the manifests T82 and T83 will
ship, not examples written to fit the parser"*. That claim is now checkable, so it is checked: the
three carry the versions, the roots, the artifact targets and the `[web-app.config]` text the roster
ships, and a fourth is added for Adminer.

**Not byte for byte, and the fixtures' own header says why**: *"The hashes are placeholders … a real
hash here would be a fact that goes stale with the next release."* That is still true and is the one
field that stays a placeholder — with the header amended to say that the rest no longer is, so the
next reader knows which half of the file to trust. `sendmail.toml` stays as the one fixture with no
product behind it — a `kind = "recipe"` on its own, a shape the format has and the roster does not.

A fixture that has drifted from the roster is a test suite proving a format nobody publishes.

## Data flow

```
mix extension install phpmyadmin
  └─ registry: verified extensions.json → one entry → manifest::read_value
     └─ plan
        ├─ artifact_for_host                 → the zip for this OS
        ├─ site_for (T81b)                   → domain, pool frozen, doc_root
        └─ database_for            [D4]      → mariadb@main, or a refusal naming what to install
     └─ consent
     └─ install
        ├─ Installer::install(NotAnArchive::OneFile)     the zip, staged and renamed —
        │                                                an extension always allows either, and
        │                                                this artifact happens to be an archive
        ├─ extension row + ports
        ├─ services row  (none: this is a web-app)
        ├─ sites row, services = [mariadb@main]          [D4]
        └─ daemon: keyring entry for {secret}            [D7]
     └─ daemon: Extensions::configure
        └─ generate: endpoint of mariadb@main → {db_host} {db_port} {db_user}
           render text with Destination::PhpSource       [D8]
           write <root>/config.inc.php atomically        [D1, D2]
     └─ hosts, certificate, regeneration (T81b)
```

At boot, `Extensions::configure` runs again for every installed `web-app`, which is what makes the
file disposable rather than a thing written once and hoped about.

## Errors

| When | What is said |
| --- | --- |
| `engines` matches no declared service | `Error::ExtensionNoDatabase`, naming the engines and the `mix service` command that would create one |
| `service.delete` on a linked service | `service.delete`'s existing fourth refusal, naming the extension's site — the link is what makes it fire |
| a linked service was force-deleted | nothing at the time; the next `Extensions::configure` skips that extension and warns, leaving its configuration alone |
| `[web-app.config].path` escapes the root | `Error::ExtensionField`, the message `rooted` already gives |
| an artifact URL whose last segment is not a bare file name | `Error::ArtifactFormat`, which it already got |
| the configuration cannot be written | `Error::Io`, naming the path — the install is already complete, so this is reported and not rolled back |

The last row is deliberate: the rows and the files are in place, the extension is installed, and what
failed is one generated file that the next `Extensions::configure` will write. Undoing an install
because a regeneration failed would throw away a sixteen-megabyte download over something a restart
fixes.

## Testing

- **Unit, `mixengine-core`:** `[web-app.config]` parsed, and its `path` refused when it escapes;
  `[web-app.database]` parsed; `{db_*}` refused as a `[ports]` key; `Destination::PhpSource` leaving
  `function f() {` alone while substituting the placeholder after it; a one-file artifact's URL
  refused when its last segment is not a bare name; `Recipe::administrator` for all five recipes.
- **Unit, resolution:** `engines` picking in declared order; the default instance winning; the
  refusal when nothing matches.
- **Integration, `mixengine-cli`:** install a `web-app` from `--path` with a `[web-app.config]` and
  assert the file lands in the doc root with the placeholders substituted; assert `service.delete` on
  the linked database is refused *without a line of new refusal code*, and that `--force` then leaves
  the configuration alone rather than rewriting it; assert uninstall takes the file and the keyring
  entry away and leaves `config.user.php`.
- **The real run, on this machine.** `mix extension install --path` is the only entry that reads a
  manifest off disk, and it *copies* a directory rather than downloading — so the three are staged
  the way their artifacts unpack (phpMyAdmin's zip extracted under the id, Adminer's one file beside
  its manifest) and installed against a real MariaDB and a real PHP: Mailpit captures a `mail()`,
  phpMyAdmin's login page renders with no configuration banner and the server preselected, Adminer's
  wrapper loads. **This is the test D8 and D3 actually rest on** — a renderer that agrees with itself
  proves nothing about whether PHP will parse what it wrote.
- **The download half is proved after publication**, not before: nothing can install these from the
  registry until `mixengine-packages` has cut `extensions.json`, which is this task's second half and
  its own pull request. `mix extension available` listing the three, and one real
  `mix extension install mailpit`, is the last thing done and the thing that says T82 is finished.

## Documentation

- [features/extensions.md](../../../.claude/features/extensions.md): `[web-app].template` becomes
  `[web-app.config]` with D1's reasoning; the `web-app` section gains D2, D4 and D5; the acceptance
  criterion about keyring credentials is split between T82 and T82a with D6's argument.
- [features/client-surface.md](../../../.claude/features/client-surface.md): a plan names the
  database it would use, so a graphical client can show it.
- [roadmap/phase-8-differentiators.md](../../../.claude/roadmap/phase-8-differentiators.md): T82
  ticked and written up, T82a added after it.
- `mixnz/mixengine-packages`: `data/extensions/README.md` no longer says the directory is empty.
