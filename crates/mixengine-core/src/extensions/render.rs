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

use mixengine_proto::{
    EnvValue, ExtensionId, HealthCheck, HealthProbe, ReadyCheck, ServiceSpec, ServiceSpecBuilder,
};

use super::manifest::{
    ExtensionManifest, HealthProbeTemplate, HealthTemplate, ReadyTemplate, ServiceTemplate,
};
use crate::{Error, Paths, Result};

/// The three placeholders that are not ports, spelled as they appear in a file.
pub const INSTALL_DIR: &str = "{install_dir}";

/// See [`INSTALL_DIR`].
pub const DATA_DIR: &str = "{data_dir}";

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
        let id = manifest.extension.id.as_str();

        Self {
            install_dir: paths.extensions().join(id),
            // **Not under the install directory** — roadmap task **T81**, its design's D13. An
            // uninstall removes `install_dir` whole and keeps this unless somebody asks otherwise,
            // which is a promise a `data` nested inside it could not keep; the alternative, deleting
            // everything under the install directory except one child, is a rule to remember at
            // every future site that removes an extension. Two directories, two lifetimes: one
            // belongs to the version installed and goes with it, the other belongs to the person and
            // outlives every upgrade.
            data_dir: paths.data().join("extensions").join(id),
            ports: manifest.ports.clone(),
            listen: manifest.permissions.network.listen_address(),
        }
    }

    /// What one placeholder stands for, and whether what it stands for is a path.
    ///
    /// The second half is why this returns a pair: a manifest writes `{install_dir}/mailpit` with
    /// the slash every author types, and on a system that spells a path with backslashes the raw
    /// substitution is `C:\\…\\mailpit/mailpit` — which works, and reads like a bug in every
    /// rendering, log line and generated file it lands in. See [`text`].
    fn placeholder(&self, name: &str) -> Option<(String, bool)> {
        match name {
            "install_dir" => Some((self.install_dir.display().to_string(), true)),
            "data_dir" => Some((self.data_dir.display().to_string(), true)),
            "listen" => Some((self.listen.to_string(), false)),
            port => self
                .ports
                .get(port)
                .map(|number| (number.to_string(), false)),
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
    spelled(id, field, template, context, Destination::Field)
}

/// Where a substitution is landing — roadmap task **T81c**, the design's D4.
///
/// Two things a placeholder cannot decide for itself, and both of them are properties of the
/// document rather than of the extension.
///
/// **How a path is spelled.** The same `{install_dir}` is a command-line argument in one field and a
/// directive in a generated `nginx.conf` in another, and nginx does not read a Windows path:
/// `ngx_conf_read_token` treats `\` inside a quoted string as an escape, so
/// `C:\Users\Nguyen Hai Quang` arrives with every separator eaten. nginx accepts `/` on all three
/// systems, which is why `nginx.conf` is rendered through a `replace` filter and why a fragment
/// substituted into it takes the same spelling.
///
/// **Whether a `{…}` that is not ours is a mistake.** In a field whose whole content is MixEngine's
/// vocabulary it is: `{home_dir}` surviving into an argument is a literal brace handed to a program,
/// reported as whatever that program does with it. In a *fragment* it is not, and this is the one
/// place the T80 rule does not hold — a brace is the destination language's own punctuation.
/// `location / { return 404; }` is nginx, `handle { file_server }` is Caddy, and `{host}` is a Caddy
/// placeholder spelled exactly the way ours are. Refusing what we do not recognise would refuse
/// every real fragment; so an unrecognised `{…}` is copied verbatim, and what takes over the job of
/// catching a misspelling is the front end's own parser, run over the fragment before the install is
/// allowed to proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    /// A field whose whole content is this vocabulary: an argument, an environment value, a
    /// `php.ini` value. This system's separator, and an unknown placeholder is refused.
    Field,

    /// A `[[recipe.front_end]]` fragment written for a Caddyfile. This system's separator — a
    /// backtick-quoted Caddyfile token takes a Windows path as it is written.
    Caddyfile,

    /// A `[[recipe.front_end]]` fragment written for an `nginx.conf`. Forward slashes.
    NginxConf,
}

impl Destination {
    /// The character a separator is written as here.
    const fn separator(self) -> char {
        match self {
            Self::Field | Self::Caddyfile => std::path::MAIN_SEPARATOR,
            Self::NginxConf => '/',
        }
    }

    /// Whether a `{…}` this vocabulary does not contain is a mistake or the destination's own.
    const fn unknown_is_a_mistake(self) -> bool {
        matches!(self, Self::Field)
    }

    /// One substituted path value, respelled.
    ///
    /// Applied to the *value* rather than to the whole rendering, and that is the point: a fragment
    /// is arbitrary configuration, and an nginx `location ~ \.php$` is a backslash that means
    /// something. Only the text a placeholder produced is a path.
    fn spell(self, value: String) -> String {
        match self {
            Self::Field | Self::Caddyfile => value,
            Self::NginxConf => value.replace('\\', "/"),
        }
    }
}

impl From<mixengine_proto::FrontEndServer> for Destination {
    fn from(server: mixengine_proto::FrontEndServer) -> Self {
        match server {
            mixengine_proto::FrontEndServer::Caddy => Self::Caddyfile,
            mixengine_proto::FrontEndServer::Nginx => Self::NginxConf,
        }
    }
}

/// [`text`], told where what it renders is going.
fn spelled(
    id: &ExtensionId,
    field: &str,
    template: &str,
    context: &Context,
    destination: Destination,
) -> Result<String> {
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;

    // Whether what is being copied is the tail of a path that began at `{install_dir}` or
    // `{data_dir}`. While it is, a `/` the author typed is written the way the destination writes a
    // separator; whitespace ends the path, so `{install_dir}/mailpit sendmail --addr a/b` converts
    // the first slash and leaves the argument after it alone. On a system whose separator already
    // *is* `/` every branch of this is a copy.
    let mut in_path = false;

    while let Some(open) = rest.find('{') {
        copy(&mut rendered, &rest[..open], &mut in_path, destination);
        let after = &rest[open + 1..];

        let Some(close) = after.find('}') else {
            if destination.unknown_is_a_mistake() {
                return Err(refuse(id, field, "has a `{` with no `}` after it"));
            }

            // An unbalanced brace in a fragment is the author's business and the front end's to
            // complain about. There is nothing left to substitute, so the rest is literal.
            copy(&mut rendered, &rest[open..], &mut in_path, destination);
            return Ok(rendered);
        };

        let name = &after[..close];

        match context.placeholder(name) {
            Some((value, true)) => {
                rendered.push_str(&destination.spell(value));
                in_path = true;
            }
            Some((value, false)) => rendered.push_str(&value),
            None if destination.unknown_is_a_mistake() => {
                return Err(refuse(
                    id,
                    field,
                    &format!(
                        "uses `{{{name}}}`, which is not a placeholder this manifest declares"
                    ),
                ));
            }
            None => {
                // The destination's own, and copied with its braces: `{host}` is Caddy's and
                // `${scheme}` is nginx's, and neither is ours to read. A brace also ends whatever
                // path was being copied — nothing MixEngine handed out continues through one.
                rendered.push('{');
                rendered.push_str(name);
                rendered.push('}');
                in_path = false;
            }
        }

        rest = &after[close + 1..];
    }

    copy(&mut rendered, rest, &mut in_path, destination);

    Ok(rendered)
}

/// Copy literal text, spelling a separator the destination's way while inside a path.
fn copy(rendered: &mut String, literal: &str, in_path: &mut bool, destination: Destination) {
    for character in literal.chars() {
        if character.is_whitespace() {
            *in_path = false;
        }

        match *in_path && character == '/' {
            true => rendered.push(destination.separator()),
            false => rendered.push(character),
        }
    }
}

/// Substitute a path, insisting it grew from one of `allowed` and climbs out of nothing.
///
/// This function is what `filesystem = ["own-data"]` *is* (the T80 design, D4): an extension
/// reaches the paths it was handed because it has no way to write down any other.
///
/// # Errors
///
/// [`Error::ExtensionField`] naming the field, for a path that starts somewhere else or climbs out
/// with `..`.
pub fn rooted(
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

    // **Rebuilt from its components, which normalises the separator.** A manifest writes
    // `{install_dir}/mailpit` with the slash every author types, and on Windows the placeholder
    // renders with backslashes — so the raw substitution is `C:\…\mailpit/mailpit`, which works and
    // reads like a bug in every rendering, log line and generated file it appears in. A *path* is
    // MixEngine's to spell; an argument is not, and `args` is left exactly as it will be passed.
    Ok(path.components().collect())
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

/// Substitute a field that may not name a host: an argument, an environment value, a recipe's own
/// value.
///
/// # Errors
///
/// [`Error::ExtensionField`] naming the field, for a written-out address or an unknown placeholder.
pub fn value(id: &ExtensionId, field: &str, template: &str, context: &Context) -> Result<String> {
    no_address(id, field, template)?;

    text(id, field, template, context)
}

/// Substitute a `[[recipe.front_end]]` fragment — roadmap task **T81c**.
///
/// [`value`] pointed at a configuration language rather than at a field of ours — see
/// [`Destination`] for the two things that changes — and held to the same address rule for the same
/// reason: a fragment lands in the configuration of the one process every site on this machine is
/// reached through, so an author who could write a host there could reach further from the front end
/// than the permission they declared reaches from their own program.
///
/// # Errors
///
/// [`Error::ExtensionField`] naming the field, for a written-out address.
pub fn fragment(
    id: &ExtensionId,
    field: &str,
    template: &str,
    context: &Context,
    destination: Destination,
) -> Result<String> {
    no_address(id, field, template)?;

    spelled(id, field, template, context, destination)
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

    service_builder(manifest, template, context)?
        .build()
        .map_err(|source| Error::ExtensionSpec {
            id: id.as_str().to_owned(),
            source,
        })
}

/// The same, stopping one step short of a finished spec — roadmap task **T81**.
///
/// **A builder is what a [`Recipe`](crate::generate::Recipe) owes**, because the parts of a spec
/// that come from the row rather than from the declaration — the resource limits a machine's owner
/// set — are applied by the generator afterwards. `inspect` wants the finished thing and the
/// supervisor's path wants the half-finished one; building it twice from two renderings would be
/// two answers to what one manifest says.
///
/// # Errors
///
/// As [`service_spec`], minus what building costs.
pub fn service_builder(
    manifest: &ExtensionManifest,
    template: &ServiceTemplate,
    context: &Context,
) -> Result<ServiceSpecBuilder> {
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
        arguments.push(value(id, &field, argument, context)?);
    }

    let mut builder = ServiceSpec::builder(id.service_id().clone(), program)
        .cwd(cwd)
        .args(arguments)
        .ports(manifest.ports.values().copied())
        .ready(ready(id, &template.ready, context)?)
        .restart(template.restart)
        .stop(template.stop.clone());

    for (key, declared) in &template.env {
        let field = format!("service.env.{key}");

        match declared {
            EnvValue::Literal { value: literal } => {
                builder = builder.env(key, value(id, &field, literal, context)?);
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

    Ok(builder)
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
        assert!(spec.cwd().ends_with("mailpit"));
    }

    /// A manifest writes `{install_dir}/mailpit` with the slash every author types, and the
    /// placeholder renders with this system's separator. The rendered *path* uses one of them, not
    /// both — an argument is left as it will be passed, but a path is MixEngine's to spell.
    #[test]
    fn a_rendered_path_uses_one_separator() {
        let spec = render(mixengine_testkit::extension::MAILPIT).expect("renders");

        let mut written = vec![spec.program().display().to_string()];
        written.extend(spec.args().iter().cloned());

        for shown in written {
            // `--listen` and the address beside it carry no separator at all, so they pass this by
            // having nothing to say; what it is really asserting is the two arguments that begin at
            // a placeholder.
            assert!(
                shown
                    .chars()
                    .filter(|character| *character == '/' || *character == '\\')
                    .all(|character| character == std::path::MAIN_SEPARATOR),
                "{shown} mixes separators"
            );
        }
    }

    /// **The data directory is not inside the install directory** — roadmap task **T81**, its
    /// design's D13.
    ///
    /// An uninstall removes `install_dir` whole and keeps what a person accumulated unless they ask
    /// otherwise, and a `data` nested inside it would make that promise unkeepable. The alternative
    /// — deleting everything under the install directory except one child — is a rule somebody has
    /// to remember at every future site that removes an extension.
    #[test]
    fn data_outlives_the_install_it_came_with() {
        let home = tempfile::tempdir().expect("a directory");
        let paths = Paths::new(home.path().to_path_buf(), &PathOverrides::default());
        let file = manifest::read(
            Path::new("probe").join(manifest::FILE_NAME).as_path(),
            mixengine_testkit::extension::MAILPIT,
        )
        .expect("a manifest");

        let context = Context::planned(&paths, &file);

        assert!(
            !context.data_dir.starts_with(&context.install_dir),
            "data at {} sits inside the install at {}",
            context.data_dir.display(),
            context.install_dir.display()
        );
        assert_eq!(
            context.data_dir,
            paths.data().join("extensions").join("mailpit")
        );
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

    /// A context pointing at a home under `root`, for the fragment tests.
    fn probe_context(root: &Path) -> (ExtensionId, Context) {
        let id = ExtensionId::parse("probe").expect("an id");
        let context = Context {
            install_dir: root.join("extensions").join("probe"),
            data_dir: root.join("data"),
            ports: BTreeMap::from([("ui_port".to_owned(), 8025)]),
            listen: IpAddr::V4(Ipv4Addr::LOCALHOST),
        };

        (id, context)
    }

    /// **T81c, D4.** nginx eats a backslash, so a path substituted into an nginx fragment is
    /// forward-slashed whatever this system spells one with; the same path bound for a Caddyfile is
    /// left as written, because a backtick-quoted token takes it as it is.
    #[test]
    fn a_fragment_spells_a_path_the_way_its_server_does() {
        let home = tempfile::tempdir().expect("a directory");
        let (id, context) = probe_context(home.path());

        let nginx = fragment(
            &id,
            "recipe.front_end[0]",
            "root \"{install_dir}/public\";",
            &context,
            Destination::NginxConf,
        )
        .expect("an nginx fragment renders");

        assert!(!nginx.contains('\\'), "{nginx}");

        let caddy = fragment(
            &id,
            "recipe.front_end[1]",
            "root * `{install_dir}/public`",
            &context,
            Destination::Caddyfile,
        )
        .expect("a caddy fragment renders");

        assert!(
            caddy.contains(&format!("probe{}public", std::path::MAIN_SEPARATOR)),
            "{caddy}"
        );
    }

    /// **A brace is the destination language's punctuation** — the T81c design, D4. A block, a
    /// Caddy placeholder and an nginx variable all pass through untouched, and a backslash that is
    /// not part of a substituted path stays the author's: `location ~ \.php$` means something.
    #[test]
    fn a_fragment_leaves_the_destination_s_own_syntax_alone() {
        let home = tempfile::tempdir().expect("a directory");
        let (id, context) = probe_context(home.path());

        let nginx = fragment(
            &id,
            "recipe.front_end[0]",
            "location ~ \\.php$ { return 404; }",
            &context,
            Destination::NginxConf,
        )
        .expect("an nginx fragment renders");

        assert_eq!(nginx, "location ~ \\.php$ { return 404; }");

        let caddy = fragment(
            &id,
            "recipe.front_end[1]",
            "handle { header_up Host {host} }",
            &context,
            Destination::Caddyfile,
        )
        .expect("a caddy fragment renders");

        assert_eq!(caddy, "handle { header_up Host {host} }");
    }

    /// And a field whose whole content *is* this vocabulary keeps the refusal T80 argued for: an
    /// unknown placeholder there is a literal brace handed to a program.
    #[test]
    fn a_field_still_refuses_a_placeholder_it_does_not_know() {
        let home = tempfile::tempdir().expect("a directory");
        let (id, context) = probe_context(home.path());

        assert!(matches!(
            value(&id, "service.args[0]", "{home_dir}/x", &context),
            Err(Error::ExtensionField { .. })
        ));
    }

    /// A fragment lands in the configuration of the process every site is reached through, so it is
    /// held to the address rule an argument is held to.
    #[test]
    fn a_fragment_may_not_write_out_an_address() {
        let home = tempfile::tempdir().expect("a directory");
        let (id, context) = probe_context(home.path());

        assert!(matches!(
            fragment(
                &id,
                "recipe.front_end[0]",
                "proxy_pass http://127.0.0.1:8025;",
                &context,
                Destination::NginxConf,
            ),
            Err(Error::ExtensionField { .. })
        ));
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
