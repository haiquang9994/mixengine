//! Turning a manifest's templates into the thing that would run — roadmap task **T80**.
//!
//! **The manifest never writes an address** (the T80 design, D2). [`text`] substitutes `{listen}`
//! from `permissions.network` and from nothing else, and a host written out anywhere in the file is
//! refused before substitution — including `127.0.0.1`, which is the one an author would write in
//! good faith. So an extension that declared `loopback` has no way to spell any other address:
//! there is no check here that a later feature could forget to consult, because there is nothing to
//! check.
//!
//! **And every path grows from a placeholder** (D4), which is what `filesystem = ["own-data"]`
//! *is*. An extension cannot reach a path it was not handed, because it cannot write one down.
//!
//! The refusals are checked against what the *author* wrote rather than against the rendering,
//! because after substitution `{listen}` has become an address and the two would be
//! indistinguishable.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::{Component, PathBuf};

use mixengine_proto::{EnvValue, ExtensionId, HealthCheck, HealthProbe, ReadyCheck, ServiceSpec};

use super::manifest::{
    ExtensionManifest, HealthProbeTemplate, HealthTemplate, ReadyTemplate, ServiceTemplate,
};
use crate::{Error, Paths, Result};

/// The three placeholders that are not ports, spelled as they appear in a file.
const INSTALL_DIR: &str = "{install_dir}";

/// See [`INSTALL_DIR`].
const DATA_DIR: &str = "{data_dir}";

/// The scheme prefixes a readiness or health url may start with, each followed by `{listen}`.
const URL_PREFIXES: [&str; 2] = ["http://{listen}", "https://{listen}"];

/// Where one extension's placeholders point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    /// `{install_dir}` — where the extension's own files are.
    pub install_dir: PathBuf,

    /// `{data_dir}` — where what it writes goes.
    pub data_dir: PathBuf,

    /// Each port placeholder, by the name `[ports]` gave it.
    pub ports: BTreeMap<String, u16>,

    /// `{listen}`.
    pub listen: IpAddr,
}

impl Context {
    /// The context an install on this machine *would* build.
    ///
    /// **The ports here are what the extension asked for, not what it has.** Allocation is T81's,
    /// through [`Port::Allocate`](crate::services::Port), and nothing is reserved by rendering —
    /// a line that read like a reservation is how somebody concludes a port is held.
    #[must_use]
    pub fn planned(paths: &Paths, manifest: &ExtensionManifest) -> Self {
        let install_dir = paths.extensions().join(manifest.extension.id.as_str());

        Self {
            data_dir: install_dir.join("data"),
            install_dir,
            ports: manifest.ports.clone(),
            listen: manifest.permissions.network.listen_address(),
        }
    }

    /// What one placeholder stands for.
    fn placeholder(&self, name: &str) -> Option<String> {
        match name {
            "install_dir" => Some(self.install_dir.display().to_string()),
            "data_dir" => Some(self.data_dir.display().to_string()),
            "listen" => Some(self.listen.to_string()),
            port => self.ports.get(port).map(u16::to_string),
        }
    }
}

/// Substitute every placeholder in one field, refusing any the vocabulary does not contain.
///
/// # Errors
///
/// [`Error::ExtensionField`] naming the field and the placeholder. An unknown one is refused rather
/// than left standing, because a `{home_dir}` that survived into an argument would be a literal
/// brace handed to a program, which is a bug reported by whatever that program does with it.
pub fn text(id: &ExtensionId, field: &str, template: &str, context: &Context) -> Result<String> {
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(open) = rest.find('{') {
        rendered.push_str(&rest[..open]);
        let after = &rest[open + 1..];

        let Some(close) = after.find('}') else {
            return Err(refuse(id, field, "has a `{` with no `}` after it"));
        };

        let name = &after[..close];
        let Some(value) = context.placeholder(name) else {
            return Err(refuse(
                id,
                field,
                &format!("uses `{{{name}}}`, which is not a placeholder this manifest declares"),
            ));
        };

        rendered.push_str(&value);
        rest = &after[close + 1..];
    }

    rendered.push_str(rest);

    Ok(rendered)
}

/// Substitute a path, insisting it grew from one of `allowed` and climbs out of nothing.
fn rooted(
    id: &ExtensionId,
    field: &str,
    template: &str,
    allowed: &[&str],
    context: &Context,
) -> Result<PathBuf> {
    if !allowed.iter().any(|root| template.starts_with(root)) {
        return Err(refuse(
            id,
            field,
            &format!(
                "must start with {}: an extension reaches the paths it was handed and no others",
                allowed.join(" or ")
            ),
        ));
    }

    let path = PathBuf::from(text(id, field, template, context)?);

    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(refuse(
            id,
            field,
            "climbs out of the directory it starts in with `..`",
        ));
    }

    Ok(path)
}

/// Whether a template writes out a host itself.
///
/// Run on what the author wrote, never on the rendering. Two shapes are looked for: anything that
/// parses as an IP address — on its own, or as the host half of a `host:port` — and the word
/// `localhost`. Both are found inside a longer string, because `http://127.0.0.1:8025/health` is
/// the same address written where a scanner splitting on whitespace would miss it.
fn names_an_address(template: &str) -> bool {
    let hostish =
        |character: char| character.is_ascii_hexdigit() || character == ':' || character == '.';

    let runs = template.split(|character: char| !hostish(character));

    for run in runs {
        if run.parse::<IpAddr>().is_ok() {
            return true;
        }

        if let Some((host, _)) = run.rsplit_once(':')
            && host.parse::<IpAddr>().is_ok()
        {
            return true;
        }
    }

    template
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '-'))
        .any(|word| word.eq_ignore_ascii_case("localhost"))
}

/// Refuse a template that writes out a host.
fn no_address(id: &ExtensionId, field: &str, template: &str) -> Result<()> {
    if names_an_address(template) {
        return Err(refuse(
            id,
            field,
            "writes out an address: an extension declares how far it reaches with \
             `permissions.network`, and `{listen}` is where the address comes from",
        ));
    }

    Ok(())
}

/// Substitute an address, insisting the host half is `{listen}`.
fn address(id: &ExtensionId, field: &str, template: &str, context: &Context) -> Result<String> {
    if !template.starts_with("{listen}:") {
        return Err(refuse(
            id,
            field,
            "must be `{listen}:<port>`: the host is not the manifest's to choose",
        ));
    }

    text(id, field, template, context)
}

/// Substitute a url, insisting its host is `{listen}`.
fn url(id: &ExtensionId, field: &str, template: &str, context: &Context) -> Result<String> {
    if !URL_PREFIXES
        .iter()
        .any(|prefix| template.starts_with(prefix))
    {
        return Err(refuse(
            id,
            field,
            "must begin `http://{listen}` or `https://{listen}`: the host is not the manifest's \
             to choose",
        ));
    }

    text(id, field, template, context)
}

/// Render a `[service]` into the spec that would run.
///
/// # Errors
///
/// [`Error::ExtensionField`] for anything the format refuses — an address written out, a path that
/// does not grow from a placeholder, an unknown placeholder — and [`Error::ExtensionSpec`] where
/// the rendering is a spec [`ServiceSpec`] will not accept, reported against the extension rather
/// than against a spec nobody wrote. The rules above are stricter than most of what `validate`
/// checks, so what reaches that second error is a `restart` policy the manifest is free to state
/// and the supervisor is not free to run.
pub fn service_spec(
    manifest: &ExtensionManifest,
    template: &ServiceTemplate,
    context: &Context,
) -> Result<ServiceSpec> {
    let id = &manifest.extension.id;

    let program = rooted(
        id,
        "service.program",
        &template.program,
        &[INSTALL_DIR],
        context,
    )?;
    let cwd = rooted(
        id,
        "service.cwd",
        &template.cwd,
        &[INSTALL_DIR, DATA_DIR],
        context,
    )?;

    let mut arguments = Vec::with_capacity(template.args.len());
    for (index, argument) in template.args.iter().enumerate() {
        let field = format!("service.args[{index}]");
        no_address(id, &field, argument)?;
        arguments.push(text(id, &field, argument, context)?);
    }

    let mut builder = ServiceSpec::builder(id.service_id().clone(), program)
        .cwd(cwd)
        .args(arguments)
        .ports(manifest.ports.values().copied())
        .ready(ready(id, &template.ready, context)?)
        .restart(template.restart)
        .stop(template.stop.clone());

    for (key, value) in &template.env {
        let field = format!("service.env.{key}");

        match value {
            EnvValue::Literal { value } => {
                no_address(id, &field, value)?;
                builder = builder.env(key, text(id, &field, value, context)?);
            }
            EnvValue::Keyring {
                service,
                key: entry,
            } => {
                builder = builder.env_from_keyring(key, service, entry);
            }
        }
    }

    if let Some(check) = &template.health {
        builder = builder.health(health(id, check, context)?);
    }

    if let Some(reload) = &template.reload {
        builder = builder.reload(reload.clone());
    }

    builder.build().map_err(|source| Error::ExtensionSpec {
        id: id.as_str().to_owned(),
        source,
    })
}

/// Render `ready`.
fn ready(id: &ExtensionId, template: &ReadyTemplate, context: &Context) -> Result<ReadyCheck> {
    Ok(match template {
        ReadyTemplate::Tcp { addr, timeout } => ReadyCheck::Tcp {
            addr: socket(id, "service.ready.addr", addr, context)?,
            timeout: *timeout,
        },
        ReadyTemplate::UnixSocket { path, timeout } => ReadyCheck::UnixSocket {
            path: rooted(
                id,
                "service.ready.path",
                path,
                &[INSTALL_DIR, DATA_DIR],
                context,
            )?,
            timeout: *timeout,
        },
        ReadyTemplate::Http {
            url: template,
            expect_status,
            timeout,
        } => ReadyCheck::Http {
            url: url(id, "service.ready.url", template, context)?,
            expect_status: *expect_status,
            timeout: *timeout,
        },
        ReadyTemplate::LogPattern { regex, timeout } => ReadyCheck::LogPattern {
            regex: regex.clone(),
            timeout: *timeout,
        },
        ReadyTemplate::PidAlive { settle } => ReadyCheck::PidAlive { settle: *settle },
    })
}

/// Render `health`.
fn health(id: &ExtensionId, template: &HealthTemplate, context: &Context) -> Result<HealthCheck> {
    let probe = match &template.probe {
        HealthProbeTemplate::Tcp { addr } => HealthProbe::Tcp {
            addr: socket(id, "service.health.probe.addr", addr, context)?,
        },
        HealthProbeTemplate::UnixSocket { path } => HealthProbe::UnixSocket {
            path: rooted(
                id,
                "service.health.probe.path",
                path,
                &[INSTALL_DIR, DATA_DIR],
                context,
            )?,
        },
        HealthProbeTemplate::Http {
            url: template,
            expect_status,
        } => HealthProbe::Http {
            url: url(id, "service.health.probe.url", template, context)?,
            expect_status: *expect_status,
        },
    };

    Ok(HealthCheck {
        probe,
        interval: template.interval,
        timeout: template.timeout,
        failures_before_degraded: template.failures_before_degraded,
        successes_before_running: template.successes_before_running,
    })
}

/// Render a `{listen}:<port>` into the address it becomes.
fn socket(
    id: &ExtensionId,
    field: &str,
    template: &str,
    context: &Context,
) -> Result<std::net::SocketAddr> {
    let rendered = address(id, field, template, context)?;

    rendered.parse().map_err(|_| {
        refuse(
            id,
            field,
            &format!("renders to `{rendered}`, which is not an address and a port"),
        )
    })
}

/// One refusal, phrased for whoever wrote the file.
fn refuse(id: &ExtensionId, field: &str, reason: &str) -> Error {
    Error::ExtensionField {
        id: id.as_str().to_owned(),
        field: field.to_owned(),
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::path::Path;

    use super::*;
    use crate::config::PathOverrides;
    use crate::extensions::manifest::{self, Body};

    /// The whole of D2, proved by reading what came out rather than by trusting what went in —
    /// T77's method for "never data, credentials or absolute paths".
    #[test]
    fn a_loopback_extension_renders_no_other_address() {
        let spec = render(mixengine_testkit::extension::MAILPIT).expect("renders");

        let mut written = spec.args().join(" ");
        if let ReadyCheck::Tcp { addr, .. } = spec.ready() {
            written.push(' ');
            written.push_str(&addr.to_string());
        }

        for token in written.split_whitespace() {
            let host = token.rsplit_once(':').map_or(token, |(host, _)| host);
            if let Ok(address) = host.parse::<IpAddr>() {
                assert_eq!(
                    address,
                    IpAddr::V4(Ipv4Addr::LOCALHOST),
                    "a loopback extension rendered {address}"
                );
            }
        }
    }

    /// Ports come out of `[ports]` and paths out of the context, so the rendered spec is what
    /// would run rather than a description of it.
    #[test]
    fn the_rendered_spec_is_what_would_run() {
        let spec = render(mixengine_testkit::extension::MAILPIT).expect("renders");

        assert_eq!(spec.id().as_str(), "mailpit");
        assert!(spec.program().is_absolute());
        assert!(spec.args().contains(&"127.0.0.1:8025".to_owned()));
        assert!(spec.args().contains(&"127.0.0.1:1025".to_owned()));
        assert!(spec.cwd().ends_with("data"));
    }

    /// `lan` is the other rendering, and there is no third.
    #[test]
    fn lan_renders_every_interface() {
        let text = mixengine_testkit::extension::MAILPIT
            .replace("network = \"loopback\"", "network = \"lan\"");

        let spec = render(&text).expect("renders");

        assert!(spec.args().contains(&"0.0.0.0:8025".to_owned()));
    }

    /// **The refusal table.** Every row is a way of writing something the manifest may not say,
    /// and every one is refused with the field named.
    ///
    /// Each row is a whole `[service]` table rather than one line added to a default, because half
    /// of them replace `program` or `cwd` — appended, those would be a duplicate TOML key and the
    /// test would pass on the wrong refusal.
    #[test]
    fn the_manifest_may_not_say_these() {
        let rows: [(&str, &str); 9] = [
            (
                "a written-out loopback address",
                "args = [\"--listen\", \"127.0.0.1:{ui_port}\"]",
            ),
            (
                "every interface, written out",
                "args = [\"--listen\", \"0.0.0.0:{ui_port}\"]",
            ),
            (
                "a host name",
                "args = [\"--listen\", \"localhost:{ui_port}\"]",
            ),
            (
                "an IPv6 address",
                "args = [\"--listen\", \"[::1]:{ui_port}\"]",
            ),
            (
                "an address hidden in an environment value",
                "[service.env]\nBIND = \"127.0.0.1\"",
            ),
            ("an absolute program", "program = \"/usr/local/bin/x\""),
            ("a relative program", "program = \"x\""),
            ("a path climbing out", "cwd = \"{data_dir}/../../etc\""),
            (
                "an unknown placeholder",
                "args = [\"--home\", \"{home_dir}\"]",
            ),
        ];

        for (what, extra) in rows {
            let outcome = render(&probe(extra));

            assert!(
                matches!(outcome, Err(Error::ExtensionField { .. })),
                "{what} should be refused, got {outcome:?}"
            );
        }
    }

    /// A url is the same escape by another door.
    #[test]
    fn a_ready_url_may_not_name_a_host() {
        let outcome = render(&probe(
            "ready = { type = \"http\", url = \"http://127.0.0.1:{ui_port}/health\", expect_status = 200, timeout = \"10s\" }",
        ));

        assert!(matches!(outcome, Err(Error::ExtensionField { .. })));
    }

    /// And `{listen}` in a url is how the same check is written correctly.
    #[test]
    fn a_ready_url_may_carry_the_placeholder() {
        let outcome = render(&probe(
            "ready = { type = \"http\", url = \"http://{listen}:{ui_port}/health\", expect_status = 200, timeout = \"10s\" }",
        ));

        assert!(outcome.is_ok(), "{outcome:?}");
    }

    /// A manifest whose rendering is not a usable spec is reported against the extension, not
    /// against a spec nobody wrote. `restart` is the one field an author may state that the
    /// supervisor will refuse — every other rule here is stricter than `validate`'s.
    #[test]
    fn an_unusable_rendering_names_the_extension() {
        let outcome = render(&probe(
            "restart = { type = \"on_failure\", max_retries = 0, window = \"5m\", backoff = { initial = \"1s\", max = \"30s\", multiplier_percent = 200, jitter_percent = 10 } }",
        ));

        assert!(
            matches!(outcome, Err(Error::ExtensionSpec { .. })),
            "{outcome:?}"
        );
    }

    /// A whole manifest for a `service` extension, whose `[service]` table is `extra` plus
    /// whichever of `program`, `cwd` and `ready` `extra` did not itself supply.
    fn probe(extra: &str) -> String {
        let mut body = String::new();

        for (key, line) in [
            ("program", "program = \"{install_dir}/x\""),
            ("cwd", "cwd = \"{data_dir}\""),
            ("ready", "ready = { type = \"pid_alive\", settle = \"1s\" }"),
            ("restart", ""),
        ] {
            if !line.is_empty() && !extra.starts_with(key) {
                body.push_str(line);
                body.push('\n');
            }
        }

        format!(
            "schema = 1\n\n[extension]\nid = \"probe\"\nname = \"Probe\"\nversion = \"1.0.0\"\nkind = \"service\"\n\n[ports]\nui_port = 8025\n\n[service]\n{body}{extra}\n"
        )
    }

    /// Read a manifest and render its service against a throwaway home.
    fn render(text: &str) -> Result<ServiceSpec> {
        let home = tempfile::tempdir().expect("a directory");
        let paths = Paths::new(home.path().to_path_buf(), &PathOverrides::default());

        let file = manifest::read(Path::new("probe").join(manifest::FILE_NAME).as_path(), text)?;
        let Body::Service(template) = &file.body else {
            panic!("a service");
        };
        let context = Context::planned(&paths, &file);

        service_spec(&file, template, &context)
    }
}
