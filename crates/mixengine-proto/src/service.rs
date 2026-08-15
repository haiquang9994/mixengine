//! What a supervised service *is* — the vocabulary `mixengine-core` writes and
//! `mixengine-supervisor` runs.
//!
//! This lives in `proto` rather than in either of those crates because they are siblings that cannot
//! depend on each other, and because a [`ServiceSpec`] is not a supervisor implementation detail: a
//! row in `services` stores it, the GUI's Services screen edits it, and an `extension.toml` declares
//! one. The reasoning, and the rule it sets for the rest of the workspace, are in
//! `.claude/decisions/0006-servicespec-in-proto-and-secret-free.md`.
//!
//! The consequence worth stating twice: **a spec cannot express a secret by value.** See
//! [`EnvValue`].
//!
//! Three fields describe behaviour no code implements yet — [`ServiceSpec::limits`],
//! [`ServiceSpec::idle`] and [`ServiceSpec::logs`], enforced by roadmap tasks T68, T69 and T16. They
//! are declared now so those phases add an implementation rather than a field, and so a spec written
//! today does not have to be revisited to gain one.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::Millis;

/// A service's human-stable identity: `caddy`, `mariadb@main`, `php-fpm@8.3`.
///
/// Validated on construction *and* on deserialisation, because this string is not only an
/// identifier. It names a directory — `logs/services/<id>/`, `etc/<id>/` — so a value containing a
/// path separator, or one Windows refuses as a filename, is a broken install rather than a bad
/// lookup. The charset is narrow enough that the interesting cases cannot be spelled at all.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct ServiceId(String);

impl ServiceId {
    /// The longest an id may be, counted in bytes.
    ///
    /// Not a filesystem limit — every target allows 255 — but a limit on what belongs in a table
    /// column, a log prefix and a GUI tile. Anything approaching it is a naming mistake.
    pub const MAX_LEN: usize = 64;

    /// Names Windows refuses as a file or directory, with or without an extension.
    ///
    /// `con` is a plausible service name for a console tool and the failure it causes — a directory
    /// that cannot be created, on one OS only — is exactly the kind the cross-platform rule in
    /// `CLAUDE.md` exists to catch before merge rather than after.
    ///
    /// Matched against the *whole* id, because the whole id is the directory name. Windows refuses
    /// `con`; it does not refuse `mariadb@aux`, which is an ordinary name that merely contains one.
    const RESERVED: [&'static str; 24] = [
        "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
        "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8",
        "lpt9",
    ];

    /// Parse an id, rejecting anything that cannot also be a directory name.
    ///
    /// The shape is `name` or `name@instance`. Both halves start with an ASCII letter or digit and
    /// continue with those plus `-`; an instance may also contain `.`, because it carries version
    /// numbers (`php-fpm@8.3`). Lowercase only, so two ids cannot differ by case alone and then
    /// collide on a case-insensitive filesystem, and never a trailing `.`, which Windows strips from
    /// a directory name and so would collide the same way.
    ///
    /// # Errors
    ///
    /// [`SpecError::ServiceId`] naming what is wrong with the value, phrased for whoever typed it.
    pub fn parse(value: impl Into<String>) -> Result<Self, SpecError> {
        let value = value.into();

        let reject = |reason: &str| {
            Err(SpecError::ServiceId {
                value: value.clone(),
                reason: reason.to_owned(),
            })
        };

        if value.is_empty() {
            return reject("it is empty");
        }
        if value.len() > Self::MAX_LEN {
            return reject(&format!("it is longer than {} characters", Self::MAX_LEN));
        }

        let (name, instance) = match value.split_once('@') {
            Some((_, instance)) if instance.contains('@') => {
                return reject("it has more than one `@`");
            }
            Some((name, instance)) => (name, Some(instance)),
            None => (value.as_str(), None),
        };

        for (part, label, extra) in [
            (name, "name", ""),
            (instance.unwrap_or("x"), "instance", "."),
        ] {
            if part.is_empty() {
                return reject(&format!("its {label} is empty"));
            }
            if !part.starts_with(|character: char| {
                character.is_ascii_lowercase() || character.is_ascii_digit()
            }) {
                return reject(&format!(
                    "its {label} must start with a lowercase letter or a digit"
                ));
            }
            if !part.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || character == '-'
                    || extra.contains(character)
            }) {
                return reject(&format!(
                    "its {label} may only contain lowercase letters, digits, `-`{}",
                    if extra.is_empty() { "" } else { " and `.`" }
                ));
            }
            if part.ends_with('.') {
                return reject(&format!(
                    "its {label} ends with `.`, which Windows strips from a directory name"
                ));
            }
        }

        // On the whole id rather than on either half, because the whole id is the directory name:
        // Windows refuses `con`, not every name that happens to contain one, and `mariadb@aux` is
        // an ordinary directory. Windows matches the part before the first `.`, which needs no
        // handling here — a `.` only ever appears in an instance, so the part before it always
        // carries the `@` that makes the whole thing not a device name.
        if Self::RESERVED.contains(&value.as_str()) {
            return reject(&format!(
                "`{value}` is a reserved device name on Windows, so it cannot be a directory"
            ));
        }

        Ok(Self(value))
    }

    /// The id as it is written everywhere: on the wire, in the database, as a directory name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The part before `@` — the package this is an instance of.
    #[must_use]
    pub fn name(&self) -> &str {
        self.0
            .split_once('@')
            .map_or(self.0.as_str(), |(name, _)| name)
    }

    /// The part after `@`, when there is one.
    ///
    /// `None` means a service that exists once: there is exactly one Caddy, and `caddy@main` would
    /// be a distinction without a difference.
    #[must_use]
    pub fn instance(&self) -> Option<&str> {
        self.0.split_once('@').map(|(_, instance)| instance)
    }
}

impl fmt::Display for ServiceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for ServiceId {
    /// Validating, unlike most wire types.
    ///
    /// An unknown [`crate::ErrorCode`] is answered leniently because the sentence beside it still
    /// helps a person. There is no equivalent here: an id that cannot be a directory name will fail
    /// later, further from the cause, in the middle of starting something.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize<'_>>::deserialize(deserializer)?;

        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// One environment variable's value.
///
/// **This is the type that keeps a password out of a spec.** MariaDB's generated root password lives
/// in the OS keyring; a spec names it and the supervisor resolves it through the platform `Keyring`
/// capability at the moment it builds the child's `Command`, so the value exists nowhere that is
/// persisted, serialised or logged.
///
/// The shape is Kubernetes' `env.valueFrom.secretKeyRef` and systemd's `LoadCredential=`, both of
/// which exist because the obvious design leaks: `docker inspect` prints environment variables, and
/// so does `systemctl show`.
///
/// **Written tagged, read tagged or bare.** The canonical form — what the daemon sends and what the
/// database stores — always carries `from`. Deserialisation additionally accepts a plain string as a
/// [`Literal`](EnvValue::Literal), because `TZ = "UTC"` is what an `extension.toml` author writes
/// and the tagged spelling buys nothing for the variant that names nothing. The same asymmetry
/// [`crate::Millis`] has, and for the same reason.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum EnvValue {
    /// A value written out in full — a port, a path, a log level.
    ///
    /// **Non-secret by contract.** Nothing enforces it, which is why the other variant exists and is
    /// the one to reach for whenever the answer is "it depends what the password is".
    Literal {
        /// The value, exactly as the child will see it.
        value: String,
    },

    /// A value fetched from the OS keyring at spawn time.
    Keyring {
        /// The keyring service name the credential was stored under.
        service: String,
        /// The account or key within it.
        key: String,
    },
}

impl EnvValue {
    /// A literal value.
    ///
    /// Shorthand, because the struct variant reads badly at a call site that is passing `"8.3"`.
    #[must_use]
    pub fn literal(value: impl Into<String>) -> Self {
        Self::Literal {
            value: value.into(),
        }
    }

    /// A value the supervisor will fetch from the keyring.
    #[must_use]
    pub fn keyring(service: impl Into<String>, key: impl Into<String>) -> Self {
        Self::Keyring {
            service: service.into(),
            key: key.into(),
        }
    }
}

impl<'de> serde::Deserialize<'de> for EnvValue {
    /// Reads the tagged form this type writes, and a bare string besides — see [`EnvValue`].
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(EnvValueVisitor)
    }
}

/// The two shapes [`EnvValue`] accepts on the way in.
struct EnvValueVisitor;

impl<'de> serde::de::Visitor<'de> for EnvValueVisitor {
    type Value = EnvValue;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "a literal value as a string, or a table with `from = \"literal\"` or `from = \"keyring\"`",
        )
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(EnvValue::literal(value))
    }

    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(EnvValue::Literal { value })
    }

    /// Collected field by field rather than by `#[serde(tag = …)]` so that the one dangerous
    /// combination can be named.
    ///
    /// A `value` beside `from = "keyring"` is refused rather than ignored: it is the single mistake
    /// that puts a password into a spec, and an impl that dropped the field silently would let one
    /// sit in a manifest looking like it worked, then travel with everything the spec is copied
    /// into. Unknown fields are refused for the smaller version of the same reason — `secret = …`
    /// should not look accepted.
    fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        use serde::de::Error as _;

        const FIELDS: &[&str] = &["from", "value", "service", "key"];

        let (mut from, mut value, mut service, mut key): (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = (None, None, None, None);

        while let Some(field) = map.next_key::<String>()? {
            let (slot, name) = match field.as_str() {
                "from" => (&mut from, "from"),
                "value" => (&mut value, "value"),
                "service" => (&mut service, "service"),
                "key" => (&mut key, "key"),
                other => return Err(M::Error::unknown_field(other, FIELDS)),
            };

            if slot.is_some() {
                return Err(M::Error::duplicate_field(name));
            }
            *slot = Some(map.next_value()?);
        }

        match from.as_deref() {
            None | Some("literal") => {
                if service.is_some() || key.is_some() {
                    return Err(M::Error::custom(
                        "a literal value names no keyring entry, so `service` and `key` mean nothing beside it",
                    ));
                }

                Ok(EnvValue::Literal {
                    value: value.ok_or_else(|| M::Error::missing_field("value"))?,
                })
            }
            Some("keyring") => {
                if value.is_some() {
                    return Err(M::Error::custom(
                        "`from = \"keyring\"` names a credential instead of carrying one, so a `value` beside it is a password written into a spec",
                    ));
                }

                Ok(EnvValue::Keyring {
                    service: service.ok_or_else(|| M::Error::missing_field("service"))?,
                    key: key.ok_or_else(|| M::Error::missing_field("key"))?,
                })
            }
            Some(other) => Err(M::Error::unknown_variant(other, &["literal", "keyring"])),
        }
    }
}

/// *Can traffic be routed to it yet?*
///
/// Distinct from [`HealthCheck`], which asks whether it is still fine, because MariaDB's first boot
/// initialises a schema and is slow while its steady-state ping is cheap. Every variant carries its
/// own timeout: exceeding it is what turns `Starting` into `Failed`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReadyCheck {
    /// A TCP connection succeeds.
    Tcp {
        /// Where to connect.
        addr: SocketAddr,
        /// How long to keep trying before giving up.
        timeout: Millis,
    },

    /// A Unix domain socket accepts a connection. Unavailable on Windows, where php-fpm listens on
    /// TCP instead — a spec that names one there fails with `unsupported_platform` rather than
    /// hanging until its timeout.
    UnixSocket {
        /// The socket path.
        path: PathBuf,
        /// How long to keep trying before giving up.
        timeout: Millis,
    },

    /// An HTTP request comes back with the expected status.
    Http {
        /// The URL to request.
        url: String,
        /// The status that means ready. Often `200`, but a service whose root redirects says `302`.
        expect_status: u16,
        /// How long to keep trying before giving up.
        timeout: Millis,
    },

    /// A line matching a regex appears on stdout or stderr.
    ///
    /// For services that announce themselves and listen on nothing we can poll. The regex is
    /// compiled by the supervisor, not here — a spec is data, and this crate has no regex engine.
    LogPattern {
        /// The pattern, in the `regex` crate's syntax.
        regex: String,
        /// How long to keep reading before giving up.
        timeout: Millis,
    },

    /// The process is simply still alive after a settling period.
    ///
    /// The last resort, and it is genuinely weak: it cannot distinguish a service that is ready from
    /// one that is about to crash. Use it only where nothing better exists.
    PidAlive {
        /// How long the process must stay up to count as ready.
        settle: Millis,
    },
}

impl ReadyCheck {
    /// The deadline this check gives up at, whichever field carries it.
    #[must_use]
    pub fn timeout(&self) -> Millis {
        match self {
            Self::Tcp { timeout, .. }
            | Self::UnixSocket { timeout, .. }
            | Self::Http { timeout, .. }
            | Self::LogPattern { timeout, .. } => *timeout,
            Self::PidAlive { settle } => *settle,
        }
    }
}

/// *Is it still fine?* — polled periodically, only once ready.
///
/// Failing it makes a service `Degraded`, not `Failed`: the process is alive and the distinction is
/// what the GUI shows in amber and what `mix doctor` explains.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HealthCheck {
    /// What to ask.
    pub probe: HealthProbe,

    /// How often to ask it.
    pub interval: Millis,

    /// How long one probe may take before it counts as a failure.
    pub timeout: Millis,

    /// Consecutive failures before `Running` becomes `Degraded`.
    ///
    /// More than one, always: a single missed probe during a checkpoint flush is not a sick service,
    /// and a dashboard that flickers amber teaches people to ignore it.
    pub failures_before_degraded: u32,

    /// Consecutive successes before `Degraded` becomes `Running` again.
    pub successes_before_running: u32,
}

/// What a [`HealthCheck`] asks.
///
/// Deliberately not [`ReadyCheck`]: two of those variants only make sense once. `LogPattern` matches
/// a line printed during startup and would never match again, and `PidAlive` asks something the
/// supervisor already knows without probing — a health check that cannot fail is worse than none,
/// because it reports health it never measured.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HealthProbe {
    /// A TCP connection still succeeds.
    Tcp {
        /// Where to connect.
        addr: SocketAddr,
    },

    /// A Unix domain socket still accepts a connection.
    UnixSocket {
        /// The socket path.
        path: PathBuf,
    },

    /// An HTTP request still returns the expected status.
    Http {
        /// The URL to request.
        url: String,
        /// The status that means healthy.
        expect_status: u16,
    },

    /// A command exits zero — `mariadb-admin ping`, `pg_isready`, `redis-cli ping`.
    ///
    /// The honest check for a database: a TCP accept only proves the listener is up, which stays
    /// true while the server refuses every query.
    Command {
        /// The program to run.
        program: PathBuf,
        /// Its arguments.
        args: Vec<String>,
    },
}

/// How long to wait between restart attempts.
///
/// Exponential with a ceiling and a random spread, so a service whose dependency is down does not
/// retry in a tight loop and several restarting at once do not synchronise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Backoff {
    /// The wait before the first retry.
    pub initial: Millis,

    /// The longest wait, however many retries have happened.
    pub max: Millis,

    /// What each wait is multiplied by, as a percentage: `200` doubles it.
    ///
    /// An integer percentage rather than an `f64` because this type is compared and hashed like
    /// everything else in this crate, and because a `NaN` multiplier arriving from a hand-written
    /// manifest should not be representable in the first place.
    pub multiplier_percent: u32,

    /// How much to spread each wait by, as a percentage of itself: `20` means ±20 %.
    ///
    /// Under 100, always. At 100 the low end of the spread is zero, which is the tight retry loop
    /// the whole type exists to prevent — the same reason an `initial` of zero is refused.
    pub jitter_percent: u8,
}

impl Default for Backoff {
    /// The curve `.claude/architecture/process-supervision.md` specifies: 500 ms doubling to 30 s.
    fn default() -> Self {
        Self {
            initial: Millis(500),
            max: Millis::from_secs(30),
            multiplier_percent: 200,
            jitter_percent: 20,
        }
    }
}

/// What to do when a supervised process exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RestartPolicy {
    /// Leave it stopped. For one-shot initialisation and for anything a user starts by hand.
    Never,

    /// Restart it when it exits non-zero, up to a point.
    OnFailure {
        /// How many restarts are allowed inside [`window`](RestartPolicy::OnFailure::window) before
        /// the service goes `Failed` and stays there until an explicit `service.start`.
        max_retries: u32,
        /// The span the retries are counted over. A service that crashes once a day is not in a
        /// crash loop, and counting since boot would eventually say it was.
        window: Millis,
        /// The wait between attempts.
        backoff: Backoff,
    },

    /// Restart it whenever it exits, zero or not, forever.
    ///
    /// No retry ceiling on purpose — this is for a service whose absence is the failure. The backoff
    /// is what keeps it from spinning.
    Always {
        /// The wait between attempts.
        backoff: Backoff,
    },
}

impl Default for RestartPolicy {
    /// `OnFailure`, five retries in five minutes — the crash-loop cutoff in
    /// `.claude/architecture/process-supervision.md`.
    fn default() -> Self {
        Self::OnFailure {
            max_retries: 5,
            window: Millis::from_secs(300),
            backoff: Backoff::default(),
        }
    }
}

/// How to ask a service to stop before making it.
///
/// Every variant ends the same way: when the grace period expires the process group is killed. What
/// differs is what is tried first.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum StopBehaviour {
    /// Signal the process group and wait — `SIGTERM` to `-pgid` on Unix, the console control event
    /// on Windows.
    Signal {
        /// How long to wait for it to leave on its own.
        grace: Millis,
    },

    /// Run a shutdown command and wait — `mariadb-admin shutdown`, `caddy stop`.
    ///
    /// For a service that needs to flush before it exits, where a signal loses data or leaves a
    /// recovery to do on next boot.
    Command {
        /// The program to run.
        program: PathBuf,
        /// Its arguments.
        args: Vec<String>,
        /// How long to wait for it to work.
        grace: Millis,
    },

    /// Kill the process group immediately.
    ///
    /// Correct for something stateless that has nothing to flush, and honest about it — the
    /// alternative is a grace period nobody needs, paid on every stop.
    Kill,
}

impl Default for StopBehaviour {
    /// A signal and ten seconds — the same grace period `daemon.shutdown` uses.
    fn default() -> Self {
        Self::Signal {
            grace: Millis::from_secs(10),
        }
    }
}

/// Scheduling priority, the one resource control every OS can honour.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// The default: competes with everything else the user is running.
    #[default]
    Normal,

    /// Yields to foreground work — macOS background QoS, `nice` on Linux, `BELOW_NORMAL` on
    /// Windows. The right setting for anything the user is not waiting on.
    Background,
}

/// Per-service CPU and memory caps.
///
/// Applied through the platform layer, which cannot honour all of it everywhere: Windows has Job
/// Objects and Linux has cgroup v2, but **macOS has no hard memory cap**, so a limit there becomes a
/// watchdog rather than a wall. That asymmetry is a fact the GUI must show rather than hide — see
/// `.claude/features/resource-isolation.md`. Enforcement is roadmap task T68; the fields exist now
/// so a spec written before it does not have to be revisited.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
// **Every field is optional on the way in**, which `Option` alone does not say: serde asks for a
// field even when its type can express absence. The document this is read from is
// `services.limits_json`, whose column default is `{}` — the ordinary state of a service nobody has
// capped — and an `extension.toml` that writes `memory_mb` and nothing else is the ordinary shape of
// one that has. Without this, both are a parse failure naming a field the author was right to leave
// out.
#[serde(default)]
pub struct ResourceLimits {
    /// A ceiling on CPU, as a percentage of one core. `None` is uncapped.
    pub cpu_percent: Option<u8>,

    /// A ceiling on resident memory, in megabytes. `None` is uncapped.
    pub memory_mb: Option<u32>,

    /// How this service competes for CPU when nothing is capped.
    pub priority: Priority,
}

/// How to tell that a service has nothing to do.
///
/// Measured, never assumed: an idle policy that only watched the clock would stop a database in the
/// middle of a long import. A service with an open connection is never idle.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum IdleProbe {
    /// Count established connections to a port, from the service's own status endpoint where it has
    /// one and the OS socket table otherwise.
    Connections {
        /// The port whose connections mean "in use".
        port: u16,
    },

    /// Read a monotonic counter out of a JSON status endpoint — php-fpm's `accepted conn`, a
    /// database's query count — and compare it with the previous sample.
    HttpCounter {
        /// The status URL to read.
        url: String,
        /// The field within it holding the counter.
        field: String,
    },
}

/// When to stop a service that nothing is using.
///
/// The product's central promise is that idle costs nothing, and this is the mechanism. `None` on a
/// [`ServiceSpec`] means never idle-stop — correct for the front-end web server, which is the thing
/// that starts everything else back up. Enforcement is roadmap task T69.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IdlePolicy {
    /// How long the service must look idle before it is stopped.
    pub after: Millis,

    /// What "idle" is measured with.
    pub probe: IdleProbe,
}

/// How much of a service's output to keep, and where the ceiling is.
///
/// Rotation is the supervisor's own job rather than an external logrotate's, because the supervisor
/// is the process holding the file handle. Enforcement is roadmap task T16.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct LogPolicy {
    /// Rotate once the live file passes this size.
    ///
    /// *Passes*, not *reaches*: a line is never split across two files, so one long backtrace can
    /// carry the file over its limit.
    pub max_file_bytes: u64,

    /// How many rotated copies to keep beside the live file.
    pub max_files: u8,

    /// How many recent lines to keep in memory, so `service.logs` and the GUI's log panel answer
    /// instantly instead of reading from disk.
    pub ring_lines: u16,
}

impl Default for LogPolicy {
    /// 10 MB × 5 files, 500 lines in memory — the defaults in
    /// `.claude/architecture/process-supervision.md`.
    fn default() -> Self {
        Self {
            max_file_bytes: 10 * 1024 * 1024,
            max_files: 5,
            ring_lines: 500,
        }
    }
}

/// Everything needed to run and babysit one process.
///
/// Built through [`ServiceSpec::builder`], which is where the invariants are checked — see
/// [`ServiceSpecBuilder::build`].
///
/// **The fields are private and read through accessors**, which is what makes that sentence true
/// rather than merely intended: a public `program` could be reassigned to a relative path after
/// `build` refused one, and a public `depends_on` could be pushed onto until it named the service
/// itself. Nothing needs to mutate a spec — a changed service is a new spec, built and checked the
/// same way — so the ability to is only the ability to get it wrong. Private fields also leave T16,
/// T68 and T69 free to grow the struct, which is what `#[non_exhaustive]` would otherwise be here
/// for.
///
/// **Deserialisation does not run those checks.** A spec arriving from SQLite or from an
/// `extension.toml` is validated by whoever loads it, at the point where a bad one can be reported
/// against its source; a `Deserialize` impl that refused would fail with no idea which row or which
/// file it was reading. [`ServiceId`] is the exception, and validates either way, because it becomes
/// a directory name. What a loader calls is [`ServiceSpec::validate`] — the same function `build`
/// runs, so there is one definition of "usable" rather than a second one written out by hand.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceSpec {
    /// What this service is called, everywhere.
    id: ServiceId,

    /// The binary to execute. Absolute — a spec is never resolved against a `PATH`, because which
    /// `PATH` would be ambiguous exactly where it matters.
    program: PathBuf,

    /// Arguments, already split. Never a command line to be parsed, so a data directory with a space
    /// in it needs no quoting and no escaping rule to get wrong.
    args: Vec<String>,

    /// The child's entire environment.
    ///
    /// **Not inherited wholesale.** The daemon's own environment carries whatever launched it —
    /// a shell's `PATH`, another version manager's hooks, variables from a login session — and a
    /// service that behaves differently depending on how the daemon was started is a bug nobody can
    /// reproduce.
    env: BTreeMap<String, EnvValue>,

    /// The working directory. Also where a relative path in the service's own config resolves from,
    /// which is why it is explicit rather than inherited.
    cwd: PathBuf,

    /// When traffic may be routed to it.
    ready: ReadyCheck,

    /// Whether it is still fine, once it is ready. `None` for a service with nothing cheap to poll.
    health: Option<HealthCheck>,

    /// What to do when it exits.
    restart: RestartPolicy,

    /// How to ask it to stop.
    stop: StopBehaviour,

    /// Services that must be ready before this one starts, and that stop after it.
    ///
    /// The edges of a DAG; a cycle among several specs is caught when they are assembled (roadmap
    /// task T17). The one case a single spec can see for itself — depending on itself — is rejected
    /// by [`ServiceSpecBuilder::build`].
    depends_on: Vec<ServiceId>,

    /// CPU and memory ceilings. Enforcement is roadmap task T68.
    limits: ResourceLimits,

    /// When to stop it for being unused. Enforcement is roadmap task T69.
    idle: Option<IdlePolicy>,

    /// How much of its output to keep. Enforcement is roadmap task T16.
    logs: LogPolicy,
}

impl ServiceSpec {
    /// What this service is called, everywhere.
    #[must_use]
    pub fn id(&self) -> &ServiceId {
        &self.id
    }

    /// The binary to execute. Always absolute.
    #[must_use]
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Its arguments, already split.
    #[must_use]
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// The child's entire environment — not merged with the daemon's.
    #[must_use]
    pub fn env(&self) -> &BTreeMap<String, EnvValue> {
        &self.env
    }

    /// The working directory. Always absolute.
    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// When traffic may be routed to it.
    #[must_use]
    pub fn ready(&self) -> &ReadyCheck {
        &self.ready
    }

    /// Whether it is still fine, once it is ready.
    #[must_use]
    pub fn health(&self) -> Option<&HealthCheck> {
        self.health.as_ref()
    }

    /// What to do when it exits.
    #[must_use]
    pub fn restart(&self) -> RestartPolicy {
        self.restart
    }

    /// How to ask it to stop.
    #[must_use]
    pub fn stop(&self) -> &StopBehaviour {
        &self.stop
    }

    /// Services that must be ready before this one starts, and that stop after it.
    #[must_use]
    pub fn depends_on(&self) -> &[ServiceId] {
        &self.depends_on
    }

    /// CPU and memory ceilings. Enforcement is roadmap task T68.
    #[must_use]
    pub fn limits(&self) -> ResourceLimits {
        self.limits
    }

    /// When to stop it for being unused. Enforcement is roadmap task T69.
    #[must_use]
    pub fn idle(&self) -> Option<&IdlePolicy> {
        self.idle.as_ref()
    }

    /// How much of its output to keep. Enforcement is roadmap task T16.
    #[must_use]
    pub fn logs(&self) -> LogPolicy {
        self.logs
    }

    /// Check everything a spec can know about itself.
    ///
    /// [`ServiceSpecBuilder::build`] runs this, so a spec that came from the builder is already
    /// checked and calling it again is redundant. It is public for the other way a spec arrives — a
    /// `services` row or an `extension.toml`, through [`serde::Deserialize`], which deliberately does
    /// not validate because the error belongs to whoever knows which row or which file it came from.
    /// That caller runs this rather than restating the rules, which is what keeps the two paths from
    /// disagreeing about what a usable spec is.
    ///
    /// What is checked is what one spec can know on its own. Cycles across several specs belong to
    /// whoever assembles them (roadmap task T17), and whether `program` exists on disk belongs to the
    /// supervisor, which is the only layer allowed to look.
    ///
    /// # Errors
    ///
    /// [`SpecError::Invalid`] naming the field at fault, or [`SpecError::Missing`] for a program that
    /// is an empty path rather than a bad one.
    pub fn validate(&self) -> Result<(), SpecError> {
        let invalid = |field: &str, reason: String| SpecError::Invalid {
            id: self.id.clone(),
            field: field.to_owned(),
            reason,
        };

        check_program(&self.id, "program", &self.program)?;

        if !self.cwd.is_absolute() {
            return Err(invalid(
                "cwd",
                format!("`{}` is relative", self.cwd.display()),
            ));
        }

        if self.ready.timeout().is_zero() {
            return Err(invalid("ready", "its timeout is zero".to_owned()));
        }

        // Windows compares environment variable names case-insensitively and Unix does not, so a
        // spec holding both `Path` and `PATH` is two variables on one OS and one — whichever the
        // block happened to end with — on the other.
        let mut folded: BTreeMap<String, &String> = BTreeMap::new();
        for (key, value) in &self.env {
            // `Command::env` on Unix builds `KEY=VALUE`, so a key carrying `=` silently renames the
            // variable and a key carrying NUL truncates the block.
            if key.is_empty() {
                return Err(invalid(
                    "env",
                    "an environment variable has no name".to_owned(),
                ));
            }
            if key.contains('=') || key.contains('\0') {
                return Err(invalid(
                    "env",
                    format!("`{key}` may not contain `=` or a NUL byte"),
                ));
            }
            if let Some(earlier) = folded.insert(key.to_lowercase(), key) {
                return Err(invalid(
                    "env",
                    format!(
                        "`{earlier}` and `{key}` differ only by case, which is two variables on Unix and one on Windows"
                    ),
                ));
            }
            // A value is free to contain `=` — only the first one separates. A NUL is a different
            // matter: the Windows environment block is NUL-separated, so one there ends the
            // variable early rather than being part of it.
            if matches!(value, EnvValue::Literal { value } if value.contains('\0')) {
                return Err(invalid(
                    "env",
                    format!("the value of `{key}` may not contain a NUL byte"),
                ));
            }
        }

        if let Some(health) = &self.health {
            if health.interval.is_zero() {
                return Err(invalid("health", "its interval is zero".to_owned()));
            }
            if health.timeout.is_zero() {
                return Err(invalid("health", "its timeout is zero".to_owned()));
            }
            if health.timeout > health.interval {
                return Err(invalid(
                    "health",
                    format!(
                        "one probe may take {} but they are {} apart, so they would overlap",
                        health.timeout, health.interval
                    ),
                ));
            }
            if health.failures_before_degraded == 0 || health.successes_before_running == 0 {
                return Err(invalid(
                    "health",
                    "a threshold of zero would decide before it measured".to_owned(),
                ));
            }
            if let HealthProbe::Command { program, .. } = &health.probe {
                check_program(&self.id, "health", program)?;
            }
        }

        match &self.stop {
            StopBehaviour::Signal { grace } => {
                if grace.is_zero() {
                    return Err(invalid("stop", ZERO_GRACE.to_owned()));
                }
            }
            StopBehaviour::Command { program, grace, .. } => {
                check_program(&self.id, "stop", program)?;
                if grace.is_zero() {
                    return Err(invalid("stop", ZERO_GRACE.to_owned()));
                }
            }
            StopBehaviour::Kill => {}
        }

        if let Some(backoff) = self.restart.backoff() {
            if backoff.initial.is_zero() {
                return Err(invalid("restart", "its initial backoff is zero".to_owned()));
            }
            if backoff.max < backoff.initial {
                return Err(invalid(
                    "restart",
                    format!(
                        "its backoff starts at {} and is capped at {}",
                        backoff.initial, backoff.max
                    ),
                ));
            }
            if backoff.multiplier_percent < 100 {
                return Err(invalid(
                    "restart",
                    format!(
                        "a multiplier of {}% shrinks the wait on every retry",
                        backoff.multiplier_percent
                    ),
                ));
            }
            // At 100 the low end of the spread is already zero, which is the same tight retry loop
            // an `initial` of zero would be — refused two lines up.
            if backoff.jitter_percent >= 100 {
                return Err(invalid(
                    "restart",
                    format!(
                        "a jitter of ±{}% spreads a wait down to nothing, which is the retry loop the backoff exists to prevent",
                        backoff.jitter_percent
                    ),
                ));
            }
        }
        if let RestartPolicy::OnFailure {
            max_retries,
            window,
            ..
        } = self.restart
        {
            if window.is_zero() {
                return Err(invalid(
                    "restart",
                    "its crash-loop window is zero".to_owned(),
                ));
            }
            if max_retries == 0 {
                return Err(invalid(
                    "restart",
                    "it allows no retries, which is `never` said in a way that reports a crash loop"
                        .to_owned(),
                ));
            }
        }

        let mut seen = BTreeSet::new();
        for dependency in &self.depends_on {
            if *dependency == self.id {
                return Err(invalid("depends_on", "it depends on itself".to_owned()));
            }
            if !seen.insert(dependency) {
                return Err(invalid(
                    "depends_on",
                    format!("`{dependency}` is listed twice"),
                ));
            }
        }

        if let Some(percent) = self.limits.cpu_percent {
            if percent == 0 {
                return Err(invalid(
                    "limits",
                    "a CPU cap of 0% would never let it run".to_owned(),
                ));
            }
            if percent > 100 {
                return Err(invalid(
                    "limits",
                    format!(
                        "a CPU cap of {percent}% is more than the one core it is a percentage of"
                    ),
                ));
            }
        }
        if matches!(self.limits.memory_mb, Some(0)) {
            return Err(invalid(
                "limits",
                "a memory cap of 0 MB would never let it start".to_owned(),
            ));
        }

        if let Some(idle) = &self.idle
            && idle.after.is_zero()
        {
            return Err(invalid(
                "idle",
                "it would be stopped the moment it went quiet".to_owned(),
            ));
        }

        if self.logs.max_file_bytes == 0 || self.logs.max_files == 0 || self.logs.ring_lines == 0 {
            return Err(invalid(
                "logs",
                "a zero here throws output away rather than limiting it".to_owned(),
            ));
        }

        Ok(())
    }

    /// Start describing a service.
    ///
    /// The two arguments are the two things with no possible default. Everything else has one, apart
    /// from [`cwd`](ServiceSpecBuilder::cwd) and [`ready`](ServiceSpecBuilder::ready), which
    /// [`ServiceSpecBuilder::build`] insists on: a wrong working directory is a subtle bug, and a
    /// missing readiness check would silently become "assume it worked".
    pub fn builder(id: ServiceId, program: impl Into<PathBuf>) -> ServiceSpecBuilder {
        ServiceSpecBuilder {
            id,
            program: program.into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            ready: None,
            health: None,
            restart: RestartPolicy::default(),
            stop: StopBehaviour::default(),
            depends_on: Vec::new(),
            limits: ResourceLimits::default(),
            idle: None,
            logs: LogPolicy::default(),
        }
    }
}

/// Builds a [`ServiceSpec`], checking it once at the end.
///
/// Every setter overwrites; none can fail. Collecting the mistakes and reporting them from
/// [`build`](ServiceSpecBuilder::build) keeps the call site readable, and means a spec that exists is
/// a spec that was checked.
#[derive(Debug, Clone)]
#[must_use = "a builder does nothing until `build` is called"]
pub struct ServiceSpecBuilder {
    id: ServiceId,
    program: PathBuf,
    args: Vec<String>,
    env: BTreeMap<String, EnvValue>,
    cwd: Option<PathBuf>,
    ready: Option<ReadyCheck>,
    health: Option<HealthCheck>,
    restart: RestartPolicy,
    stop: StopBehaviour,
    depends_on: Vec<ServiceId>,
    limits: ResourceLimits,
    idle: Option<IdlePolicy>,
    logs: LogPolicy,
}

impl ServiceSpecBuilder {
    /// Append one argument.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append several arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Set an environment variable to a literal value.
    ///
    /// For anything that is not a secret. Use [`env_from_keyring`](Self::env_from_keyring) when it
    /// is — see [`EnvValue`].
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), EnvValue::literal(value));
        self
    }

    /// Set an environment variable to a credential the supervisor fetches at spawn time.
    pub fn env_from_keyring(
        mut self,
        key: impl Into<String>,
        service: impl Into<String>,
        entry: impl Into<String>,
    ) -> Self {
        self.env
            .insert(key.into(), EnvValue::keyring(service, entry));
        self
    }

    /// Set the working directory. Required.
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Set the readiness check. Required.
    pub fn ready(mut self, ready: ReadyCheck) -> Self {
        self.ready = Some(ready);
        self
    }

    /// Set the periodic health check.
    pub fn health(mut self, health: HealthCheck) -> Self {
        self.health = Some(health);
        self
    }

    /// Set the restart policy. Defaults to [`RestartPolicy::default`].
    pub fn restart(mut self, restart: RestartPolicy) -> Self {
        self.restart = restart;
        self
    }

    /// Set the stop behaviour. Defaults to [`StopBehaviour::default`].
    pub fn stop(mut self, stop: StopBehaviour) -> Self {
        self.stop = stop;
        self
    }

    /// Append a dependency that must be ready first.
    pub fn depends_on(mut self, id: ServiceId) -> Self {
        self.depends_on.push(id);
        self
    }

    /// Set the resource limits. Defaults to uncapped at normal priority.
    pub fn limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Set the idle policy. Defaults to never idle-stopping.
    pub fn idle(mut self, idle: IdlePolicy) -> Self {
        self.idle = Some(idle);
        self
    }

    /// Set the log policy. Defaults to [`LogPolicy::default`].
    pub fn logs(mut self, logs: LogPolicy) -> Self {
        self.logs = logs;
        self
    }

    /// Check everything and produce the spec.
    ///
    /// Only the two fields with no default are checked here, because only the builder can tell a
    /// field that was left out from one that was set to something unusable — once the spec exists,
    /// `cwd` is a `PathBuf` either way. Everything else is [`ServiceSpec::validate`], which is also
    /// what a spec that arrived by `Deserialize` is put through.
    ///
    /// # Errors
    ///
    /// [`SpecError`], naming the field at fault.
    pub fn build(self) -> Result<ServiceSpec, SpecError> {
        let id = self.id;
        let missing = |field: &str| SpecError::Missing {
            id: id.clone(),
            field: field.to_owned(),
        };

        let cwd = self.cwd.ok_or_else(|| missing("cwd"))?;
        let ready = self.ready.ok_or_else(|| missing("ready"))?;

        let spec = ServiceSpec {
            id,
            program: self.program,
            args: self.args,
            env: self.env,
            cwd,
            ready,
            health: self.health,
            restart: self.restart,
            stop: self.stop,
            depends_on: self.depends_on,
            limits: self.limits,
            idle: self.idle,
            logs: self.logs,
        };
        spec.validate()?;

        Ok(spec)
    }
}

/// Why a grace period of zero is refused, said once because two variants carry one.
const ZERO_GRACE: &str =
    "its grace period is zero, which is `kill` said in a way that promises a graceful stop";

/// Check one of the programs a spec asks the supervisor to spawn.
///
/// Applied to all three of them — [`ServiceSpec::program`], the command a
/// [`StopBehaviour::Command`] runs and the one a [`HealthProbe::Command`] runs — because they are
/// the same thing: a path the supervisor hands to the OS. A relative one is resolved against the
/// child's `PATH` and working directory at the moment it runs, which is the ambiguity the rule
/// exists to refuse, and enforcing it on the first only would leave two unguarded ways to run
/// whatever happens to be first on a `PATH`.
///
/// # Errors
///
/// [`SpecError::Missing`] for an empty path, [`SpecError::Invalid`] for a relative one, naming
/// `field` either way.
fn check_program(id: &ServiceId, field: &str, program: &Path) -> Result<(), SpecError> {
    if program.as_os_str().is_empty() {
        return Err(SpecError::Missing {
            id: id.clone(),
            field: field.to_owned(),
        });
    }
    if !program.is_absolute() {
        return Err(SpecError::Invalid {
            id: id.clone(),
            field: field.to_owned(),
            reason: format!(
                "`{}` is relative; a spec is never resolved against a PATH",
                program.display()
            ),
        });
    }

    Ok(())
}

impl RestartPolicy {
    /// The backoff curve this policy retries on, if it retries at all.
    #[must_use]
    pub fn backoff(&self) -> Option<Backoff> {
        match self {
            Self::Never => None,
            Self::OnFailure { backoff, .. } | Self::Always { backoff } => Some(*backoff),
        }
    }
}

/// A spec that could not be built, or an identifier that could not be parsed.
///
/// This crate's own `thiserror` enum, separate from [`crate::Error`]: that one is the wire failure,
/// and `.claude/architecture/daemon-and-ipc.md` reserves deciding a code and writing a hint for the
/// daemon boundary. Every variant here maps to `invalid_argument` there.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SpecError {
    /// A service id does not have the shape of one.
    #[error("`{value}` is not a valid service id: {reason}")]
    ServiceId {
        /// What was offered.
        value: String,
        /// Why it was refused, as a clause that completes the sentence.
        reason: String,
    },

    /// A field with no default was never set.
    #[error("service `{id}` needs a `{field}`")]
    Missing {
        /// The service being described.
        id: ServiceId,
        /// The field that was left out.
        field: String,
    },

    /// A field was set to something that cannot work.
    #[error("service `{id}` has an unusable `{field}`: {reason}")]
    Invalid {
        /// The service being described.
        id: ServiceId,
        /// The field at fault.
        field: String,
        /// Why it cannot work, as a clause that completes the sentence.
        reason: String,
    },
}

/// Builds a path the tests below can rely on being absolute on every target.
///
/// Test-only: it exists because `/opt/mixengine/caddy` is not absolute on Windows and
/// `C:\MixEngine\caddy` is not a path on Unix, and a spec insists on absolute paths.
#[cfg(test)]
fn absolute(relative: &str) -> PathBuf {
    let root = if cfg!(windows) {
        r"C:\MixEngine"
    } else {
        "/opt/mixengine"
    };

    std::path::Path::new(root).join(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ServiceSpecBuilder {
        ServiceSpec::builder(
            ServiceId::parse("mariadb@main").unwrap(),
            absolute("packages/mariadb/11.4.3/bin/mariadbd"),
        )
        .cwd(absolute("data/mariadb/main"))
        .ready(ReadyCheck::Tcp {
            addr: "127.0.0.1:3306".parse().unwrap(),
            timeout: Millis::from_secs(30),
        })
    }

    #[test]
    fn an_id_splits_into_a_name_and_an_optional_instance() {
        let instanced = ServiceId::parse("mariadb@main").unwrap();
        assert_eq!(instanced.name(), "mariadb");
        assert_eq!(instanced.instance(), Some("main"));

        let plain = ServiceId::parse("caddy").unwrap();
        assert_eq!(plain.name(), "caddy");
        assert_eq!(plain.instance(), None);
    }

    #[test]
    fn the_ids_the_documentation_uses_are_all_valid() {
        for id in [
            "caddy",
            "mariadb@main",
            "php-fpm@8.3",
            "redis@main",
            "nginx",
        ] {
            assert!(ServiceId::parse(id).is_ok(), "{id} should parse");
        }
    }

    /// Every one of these would become a directory under `logs/services/`.
    #[test]
    fn an_id_cannot_be_something_that_is_not_a_directory_name() {
        for id in [
            "",
            "..",
            ".",
            "a/b",
            "a\\b",
            "MariaDB", // case would collide on a case-insensitive filesystem
            "-leading",
            "@main",
            "mariadb@",
            "mariadb@a@b",
            "php.fpm",    // a dot belongs to the instance, not the name
            "con",        // reserved on Windows
            "php-fpm@8.", // Windows strips the trailing dot, colliding with `php-fpm@8`
        ] {
            assert!(ServiceId::parse(id).is_err(), "{id:?} should be refused");
        }
    }

    /// The whole list, because a gap in it is invisible until a directory cannot be created — and
    /// only on Windows. `lpt8` and `lpt9` were the gap.
    #[test]
    fn every_windows_device_name_is_refused() {
        let mut devices = vec![
            "con".to_owned(),
            "prn".to_owned(),
            "aux".to_owned(),
            "nul".to_owned(),
        ];
        for number in 0..=9 {
            devices.push(format!("com{number}"));
            devices.push(format!("lpt{number}"));
        }

        for device in devices {
            assert!(
                ServiceId::parse(device.as_str()).is_err(),
                "{device} is a device name on Windows and cannot be a directory"
            );
        }
    }

    /// The rule is about the directory name, which is the whole id — and Windows refuses `con`, not
    /// every name that contains one. Checking each half separately turned `mariadb@aux` into an
    /// error about a device nobody was naming.
    #[test]
    fn a_name_that_merely_contains_a_device_name_is_an_ordinary_id() {
        for id in ["mariadb@aux", "redis@com1", "lpt1@main", "console"] {
            assert!(ServiceId::parse(id).is_ok(), "{id} should parse");
        }
    }

    #[test]
    fn an_id_that_is_not_a_directory_name_is_refused_on_the_way_in_too() {
        let error = serde_json::from_str::<ServiceId>(r#""../../etc""#)
            .unwrap_err()
            .to_string();

        assert!(error.contains("not a valid service id"), "{error}");
    }

    /// A built spec is read, never edited: the checks in `build` are only worth running if nothing
    /// afterwards can undo them. This test is what fails if a field is ever made `pub` again — it
    /// stops compiling rather than asserting.
    #[test]
    fn a_built_spec_is_read_through_accessors() {
        let built = spec()
            .arg("--defaults-file")
            .env("TZ", "UTC")
            .build()
            .unwrap();

        assert_eq!(built.id().as_str(), "mariadb@main");
        assert!(built.program().is_absolute());
        assert!(built.cwd().is_absolute());
        assert_eq!(built.args(), ["--defaults-file"]);
        assert_eq!(built.env()["TZ"], EnvValue::literal("UTC"));
        assert!(built.health().is_none());
        assert!(built.idle().is_none());
        assert!(built.depends_on().is_empty());
        assert_eq!(built.restart(), RestartPolicy::default());
        assert_eq!(built.stop(), &StopBehaviour::default());
        assert_eq!(built.limits(), ResourceLimits::default());
        assert_eq!(built.logs(), LogPolicy::default());
        assert!(matches!(built.ready(), ReadyCheck::Tcp { .. }));
    }

    #[test]
    fn a_spec_round_trips_through_json() {
        let built = spec()
            .args(["--defaults-file", "my.cnf"])
            .env("TZ", "UTC")
            .env_from_keyring("MARIADB_ROOT_PASSWORD", "mixengine", "mariadb@main/root")
            .depends_on(ServiceId::parse("caddy").unwrap())
            .build()
            .unwrap();

        let encoded = serde_json::to_string(&built).unwrap();
        assert_eq!(
            serde_json::from_str::<ServiceSpec>(&encoded).unwrap(),
            built
        );
    }

    /// A spec off the wire skips the builder, so the checks have to be reachable without one —
    /// otherwise the loader that is supposed to run them would have to restate them, and the two
    /// definitions of "usable" would drift.
    #[test]
    fn a_spec_that_skipped_the_builder_can_still_be_checked() {
        let built = spec().build().unwrap();
        assert!(built.validate().is_ok());

        let mut encoded = serde_json::to_value(&built).unwrap();
        encoded["program"] = serde_json::Value::String("mariadbd".to_owned());

        let deserialised = serde_json::from_value::<ServiceSpec>(encoded).unwrap();
        assert!(
            matches!(deserialised.validate(), Err(SpecError::Invalid { ref field, .. }) if field == "program")
        );
    }

    /// The point of [`EnvValue`]: the password is not in the spec, so it cannot leave with it.
    #[test]
    fn a_serialised_spec_names_a_credential_instead_of_carrying_one() {
        let built = spec()
            .env_from_keyring("MARIADB_ROOT_PASSWORD", "mixengine", "mariadb@main/root")
            .build()
            .unwrap();

        let encoded = serde_json::to_value(&built).unwrap();
        let value = &encoded["env"]["MARIADB_ROOT_PASSWORD"];

        assert_eq!(value["from"], "keyring");
        assert_eq!(value["service"], "mixengine");
        assert_eq!(value["key"], "mariadb@main/root");
        assert!(value.get("value").is_none(), "{value}");
    }

    /// What an `extension.toml` author writes. The tagged form is still what the type *emits*, so
    /// the wire and the database are unchanged.
    #[test]
    fn a_literal_environment_value_can_be_written_the_way_a_person_writes_one() {
        assert_eq!(
            serde_json::from_str::<EnvValue>(r#""UTC""#).unwrap(),
            EnvValue::literal("UTC")
        );
        assert_eq!(
            serde_json::from_str::<EnvValue>(r#"{"from":"literal","value":"UTC"}"#).unwrap(),
            EnvValue::literal("UTC")
        );
        assert_eq!(
            serde_json::to_string(&EnvValue::literal("UTC")).unwrap(),
            r#"{"from":"literal","value":"UTC"}"#
        );
    }

    /// The one mistake that would put a password in a spec. Ignoring the field would let it
    /// round-trip out of a manifest looking like it had been accepted.
    #[test]
    fn a_keyring_entry_cannot_also_carry_the_credential() {
        let error = serde_json::from_str::<EnvValue>(
            r#"{"from":"keyring","service":"mixengine","key":"mariadb@main/root","value":"hunter2"}"#,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("password written into a spec"), "{error}");
        assert!(!error.contains("hunter2"), "{error}");
    }

    #[test]
    fn an_environment_value_refuses_what_it_cannot_mean() {
        for json in [
            r#"{"from":"secret","service":"a","key":"b"}"#, // no such source
            r#"{"from":"keyring","service":"mixengine"}"#,  // no key
            r#"{"from":"literal"}"#,                        // no value
            r#"{"value":"UTC","service":"mixengine"}"#,     // a literal naming a keyring entry
            r#"{"from":"literal","value":"UTC","secret":"x"}"#, // should not look accepted
        ] {
            assert!(
                serde_json::from_str::<EnvValue>(json).is_err(),
                "{json} should be refused"
            );
        }
    }

    /// `Debug` reaches logs and bug reports, and there is nothing in a spec that must not.
    #[test]
    fn debug_shows_a_credential_by_name_and_has_no_value_to_show() {
        let built = spec()
            .env_from_keyring("MARIADB_ROOT_PASSWORD", "mixengine", "mariadb@main/root")
            .build()
            .unwrap();

        let rendered = format!("{built:?}");
        assert!(rendered.contains("Keyring"), "{rendered}");
        assert!(rendered.contains("mariadb@main/root"), "{rendered}");
    }

    #[test]
    fn a_spec_insists_on_the_two_fields_with_no_sensible_default() {
        let id = ServiceId::parse("caddy").unwrap();
        let program = absolute("packages/caddy/2.8.4/caddy");

        let no_cwd = ServiceSpec::builder(id.clone(), &program)
            .ready(ReadyCheck::PidAlive {
                settle: Millis::from_secs(1),
            })
            .build();
        assert!(matches!(no_cwd, Err(SpecError::Missing { ref field, .. }) if field == "cwd"));

        let no_ready = ServiceSpec::builder(id, &program)
            .cwd(absolute("etc/caddy"))
            .build();
        assert!(matches!(no_ready, Err(SpecError::Missing { ref field, .. }) if field == "ready"));
    }

    #[test]
    fn a_spec_is_never_resolved_against_a_path() {
        let built = ServiceSpec::builder(ServiceId::parse("caddy").unwrap(), "caddy")
            .cwd(absolute("etc/caddy"))
            .ready(ReadyCheck::PidAlive {
                settle: Millis::from_secs(1),
            })
            .build();

        assert!(matches!(built, Err(SpecError::Invalid { ref field, .. }) if field == "program"));
    }

    /// `program` is not the only program a spec can spawn, and the other two go through the same
    /// `Command`. A rule enforced on one of three is a rule with two ways around it.
    #[test]
    fn neither_is_a_command_a_stop_or_a_health_check_runs() {
        let stop = spec()
            .stop(StopBehaviour::Command {
                program: "mariadb-admin".into(),
                args: vec!["shutdown".to_owned()],
                grace: Millis::from_secs(10),
            })
            .build();
        assert!(matches!(stop, Err(SpecError::Invalid { ref field, .. }) if field == "stop"));

        let health = spec()
            .health(HealthCheck {
                probe: HealthProbe::Command {
                    program: "mariadb-admin".into(),
                    args: vec!["ping".to_owned()],
                },
                interval: Millis::from_secs(10),
                timeout: Millis::from_secs(2),
                failures_before_degraded: 3,
                successes_before_running: 1,
            })
            .build();
        assert!(matches!(health, Err(SpecError::Invalid { ref field, .. }) if field == "health"));
    }

    /// An empty program is the field left out rather than a path, and says so.
    #[test]
    fn a_command_with_no_program_is_a_missing_field() {
        let built = spec()
            .stop(StopBehaviour::Command {
                program: PathBuf::new(),
                args: Vec::new(),
                grace: Millis::from_secs(10),
            })
            .build();

        assert!(matches!(built, Err(SpecError::Missing { ref field, .. }) if field == "stop"));
    }

    /// Every other duration of zero is refused; a grace period is the one where the default —
    /// killing the process group — is destructive rather than merely wrong.
    #[test]
    fn a_graceful_stop_with_no_grace_would_be_a_kill_that_promised_otherwise() {
        for stop in [
            StopBehaviour::Signal { grace: Millis(0) },
            StopBehaviour::Command {
                program: absolute("packages/mariadb/11.4.3/bin/mariadb-admin"),
                args: vec!["shutdown".to_owned()],
                grace: Millis(0),
            },
        ] {
            let built = spec().stop(stop.clone()).build();

            assert!(
                matches!(built, Err(SpecError::Invalid { ref field, .. }) if field == "stop"),
                "{stop:?} should be refused"
            );
        }

        // `Kill` has no grace period to be zero, and is the honest way to say it.
        assert!(spec().stop(StopBehaviour::Kill).build().is_ok());
    }

    /// `cpu_percent` is documented as a percentage *of one core*, so 101 is not a bigger cap — it
    /// is a spec whose author meant something the field cannot express.
    #[test]
    fn a_cpu_cap_cannot_exceed_the_core_it_is_a_percentage_of() {
        for percent in [0, 101, 255] {
            let built = spec()
                .limits(ResourceLimits {
                    cpu_percent: Some(percent),
                    ..ResourceLimits::default()
                })
                .build();

            assert!(
                matches!(built, Err(SpecError::Invalid { ref field, .. }) if field == "limits"),
                "{percent}% should be refused"
            );
        }

        assert!(
            spec()
                .limits(ResourceLimits {
                    cpu_percent: Some(100),
                    ..ResourceLimits::default()
                })
                .build()
                .is_ok()
        );
    }

    #[test]
    fn a_spec_cannot_depend_on_itself() {
        let built = spec()
            .depends_on(ServiceId::parse("mariadb@main").unwrap())
            .build();

        assert!(
            matches!(built, Err(SpecError::Invalid { ref field, .. }) if field == "depends_on")
        );
    }

    #[test]
    fn a_spec_cannot_name_a_dependency_twice() {
        let built = spec()
            .depends_on(ServiceId::parse("caddy").unwrap())
            .depends_on(ServiceId::parse("caddy").unwrap())
            .build();

        assert!(
            matches!(built, Err(SpecError::Invalid { ref field, .. }) if field == "depends_on")
        );
    }

    #[test]
    fn an_environment_variable_cannot_smuggle_a_second_one() {
        let built = spec().env("TZ=UTC\0PATH", "/usr/bin").build();

        assert!(matches!(built, Err(SpecError::Invalid { ref field, .. }) if field == "env"));
    }

    /// The Windows environment block is NUL-separated, so a NUL in a value ends the variable
    /// early instead of being part of it.
    #[test]
    fn an_environment_value_cannot_end_itself_early() {
        let built = spec().env("PATH", "/usr/bin\0TZ=UTC").build();

        assert!(matches!(built, Err(SpecError::Invalid { ref field, .. }) if field == "env"));

        // A `=` in a value is ordinary — only the first one separates.
        assert!(spec().env("QUERY", "a=1&b=2").build().is_ok());
    }

    #[test]
    fn a_backoff_that_shrinks_or_starts_at_zero_is_refused() {
        for backoff in [
            Backoff {
                initial: Millis(0),
                ..Backoff::default()
            },
            Backoff {
                initial: Millis::from_secs(60),
                max: Millis::from_secs(30),
                ..Backoff::default()
            },
            Backoff {
                multiplier_percent: 50,
                ..Backoff::default()
            },
            Backoff {
                jitter_percent: 150,
                ..Backoff::default()
            },
            // ±100% reaches zero, which is the retry loop `initial: Millis(0)` is refused for.
            Backoff {
                jitter_percent: 100,
                ..Backoff::default()
            },
        ] {
            let built = spec().restart(RestartPolicy::Always { backoff }).build();

            assert!(
                matches!(built, Err(SpecError::Invalid { ref field, .. }) if field == "restart"),
                "{backoff:?} should be refused"
            );
        }

        assert!(
            spec()
                .restart(RestartPolicy::Always {
                    backoff: Backoff {
                        jitter_percent: 99,
                        ..Backoff::default()
                    }
                })
                .build()
                .is_ok()
        );
    }

    /// Windows compares environment variable names case-insensitively and Unix does not, so this
    /// spec is two variables on one OS and one on the other — the same trap `ServiceId`'s
    /// lowercase-only rule closes for directory names.
    #[test]
    fn two_environment_names_cannot_differ_only_by_case() {
        let built = spec()
            .env("PATH", "/usr/bin")
            .env("Path", "/usr/local/bin")
            .build();

        assert!(matches!(built, Err(SpecError::Invalid { ref field, .. }) if field == "env"));

        // Names that differ by more than case are two variables everywhere.
        assert!(
            spec()
                .env("PATH", "/usr/bin")
                .env("PATHEXT", ".EXE")
                .build()
                .is_ok()
        );
    }

    #[test]
    fn health_probes_cannot_be_slower_than_the_gap_between_them() {
        let built = spec()
            .health(HealthCheck {
                probe: HealthProbe::Tcp {
                    addr: "127.0.0.1:3306".parse().unwrap(),
                },
                interval: Millis::from_secs(5),
                timeout: Millis::from_secs(10),
                failures_before_degraded: 3,
                successes_before_running: 1,
            })
            .build();

        assert!(matches!(built, Err(SpecError::Invalid { ref field, .. }) if field == "health"));
    }

    #[test]
    fn a_never_restart_policy_has_no_backoff_to_check() {
        assert_eq!(RestartPolicy::Never.backoff(), None);
        assert!(spec().restart(RestartPolicy::Never).build().is_ok());
    }

    /// `OnFailure` with no retries is `Never` with a crash-loop report attached, and a spec that can
    /// say one thing two ways is a spec two pieces of code will read differently.
    #[test]
    fn a_restart_policy_that_never_retries_says_never() {
        let built = spec()
            .restart(RestartPolicy::OnFailure {
                max_retries: 0,
                window: Millis::from_secs(300),
                backoff: Backoff::default(),
            })
            .build();

        assert!(matches!(built, Err(SpecError::Invalid { ref field, .. }) if field == "restart"));
    }

    /// The defaults are the numbers `.claude/architecture/process-supervision.md` publishes. If this
    /// test needs editing, that document does too.
    #[test]
    fn the_defaults_are_the_documented_ones() {
        assert_eq!(
            Backoff::default(),
            Backoff {
                initial: Millis(500),
                max: Millis::from_secs(30),
                multiplier_percent: 200,
                jitter_percent: 20,
            }
        );
        assert_eq!(
            LogPolicy::default(),
            LogPolicy {
                max_file_bytes: 10 * 1024 * 1024,
                max_files: 5,
                ring_lines: 500,
            }
        );
        assert!(matches!(
            RestartPolicy::default(),
            RestartPolicy::OnFailure {
                max_retries: 5,
                window,
                ..
            } if window == Millis::from_secs(300)
        ));
    }

    /// Every enum in this module carries its discriminator inside the object, the way
    /// `DaemonEvent` does — one handler in the GUI, and an unknown variant is an object an older
    /// client can recognise and ignore.
    #[test]
    fn every_choice_is_internally_tagged() {
        let ready = serde_json::to_value(ReadyCheck::PidAlive {
            settle: Millis::from_secs(2),
        })
        .unwrap();
        assert_eq!(ready["type"], "pid_alive");
        assert_eq!(ready["settle"], 2000);

        let stop = serde_json::to_value(StopBehaviour::Kill).unwrap();
        assert_eq!(stop["type"], "kill");

        let probe = serde_json::to_value(IdleProbe::Connections { port: 3306 }).unwrap();
        assert_eq!(probe["type"], "connections");
    }
}
