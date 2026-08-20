# T28 — PHP extensions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An installed PHP carries a generated `conf.d` that both its pool and the `php` on the
terminal read, and `mix runtime ext enable|disable` moves one line in it and tells the pool.

**Architecture:** The artifact's own extension facts are copied into `runtime_installs` at install
time; the user's *deviations* from them live in a third column. The effective set is rendered
through T30's `Document`/`install` into `etc/<kind>/<version>/conf.d/`, and reaches PHP through
`PHP_INI_SCAN_DIR` set in exactly two places — the php-fpm pool spec and the shim.

**Tech Stack:** Rust (workspace), `sqlx` with offline query data in `.sqlx/`, `serde_json`, `tokio`,
`clap` (CLI), the existing `mixengine-testkit` harness (`MockRegistry`, `FakePackage`,
`fastcgi::Pool`).

**Spec:** [`docs/superpowers/specs/2026-08-20-t28-php-extensions-design.md`](../specs/2026-08-20-t28-php-extensions-design.md)

## Global Constraints

- **No business logic in clients.** The CLI renders what `runtime.list_extensions` /
  `runtime.set_extension` answer; it never merges defaults with choices itself.
- **No `#[cfg(windows)]` in core or daemon code.** Platform differences are read out of the index
  (`extensions.static` vs `extensions.shared`) or expressed with `cfg!()` as a *value*, as
  `php_fpm.rs` already does.
- **Generated config is disposable and never parsed back.** Nothing reads a file under `etc/` into
  state; `document::install` compares checksums only.
- **Cross-platform or not merged.** Every task must compile on Windows, macOS and Linux.
- **`*_json` columns**: nothing queries into them; the whole document is read for one runtime and
  looked up in memory (migration `0002`'s argument).
- Exact generated values, copied from the spec and not to be re-invented:
  `memory_limit = 512M`, `upload_max_filesize = 128M`, `post_max_size = 128M`,
  `max_execution_time = 120`, `display_errors = On`, `error_reporting = E_ALL`,
  `date.timezone = UTC`, `opcache.enable = 1`, `opcache.revalidate_freq = 0`.
- Load-order prefixes: `20` igbinary, `40` opcache, `50` everything else, `90` xdebug.
- `zend_extension=` for exactly two names: `opcache`, `xdebug`. Everything else is `extension=`.
- `extension_dir` is always written, and always **absolute**.
- No `php.ini` is generated. No user-editable ini settings. No per-project or per-site sets.
- Commit messages: conventional prefix, imperative, **no `Co-Authored-By` trailer**.
- Work happens on the current feature branch `feat/php-extensions-conf-d`, never on `master`.

## Two decisions this plan makes that the spec left implicit

1. **The conf.d directory is swept.** `document::install` writes the documents it is given and
   prunes nothing, so disabling an extension would leave its `90-xdebug.ini` behind and the
   extension would go on being loaded. Task 4 removes every `*.ini` in the directory that is not one
   of the rendered documents, and counts a removal as a change for the reload decision.
2. **`etc/<kind>/<version>/conf.d/` and not `etc/php/…` literally.** The path is built from
   `kind.as_str()`, so nothing here is a `match` on PHP; a kind whose artifact declares no
   `extension_dir` renders nothing and gets no directory.

## File Structure

**Created**

| File | Responsibility |
| --- | --- |
| `crates/mixengine-core/migrations/0005_runtime_extensions.sql` | Three additive columns on `runtime_installs`. |
| `crates/mixengine-core/src/runtimes/extensions.rs` | The whole model: read the row, merge choices, render the documents, install them, sweep, and the `conf.d` path both consumers use. |
| `crates/mixengine-daemon/src/extensions.rs` | `runtime.list_extensions` and `runtime.set_extension`, and the pool reload that follows a change. |
| `crates/mixengine-cli/tests/php_extensions.rs` | The `#[ignore]`d suite against a real PHP: both consumers agree, before and after a toggle. |

**Modified**

| File | Change |
| --- | --- |
| `crates/mixengine-core/src/index/format.rs` | `Extensions` grows `enabled`. |
| `crates/mixengine-core/src/runtimes.rs` | `pub mod extensions;`, `Installation` carries the artifact's extension facts, `remember` writes them. |
| `crates/mixengine-core/src/lib.rs` | One new `Error` variant for a compiled-in extension. |
| `crates/mixengine-core/src/generate.rs`, `generate/recipe.rs` | `Context` learns the home's `etc/` root. |
| `crates/mixengine-core/src/generate/recipes/php_fpm.rs` | `PHP_INI_SCAN_DIR` on both arms of the builder. |
| `crates/mixengine-shim/src/main.rs` | The same variable, beside `PATH`. |
| `crates/mixengine-proto/src/runtime_api.rs`, `rpc.rs`, `lib.rs` | The extension vocabulary and two method names. |
| `crates/mixengine-daemon/src/runtimes.rs`, `main.rs`, `api/mod.rs`, `api/rpc.rs`, `error.rs`, `services/mod.rs` | Render after install, at boot, and remove on uninstall; wire the new module in; one reload accessor. |
| `crates/mixengine-cli/src/main.rs`, `render.rs` | `mix runtime ext list|enable|disable`. |
| `crates/mixengine-testkit/src/declare.rs` | A PHP row carrying extension facts, for the daemon tests. |
| `.github/workflows/ci.yml`, `.github/scripts/test-no-network.sh` | Run the new suite where a real PHP exists. |
| `.claude/features/runtime-versions.md`, `.claude/roadmap/phase-2-runtimes.md`, `.claude/roadmap/todo.md` | Record the two deviations, tick T28. |

---

### Task 1: The index reads `enabled`

The index has published `extensions.enabled` since `mixengine-packages` P2 and this build drops it on
the floor. Everything downstream needs it, so it goes first and alone.

**Files:**
- Modify: `crates/mixengine-core/src/index/format.rs` (the `Extensions` struct and its `is_empty`, ~line 164-180)
- Test: `crates/mixengine-core/src/index/format.rs` (the existing `#[cfg(test)] mod tests`, ~line 395)

**Interfaces:**
- Consumes: nothing.
- Produces: `mixengine_core::index::Extensions { compiled_in: Vec<String>, shared: Vec<String>, enabled: Vec<String> }`, every field `#[serde(default)]`.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` at the bottom of `format.rs`:

```rust
    /// The index says which of `shared` an installer is expected to switch on, and dropping it is
    /// the difference between a Windows PHP that behaves like its Unix twin and one that starts
    /// without `mbstring`.
    #[test]
    fn an_artifact_says_which_shared_extensions_are_on_by_default() {
        let artifact: Artifact = serde_json::from_value(serde_json::json!({
            "os": "windows",
            "arch": "x86_64",
            "url": "https://example.invalid/php-8.3.33-windows-x86_64.zip",
            "sha256": "00",
            "size": 1,
            "provides": {"php": "php.exe"},
            "extension_dir": "ext",
            "extensions": {
                "static": ["core", "date"],
                "shared": ["curl", "mbstring", "xdebug"],
                "enabled": ["curl", "mbstring"]
            }
        }))
        .expect("an artifact the published schema allows");

        assert_eq!(artifact.extensions.enabled, ["curl", "mbstring"]);
        assert!(
            !artifact.extensions.enabled.contains(&"xdebug".to_owned()),
            "a shared extension the publisher does not switch on is not enabled by being shipped"
        );
    }

    /// An index from before this field, and an artifact that loads nothing, are both silent.
    #[test]
    fn an_artifact_with_no_extensions_at_all_stays_empty() {
        let extensions: Extensions = serde_json::from_str("{}").expect("an empty object");

        assert!(extensions.is_empty());
        assert_eq!(serde_json::to_string(&extensions).expect("json"), "{}");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mixengine-core --lib index::format`
Expected: FAIL — `no field 'enabled' on type 'Extensions'`.

- [ ] **Step 3: Add the field**

In `crates/mixengine-core/src/index/format.rs`, inside `pub struct Extensions`, after `shared`:

```rust
    /// Which of [`shared`](Self::shared) an installer is expected to switch on, so that the cells
    /// of one version behave alike.
    ///
    /// Published per artifact and not per version, which is the whole reason it exists: Windows
    /// ships `curl`, `mbstring`, `intl` and a dozen more as DLLs where Unix compiles them in, so
    /// "the extensions a user expects to be there" is a different set on each system and only the
    /// publisher knows which.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enabled: Vec<String>,
```

and extend `is_empty`:

```rust
    pub fn is_empty(&self) -> bool {
        self.compiled_in.is_empty() && self.shared.is_empty() && self.enabled.is_empty()
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mixengine-core --lib index::format`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/mixengine-core/src/index/format.rs
git commit -m "feat(index): read which shared extensions an artifact enables by default (T28)"
```

---

### Task 2: The install writes the artifact's extension facts down

A row must be able to answer "can `redis` be enabled for this PHP" with the network down and the
six-hour index cache expired, which is `provides_json`'s argument applied a second time.

**Files:**
- Create: `crates/mixengine-core/migrations/0005_runtime_extensions.sql`
- Modify: `crates/mixengine-core/src/runtimes.rs` (`Installation`, `remember`)
- Modify: `crates/mixengine-daemon/src/runtimes.rs` (the `Installation` literal in `perform`, ~line 355)
- Test: `crates/mixengine-core/src/runtimes.rs` (`mod tests`), `crates/mixengine-core/tests/store.rs`

**Interfaces:**
- Consumes: `index::Extensions` from Task 1.
- Produces: `runtimes::Installation { …, extension_dir: Option<String>, extensions: crate::index::Extensions }`; the columns `extension_dir TEXT NOT NULL DEFAULT ''`, `extensions_json TEXT NOT NULL DEFAULT '{}'`, `extension_choices_json TEXT NOT NULL DEFAULT '{}'`.

- [ ] **Step 1: Write the failing tests**

In `crates/mixengine-core/src/runtimes.rs`, extend the test helper and add a test:

```rust
    fn installation(kind: RuntimeKind, text: &str) -> Installation {
        Installation {
            // … existing fields unchanged …
            extension_dir: Some("lib/php/extensions".to_owned()),
            extensions: crate::index::Extensions {
                compiled_in: vec!["core".to_owned(), "date".to_owned()],
                shared: vec!["redis".to_owned(), "xdebug".to_owned()],
                enabled: vec!["redis".to_owned()],
            },
        }
    }

    /// The index is a cache with a network behind it; whether `redis` can be enabled for a PHP that
    /// is on this disk must not depend on either.
    #[tokio::test]
    async fn what_a_build_offers_is_written_down_beside_it() {
        let (_home, store) = store().await;
        remember(&store, &installation(RuntimeKind::Php, "8.3.33"), NOW)
            .await
            .expect("a row");

        let row = sqlx::query!(
            "SELECT extension_dir, extensions_json, extension_choices_json
             FROM runtime_installs WHERE kind = 'php' AND version = '8.3.33'"
        )
        .fetch_one(store.pool())
        .await
        .expect("the row");

        assert_eq!(row.extension_dir, "lib/php/extensions");

        let offered: crate::index::Extensions =
            serde_json::from_str(&row.extensions_json).expect("what the artifact published");
        assert_eq!(offered.enabled, ["redis"]);
        assert_eq!(offered.shared, ["redis", "xdebug"]);

        assert_eq!(
            row.extension_choices_json, "{}",
            "an install has made no choices on the user's behalf"
        );
    }
```

And in `crates/mixengine-core/tests/store.rs`, the schema-level half — that an existing row survives
the migration meaning something honest:

```rust
/// The three columns T28 adds are additive, and a row written before them says so rather than
/// failing to be read.
#[tokio::test]
async fn a_runtime_installed_before_extensions_existed_offers_none() {
    let (_temp, store) = store().await;

    sqlx::query(
        "INSERT INTO runtime_installs
             (kind, version, channel, install_path, installed_at, size_bytes, source_url, sha256,
              is_default)
         VALUES ('php', '7.4.33', 'stable', '/runtimes/php/7.4.33', '2026-08-11T09:00:00Z',
                 1, 'https://example.invalid/php.tar.zst', 'abc', 1)",
    )
    .execute(store.pool())
    .await
    .expect("a row from a build that had never heard of extensions");

    let row = sqlx::query(
        "SELECT extension_dir, extensions_json, extension_choices_json
         FROM runtime_installs WHERE version = '7.4.33'",
    )
    .fetch_one(store.pool())
    .await
    .expect("the row");

    assert_eq!(row.get::<String, _>("extension_dir"), "");
    assert_eq!(row.get::<String, _>("extensions_json"), "{}");
    assert_eq!(row.get::<String, _>("extension_choices_json"), "{}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mixengine-core --lib runtimes:: && cargo test -p mixengine-core --test store`
Expected: FAIL — no such field `extension_dir`, and `no such column: extension_dir`.

- [ ] **Step 3: Write the migration**

Create `crates/mixengine-core/migrations/0005_runtime_extensions.sql`:

```sql
-- What each installed PHP can load, and the extensions the user turned round — roadmap task T28.
--
-- Three columns and two different kinds of fact, which is why they are three and not one.
--
-- `extension_dir` and `extensions_json` are **the artifact's own, copied down at install time**, on
-- 0002's argument: the index is a cache with a six-hour life and a network behind it, and whether
-- `redis` can be enabled for a PHP that is on this disk must not depend on either.
--
-- `extension_choices_json` is the user's, and it holds **deviations rather than a set**:
-- `{"xdebug": true, "mongodb": false}`. The effective set is what the build enables, plus what was
-- turned on, minus what was turned off, intersected with what the build ships as loadable. Storing
-- the resulting list instead would freeze 8.3.33's answer and carry it silently onto 8.3.34 — a
-- reinstall or a patch upgrade is supposed to bring the new build's defaults with it and keep only
-- the extensions somebody deliberately touched.
--
-- `*_json` for 0002's other reason: nothing queries into these. One runtime's whole map is read and
-- looked up in memory.
--
-- The defaults are what make this additive. A row from before these columns describes a runtime
-- whose extensions nobody recorded: no directory, nothing offered, nothing chosen — which is
-- exactly what a listing for it should say, and is repaired by reinstalling that version.
ALTER TABLE runtime_installs ADD COLUMN extension_dir          TEXT NOT NULL DEFAULT '';
ALTER TABLE runtime_installs ADD COLUMN extensions_json        TEXT NOT NULL DEFAULT '{}';
ALTER TABLE runtime_installs ADD COLUMN extension_choices_json TEXT NOT NULL DEFAULT '{}';
```

- [ ] **Step 4: Carry the facts through `Installation`**

In `crates/mixengine-core/src/runtimes.rs`, add to `pub struct Installation`:

```rust
    /// Where this build keeps its loadable extensions, relative to the install directory.
    ///
    /// [`Artifact::extension_dir`](crate::index::Artifact::extension_dir), and [`None`] for a
    /// runtime that can load none — which is every Node, Python and Ruby this build installs, and is
    /// what keeps the whole of [`extensions`] from being a `match` on the kind.
    pub extension_dir: Option<String>,

    /// What this build offers, split by whether it can be turned off.
    ///
    /// Copied down for [`provides`](Self::provides)' reason: the answer has to survive a network
    /// that is not there and an index cache that has expired.
    pub extensions: crate::index::Extensions,
```

In `remember`, beside the existing `provides` serialisation:

```rust
    let extension_dir = installation.extension_dir.clone().unwrap_or_default();
    // The same fallback as `provides` above, and for the same reason: this cannot fail, and an
    // empty object is what a row with nothing recorded already means.
    let extensions =
        serde_json::to_string(&installation.extensions).unwrap_or_else(|_| "{}".to_owned());
```

and extend the insert — the choices column is left to its default, because an install makes no
choices:

```rust
    let inserted = sqlx::query!(
        "INSERT INTO runtime_installs
             (kind, version, channel, install_path, installed_at, size_bytes, source_url, sha256,
              is_default, provides_json, extension_dir, extensions_json)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (kind, version) DO NOTHING",
        kind,
        version,
        channel,
        path,
        installed_at,
        bytes,
        installation.url,
        installation.sha256,
        is_default,
        provides,
        extension_dir,
        extensions
    )
```

- [ ] **Step 5: Fill the new fields in at the one call site**

In `crates/mixengine-daemon/src/runtimes.rs`, inside `perform`'s `runtimes::Installation { … }`,
after `provides: artifact.provides.clone(),`:

```rust
                // The other half of what the index knows and the daemon would otherwise consult
                // once and forget. See migration 0005.
                extension_dir: artifact.extension_dir.clone(),
                extensions: artifact.extensions.clone(),
```

- [ ] **Step 6: Regenerate the offline query data**

Run: `cargo sqlx prepare --workspace -- --all-targets --all-features`
Expected: `.sqlx/` gains or rewrites the entries for the two queries touched above.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p mixengine-core --lib runtimes:: && cargo test -p mixengine-core --test store && cargo check --workspace --all-targets`
Expected: PASS, and the daemon still compiles.

- [ ] **Step 8: Commit**

```bash
git add crates/mixengine-core/migrations/0005_runtime_extensions.sql crates/mixengine-core/src/runtimes.rs crates/mixengine-core/tests/store.rs crates/mixengine-daemon/src/runtimes.rs .sqlx
git commit -m "feat(runtimes): record what an installed build can load (T28)"
```

---

### Task 3: The effective set, and the two refusals

Pure logic over the row: what is loaded, what a listing says about each name, and what happens when
somebody asks for something the build cannot do.

**Files:**
- Create: `crates/mixengine-core/src/runtimes/extensions.rs`
- Modify: `crates/mixengine-core/src/runtimes.rs` (add `pub mod extensions;` under the module doc)
- Modify: `crates/mixengine-core/src/lib.rs` (one new `Error` variant)
- Modify: `crates/mixengine-daemon/src/error.rs` (its wire code)
- Test: `crates/mixengine-core/src/runtimes/extensions.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: Task 2's columns.
- Produces:
  ```rust
  pub struct State { pub kind: RuntimeKind, pub version: PackageVersion, pub install_path: PathBuf,
                     pub directory: Option<String>, pub offered: crate::index::Extensions,
                     pub choices: BTreeMap<String, bool> }
  impl State {
      pub fn loaded(&self) -> Vec<String>;                 // effective shared set, name order
      pub fn listing(&self) -> Vec<Extension>;             // static first, then shared, each by name
      pub fn decide(&self, name: &str, enabled: bool) -> Result<BTreeMap<String, bool>>;
  }
  pub struct Extension { pub name: String, pub linkage: Linkage, pub enabled: bool, pub source: Source }
  pub enum Linkage { Static, Shared }        // derives Ord; Static sorts first
  pub enum Source { BuildDefault, User }
  ```
- Every later task uses exactly these names.

- [ ] **Step 1: Write the failing tests**

Create `crates/mixengine-core/src/runtimes/extensions.rs` holding, for now, only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn state(choices: &str) -> State {
        State {
            kind: RuntimeKind::Php,
            version: PackageVersion::parse("8.3.33").expect("a version"),
            install_path: PathBuf::from("/runtimes/php/8.3.33"),
            directory: Some("lib/php/extensions".to_owned()),
            offered: crate::index::Extensions {
                compiled_in: vec!["opcache".to_owned(), "core".to_owned()],
                shared: vec![
                    "igbinary".to_owned(),
                    "redis".to_owned(),
                    "mongodb".to_owned(),
                    "xdebug".to_owned(),
                ],
                enabled: vec![
                    "igbinary".to_owned(),
                    "redis".to_owned(),
                    "mongodb".to_owned(),
                ],
            },
            choices: serde_json::from_str(choices).expect("choices"),
        }
    }

    /// What the build enables is what is loaded when nobody has said otherwise.
    #[test]
    fn a_build_with_no_choices_loads_what_it_enables() {
        assert_eq!(state("{}").loaded(), ["igbinary", "mongodb", "redis"]);
    }

    /// Deviations in both directions, which is the whole point of storing them rather than a set.
    #[test]
    fn a_choice_turns_one_name_round_and_leaves_the_rest() {
        let loaded = state(r#"{"xdebug": true, "mongodb": false}"#).loaded();

        assert_eq!(loaded, ["igbinary", "redis", "xdebug"]);
    }

    /// A choice about a name this build does not ship loadable cannot smuggle it in.
    #[test]
    fn a_choice_is_intersected_with_what_the_build_ships() {
        assert_eq!(
            state(r#"{"imagick": true}"#).loaded(),
            ["igbinary", "mongodb", "redis"]
        );
    }

    /// A listing says *why* something is on, because the question is asked when the answer is
    /// surprising.
    #[test]
    fn a_listing_says_whether_the_build_or_the_user_decided() {
        let listed = state(r#"{"xdebug": true}"#).listing();
        let of = |name: &str| {
            listed
                .iter()
                .find(|extension| extension.name == name)
                .cloned()
                .unwrap_or_else(|| panic!("{name} is not in the listing"))
        };

        assert_eq!(of("opcache").linkage, Linkage::Static);
        assert!(of("opcache").enabled, "compiled in and therefore always loaded");
        assert_eq!(of("opcache").source, Source::BuildDefault);

        assert_eq!(of("redis").source, Source::BuildDefault);
        assert!(of("redis").enabled);

        assert_eq!(of("xdebug").source, Source::User);
        assert!(of("xdebug").enabled);
    }

    /// Compiled into this build, and a different build is what it would take.
    #[test]
    fn a_compiled_in_extension_cannot_be_turned_off() {
        let error = state("{}")
            .decide("opcache", false)
            .expect_err("a static extension is not disableable");

        assert!(matches!(error, Error::ExtensionCompiledIn { .. }), "{error:?}");
        assert!(error.to_string().contains("opcache"));
    }

    /// A name this build has never heard of is refused rather than written down.
    #[test]
    fn an_unknown_name_is_refused() {
        let error = state("{}")
            .decide("swoole", true)
            .expect_err("a name no cell carries");

        assert!(
            matches!(error, Error::NotFound { kind: "extension", .. }),
            "{error:?}"
        );
    }

    /// Choosing what the build already does is not stored: a deviation that deviates from nothing
    /// would survive the upgrade that changes the default, which is what this model exists to avoid.
    #[test]
    fn a_choice_that_agrees_with_the_build_is_forgotten() {
        let choices = state(r#"{"xdebug": true}"#)
            .decide("xdebug", false)
            .expect("turning it back off is allowed");

        assert!(choices.is_empty(), "{choices:?}");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mixengine-core --lib runtimes::extensions`
Expected: FAIL — the module does not compile; `State`, `Linkage`, `Source`, `decide` are undefined.

- [ ] **Step 3: Add the error variant and its wire code**

In `crates/mixengine-core/src/lib.rs`, beside the other runtime variants:

```rust
    /// An extension that is compiled into this build was asked to be turned off.
    ///
    /// Not a rewritten file that quietly does nothing: `opcache` is static on the Unix cells and a
    /// DLL on Windows, so the same request is answerable on one machine and not on the other, and
    /// what it would take here is a different build rather than a different setting.
    #[error("{name} is compiled into {kind} {version} and cannot be turned off")]
    ExtensionCompiledIn {
        /// Which language.
        kind: RuntimeKind,
        /// Which version.
        version: PackageVersion,
        /// The extension that was asked about.
        name: String,
    },
```

In `crates/mixengine-daemon/src/error.rs`, in the `match` over `Core`:

```rust
            Core::ExtensionCompiledIn { .. } => {
                Error::new(ErrorCode::UnsupportedPlatform, chain(self)).with_hint(
                    "this build has it linked in, so it is always loaded; nothing can unload it \
                     short of a build that ships it as a module",
                )
            }
```

- [ ] **Step 4: Write the module**

Above the test module in `crates/mixengine-core/src/runtimes/extensions.rs`:

```rust
//! What an installed build can load, what it does load, and the file that says so — roadmap task
//! **T28**.
//!
//! Three facts meet here. Two are the artifact's, written down at install time by
//! [`super::remember`]: which of its extensions are linked in, and which are files it could load.
//! The third is the user's, and it is stored as a **deviation** — `{"xdebug": true}` — so that a
//! reinstall or a patch upgrade brings the new build's defaults with it and keeps only what somebody
//! deliberately turned round.
//!
//! # It is not a `match` on the kind
//!
//! Everything here keys off the artifact declaring an extension directory. A Node install declares
//! none, so its state is empty, it renders no documents, and it gets no directory under `etc/`. The
//! day a runtime that is not PHP publishes loadable modules, this needs no edit.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use mixengine_proto::{PackageVersion, RuntimeKind};

use crate::{Error, Result, Store};

/// Whether an extension can be turned off at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Linkage {
    /// Compiled in. Always loaded, and no file switches it on or off.
    Static,
    /// A file inside the install that an ini line loads.
    Shared,
}

/// Why an extension is in the state it is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// This build's own answer.
    BuildDefault,
    /// Somebody said otherwise.
    User,
}

/// One extension, as a listing describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extension {
    /// What it is called, as the index spells it.
    pub name: String,
    /// Whether it can be turned off.
    pub linkage: Linkage,
    /// Whether it is loaded.
    pub enabled: bool,
    /// Whether that is this build's answer or somebody's choice.
    pub source: Source,
}

/// Everything one installed runtime says about its extensions.
#[derive(Debug, Clone)]
pub struct State {
    /// Which language.
    pub kind: RuntimeKind,
    /// Which version.
    pub version: PackageVersion,
    /// Where the runtime is, so a rendered `extension_dir` can be absolute.
    pub install_path: PathBuf,
    /// Where its loadable extensions are inside that directory, when it has any.
    pub directory: Option<String>,
    /// What the artifact published.
    pub offered: crate::index::Extensions,
    /// What somebody turned round, by name.
    pub choices: BTreeMap<String, bool>,
}

impl State {
    /// The shared extensions this runtime loads, in name order.
    ///
    /// `enabled ∪ {chosen on} − {chosen off}`, intersected with `shared` — the intersection being
    /// what stops a choice about a name this build does not ship from producing a line PHP warns
    /// about on every start.
    #[must_use]
    pub fn loaded(&self) -> Vec<String> {
        let shared: BTreeSet<&String> = self.offered.shared.iter().collect();

        let mut loaded: BTreeSet<String> = self
            .offered
            .enabled
            .iter()
            .filter(|name| shared.contains(name))
            .cloned()
            .collect();

        for (name, wanted) in &self.choices {
            if !shared.contains(name) {
                continue;
            }

            if *wanted {
                loaded.insert(name.clone());
            } else {
                loaded.remove(name);
            }
        }

        loaded.into_iter().collect()
    }

    /// Every extension this build has, and what is true of each.
    #[must_use]
    pub fn listing(&self) -> Vec<Extension> {
        let loaded: BTreeSet<String> = self.loaded().into_iter().collect();

        let compiled_in = self.offered.compiled_in.iter().map(|name| Extension {
            name: name.clone(),
            linkage: Linkage::Static,
            enabled: true,
            source: Source::BuildDefault,
        });

        let shared = self.offered.shared.iter().map(|name| Extension {
            name: name.clone(),
            linkage: Linkage::Shared,
            enabled: loaded.contains(name),
            // A choice that agrees with the build is never stored — see `decide` — so a key being
            // present is the whole of the question.
            source: if self.choices.contains_key(name) {
                Source::User
            } else {
                Source::BuildDefault
            },
        });

        let mut listing: Vec<Extension> = compiled_in.chain(shared).collect();
        listing.sort_by(|left, right| {
            left.linkage
                .cmp(&right.linkage)
                .then_with(|| left.name.cmp(&right.name))
        });
        listing
    }

    /// The choices this runtime would have after `name` is turned `enabled`.
    ///
    /// **A choice that agrees with the build is removed rather than written.** A stored deviation
    /// that deviates from nothing would survive the upgrade that changes the default and would then
    /// silently keep the old answer — which is the exact failure storing deviations avoids.
    ///
    /// # Errors
    ///
    /// [`Error::ExtensionCompiledIn`] for a name this build links in, and [`Error::NotFound`] for a
    /// name it has never heard of.
    pub fn decide(&self, name: &str, enabled: bool) -> Result<BTreeMap<String, bool>> {
        if self.offered.compiled_in.iter().any(|linked| linked == name) {
            return Err(Error::ExtensionCompiledIn {
                kind: self.kind,
                version: self.version.clone(),
                name: name.to_owned(),
            });
        }

        if !self.offered.shared.iter().any(|shared| shared == name) {
            return Err(Error::NotFound {
                kind: "extension",
                id: name.to_owned(),
            });
        }

        let mut choices = self.choices.clone();
        let by_default = self.offered.enabled.iter().any(|on| on == name);

        if enabled == by_default {
            choices.remove(name);
        } else {
            choices.insert(name.to_owned(), enabled);
        }

        Ok(choices)
    }
}
```

- [ ] **Step 5: Declare the module**

In `crates/mixengine-core/src/runtimes.rs`, after the module documentation and before the `use`
block:

```rust
pub mod extensions;
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p mixengine-core --lib runtimes::extensions`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/mixengine-core/src/runtimes.rs crates/mixengine-core/src/runtimes/extensions.rs crates/mixengine-core/src/lib.rs crates/mixengine-daemon/src/error.rs
git commit -m "feat(runtimes): merge a build's extensions with the choices made about them (T28)"
```

---

### Task 4: The generated `conf.d`

Rendering the files, installing them through T30's machinery, and sweeping what an earlier state left
behind.

**Files:**
- Modify: `crates/mixengine-core/src/runtimes/extensions.rs`
- Test: the same file's `mod tests`

**Interfaces:**
- Consumes: `State`, `Linkage`, `Source` from Task 3; `crate::generate::{Document, document::install}`.
- Produces:
  ```rust
  pub const SCAN_DIR_ENV: &str = "PHP_INI_SCAN_DIR";
  pub fn conf_d(etc: &Path, kind: RuntimeKind, version: &str) -> PathBuf; // <etc>/<kind>/<version>/conf.d
  impl State { pub fn documents(&self) -> Vec<crate::generate::Document>; }
  pub async fn state(store: &Store, kind: RuntimeKind, version: &PackageVersion) -> Result<State>;
  pub async fn render(paths: &Paths, state: &State) -> Result<bool>;      // true when anything moved
  ```
  `conf_d` takes the version as **text**, because the pool recipe holds it as a `String` and the shim
  holds a `PackageVersion`; one join in one place serves both.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests`:

```rust
    fn rendered(state: &State) -> BTreeMap<String, String> {
        state
            .documents()
            .into_iter()
            .map(|document| {
                (
                    document.relative().display().to_string(),
                    document.contents().to_owned(),
                )
            })
            .collect()
    }

    /// `extension_dir` is absolute, and written even where PHP would find its own — upstream PHP for
    /// Windows bakes an absolute `C:\php\ext` into the binary that would otherwise be consulted by
    /// accident on a machine where that path happens to exist.
    #[test]
    fn the_extension_directory_is_written_as_an_absolute_path() {
        let files = rendered(&state("{}"));
        let mixengine = &files["00-mixengine.ini"];
        let expected = PathBuf::from("/runtimes/php/8.3.33").join("lib/php/extensions");

        assert!(
            mixengine.contains(&format!("extension_dir = \"{}\"", expected.display())),
            "{mixengine}"
        );
    }

    /// The dev-tuned block is MixEngine's opinion, and it is written whole.
    #[test]
    fn a_development_machine_gets_the_settings_it_wants() {
        let files = rendered(&state("{}"));
        let mixengine = &files["00-mixengine.ini"];

        for line in [
            "memory_limit = 512M",
            "upload_max_filesize = 128M",
            "post_max_size = 128M",
            "max_execution_time = 120",
            "display_errors = On",
            "error_reporting = E_ALL",
            "date.timezone = UTC",
            "opcache.enable = 1",
            "opcache.revalidate_freq = 0",
        ] {
            assert!(mixengine.contains(line), "{line} is missing\n{mixengine}");
        }
    }

    /// `conf.d` is scanned in name order, and order is load order.
    #[test]
    fn the_file_names_carry_the_load_order() {
        let files = rendered(&state(r#"{"xdebug": true}"#));
        let names: Vec<&str> = files.keys().map(String::as_str).collect();

        assert_eq!(
            names,
            [
                "00-mixengine.ini",
                "20-igbinary.ini",
                "50-mongodb.ini",
                "50-redis.ini",
                "90-xdebug.ini"
            ],
            "igbinary loads before the redis that links against it, and xdebug wraps everything"
        );
    }

    /// Two names are engine extensions and the rest are not. A `zend_extension` PHP cannot load is a
    /// startup warning rather than a refusal to start, which is why this is asserted here and again
    /// against a real PHP in `crates/mixengine-cli/tests/php_extensions.rs`.
    #[test]
    fn the_two_zend_extensions_are_spelled_as_such() {
        let files = rendered(&state(r#"{"xdebug": true}"#));

        assert!(files["90-xdebug.ini"].contains("zend_extension = xdebug"));
        assert!(files["50-redis.ini"].contains("extension = redis"));
        assert!(
            !files["50-redis.ini"].contains("zend_extension"),
            "an ordinary extension loaded as an engine one is a PHP that will not start"
        );
    }

    /// The same state on two systems, decided by the index rather than by a `cfg`: opcache is
    /// compiled in on the Unix cells and is a DLL on Windows, so one gets a file and the other does
    /// not — while both are told `opcache.enable = 1`, because a static opcache is present and idle
    /// until an ini says otherwise.
    #[test]
    fn opcache_renders_from_what_the_index_says_about_this_artifact() {
        let unix = rendered(&state("{}"));
        assert!(!unix.contains_key("40-opcache.ini"), "compiled in here");
        assert!(unix["00-mixengine.ini"].contains("opcache.enable = 1"));

        let mut windows = state("{}");
        windows.offered.compiled_in = vec!["core".to_owned()];
        windows.offered.shared.push("opcache".to_owned());
        windows.offered.enabled.push("opcache".to_owned());

        let windows = rendered(&windows);
        assert!(windows["40-opcache.ini"].contains("zend_extension = opcache"));
        assert!(windows["00-mixengine.ini"].contains("opcache.enable = 1"));
    }

    /// A runtime that can load nothing renders nothing — which is what keeps Node, Python and Ruby
    /// out of this without anything asking what kind it is.
    #[test]
    fn a_runtime_that_declares_no_extension_directory_renders_nothing() {
        let mut node = state("{}");
        node.directory = None;
        node.offered = crate::index::Extensions::default();

        assert!(node.documents().is_empty());
    }

    /// **The sweep.** `document::install` prunes nothing, so an extension turned off would leave its
    /// file behind and go on being loaded.
    #[tokio::test]
    async fn a_file_left_by_an_earlier_state_is_removed() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let paths = crate::Paths::new(
            home.path().to_path_buf(),
            &crate::config::PathOverrides::default(),
        );

        let on = state(r#"{"xdebug": true}"#);
        assert!(render(&paths, &on).await.expect("a rendering"));

        let directory = conf_d(paths.etc(), RuntimeKind::Php, on.version.as_str());
        assert!(directory.join("90-xdebug.ini").is_file());

        let off = state("{}");
        assert!(
            render(&paths, &off).await.expect("a rendering"),
            "removing a file is a change, and the pool has to hear about it"
        );
        assert!(
            !directory.join("90-xdebug.ini").exists(),
            "xdebug was turned off and its file went on loading it"
        );
        assert!(
            directory.join("50-redis.ini").is_file(),
            "the rest of the set is untouched"
        );

        assert!(
            !render(&paths, &off).await.expect("a rendering"),
            "a second render of the same state changes nothing, or every daemon start reloads pools"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mixengine-core --lib runtimes::extensions`
Expected: FAIL — `documents`, `render`, `conf_d`, `state` are undefined.

- [ ] **Step 3: Write the rendering**

Add to `crates/mixengine-core/src/runtimes/extensions.rs` (and add `use std::path::Path;` and
`use crate::Paths;` to its imports):

```rust
/// The variable both consumers set, and the only way the generated set reaches PHP.
///
/// **There is no `php.ini`.** This was measured to work on all three systems during T32, and a
/// second file is a second place for the truth to live.
pub const SCAN_DIR_ENV: &str = "PHP_INI_SCAN_DIR";

/// The file that carries `extension_dir` and MixEngine's opinion about a development machine.
const MIXENGINE_INI: &str = "00-mixengine.ini";

/// The two names PHP loads as engine extensions rather than as ordinary ones.
///
/// A fact about PHP and not about the index, which is why it is written here beside
/// [`super::smoke_test`] — the same place, and for the same reason, that "which flag prints a
/// version" lives. The value is the bare name on both systems; modern PHP resolves it to
/// `php_<name>.dll` on Windows itself.
const ZEND: [&str; 2] = ["opcache", "xdebug"];

/// What one extension's file is called, which is what decides load order.
///
/// `conf.d` is scanned in name order:
///
/// - `20` `igbinary`, because `redis` links against it when it can find it and silently stores a
///   serialisation nothing else reads when it cannot;
/// - `40` `opcache`, because an optimiser wants to be under whatever wraps it;
/// - `90` `xdebug`, which wants to be outermost and is the one whose presence changes how everything
///   else behaves;
/// - `50` for everything else.
fn prefix(name: &str) -> &'static str {
    match name {
        "igbinary" => "20",
        "opcache" => "40",
        "xdebug" => "90",
        _ => "50",
    }
}

/// Where this runtime's generated ini set lives: `etc/<kind>/<version>/conf.d/`.
///
/// **Under `etc/` and not inside the install**, which is what `.claude/features/runtime-versions.md`
/// said before T28 and what this changes: an install is a rename of a staging directory over the
/// destination, so a generated `conf.d` living inside it is destroyed by reinstalling the same
/// version — and generated configuration is disposable by the project's own rule.
#[must_use]
pub fn conf_d(etc: &Path, kind: RuntimeKind, version: &str) -> PathBuf {
    etc.join(kind.as_str()).join(version).join("conf.d")
}

impl State {
    /// Every file this runtime's ini set is made of, in the order they will be scanned.
    ///
    /// Empty for a runtime that declares no extension directory.
    #[must_use]
    pub fn documents(&self) -> Vec<crate::generate::Document> {
        let Some(directory) = &self.directory else {
            return Vec::new();
        };

        let mut documents = vec![crate::generate::Document::new(
            MIXENGINE_INI,
            self.mixengine_ini(directory),
        )];

        for name in self.loaded() {
            let directive = if ZEND.contains(&name.as_str()) {
                "zend_extension"
            } else {
                "extension"
            };

            documents.push(crate::generate::Document::new(
                format!("{}-{name}.ini", prefix(&name)),
                format!(
                    "; Generated by MixEngine for {} {}. Edits are overwritten.\n\
                     {directive} = {name}\n",
                    self.kind, self.version
                ),
            ));
        }

        documents
    }

    /// `extension_dir`, then the settings a development machine wants instead of PHP's shipping
    /// defaults.
    fn mixengine_ini(&self, directory: &str) -> String {
        // Absolute, and always written — see `conf_d` and the test beside it.
        let absolute = self.install_path.join(directory);

        format!(
            "; Generated by MixEngine for {} {}. Edits are overwritten, and nothing reads this file\n\
             ; back into state.\n\
             extension_dir = \"{}\"\n\
             \n\
             ; A development machine's defaults, which are not PHP's.\n\
             memory_limit = 512M\n\
             upload_max_filesize = 128M\n\
             post_max_size = 128M\n\
             max_execution_time = 120\n\
             display_errors = On\n\
             error_reporting = E_ALL\n\
             date.timezone = UTC\n\
             \n\
             ; Present and idle until an ini says otherwise, whether it is linked in or loaded.\n\
             ; `revalidate_freq = 0` is the difference between opcache in production and opcache on\n\
             ; a laptop: an edited file takes effect on the next request.\n\
             opcache.enable = 1\n\
             opcache.revalidate_freq = 0\n",
            self.kind,
            self.version,
            absolute.display()
        )
    }
}

/// Read one runtime's extension state out of its row.
///
/// # Errors
///
/// [`Error::NotFound`] when that version is not installed, [`Error::UnreadableRuntimeRow`] when
/// either JSON column holds something this build cannot read, and [`Error::Database`] when the table
/// cannot be read.
pub async fn state(store: &Store, kind: RuntimeKind, version: &PackageVersion) -> Result<State> {
    let (kind_column, version_column) = (kind.as_str(), version.as_str());

    let row = sqlx::query!(
        "SELECT install_path, extension_dir, extensions_json, extension_choices_json
         FROM runtime_installs WHERE kind = ? AND version = ?",
        kind_column,
        version_column
    )
    .fetch_optional(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?
    .ok_or_else(|| Error::NotFound {
        kind: "runtime",
        id: format!("{kind} {version}"),
    })?;

    let unreadable = |column: &'static str, value: &str| Error::UnreadableRuntimeRow {
        column,
        value: value.to_owned(),
    };

    let offered: crate::index::Extensions = serde_json::from_str(&row.extensions_json)
        .map_err(|_| unreadable("extensions_json", &row.extensions_json))?;
    let choices: BTreeMap<String, bool> = serde_json::from_str(&row.extension_choices_json)
        .map_err(|_| unreadable("extension_choices_json", &row.extension_choices_json))?;

    Ok(State {
        kind,
        version: version.clone(),
        install_path: PathBuf::from(row.install_path),
        directory: Some(row.extension_dir).filter(|dir| !dir.is_empty()),
        offered,
        choices,
    })
}

/// Put this runtime's ini set on disk, and say whether anything about it moved.
///
/// **The sweep is the half [`crate::generate::document::install`] does not do.** It writes what it is
/// given and prunes nothing, so an extension turned off would leave its file behind and go on being
/// loaded. Anything in the directory that is not one of the documents is removed, which also repairs
/// a directory left by a build that named its files differently.
///
/// # Errors
///
/// [`Error::Io`] naming the file or directory that could not be read, written or removed.
pub async fn render(paths: &Paths, state: &State) -> Result<bool> {
    let directory = conf_d(paths.etc(), state.kind, state.version.as_str());
    let documents = state.documents();

    if documents.is_empty() {
        return Ok(false);
    }

    let written = crate::generate::document::install(&directory, &documents, None).await?;
    let mut changed = written.iter().any(|one| one.changed());

    let ours: BTreeSet<PathBuf> = documents
        .iter()
        .map(|document| document.relative().to_path_buf())
        .collect();

    let unreadable = |source| Error::Io {
        action: "read the generated directory at",
        path: directory.clone(),
        source,
    };

    let mut entries = tokio::fs::read_dir(&directory).await.map_err(unreadable)?;

    while let Some(entry) = entries.next_entry().await.map_err(unreadable)? {
        let name = PathBuf::from(entry.file_name());

        if ours.contains(&name) || name.extension().is_none_or(|kind| kind != "ini") {
            continue;
        }

        tokio::fs::remove_file(entry.path())
            .await
            .map_err(|source| Error::Io {
                action: "remove the generated file at",
                path: entry.path(),
                source,
            })?;

        tracing::debug!(
            file = %entry.path().display(),
            "an extension that is no longer loaded left a file behind"
        );
        changed = true;
    }

    Ok(changed)
}
```

If the closure passed to `map_err` cannot be reused because `Error::Io` moves `directory`, write the
two `map_err`s out longhand rather than cloning the closure — the point is that both name the
directory.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mixengine-core --lib runtimes::extensions`
Expected: PASS.

- [ ] **Step 5: Regenerate the offline query data**

Run: `cargo sqlx prepare --workspace -- --all-targets --all-features`
Expected: one new entry in `.sqlx/` for the `state` query.

- [ ] **Step 6: Commit**

```bash
git add crates/mixengine-core/src/runtimes/extensions.rs .sqlx
git commit -m "feat(runtimes): render an installed PHP's conf.d from its state (T28)"
```

---

### Task 5: Choosing, and the three moments the set is rebuilt

**Files:**
- Modify: `crates/mixengine-core/src/runtimes/extensions.rs`
- Modify: `crates/mixengine-daemon/src/runtimes.rs` (`perform` renders, `uninstall` removes)
- Modify: `crates/mixengine-daemon/src/main.rs` (render at boot, beside `pools::ensure`, ~line 707)
- Test: `crates/mixengine-core/src/runtimes/extensions.rs`

**Interfaces:**
- Consumes: `state`, `render`, `State::decide` from Tasks 3–4.
- Produces:
  ```rust
  pub async fn choose(store: &Store, kind: RuntimeKind, version: &PackageVersion,
                      name: &str, enabled: bool) -> Result<State>;
  pub async fn refresh_all(store: &Store, paths: &Paths) -> Result<Vec<PackageVersion>>;
  pub async fn discard(paths: &Paths, kind: RuntimeKind, version: &PackageVersion) -> Result<()>;
  ```

- [ ] **Step 1: Write the failing tests**

Add to `mod tests`:

```rust
    /// A store with one PHP in it, recorded the way an install records one.
    async fn installed() -> (tempfile::TempDir, Store, crate::Paths) {
        let home = tempfile::tempdir().expect("a temporary directory");
        let paths = crate::Paths::new(
            home.path().to_path_buf(),
            &crate::config::PathOverrides::default(),
        );
        let store = Store::open(paths.database_file()).await.expect("a database");

        super::remember(
            &store,
            &super::Installation {
                kind: RuntimeKind::Php,
                version: PackageVersion::parse("8.3.33").expect("a version"),
                channel: mixengine_proto::PackageChannel::Stable,
                path: PathBuf::from("/runtimes/php/8.3.33"),
                bytes: 1,
                url: "https://example.invalid/php.tar.zst".to_owned(),
                sha256: "00".to_owned(),
                provides: BTreeMap::new(),
                extension_dir: Some("lib/php/extensions".to_owned()),
                extensions: crate::index::Extensions {
                    compiled_in: vec!["opcache".to_owned()],
                    shared: vec!["redis".to_owned(), "xdebug".to_owned()],
                    enabled: vec!["redis".to_owned()],
                },
            },
            mixengine_proto::Timestamp(1_760_000_000_000),
        )
        .await
        .expect("a row");

        (home, store, paths)
    }

    /// A choice is stored as a deviation and read back as one, and both refusals survive the round
    /// trip through the database.
    #[tokio::test]
    async fn a_choice_is_written_down_and_the_refusals_survive_the_round_trip() {
        let (_home, store, _paths) = installed().await;
        let version = PackageVersion::parse("8.3.33").expect("a version");

        let after = choose(&store, RuntimeKind::Php, &version, "xdebug", true)
            .await
            .expect("xdebug is shared here");
        assert_eq!(after.loaded(), ["redis", "xdebug"]);

        let reread = state(&store, RuntimeKind::Php, &version)
            .await
            .expect("the row");
        assert_eq!(reread.choices, BTreeMap::from([("xdebug".to_owned(), true)]));

        let refused = choose(&store, RuntimeKind::Php, &version, "opcache", false)
            .await
            .expect_err("compiled in here");
        assert!(
            matches!(refused, Error::ExtensionCompiledIn { .. }),
            "{refused:?}"
        );

        let unknown = choose(&store, RuntimeKind::Php, &version, "swoole", true)
            .await
            .expect_err("no cell carries it");
        assert!(
            matches!(unknown, Error::NotFound { kind: "extension", .. }),
            "{unknown:?}"
        );
    }

    /// Boot renders every installed runtime, and an uninstall takes the whole version directory.
    #[tokio::test]
    async fn every_installed_runtime_is_rendered_at_boot_and_removed_with_its_directory() {
        let (_home, store, paths) = installed().await;
        let version = PackageVersion::parse("8.3.33").expect("a version");

        let moved = refresh_all(&store, &paths).await.expect("a walk");
        assert_eq!(moved.len(), 1, "{moved:?}");

        let directory = conf_d(paths.etc(), RuntimeKind::Php, version.as_str());
        assert!(directory.join("00-mixengine.ini").is_file());

        discard(&paths, RuntimeKind::Php, &version)
            .await
            .expect("removed");
        assert!(!directory.exists());
        assert!(
            !directory.parent().expect("the version directory").exists(),
            "the version directory goes with it, or `etc/php/` fills with empty shells"
        );

        // Removing what is not there is not a failure: an uninstall of a runtime that never had a
        // set must not fail on its way out.
        discard(&paths, RuntimeKind::Php, &version)
            .await
            .expect("idempotent");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mixengine-core --lib runtimes::extensions`
Expected: FAIL — `choose`, `refresh_all`, `discard` are undefined.

- [ ] **Step 3: Write the three functions**

```rust
/// Turn one extension on or off, and answer with the state that leaves.
///
/// **Validated before anything is written**, which is where the two refusals come from: a request
/// this build cannot satisfy leaves the row exactly as it was rather than producing a file that
/// quietly does nothing.
///
/// # Errors
///
/// [`Error::ExtensionCompiledIn`], [`Error::NotFound`] for an unknown name or an uninstalled
/// version, and [`Error::Database`] when the row cannot be written.
pub async fn choose(
    store: &Store,
    kind: RuntimeKind,
    version: &PackageVersion,
    name: &str,
    enabled: bool,
) -> Result<State> {
    let current = state(store, kind, version).await?;
    let choices = current.decide(name, enabled)?;

    let encoded = serde_json::to_string(&choices).unwrap_or_else(|_| "{}".to_owned());
    let (kind_column, version_column) = (kind.as_str(), version.as_str());

    sqlx::query!(
        "UPDATE runtime_installs SET extension_choices_json = ? WHERE kind = ? AND version = ?",
        encoded,
        kind_column,
        version_column
    )
    .execute(store.pool())
    .await
    .map_err(|source| store.failure("write", source))?;

    tracing::info!(
        kind = kind_column,
        version = version_column,
        name,
        enabled,
        "an extension was turned round"
    );

    Ok(State { choices, ..current })
}

/// Render every installed runtime's ini set, and answer with the versions whose set moved.
///
/// [`crate::shims`]' policy rather than a new one: this is a projection of a table, so it is rebuilt
/// on every daemon start as well as after each change, and a home whose `etc/php/` was deleted is
/// repaired by starting the daemon.
///
/// # Errors
///
/// The first failure that stops a runtime from being rendered — a table that cannot be read, a
/// directory that cannot be written.
pub async fn refresh_all(store: &Store, paths: &Paths) -> Result<Vec<PackageVersion>> {
    let mut moved = Vec::new();

    for summary in super::records(store, None).await? {
        let state = state(store, summary.kind, &summary.version).await?;

        if render(paths, &state).await? {
            moved.push(summary.version);
        }
    }

    Ok(moved)
}

/// Remove a runtime's generated ini set, directory and all.
///
/// Called by `runtime.uninstall`, which already removes a pool's `etc/<service-id>/` — this is the
/// second directory that rule now covers. **Removing what is not there is not a failure**: a runtime
/// installed before this build had none.
///
/// # Errors
///
/// [`Error::Io`] when the directory is there and cannot be removed.
pub async fn discard(paths: &Paths, kind: RuntimeKind, version: &PackageVersion) -> Result<()> {
    // The version's directory rather than its `conf.d`, or `etc/php/` fills with empty shells.
    let directory = paths.etc().join(kind.as_str()).join(version.as_str());

    match tokio::fs::remove_dir_all(&directory).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::Io {
            action: "remove the generated directory at",
            path: directory,
            source,
        }),
    }
}
```

- [ ] **Step 4: Call them at the daemon's three moments**

In `crates/mixengine-daemon/src/runtimes.rs`, in `perform`, **after** the `pools::ensure` block and
before `serde_json::to_value(&summary)`:

```rust
        // The ini set this build can load, rendered before anything runs out of it — reported rather
        // than fatal, on the pool hook's reasoning above: a PHP with no `conf.d` is one the next
        // daemon start gives one to, where an install rolled back for it would be eighty megabytes
        // thrown away over a file.
        match mixengine_core::runtimes::extensions::state(&self.store, kind, version).await {
            Ok(state) => {
                if let Err(error) =
                    mixengine_core::runtimes::extensions::render(&self.paths, &state).await
                {
                    tracing::warn!(
                        %error,
                        "this runtime was installed but its conf.d could not be written"
                    );
                }
            }
            Err(error) => tracing::warn!(
                %error,
                "this runtime was installed but its extensions could not be read back"
            ),
        }
```

In `uninstall`, immediately after `runtimes::discard(Path::new(&removed.path))` succeeds:

```rust
        // The second directory an uninstall owns, beside the pool's `etc/<service-id>/`.
        if let Err(error) = mixengine_core::runtimes::extensions::discard(
            &self.paths,
            target.kind,
            &target.version,
        )
        .await
        {
            tracing::warn!(%error, "the runtime is gone and its generated conf.d could not be removed");
        }
```

In `crates/mixengine-daemon/src/main.rs`, immediately after the `pools::ensure` match:

```rust
    // **And every installed runtime's ini set** — roadmap task T28, on the same policy as `bin/`
    // above: `etc/` is a projection of the database, so it is rebuilt here rather than trusted, and
    // a home whose `etc/php/` was deleted is repaired by starting the daemon. Nothing here fails the
    // start either.
    match mixengine_core::runtimes::extensions::refresh_all(store, paths).await {
        Ok(moved) if moved.is_empty() => {
            tracing::debug!("every installed runtime's conf.d is up to date");
        }
        Ok(moved) => {
            tracing::info!(runtimes = ?moved, "rewrote the generated conf.d of installed runtimes");
        }
        Err(error) => tracing::warn!(%error, "could not rebuild every installed runtime's conf.d"),
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo sqlx prepare --workspace -- --all-targets --all-features && cargo test -p mixengine-core --lib runtimes::extensions && cargo check --workspace --all-targets`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/mixengine-core/src/runtimes/extensions.rs crates/mixengine-daemon/src/runtimes.rs crates/mixengine-daemon/src/main.rs .sqlx
git commit -m "feat(runtimes): rebuild a runtime's conf.d on install, on choice and at boot (T28)"
```

---

### Task 6: Both consumers

`php -m` in a terminal and `phpinfo()` in a browser answer differently unless both are told, and on
Windows the terminal's answer would be a PHP with no `curl`, no `mbstring` and no `intl`.

**Files:**
- Modify: `crates/mixengine-core/src/generate/recipe.rs` (`Context` gains `etc_root`, its accessor, `for_test`)
- Modify: `crates/mixengine-core/src/generate.rs` (fill the new field, ~line 386)
- Modify: `crates/mixengine-core/src/generate/recipes/php_fpm.rs` (module note, both arms)
- Modify: `crates/mixengine-shim/src/main.rs` (`run`, `resolved`, a new `surroundings`)
- Test: `php_fpm.rs`'s `mod tests`, and a `mod tests` in the shim

**Interfaces:**
- Consumes: `runtimes::extensions::{conf_d, SCAN_DIR_ENV}`.
- Produces: `Context::etc_root() -> &Path`; a pool `ServiceSpec` whose environment carries
  `PHP_INI_SCAN_DIR`; `fn surroundings(command: &shims::Command, program: &Path, root: &Path,
  version: &PackageVersion) -> BTreeMap<String, OsString>` in the shim.

- [ ] **Step 1: Write the failing test for the pool**

In `php_fpm.rs`'s `mod tests`:

```rust
    /// **Both SAPIs are told the same thing.** A pool that reads the generated set while `php -m`
    /// does not is two answers to one question — and on Windows the terminal's answer is a PHP with
    /// no `curl`, no `mbstring` and no `intl`, because there those are shared modules that only an
    /// ini switches on.
    ///
    /// Both arms directly, for the reason the socket test gives: the claim is worth checking on the
    /// machine that does not take that branch.
    #[test]
    fn a_pool_reads_the_ini_set_its_runtime_carries() {
        let context = context("{}");

        for builder in [
            PhpFpm::unix(&context).expect("a spec"),
            PhpFpm::windows(&context).expect("a spec"),
        ] {
            let spec = builder.build().expect("a valid spec");
            let scan = spec
                .env()
                .get(mixengine_core::runtimes::extensions::SCAN_DIR_ENV)
                .expect("a pool that is told where its ini set is");

            assert!(
                scan.contains("conf.d"),
                "the pool is pointed somewhere that is not a conf.d: {scan}"
            );
            assert!(
                scan.contains(context.version()),
                "the pool is reading another version's extensions: {scan}"
            );
        }
    }
```

If `ServiceSpec`'s accessor for the environment is not `env()`, use whatever `ServiceSpecBuilder::env`
fills and keep the assertions as they are.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p mixengine-core --lib generate::recipes::php_fpm`
Expected: FAIL — the key is absent (and `Context::etc_root` does not exist yet).

- [ ] **Step 3: Give `Context` the home's `etc/`**

In `crates/mixengine-core/src/generate/recipe.rs`, beside the `etc` field:

```rust
    /// `etc/` itself, for the one thing a recipe needs that is not its own directory.
    ///
    /// A pool has to name the ini set of the *runtime* it runs, which is generated per version and
    /// is not a file this recipe renders — see [`crate::runtimes::extensions`].
    pub(super) etc_root: PathBuf,
```

its accessor:

```rust
    /// `etc/`, the root of everything generated.
    #[must_use]
    pub fn etc_root(&self) -> &Path {
        &self.etc_root
    }
```

in `for_test`: `etc_root: root.join("etc"),`; and in `generate.rs`'s `Context { … }` literal:
`etc_root: self.paths.etc().to_path_buf(),`.

- [ ] **Step 4: Point the pool at it**

In `php_fpm.rs`, replace the module note's "No `php.ini` and no `conf.d`" paragraph with:

```rust
//! **No `php.ini` and no `conf.d` of its own.** What a *pool* renders and what a *runtime's* ini set
//! contains are different files with different owners, and this recipe owns the first. What it does
//! do is name the second: `PHP_INI_SCAN_DIR` is set on both arms, so the pool and the `php` on
//! somebody's terminal load one set — see [`crate::runtimes::extensions`], roadmap task T28.
```

and add to **both** `unix` and `windows` builders:

```rust
            // The runtime's own ini set, which is T28's and not this recipe's. Set identically on
            // both systems, which is why it is written twice rather than in one arm: `php-cgi.exe`
            // reads it exactly as php-fpm does, and a pool that did not would disagree with `php -m`.
            .env(
                crate::runtimes::extensions::SCAN_DIR_ENV,
                crate::runtimes::extensions::conf_d(
                    context.etc_root(),
                    RuntimeKind::Php,
                    context.version(),
                )
                .to_string_lossy()
                .into_owned(),
            )
```

- [ ] **Step 5: Run the pool test to verify it passes**

Run: `cargo test -p mixengine-core --lib generate::recipes::php_fpm`
Expected: PASS.

- [ ] **Step 6: Write the failing test for the shim**

Add a `#[cfg(test)] mod tests` to `crates/mixengine-shim/src/main.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// T25 left this note: "No `PHPRC`, no `GEM_HOME` — the rest are files T28's `conf.d` model
    /// generates, and a variable pointing at a file nothing writes is worse than no variable."
    /// Something writes them now.
    #[test]
    fn a_php_shim_is_told_where_its_ini_set_is() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let root = home.path();
        let version = mixengine_proto::PackageVersion::parse("8.3.33").expect("a version");

        let command = shims::COMMANDS
            .iter()
            .find(|command| command.name == "php")
            .expect("this build fronts php");

        let conf_d = mixengine_core::runtimes::extensions::conf_d(
            &root.join("etc"),
            command.kind,
            version.as_str(),
        );
        std::fs::create_dir_all(&conf_d).expect("a generated set");

        let environment = surroundings(command, &root.join("runtimes/php/8.3.33/bin/php"), root, &version);

        assert!(environment.contains_key("PATH"), "the runtime's own tools still reach each other");
        let scan = environment
            .get(mixengine_core::runtimes::extensions::SCAN_DIR_ENV)
            .expect("a php that is told where its extensions are");
        assert!(
            scan.to_string_lossy().contains("8.3.33"),
            "the shim is pointing at another version's set: {scan:?}"
        );
    }

    /// A runtime with no generated set gets no variable, rather than one pointing at nothing.
    #[test]
    fn a_runtime_with_no_generated_set_is_told_nothing() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let version = mixengine_proto::PackageVersion::parse("20.11.0").expect("a version");

        let command = shims::COMMANDS
            .iter()
            .find(|command| command.name == "node")
            .expect("this build fronts node");

        let environment = surroundings(
            command,
            &home.path().join("runtimes/node/20.11.0/bin/node"),
            home.path(),
            &version,
        );

        assert!(!environment.contains_key(mixengine_core::runtimes::extensions::SCAN_DIR_ENV));
    }
}
```

`tempfile` will need to be a dev-dependency of `mixengine-shim` if it is not one already.

- [ ] **Step 7: Set it in the shim**

Change `resolved` to answer the program **and** the version it resolved
(`Result<(PathBuf, PackageVersion), Refusal>` — `resolved.runtime.version` is already in hand inside
it), and extract the environment into a function:

```rust
/// Everything the fronted program is given beside its own arguments.
///
/// Two variables and no more. `PATH` is what makes a runtime's own tools reach each other, and
/// `PHP_INI_SCAN_DIR` is the generated ini set the pool also reads — the whole point of it being
/// here is that `php -m` in a terminal and `phpinfo()` in a browser answer the same thing.
///
/// **Keyed off the directory existing rather than off the command being `php`**: `runtimes::
/// extensions` renders nothing for a runtime whose artifact declares no extension directory, and a
/// variable pointing at a directory nothing writes is worse than no variable.
fn surroundings(
    command: &shims::Command,
    program: &Path,
    root: &Path,
    version: &PackageVersion,
) -> BTreeMap<String, OsString> {
    let mut environment = BTreeMap::new();

    if let Some(directory) = program.parent() {
        environment.insert("PATH".to_owned(), ahead_of_the_path(directory));
    }

    let etc = Paths::new(root.to_path_buf(), &PathOverrides::default())
        .etc()
        .to_path_buf();
    let conf_d = runtimes::extensions::conf_d(&etc, command.kind, version.as_str());

    if conf_d.is_dir() {
        environment.insert(
            runtimes::extensions::SCAN_DIR_ENV.to_owned(),
            conf_d.into_os_string(),
        );
    }

    environment
}
```

`PackageVersion` has to join `VersionConstraint` in the shim's `use mixengine_proto::{…}` line, and
`tempfile` has to be a dev-dependency of `mixengine-shim` if it is not already one. `resolved`
already computes the root; hand it back alongside the program rather than resolving it
twice. Update the four-step table in the module doc so step 4 says it hands over *with* the ini set.
If `process::hand_over` takes `BTreeMap<String, String>`, keep that type and convert with
`to_string_lossy` — a home path that is not UTF-8 is already unusable for the database this shim
opens.

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p mixengine-shim && cargo test -p mixengine-core --lib generate::`
Expected: PASS.

- [ ] **Step 9: Check the shim's budget did not move**

Run: `cargo build --release -p mixengine-testkit --bin fakeservice && cargo test --release -p mixengine-shim --test overhead -- --ignored --nocapture --test-threads=1`
Expected: PASS — what was added is a `join` and one `is_dir`, against T29's 15 ms. If it regresses,
report the number rather than raising the budget.

- [ ] **Step 10: Commit**

```bash
git add crates/mixengine-core/src/generate.rs crates/mixengine-core/src/generate/recipe.rs crates/mixengine-core/src/generate/recipes/php_fpm.rs crates/mixengine-shim
git commit -m "feat(php): give the pool and the shim one ini set to read (T28)"
```

---

### Task 7: The API

**Files:**
- Modify: `crates/mixengine-proto/src/runtime_api.rs`, `src/rpc.rs`, `src/lib.rs`
- Create: `crates/mixengine-daemon/src/extensions.rs`
- Modify: `crates/mixengine-daemon/src/main.rs`, `api/mod.rs`, `api/rpc.rs`, `services/mod.rs`
- Modify: `crates/mixengine-testkit/src/declare.rs`
- Test: `crates/mixengine-daemon/tests/runtimes.rs`

**Interfaces:**
- Consumes: `runtimes::extensions::{state, choose, render}`; `services::pools::of`; the registry.
- Produces:
  ```rust
  // proto — all derive Debug, Clone, PartialEq, Eq, Serialize, Deserialize
  pub struct ExtensionChoice { #[serde(flatten)] pub runtime: RuntimeTarget, pub name: String, pub enabled: bool }
  pub struct ExtensionList { pub extensions: Vec<RuntimeExtension> }
  pub struct RuntimeExtension { pub name: String, pub linkage: Linkage, pub enabled: bool, pub source: ExtensionSource }
  pub enum Linkage { Static, Shared }                                   // #[serde(rename_all = "snake_case")]
  pub enum ExtensionSource { BuildDefault, User }                       // "build_default" | "user"
  pub struct ExtensionChange { pub extension: RuntimeExtension, pub pool: PoolOutcome }
  pub enum PoolOutcome { Reloaded, RestartRequired, PoolNotRunning }     // "reloaded" | "restart_required" | "pool_not_running"
  pub const RUNTIME_LIST_EXTENSIONS: &str = "runtime.list_extensions";
  pub const RUNTIME_SET_EXTENSION: &str = "runtime.set_extension";
  // daemon
  impl Registry { pub(crate) fn ask_to_reload(&self, id: &ServiceId) -> bool }
  ```
  `runtime.list_extensions` takes the existing `RuntimeTarget`; it is the fourth user of it and the
  module note in `runtime_api.rs` says three, so update that sentence.

- [ ] **Step 1: Add the fixture the test needs**

In `crates/mixengine-testkit/src/declare.rs`, beside `package`, following its exact style (open the
same pool, one statement, `expect` on it):

```rust
/// A PHP recorded as installed, carrying the extension facts an artifact publishes.
///
/// A row rather than an install: what the daemon tests are about is the state model and the wire
/// shape, and eighty megabytes of real PHP is `crates/mixengine-cli/tests/php_extensions.rs`'
/// business. `opcache` is static here and `xdebug` is shipped and off, which is exactly the pair
/// the two refusals and the `source` field are about.
pub async fn runtime_with_extensions(database: &Path, version: &str) {
    let pool = open(database).await;

    sqlx::query(
        "INSERT INTO runtime_installs
             (kind, version, channel, install_path, installed_at, size_bytes, source_url, sha256,
              is_default, provides_json, extension_dir, extensions_json)
         VALUES ('php', ?, 'stable', ?, '2026-08-20T09:00:00Z', 1,
                 'https://example.invalid/php.tar.zst', 'abc', 1, '{\"php\":\"bin/php\"}',
                 'lib/php/extensions',
                 '{\"static\":[\"opcache\"],\"shared\":[\"redis\",\"xdebug\"],\"enabled\":[\"redis\"]}')",
    )
    .bind(version)
    .bind(format!("/runtimes/php/{version}"))
    .execute(&pool)
    .await
    .expect("a runtime row");

    pool.close().await;
}
```

Open the pool exactly as `declare::package` does — copy its first and last lines rather than
inventing a second way in.

- [ ] **Step 2: Write the failing test**

In `crates/mixengine-daemon/tests/runtimes.rs`, in the style of the tests already there:

```rust
/// The two methods, against a runtime the fixture recorded rather than downloaded.
#[tokio::test(flavor = "multi_thread")]
async fn extensions_are_listed_with_a_reason_and_turned_round_one_at_a_time() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    declare::runtime_with_extensions(&home.database_file(), "8.3.33").await;

    let listed = json(&home.mix(&["runtime", "ext", "list", "--php", "8.3.33", "--json"]));
    let of = |name: &str| {
        listed["extensions"]
            .as_array()
            .expect("a list")
            .iter()
            .find(|extension| extension["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("{name} is missing from {listed}"))
    };

    assert_eq!(of("opcache")["linkage"], "static");
    assert_eq!(of("xdebug")["enabled"], false);
    assert_eq!(of("xdebug")["source"], "build_default");

    let changed = json(&home.mix(&[
        "runtime", "ext", "enable", "xdebug", "--php", "8.3.33", "--json",
    ]));
    assert_eq!(changed["extension"]["enabled"], true);
    assert_eq!(changed["extension"]["source"], "user");
    assert_eq!(
        changed["pool"], "pool_not_running",
        "nothing was started, so nothing was reloaded and nothing has to be restarted"
    );

    let refused = home.mix(&[
        "runtime", "ext", "disable", "opcache", "--php", "8.3.33", "--json",
    ]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("compiled into"),
        "a refusal has to say that a different build is what it would take"
    );
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p mixengine-daemon --test runtimes extensions_are_listed`
Expected: FAIL — unknown method, and an unknown CLI subcommand.

- [ ] **Step 4: Write the wire types**

Add the seven items above to `crates/mixengine-proto/src/runtime_api.rs`, re-export them from
`lib.rs` beside the other runtime types, and add the two method constants to `rpc.rs` in
`runtime.*` order. Document, from the spec's Part 5:

- why `source` exists — "the question is asked precisely when the answer is surprising, and *on
  because the build says so* and *on because you turned it on* are different answers to why xdebug is
  loaded";
- why `ExtensionChange` carries the pool's fate — so a client prints one honest sentence rather than
  guessing from the operating system it happens to be running on;
- why `PoolNotRunning` is neither a failure nor a reload.

- [ ] **Step 5: Write the daemon module**

Create `crates/mixengine-daemon/src/extensions.rs`:

```rust
//! `runtime.list_extensions` and `runtime.set_extension` — roadmap task **T28**.
//!
//! Its own module rather than two more methods on [`crate::runtimes`] because of what it needs: a
//! toggle rewrites a file a *service* is reading, so this is the one runtime operation that has to
//! reach the registry. `runtimes.rs` is the index, the download and the row, and it holds none.
//!
//! # What a change answers about the pool
//!
//! **The daemon does not restart a pool nobody asked it to restart**; it reports. That is T32's own
//! policy for a changed override:
//!
//! | | Unix | Windows |
//! | --- | --- | --- |
//! | mechanism | `SIGUSR2`, which the pool's spec already carries | none — `php-cgi.exe` reads its ini at startup |
//! | answer | `Reloaded` | `RestartRequired` |
//!
//! A pool that is not running at all is `PoolNotRunning`, which is neither a failure nor a reload: it
//! reads the new set when it is started. The two are told apart by the *spec* and not by a `cfg` —
//! a pool whose recipe gave it no `ReloadBehaviour` is one nothing can hand a configuration to,
//! which is Windows today and is any future recipe that says the same.

pub(crate) struct Extensions {
    paths: Paths,
    store: Store,
    services: Arc<crate::services::Registry>,
}

impl Extensions {
    pub(crate) fn new(
        paths: &Paths,
        store: &Store,
        services: Arc<crate::services::Registry>,
    ) -> Arc<Self> {
        Arc::new(Self {
            paths: paths.clone(),
            store: store.clone(),
            services,
        })
    }

    /// `runtime.list_extensions` — what this build has, and why each is in the state it is in.
    ///
    /// # Errors
    ///
    /// `not_found` when that version is not installed, and the wire error of a row that could not be
    /// read.
    pub(crate) async fn list(&self, target: &RuntimeTarget) -> Result<ExtensionList, Error> {
        let state =
            mixengine_core::runtimes::extensions::state(&self.store, target.kind, &target.version)
                .await
                .map_err(|error| error.to_wire())?;

        Ok(ExtensionList {
            extensions: state.listing().into_iter().map(wire).collect(),
        })
    }

    /// `runtime.set_extension` — turn one round, rewrite the set, and tell the pool.
    ///
    /// # Errors
    ///
    /// `unsupported_platform` for an extension this build compiles in, `not_found` for a name it
    /// does not ship or a version that is not installed, and the wire error of a row or a file that
    /// could not be written.
    pub(crate) async fn set(&self, choice: &ExtensionChoice) -> Result<ExtensionChange, Error> {
        let state = mixengine_core::runtimes::extensions::choose(
            &self.store,
            choice.runtime.kind,
            &choice.runtime.version,
            &choice.name,
            choice.enabled,
        )
        .await
        .map_err(|error| error.to_wire())?;

        let changed = mixengine_core::runtimes::extensions::render(&self.paths, &state)
            .await
            .map_err(|error| error.to_wire())?;

        // A set that did not move asks nothing of the pool: turning on what was already on is not a
        // reload, and `document::install`'s diff is what makes that true rather than a check here.
        let pool = if changed {
            self.tell_the_pool(&choice.runtime).await
        } else {
            PoolOutcome::PoolNotRunning
        };

        let extension = state
            .listing()
            .into_iter()
            .find(|extension| extension.name == choice.name)
            .map(wire)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::Internal,
                    "the extension that was just set is not in its own listing",
                )
            })?;

        Ok(ExtensionChange { extension, pool })
    }

    /// Which of the three things happened to the pool that runs this version.
    async fn tell_the_pool(&self, target: &RuntimeTarget) -> PoolOutcome {
        let pool = mixengine_core::services::pools::of(&self.store, target.kind, &target.version)
            .await;

        let Ok(Some(id)) = pool else {
            // No pool declared for this runtime, or a table that could not be read. Neither is a
            // failure of the toggle — the file is written either way — and both mean nothing is
            // holding the old set.
            return PoolOutcome::PoolNotRunning;
        };

        if !self.services.ask_to_reload(&id) {
            return PoolOutcome::PoolNotRunning;
        }

        // **Decided by the spec and not by a `cfg`.** A pool whose recipe gave it no
        // `ReloadBehaviour` is one nothing can hand a configuration to; that is Windows today and is
        // any future recipe that says the same.
        match self.services.graph().await {
            Ok(graph) => match graph.spec(&id) {
                Some(spec) if spec.reload().is_some() => PoolOutcome::Reloaded,
                _ => PoolOutcome::RestartRequired,
            },

            // Answered as the outcome that tells somebody to do something, rather than as the one
            // that claims something was done.
            Err(error) => {
                tracing::warn!(%id, %error, "could not tell whether this pool can be reloaded");
                PoolOutcome::RestartRequired
            }
        }
    }
}

/// One extension, as the wire spells it.
///
/// Two vocabularies rather than one shared type, on `index::Channel`'s precedent: what the domain
/// calls a linkage and what the API answers are allowed to move apart, and a variant added on either
/// side has to face this `match`.
fn wire(extension: mixengine_core::runtimes::extensions::Extension) -> RuntimeExtension {
    use mixengine_core::runtimes::extensions::{Linkage as Domain, Source as Decided};

    RuntimeExtension {
        name: extension.name,
        linkage: match extension.linkage {
            Domain::Static => Linkage::Static,
            Domain::Shared => Linkage::Shared,
        },
        enabled: extension.enabled,
        source: match extension.source {
            Decided::BuildDefault => ExtensionSource::BuildDefault,
            Decided::User => ExtensionSource::User,
        },
    }
}
```

`ServiceGraph`'s accessor for one spec may not be spelled `spec(&id)` — use whatever it offers (the
registry's `start` already reads specs out of a graph) and keep the two arms as they are.

In `crates/mixengine-daemon/src/services/mod.rs`, beside `hand_over`:

```rust
    /// Ask one running service to re-read its configuration, and say whether there was one to ask.
    ///
    /// [`Registry::hand_over`]'s single-service half, for the one caller whose change is not a file
    /// this registry generated: a runtime's ini set is rewritten by `runtime.set_extension`, and the
    /// pool reading it is supervised here.
    pub(crate) fn ask_to_reload(&self, id: &ServiceId) -> bool {
        let running = lock(&self.running);

        let Some(entry) = running.get(id) else {
            return false;
        };

        entry.asked_to_reload.notify_one();
        true
    }
```

- [ ] **Step 6: Wire it in**

`mod extensions;` in `main.rs`; build it after the registry; add it to `api::Supervision` and to
`Api`; dispatch in `api/rpc.rs` beside the other `runtime.*` arms:

```rust
                rpc::method::RUNTIME_LIST_EXTENSIONS => {
                    let target: RuntimeTarget = decode(request.params)?;
                    encode_result(&api.extensions.list(&target).await.map_err(refused)?)
                }

                rpc::method::RUNTIME_SET_EXTENSION => {
                    let choice: ExtensionChoice = decode(request.params)?;
                    encode_result(&api.extensions.set(&choice).await.map_err(refused)?)
                }
```

- [ ] **Step 7: Run the tests**

Run: `cargo test -p mixengine-daemon`
Expected: the dispatch tests pass; `extensions_are_listed_…` still fails on the missing CLI
subcommand, which Task 8 adds. Say so in the commit body rather than deleting the test.

- [ ] **Step 8: Commit**

```bash
git add crates/mixengine-proto crates/mixengine-daemon crates/mixengine-testkit
git commit -m "feat(api): list a runtime's extensions and turn one round (T28)"
```

---

### Task 8: `mix runtime ext`

**Files:**
- Modify: `crates/mixengine-cli/src/main.rs` (`RuntimeCommand::Ext`, and the `runtime` handler)
- Modify: `crates/mixengine-cli/src/render.rs` (two renderings)
- Test: `crates/mixengine-daemon/tests/runtimes.rs` — Task 7's test now passes end to end

**Interfaces:**
- Consumes: `ExtensionList`, `ExtensionChange`, `RUNTIME_LIST_EXTENSIONS`, `RUNTIME_SET_EXTENSION`.
- Produces: `mix runtime ext list|enable|disable [NAME] [--php <VERSION>] [--json]`.

- [ ] **Step 1: Add the subcommand**

In `crates/mixengine-cli/src/main.rs`, add to `RuntimeCommand`:

```rust
    /// Which extensions an installed build loads.
    ///
    /// Under `runtime` rather than as `mix php ext …`, which is what
    /// `.claude/features/runtime-versions.md` wrote: a per-language command family for one language
    /// is a noun this CLI would then owe every other runtime.
    Ext {
        #[command(subcommand)]
        command: ExtCommand,
    },
```

and beside it:

```rust
#[derive(Debug, Subcommand)]
enum ExtCommand {
    /// List what this build has, and why each is on or off.
    List(WhichPhp),

    /// Load one on every PHP process of this version.
    Enable {
        /// The extension, as the listing spells it.
        name: String,
        #[command(flatten)]
        php: WhichPhp,
    },

    /// Stop loading one.
    Disable {
        /// The extension, as the listing spells it.
        name: String,
        #[command(flatten)]
        php: WhichPhp,
    },
}

/// Which PHP this is about.
#[derive(Debug, clap::Args)]
struct WhichPhp {
    /// The version, exactly as it is installed. Defaults to the one `php` resolves to here.
    #[arg(long = "php", value_name = "VERSION", value_parser = package_version)]
    version: Option<PackageVersion>,
}
```

The default is **not** a choice the CLI makes: with no `--php`, call `runtime.resolve` with
`kind: Php` and this process's working directory, and use the version it answers — the same order the
shim and the GUI get, from the daemon.

- [ ] **Step 2: Render the answers**

In `crates/mixengine-cli/src/render.rs`:

```rust
/// One row per extension: what it is called, whether it can be turned off, and who decided.
pub fn extension_list(list: &ExtensionList) -> String {
    let rows: Vec<[String; 4]> = list
        .extensions
        .iter()
        .map(|extension| {
            [
                extension.name.clone(),
                match extension.linkage {
                    Linkage::Static => "compiled in".to_owned(),
                    Linkage::Shared => "module".to_owned(),
                },
                if extension.enabled { "on" } else { "off" }.to_owned(),
                match extension.source {
                    ExtensionSource::BuildDefault => "this build".to_owned(),
                    ExtensionSource::User => "you".to_owned(),
                },
            ]
        })
        .collect();

    table(["EXTENSION", "KIND", "STATE", "DECIDED BY"], &rows)
}

/// What a toggle did — including what it deliberately did *not* do to the pool.
pub fn extension_change(change: &ExtensionChange) -> String {
    let state = if change.extension.enabled { "enabled" } else { "disabled" };

    let pool = match change.pool {
        PoolOutcome::Reloaded => "its pool re-read its configuration".to_owned(),
        PoolOutcome::RestartRequired => {
            "the running pool is still using the previous set — restart it to pick this up".to_owned()
        }
        PoolOutcome::PoolNotRunning => {
            "its pool is not running and will read this when it starts".to_owned()
        }
    };

    format!("{} {state}; {pool}\n", change.extension.name)
}
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test -p mixengine-daemon --test runtimes && cargo test --workspace`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/mixengine-cli
git commit -m "feat(cli): mix runtime ext list, enable and disable (T28)"
```

---

### Task 9: Against a real PHP, and the record

The claim the whole task is about — a terminal and a browser agree — can only be made against the
program.

**Files:**
- Create: `crates/mixengine-cli/tests/php_extensions.rs`
- Modify: `.github/workflows/ci.yml`, `.github/scripts/test-no-network.sh`
- Modify: `.claude/features/runtime-versions.md`, `.claude/roadmap/phase-2-runtimes.md`, `.claude/roadmap/todo.md`

**Interfaces:**
- Consumes: everything above, plus the `harness` module and `mixengine_testkit::fastcgi::Pool` that
  `crates/mixengine-cli/tests/php_fpm.rs` already uses.

- [ ] **Step 1: Write the suite**

Create `crates/mixengine-cli/tests/php_extensions.rs`, reusing `php_fpm.rs`'s `installed()` shape —
copied rather than shared, as those suites do:

```rust
//! The generated ini set against a **real** PHP — roadmap task **T28**.
//!
//! **Step 3 is why this suite exists.** Everything else is provable in one process and is proved
//! there: the merge, the ordering, the two `zend_extension` names, the sweep. What cannot be proved
//! there is that the set actually reaches PHP — through the shim on a terminal *and* through the
//! pool in a browser — and on Windows that is where it fails first, because `curl`, `mbstring` and
//! `intl` are shared modules there that only an ini switches on.
//!
//! It also settles the two things the design asserted rather than measured: that php-fpm's `SIGUSR2`
//! picks up a *newly enabled* extension, and that `zend_extension = xdebug` spelled as a bare name is
//! a spelling this PHP accepts. Both fail quietly — a `zend_extension` PHP cannot load is a startup
//! warning, not a refusal to start — which is why every assertion below compares **loaded sets**
//! rather than exit codes.
//!
//! **`#[ignore]`d rather than skipped**, for `php_fpm.rs`' reason: a test that quietly returns when
//! it finds no PHP is a green suite that proved nothing on the day the download broke.

/// What `php -m` says **through the shim in `<home>/bin`**, as a set.
///
/// The shim and not the runtime's own binary: what is being proved is that the resolution puts
/// `PHP_INI_SCAN_DIR` in front of the program, and running `runtimes/php/…/bin/php` directly would
/// prove the opposite of what this suite is for.
fn through_the_terminal(home: &Home) -> BTreeSet<String> {
    let php = home
        .path()
        .join("bin")
        .join(format!("php{}", std::env::consts::EXE_SUFFIX));

    let ran = std::process::Command::new(&php)
        .arg("-m")
        .output()
        .unwrap_or_else(|error| panic!("{} did not run: {error}", php.display()));

    assert!(
        ran.status.success(),
        "`php -m` failed through the shim: {}",
        String::from_utf8_lossy(&ran.stderr)
    );

    String::from_utf8_lossy(&ran.stdout)
        .lines()
        .map(str::trim)
        // `php -m` prints `[PHP Modules]` and `[Zend Modules]` as headings.
        .filter(|line| !line.is_empty() && !line.starts_with('['))
        .map(str::to_lowercase)
        .collect()
}

/// What `get_loaded_extensions()` says **through the pool**, as a set.
fn through_the_pool(listen: &Pool, script: &Path) -> BTreeSet<String> {
    let answered = listen
        .get(script)
        .expect("the pool answered a FastCGI request");

    answered
        .body
        .rsplit('\n')
        .find(|line| line.contains(','))
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The assertion this suite exists to make, phrased so a failure names the extension and the side.
fn agree(terminal: &BTreeSet<String>, pool: &BTreeSet<String>, when: &str) {
    let only_terminal: Vec<&String> = terminal.difference(pool).collect();
    let only_pool: Vec<&String> = pool.difference(terminal).collect();

    assert!(
        only_terminal.is_empty() && only_pool.is_empty(),
        "the terminal and the pool disagree {when}\n  only `php -m`: {only_terminal:?}\n  \
         only the pool: {only_pool:?}"
    );
    assert!(!terminal.is_empty(), "neither side loaded anything at all {when}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real PHP — see the module note, and the `php` step in ci.yml"]
async fn one_ini_set_reaches_the_terminal_and_the_pool_and_moves_when_it_is_told_to() {
    // 1. install a PHP; `mix runtime ext list` reports xdebug present, `module`, off, `this build`
    // 2. `etc/php/<version>/conf.d/00-mixengine.ini` exists and the pool starts against it
    // 3. the two sets above are **equal** — asserted as a symmetric difference so a failure names
    //    the extension and the side it is missing from
    // 4. `mix runtime ext enable xdebug`: both sets contain it, and the pool answered a request
    //    across the change rather than dying into it. On Unix the pid is unchanged and the answer is
    //    `reloaded`; on Windows the answer is `restart_required` and the suite restarts the pool
    //    itself before asserting the new set.
    // 5. `mix runtime ext disable xdebug`: both agree once more, and `90-xdebug.ini` is gone
    // 6. disabling something the listing calls `compiled in` is refused, and enabling a name this
    //    build has never heard of is refused
    // 7. `mix runtime uninstall php <version>` takes `etc/php/<version>/` with it
}
```

The pool side runs a script the suite writes:

```php
<?php echo implode(",", get_loaded_extensions()), "\n";
```

Normalise both sides to a lowercased `BTreeSet<String>` before comparing, and assert on the
difference rather than on equality of strings.

- [ ] **Step 2: Run it against a real PHP**

Run, with `MIXENGINE_PHP_RUNTIME` pointing at an unpacked PHP:
`cargo test -p mixengine-cli --test php_extensions -- --ignored --nocapture`
Expected: PASS.

Two failures are anticipated by the design, and each has one prepared answer:

- **step 4 fails on Unix** — php-fpm's `SIGUSR2` does not pick up a newly enabled extension. Change
  `tell_the_pool` to answer `RestartRequired` on that path, record the measurement in the module note
  and in the spec, and keep the assertion. The design said this was one arm of one match.
- **xdebug is not loaded on Windows** — the bare `zend_extension` name is not the spelling. Give
  `ZEND` a per-platform spelling (`php_xdebug.dll`) and nothing else changes.

- [ ] **Step 3: Run it in CI's shape**

Add to `.github/workflows/ci.yml`, after the "Test against a real PHP" step:

```yaml
      # And the ini set that PHP reads — T28. Its own step for the reason the one above has: a
      # failure should name which half failed without anybody reading the log.
      - name: Test the generated PHP ini set
        if: runner.os != 'Linux'
        env:
          CARGO_NET_OFFLINE: "true"
        run: cargo test -p mixengine-cli --test php_extensions --locked --offline -- --ignored
```

and the matching block in `.github/scripts/test-no-network.sh`, inside the same
`MIXENGINE_PHP_RUNTIME` guard style, with the `::warning` fallback the others use.

- [ ] **Step 4: Record the deviations and tick the task**

In `.claude/features/runtime-versions.md`, change the two things this design moved, each with its
reason in a sentence: the ini set lives at `etc/<kind>/<version>/conf.d/` rather than inside the
install (an install is a rename over the destination, and generated config is disposable), and the
command is `mix runtime ext …` rather than `mix php ext …` (a per-language family for one language).

In `.claude/roadmap/phase-2-runtimes.md`, tick T28 and record what it settles: the two measurements
from step 2, and that "prebuilt extension artifacts" turned out to be already inside the archive —
extensions from anywhere else are a `mixengine-packages` task first, and the state model does not
have to change when one arrives.

Update the Phase 2 count in `.claude/roadmap/todo.md` to match.

- [ ] **Step 5: The full gate**

Run:
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps --document-private-items
```
Expected: all clean.

- [ ] **Step 6: Commit**

```bash
git add crates/mixengine-cli/tests/php_extensions.rs .github .claude
git commit -m "test(php): prove the terminal and the pool load one set (T28)"
```
