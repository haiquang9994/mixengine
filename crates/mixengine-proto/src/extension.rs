//! What an `extension.toml` says about itself, as every client renders it — roadmap task **T80**.
//!
//! **Not to be confused with [`crate::runtime_api`]'s `Extension*` types**, which are about a *PHP*
//! extension being switched on for one installed runtime. These are MixEngine's own extensions:
//! Mailpit, phpMyAdmin, MixDB. The two vocabularies never meet, and the older one keeps its names
//! because renaming four public types serves nothing this task is for (the T80 design, D11).
//!
//! The manifest itself lives in `mixengine-core::extensions::manifest`, for T77's reason: what
//! travels on the wire is the *answer*, and what parses a file belongs beside the file.
//!
//! **None of these enums is `#[non_exhaustive]`, deliberately.** Every other enum in this crate is,
//! because the supervisor's vocabulary grows with the phases; these four are the whole of what
//! `schema = 1` means, so a fifth [`ExtensionKind`] is a *format* change. Making one is meant to
//! break every place that decides something from a kind — [`ExtensionKind`] chooses which tables a
//! manifest may carry, and a `_` arm there is where a new kind would slip through with no table
//! rule of its own.

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr};

use crate::service::{ServiceId, SpecError};

/// An extension's identity: `mailpit`, `phpmyadmin`, `mixdb`.
///
/// **A [`ServiceId`] with no instance.** It names a directory — `extensions/<id>/` — so every rule
/// a service id carries about directory names applies unchanged, down to the names Windows refuses;
/// a second charset check written out here would be a second place for `con` to slip through. And a
/// `service` extension's supervised process is named by this id, so holding the parsed [`ServiceId`]
/// means that conversion can never fail later, at a point where failing would be a surprise.
///
/// `@` is refused on top: `mariadb@main` is one server among several, and there is one Mailpit.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct ExtensionId(ServiceId);

impl ExtensionId {
    /// Parse an id.
    ///
    /// # Errors
    ///
    /// [`SpecError::ServiceId`] naming what is wrong with the value, phrased for whoever typed it.
    pub fn parse(value: impl Into<String>) -> Result<Self, SpecError> {
        let value = value.into();

        if value.contains('@') {
            return Err(SpecError::ServiceId {
                value,
                reason: "an extension has no instances, so it has no `@`".to_owned(),
            });
        }

        ServiceId::parse(value).map(Self)
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// The service this extension's process would be called, where it has one.
    #[must_use]
    pub const fn service_id(&self) -> &ServiceId {
        &self.0
    }
}

impl std::fmt::Display for ExtensionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for ExtensionId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;

        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// What an extension *is*, which decides which tables its manifest may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExtensionKind {
    /// A binary MixEngine supervises. Mailpit.
    Service,

    /// Source served by our own stack on an internal domain. phpMyAdmin, Adminer.
    WebApp,

    /// An application we neither run nor bundle — we find it and hand it something. MixDB.
    DesktopApp,

    /// Configuration only, merged into what MixEngine generates.
    ///
    /// **`[recipe]` is not exclusive to this kind** (the T80 design, D7): Mailpit is a service
    /// *and* a `sendmail_path` recipe, and two extensions for one product would be two things to
    /// install and uninstall in step. This kind means an extension that is *only* that.
    Recipe,
}

impl ExtensionKind {
    /// The word the manifest spells it with.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Service => "service",
            Self::WebApp => "web-app",
            Self::DesktopApp => "desktop-app",
            Self::Recipe => "recipe",
        }
    }
}

impl std::fmt::Display for ExtensionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How far an extension's own listeners may be reached from.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum NetworkReach {
    /// This machine only. The default, because a missing `[permissions]` table is silence, and
    /// silence is not consent.
    #[default]
    Loopback,

    /// Anything that can route to this machine.
    Lan,
}

impl NetworkReach {
    /// The address `{listen}` renders to.
    ///
    /// **This function is the enforcement** (the T80 design, D2). A manifest cannot write an
    /// address at all, so an extension that declared [`Loopback`](Self::Loopback) has no way to
    /// spell one that is not this: there is no check anywhere that a later feature could forget to
    /// consult, because there is nothing to check.
    ///
    /// [`Lan`](Self::Lan) is `0.0.0.0` rather than the machine's current LAN address, because a
    /// specific address changes with the network and would mean re-rendering the spec and
    /// restarting the process on every DHCP renewal and every wake from sleep — the cost T76
    /// measured for a far cheaper reaction. Which address a person should *type* is a question
    /// about a URL, and T74 already answers it.
    #[must_use]
    pub const fn listen_address(self) -> IpAddr {
        match self {
            Self::Loopback => IpAddr::V4(Ipv4Addr::LOCALHOST),
            Self::Lan => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        }
    }
}

/// Which paths an extension says it needs.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum FilesystemReach {
    /// Its own installation and data directories — which is every path the placeholder vocabulary
    /// can produce, and so is the whole of what this permission *is* (the T80 design, D4).
    OwnData,

    /// **Grants nothing today** (the T80 design, D5). No placeholder yields a project path, and an
    /// extension is per-home rather than per-project, so there is no single root to hand it. It is
    /// parsed and displayed as a disclosure; the day it grants something, that is a task with a
    /// consumer rather than a field.
    #[serde(rename = "project-roots:read")]
    ProjectRootsRead,
}

/// What an extension says it would call on the daemon API.
///
/// **A disclosure, not a boundary** — see
/// `.claude/decisions/0014-an-extension-is-not-an-api-client.md`. Nothing is minted and nothing
/// checks it: an extension runs as the user's own account, and the endpoint's access control *is*
/// the account, so a token an extension held is one it could ignore by opening its own connection.
/// What this is for is telling somebody what they are about to install.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ApiAccess {
    /// Reading state.
    Read,

    /// Changing it.
    Write,
}

/// `[permissions]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionPermissions {
    /// What it says it would call. A disclosure — see [`ApiAccess`].
    #[serde(default)]
    pub services: BTreeSet<ApiAccess>,

    /// How far its listeners reach. Enforced, through [`NetworkReach::listen_address`].
    #[serde(default)]
    pub network: NetworkReach,

    /// Which paths it says it needs. `own-data` is enforced by the placeholder vocabulary itself.
    #[serde(default)]
    pub filesystem: BTreeSet<FilesystemReach>,
}

/// Which front end a `[[recipe.front_end]]` fragment is written for — roadmap task **T81c**.
///
/// **It names a configuration language rather than merely selecting a file.** A Caddyfile and an
/// `nginx.conf` are two syntaxes, and they spell a path differently as well: `ngx_conf_read_token`
/// treats a backslash inside a quoted string as an escape, so `mixengine-core` forward-slashes a
/// path it substitutes into an nginx fragment and leaves one bound for a Caddyfile as this system
/// spells it. That choice is made from this value and from nothing else.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum FrontEndServer {
    /// Caddy. The fragment is Caddyfile syntax, at the top level of the generated `Caddyfile`.
    Caddy,

    /// nginx. The fragment is `nginx.conf` syntax, inside that file's `http` block.
    Nginx,
}

impl FrontEndServer {
    /// The `packages.name` whose recipe renders a fragment written for this server.
    ///
    /// **The join between a manifest and a recipe**, and the reason it is here rather than in
    /// `mixengine-core`: the two names are the same fact, and a `match` written at the one place
    /// that needs it is a second list to keep in step.
    #[must_use]
    pub const fn package(self) -> &'static str {
        match self {
            Self::Caddy => "caddy",
            Self::Nginx => "nginx",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An extension id is a directory name, so it inherits every rule a service id has —
    /// including the Windows reserved names, which is the case a hand-written charset check
    /// would have missed.
    #[test]
    fn an_id_is_a_directory_name() {
        for good in ["mailpit", "phpmyadmin", "mixdb", "adminer", "php-my-admin"] {
            assert!(ExtensionId::parse(good).is_ok(), "{good} should parse");
        }

        for bad in [
            "", "Mailpit", "con", "mail pit", "mail/pit", "mailpit.", "-mailpit",
        ] {
            assert!(ExtensionId::parse(bad).is_err(), "{bad} should not parse");
        }
    }

    /// **An extension has no instances.** `mariadb@main` is one server among several; there is
    /// one Mailpit. Allowing `@` would also make `extensions/<id>/` a directory name carrying a
    /// separator that means something everywhere else.
    #[test]
    fn an_id_has_no_instance() {
        assert!(ExtensionId::parse("mailpit@second").is_err());
    }

    /// The id is the service id, so a `service` extension can never fail to name its service.
    #[test]
    fn an_id_is_already_a_service_id() {
        let id = ExtensionId::parse("mailpit").expect("parses");

        assert_eq!(id.service_id().as_str(), "mailpit");
    }

    /// This function is the whole of D2's enforcement: an address exists only as an answer to
    /// the declared reach.
    #[test]
    fn the_reach_decides_the_address() {
        assert_eq!(
            NetworkReach::Loopback.listen_address(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        assert_eq!(
            NetworkReach::Lan.listen_address(),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
    }

    /// A manifest that says nothing about permissions has asked for nothing and reaches
    /// loopback only. The default is the narrow end, because a missing table is silence.
    #[test]
    fn permissions_default_to_the_narrow_end() {
        let permissions: ExtensionPermissions = serde_json::from_str("{}").expect("an empty table");

        assert_eq!(permissions.network, NetworkReach::Loopback);
        assert!(permissions.services.is_empty());
        assert!(permissions.filesystem.is_empty());
    }

    /// The spellings are the file's, and a test pins them: `web-app` is not `webapp`, and
    /// `project-roots:read` carries the colon the feature document wrote.
    ///
    /// Read as JSON rather than as TOML because this crate depends on no TOML reader — the
    /// `rename` attributes being pinned are the same ones either format goes through.
    #[test]
    fn the_spellings_are_the_ones_a_person_writes() {
        #[derive(serde::Deserialize)]
        struct Wrapper {
            kind: ExtensionKind,
            permissions: ExtensionPermissions,
        }

        let read: Wrapper = serde_json::from_str(
            r#"{
                "kind": "web-app",
                "permissions": {
                    "services": ["read"],
                    "network": "lan",
                    "filesystem": ["own-data", "project-roots:read"]
                }
            }"#,
        )
        .expect("parses");

        assert_eq!(read.kind, ExtensionKind::WebApp);
        assert_eq!(read.permissions.network, NetworkReach::Lan);
        assert!(read.permissions.services.contains(&ApiAccess::Read));
        assert!(
            read.permissions
                .filesystem
                .contains(&FilesystemReach::ProjectRootsRead)
        );
    }

    /// A misspelled key is a permission somebody believes they granted.
    #[test]
    fn an_unknown_permission_key_is_refused() {
        assert!(serde_json::from_str::<ExtensionPermissions>(r#"{"netwrok":"lan"}"#).is_err());
    }
}
