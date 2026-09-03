//! The one file MixEngine generates for a `web-app` — roadmap task **T82**, the design's D1 and D2.
//!
//! # Why it lives inside the install directory
//!
//! Because the application says so. phpMyAdmin's `libraries/vendor_config.php` fixes
//! `'configFile' => ROOT_PATH . 'config.inc.php'` — measured, not assumed — with no environment
//! variable and no search path, so there is exactly one place the file can go.
//!
//! That is a smaller trespass than it reads. Nothing in this workspace verifies an install
//! directory after the install, so no integrity claim is broken; `extension.uninstall` removes the
//! directory whole, so the generated file has the lifetime it should; and the file is ours, written
//! from state in SQLite and thrown away, which is the rule `etc/` follows rather than an exception
//! to it.
//!
//! **What a person changes lives in `{data_dir}`**, which outlives an uninstall (T81's D13). A
//! manifest ends its text with an `@include` of a file there; nothing here enforces that it does,
//! because a manifest that did not would simply have no user half.
//!
//! # A database that is gone is a skip, not a rewrite
//!
//! `site_service_links.service_id` cascades, so `mix service delete <db> --force` — the one path
//! that crosses the refusal writing that link armed — leaves a `web-app` whose declared engines
//! resolve to nothing. [`of`] answers [`None`] there and the caller warns; the file already on disk
//! is left alone. Rewriting it to point nowhere would make a forced delete worse in silence, and
//! reading the old value back out of it would be parsing a generated file into state.

use std::path::PathBuf;

use super::render::{self, DatabaseEndpoint};
use super::store::Installed;
use crate::extensions::manifest::Body;
use crate::{Error, Result, Store};

/// One `web-app`'s generated configuration, and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// The absolute path, under `[web-app].root`.
    pub path: PathBuf,

    /// What to write there, with every placeholder substituted.
    pub text: String,
}

/// Render an installed `web-app`'s configuration, reading the database it was linked to.
///
/// [`None`] for three things that are all states rather than faults: an extension that is not a
/// `web-app`, one that declares no `[web-app.config]`, and one whose declared database is gone.
///
/// # Errors
///
/// [`Error::ExtensionField`] for a `[web-app].root` or a text this home cannot render, and
/// [`Error::Database`] when the tables cannot be read.
pub async fn of(store: &Store, installed: &Installed, secret: &str) -> Result<Option<Rendered>> {
    let Body::WebApp(app) = &installed.manifest.body else {
        return Ok(None);
    };

    if app.config.is_none() {
        return Ok(None);
    }

    let mut endpoint = None;

    if app.database.is_some() {
        let Some(site) = crate::sites::of_extension(store, &installed.id).await? else {
            // A `web-app` with no site row is one whose install did not finish, which
            // `extension.install` already unwound. Nothing to configure.
            return Ok(None);
        };

        // **The link, and only the link.** Re-resolving `engines` here would quietly re-point an
        // application at a different server than the one it was installed against — the pool is
        // frozen for the same reason (T81b's D5).
        let mut found = None;

        for service in &site.services {
            if let Some(one) = super::database::endpoint(store, service).await? {
                found = Some(one);
                break;
            }
        }

        let Some(one) = found else {
            return Ok(None);
        };

        endpoint = Some(one);
    }

    rendered(installed, endpoint, secret)
}

/// Render it, given everything that had to be read.
///
/// Split from [`of`] so that the substitution can be tested without a home: what goes wrong in a
/// `[web-app.config]` is a placeholder or a path, and neither needs a database to demonstrate.
///
/// # Errors
///
/// [`Error::ExtensionField`] naming the field, for a root that does not grow from `{install_dir}`
/// or a text carrying a placeholder this context cannot answer.
pub fn rendered(
    installed: &Installed,
    database: Option<DatabaseEndpoint>,
    secret: &str,
) -> Result<Option<Rendered>> {
    let Body::WebApp(app) = &installed.manifest.body else {
        return Ok(None);
    };

    let Some(config) = &app.config else {
        return Ok(None);
    };

    let id = &installed.id;
    let mut context = render::Context::installed(installed);
    context.database = database;
    context.secret = Some(secret.to_owned());

    // **The name, and only when the manifest asked for it** — roadmap task **T82a**, the design's
    // D2. Read out of the table the database is declared in, because `engines` and `signs_in` are
    // one statement: this application administers that server, signed in. What is put on the
    // context is a variable's name; the password itself is never in this crate.
    context.password_env = app
        .database
        .as_ref()
        .filter(|declared| declared.signs_in)
        .map(|_| render::CREDENTIAL_ENV.to_owned());

    let root = render::rooted(
        id,
        "web-app.root",
        &app.root,
        &[render::INSTALL_DIR],
        &context,
    )?;

    Ok(Some(Rendered {
        // `config.path` carries no placeholders and was refused at parse if it could leave the
        // root, so joining is all that is left to do — `manifest::check_config`.
        path: root.join(&config.path),
        text: render::php_source(id, "web-app.config.text", &config.text, &context)?,
    }))
}

/// Write one rendering, replacing whatever was there.
///
/// **Through a temporary file and a rename**, so that a half-written `config.inc.php` is never what
/// PHP reads: the application is being served the whole time this runs.
///
/// # Errors
///
/// [`Error::Io`] naming the path, when the directory cannot be made or the file cannot be written.
pub fn write(rendered: &Rendered) -> Result<()> {
    if let Some(parent) = rendered.path.parent() {
        crate::paths::create_dir(parent)?;
    }

    let temporary = rendered.path.with_extension("mixengine-tmp");

    std::fs::write(&temporary, &rendered.text).map_err(|source| Error::Io {
        action: "write",
        path: temporary.clone(),
        source,
    })?;

    std::fs::rename(&temporary, &rendered.path).map_err(|source| Error::Io {
        action: "write",
        path: rendered.path.clone(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::net::{IpAddr, Ipv4Addr};

    use mixengine_proto::{ExtensionId, ServiceId, Timestamp};

    use super::*;
    use crate::extensions::manifest;
    use crate::extensions::store::Source;

    /// The file lands under the root the manifest declared, with every placeholder substituted.
    #[test]
    fn a_configuration_is_rendered_under_the_declared_root() {
        let installed = a_phpmyadmin();

        let rendered = rendered(&installed, Some(a_database()), &"s".repeat(32))
            .expect("it renders")
            .expect("a web-app with a configuration has one");

        assert_eq!(
            rendered.path,
            installed.install_dir.join("pma").join("config.inc.php")
        );
        assert!(rendered.text.contains("'127.0.0.1'"), "{}", rendered.text);
        assert!(rendered.text.contains("'3306'"), "{}", rendered.text);
        assert!(rendered.text.contains("'root'"), "{}", rendered.text);
        assert!(rendered.text.contains(&"s".repeat(32)), "{}", rendered.text);
    }

    /// **The application's own braces survive** — the design's D8, proved on the shape a real
    /// `config.inc.php` has rather than on a template written to suit the renderer.
    #[test]
    fn the_php_around_the_placeholders_is_left_alone() {
        let installed = a_phpmyadmin();

        let rendered = rendered(&installed, Some(a_database()), "x")
            .expect("it renders")
            .expect("a configuration");

        assert!(rendered.text.contains("if (true) {"), "{}", rendered.text);
        assert!(rendered.text.contains("}\n"), "{}", rendered.text);
    }

    /// A kind with no configuration is not an error: most extensions have none.
    #[test]
    fn an_extension_with_no_configuration_renders_nothing() {
        let installed = a_mailpit();

        assert!(
            rendered(&installed, None, "x")
                .expect("no failure")
                .is_none()
        );
    }

    /// Writing replaces what was there and leaves no temporary behind.
    #[test]
    fn writing_replaces_the_file_and_leaves_nothing_beside_it() {
        let home = tempfile::tempdir().expect("a temporary home");
        let path = home.path().join("app").join("config.inc.php");

        for text in ["<?php // first\n", "<?php // second\n"] {
            write(&Rendered {
                path: path.clone(),
                text: text.to_owned(),
            })
            .expect("it writes");

            assert_eq!(std::fs::read_to_string(&path).expect("it is there"), text);
        }

        let beside: Vec<_> = std::fs::read_dir(home.path().join("app"))
            .expect("the directory")
            .filter_map(|entry| Some(entry.ok()?.file_name()))
            .collect();

        assert_eq!(beside, ["config.inc.php"], "no temporary file survives");
    }

    /// The database a rendering is handed.
    fn a_database() -> DatabaseEndpoint {
        DatabaseEndpoint {
            service: ServiceId::parse("mariadb@main").expect("a service id"),
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 3306,
            user: "root".to_owned(),
        }
    }

    /// An installed `web-app` whose configuration exercises every placeholder T82 adds, and whose
    /// PHP has braces of its own.
    fn a_phpmyadmin() -> Installed {
        installed(
            "phpmyadmin",
            "[web-app]\nroot = \"{install_dir}/pma\"\ndomain = \"pma\"\n\n\
             [web-app.database]\nengines = [\"mariadb\"]\n\n\
             [web-app.runtime]\nkind = \"php\"\nrequires = \"^8.0\"\n\n\
             [web-app.config]\npath = \"config.inc.php\"\ntext = \"\"\"\n\
             <?php\n\
             $cfg['blowfish_secret'] = '{secret}';\n\
             $cfg['TempDir'] = '{data_dir}';\n\
             if (true) {\n\
             \x20   $cfg['Servers'][1]['host'] = '{db_host}';\n\
             \x20   $cfg['Servers'][1]['port'] = '{db_port}';\n\
             \x20   $cfg['Servers'][1]['user'] = '{db_user}';\n\
             }\n\
             \"\"\"\n",
            "web-app",
        )
    }

    /// A `service` extension, which has no configuration to write.
    fn a_mailpit() -> Installed {
        installed(
            "mailpit",
            "[service]\nprogram = \"{install_dir}/mailpit\"\ncwd = \"{data_dir}\"\n\
             ready = { type = \"pid_alive\", settle = \"1s\" }\n",
            "service",
        )
    }

    /// A row, built from a manifest the parser accepted.
    fn installed(id: &str, body: &str, kind: &str) -> Installed {
        let text = format!(
            "schema = 1\n\n[extension]\nid = \"{id}\"\nname = \"{id}\"\nversion = \"1.0.0\"\n\
             kind = \"{kind}\"\n\n{body}"
        );

        let manifest = manifest::read(std::path::Path::new("extension.toml"), &text)
            .unwrap_or_else(|error| panic!("{id} parses: {error}"));

        let root = if cfg!(windows) { r"C:\home" } else { "/home" };

        Installed {
            id: ExtensionId::parse(id).expect("an extension id"),
            manifest,
            install_dir: std::path::Path::new(root).join("extensions").join(id),
            data_dir: std::path::Path::new(root).join("data").join(id),
            source: Source::Path,
            signed: false,
            installed_at: Timestamp::parse_rfc3339("2026-09-03T00:00:00Z").expect("a timestamp"),
            ports: BTreeMap::new(),
        }
    }
}
