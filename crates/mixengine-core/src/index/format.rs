//! The shape of `index.json`, as it is actually published.
//!
//! Written against the document at
//! <https://github.com/mixnz/mixengine-packages/releases/download/index/index.json> rather
//! than against the sketch that preceded it, which is the whole reason T20a was ordered first. The
//! two differ in exactly the places a client trips over: `provides` is a map from executable name to
//! its path inside the archive rather than a list of names, because the daemon needs to know *where*
//! a binary is and a borrowed archive keeps its publisher's layout; and `requires` hangs off the
//! artifact rather than the package, because a Windows PHP needs a VC++ redistributable and the same
//! version on Linux needs a glibc, and neither statement is true of the other.
//!
//! Everything here is `#[serde(deny_unknown_fields)]`-free on purpose, which is the opposite of the
//! rule `config.toml` follows. That file is written by a user, where a typo silently ignored is a
//! setting they believe is in effect. This one is written by us and read by builds older than it:
//! an index that adds a field must stay readable by every client already deployed, or adding one
//! becomes a breaking change and the `schema` number has to move for things that break nothing.

use std::collections::BTreeMap;

use mixengine_proto::Execution;
use serde::{Deserialize, Serialize};

/// The schema this build can read.
///
/// Bumped only for a change an existing client *cannot* read. Adding an optional field is not one —
/// see the module note on unknown fields.
pub const SCHEMA: u32 = 1;

/// A verified package index: everything MixEngine can install, and where to get it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    /// The document version. Checked against [`SCHEMA`] before anything else is believed.
    pub schema: u32,

    /// When the publishing pipeline generated this document.
    ///
    /// Load-bearing rather than informational: it is what makes a rolled-back index detectable. An
    /// older index is signed just as validly as a newer one — we signed both — so the signature
    /// cannot tell them apart and this field is the only thing that can.
    pub generated_at: Timestamp,

    /// Every version of every runtime and service, oldest schema entry first or not — the order is
    /// the generator's and nothing here depends on it.
    pub packages: Vec<Package>,
}

/// One version of one thing, across every platform it was built for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    /// `php`, `node`, `python`, `ruby`, `caddy`, `mariadb` …
    pub kind: String,

    /// Upstream's version string, exactly as upstream writes it.
    ///
    /// Not normalised and not parsed here: it is the string a user pinned in `mixengine.toml`, and
    /// a client that rewrote it would stop matching what they wrote. Comparing versions is
    /// [T24](../../../../.claude/roadmap/phase-2-runtimes.md)'s problem and it needs the constraint
    /// grammar this type does not have.
    pub version: String,

    /// Which channel this belongs to. Only `stable` is offered without a setting.
    pub channel: Channel,

    /// Upstream's end of security support, when upstream states one.
    ///
    /// A version past it stays installable and is marked in the GUI. PHP is deliberately offered
    /// years past every one of these — the people who reach for a local development environment are
    /// very often the people maintaining something old.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eol: Option<String>,

    /// One entry per OS and architecture this version was built for.
    pub artifacts: Vec<Artifact>,
}

/// One downloadable build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// Which operating system this build runs on.
    pub os: Os,

    /// Which architecture.
    pub arch: Arch,

    /// Where to fetch it. Always a release asset of the packaging repository, never an upstream
    /// URL — upstreams prune, and the index promises old versions keep working.
    pub url: String,

    /// The hash the download is checked against *after* it arrives, lowercase hex.
    pub sha256: String,

    /// The size in bytes, so the GUI can show it before the user commits to the download.
    pub size: u64,

    /// Executable name to its path inside the archive.
    ///
    /// A map rather than a list because the archive keeps the layout its publisher shipped: on
    /// Windows PHP resolves its DLLs from its own directory, so normalising every runtime into one
    /// `bin/` would produce an archive that only fails at run time.
    pub provides: BTreeMap<String, String>,

    /// What has to be true of the machine before this will run.
    #[serde(default, skip_serializing_if = "Requires::is_empty")]
    pub requires: Requires,

    /// Where loadable extensions live inside the archive, relative to its root.
    ///
    /// Present whenever the build can load any. The generated config always sets it explicitly:
    /// upstream PHP for Windows bakes an absolute `C:\php\ext` that would otherwise be consulted by
    /// accident on a machine where it happens to exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_dir: Option<String>,

    /// What this build can offer, split by whether it can be turned off.
    #[serde(default, skip_serializing_if = "Extensions::is_empty")]
    pub extensions: Extensions,
}

/// What the publisher measured off the finished artifact, as preconditions on the machine.
///
/// **Nothing in this workspace reads any of them**, and saying otherwise here would be worse than
/// saying nothing: this comment used to claim the daemon *"checks these before installing, and
/// prompts about them rather than silently satisfying"*, and roadmap task **T92** found no consumer
/// at all. The mechanism that exists is [`crate::install::SmokeTest`], whose own note argues the
/// case — every failure these fields describe is invisible until something tries, and what a
/// refusal here would have to say is what the loader says anyway.
///
/// **The published document already carries a field this type does not model.** Ten artifacts — the
/// Linux PostgreSQL cells — state a `requires.tzdata`, and it is prose rather than a version: *"the
/// system timezone database at /usr/share/zoneinfo — Debian builds PostgreSQL `--with-system-tzdata`,
/// so unlike the Windows and macOS cells this one does not carry its own"*. It parses, because this
/// module is deliberately not `deny_unknown_fields`, and it is dropped. Carrying a sentence like
/// that to a person at install time is a feature with a design of its own, and a fourth unread
/// field would not be it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requires {
    /// The Visual C++ redistributable year, on Windows.
    ///
    /// Per branch and not per runtime: PHP 7.0–7.1 are VC14, 7.2–7.3 VC15, 7.4–8.3 VS16, 8.4
    /// onwards VS17. An index entry naming one redistributable for "PHP" would be wrong for most of
    /// the table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcredist: Option<String>,

    /// The minimum macOS version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub macos: Option<String>,

    /// The minimum glibc, on Linux.
    ///
    /// Measured off the finished binary — the highest `GLIBC_x.y` symbol version it imports — rather
    /// than assumed from the machine that built it. A glibc build will not start on anything older,
    /// and a loader error is a worse way to learn that than a refusal naming the number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glibc: Option<String>,
}

impl Requires {
    /// Whether this artifact asks anything of the machine at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.vcredist.is_none() && self.macos.is_none() && self.glibc.is_none()
    }
}

/// What a build offers, and whether each half can be switched off.
///
/// The split is not cosmetic. A static extension is linked into the binary and is present forever;
/// a shared one is a file that has to be on disk before it can be enabled. "Enable an extension"
/// therefore means two different things per platform, and this is where a caller finds out which.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extensions {
    /// Compiled in. Always loaded, never disableable.
    #[serde(default, rename = "static", skip_serializing_if = "Vec::is_empty")]
    pub compiled_in: Vec<String>,

    /// Loadable modules shipped inside the archive.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared: Vec<String>,

    /// Which of [`shared`](Self::shared) an installer is expected to switch on, so that the cells
    /// of one version behave alike.
    ///
    /// Published per artifact and not per version, which is the whole reason it exists: Windows
    /// ships `curl`, `mbstring`, `intl` and a dozen more as DLLs where Unix compiles them in, so
    /// "the extensions a user expects to be there" is a different set on each system and only the
    /// publisher knows which.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enabled: Vec<String>,
}

impl Extensions {
    /// Whether anything is known about this build's extensions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.compiled_in.is_empty() && self.shared.is_empty() && self.enabled.is_empty()
    }
}

/// Which release channel a package version belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Channel {
    /// Offered by default.
    Stable,
    /// Behind a setting.
    Rc,
    /// Behind a setting.
    Beta,
}

impl From<Channel> for mixengine_proto::PackageChannel {
    /// The document's vocabulary, as the wire spells it.
    ///
    /// Two enums rather than one shared type, and this is the whole cost of that: what a *published
    /// document* says and what the API answers are allowed to move apart, and a channel added to
    /// this file for the publishing pipeline's own purposes should not become an API change by
    /// accident. Total rather than fallible, because the two agree today and a variant added on
    /// either side has to face this `match`.
    fn from(channel: Channel) -> Self {
        match channel {
            Channel::Stable => Self::Stable,
            Channel::Rc => Self::Rc,
            Channel::Beta => Self::Beta,
        }
    }
}

/// An operating system an artifact was built for.
///
/// Closed, unlike [`Package::kind`]: the set of operating systems MixEngine runs on is a decision
/// this project makes, not one the index gets to extend. An index naming a fourth one is describing
/// a build this client could not install anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    /// Windows.
    Windows,
    /// macOS.
    Macos,
    /// Linux.
    Linux,
}

impl Os {
    /// The operating system this build of MixEngine was compiled for.
    ///
    /// A compile-time constant of the target triple, not a question asked of the running machine —
    /// which is why it does not belong behind [`mixengine_platform`]. It is also the answer the
    /// caller actually wants: an x86_64 build running under emulation on a Windows-on-ARM machine
    /// should install x86_64 artifacts, because that is what it can execute.
    #[must_use]
    pub fn host() -> Option<Self> {
        match std::env::consts::OS {
            "windows" => Some(Self::Windows),
            "macos" => Some(Self::Macos),
            "linux" => Some(Self::Linux),
            _ => None,
        }
    }

    /// The word an index — and an `extension.toml`'s `[artifact.<target>]` key — spells it with.
    ///
    /// The same spelling `Deserialize` reads, written out once so that a caller composing a target
    /// word does not reach for `serde_json` to get a string out of an enum.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Macos => "macos",
            Self::Linux => "linux",
        }
    }
}

/// A processor architecture an artifact was built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arch {
    /// 64-bit x86.
    X86_64,
    /// 64-bit ARM.
    Aarch64,
}

impl Arch {
    /// The architecture this build of MixEngine was compiled for. See [`Os::host`].
    #[must_use]
    pub fn host() -> Option<Self> {
        match std::env::consts::ARCH {
            "x86_64" => Some(Self::X86_64),
            "aarch64" => Some(Self::Aarch64),
            _ => None,
        }
    }

    /// The word an index spells it with. See [`Os::as_str`].
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }
}

/// One operating system and architecture a build can be made for.
///
/// A pair rather than two arguments, because every question worth asking of this document takes
/// both, and because [`runnable`](Self::runnable) is a fact about the pair rather than about either
/// half: an ARM64 *Windows* machine executes an x86_64 build and an ARM64 *Linux* machine does not.
///
/// **Taken as an argument rather than read off the host**, which is what makes the rule below
/// testable at all: `test` runs on `ubuntu-latest`, `windows-latest` and `macos-latest` and on no
/// ARM runner, so a host-only reading would leave the one interesting case to be exercised for the
/// first time by a user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// Which operating system.
    pub os: Os,

    /// Which architecture.
    pub arch: Arch,
}

/// Every target MixEngine ships a build for, in the order a coverage matrix reads them.
///
/// Six, and the number is a fact about this product rather than about the index: it is what
/// [build-and-release.md](../../../../.claude/operations/build-and-release.md) produces and what
/// roadmap task **T92** measured the packaging pipeline against.
pub const TARGETS: [Target; 6] = [
    Target::new(Os::Windows, Arch::X86_64),
    Target::new(Os::Windows, Arch::Aarch64),
    Target::new(Os::Macos, Arch::X86_64),
    Target::new(Os::Macos, Arch::Aarch64),
    Target::new(Os::Linux, Arch::X86_64),
    Target::new(Os::Linux, Arch::Aarch64),
];

impl Target {
    /// Name one.
    #[must_use]
    pub const fn new(os: Os, arch: Arch) -> Self {
        Self { os, arch }
    }

    /// What this build of MixEngine was compiled for, or [`None`] on a system the index has no
    /// vocabulary for. See [`Os::host`].
    #[must_use]
    pub fn host() -> Option<Self> {
        Some(Self::new(Os::host()?, Arch::host()?))
    }

    /// Every target whose artifacts a machine of this one can execute, most preferred first.
    ///
    /// **One entry everywhere but ARM64 Windows**, and that exception is the operating system's own
    /// rather than ours: Windows 11 on ARM runs an x86_64 user-mode process under emulation, which
    /// is what [runtime-packaging.md](../../../../.claude/operations/runtime-packaging.md) already
    /// means when it says *"a Windows-on-ARM machine runs the daemon natively and PHP under
    /// emulation"*. Upstream publishes no ARM64 Windows PHP in any branch, and forty of the
    /// forty-one empty cells on that target have an x86_64 twin — see
    /// [ADR 0023](../../../../.claude/decisions/0023-an-arm64-windows-machine-runs-the-x86_64-build.md).
    ///
    /// macOS is **not** given Rosetta here, for two reasons that agree: the packaging document
    /// refuses emulation for it by name, and all four Unix targets are complete anyway, so there is
    /// no cell to fill. Linux has no emulator the operating system provides.
    ///
    /// **The native target is first and the order is load-bearing** — see [`Package::select`],
    /// which walks this list on the outside and the artifacts on the inside precisely so that a
    /// package built for both Windows cells resolves to the native one rather than to whichever the
    /// generator happened to write first.
    #[must_use]
    pub fn runnable(self) -> &'static [Self] {
        const WINDOWS_X86_64: Target = Target::new(Os::Windows, Arch::X86_64);
        const WINDOWS_AARCH64: Target = Target::new(Os::Windows, Arch::Aarch64);
        const MACOS_X86_64: Target = Target::new(Os::Macos, Arch::X86_64);
        const MACOS_AARCH64: Target = Target::new(Os::Macos, Arch::Aarch64);
        const LINUX_X86_64: Target = Target::new(Os::Linux, Arch::X86_64);
        const LINUX_AARCH64: Target = Target::new(Os::Linux, Arch::Aarch64);

        match (self.os, self.arch) {
            (Os::Windows, Arch::X86_64) => &[WINDOWS_X86_64],
            (Os::Windows, Arch::Aarch64) => &[WINDOWS_AARCH64, WINDOWS_X86_64],
            (Os::Macos, Arch::X86_64) => &[MACOS_X86_64],
            (Os::Macos, Arch::Aarch64) => &[MACOS_AARCH64],
            (Os::Linux, Arch::X86_64) => &[LINUX_X86_64],
            (Os::Linux, Arch::Aarch64) => &[LINUX_AARCH64],
        }
    }
}

/// A moment, as the index writes one: strict RFC 3339 in UTC, to the second.
///
/// # Why this is parsed rather than compared as a string
///
/// This value decides whether an index is a rollback, so getting the comparison wrong is a security
/// bug rather than a display bug. Lexicographic comparison of the strings *would* be correct for the
/// exact shape the generator emits — and silently wrong for `+00:00` instead of `Z`, for a
/// fractional second, or for an unpadded month, all of which are valid RFC 3339 and none of which
/// sorts the way the reader expects.
///
/// So the accepted shape is narrowed to one and everything else is refused. This workspace still has
/// no date library, for the reason [`crate::jobs`] records, and buying a civil-calendar dependency
/// to parse back a format we ourselves emit would be a poor trade for thirty lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp {
    /// Ordered exactly as the fields are declared, which is why the derive is enough: a later year
    /// beats any month, a later month beats any day, and so on down.
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
}

impl Timestamp {
    /// Read `YYYY-MM-DDTHH:MM:SSZ`, and nothing else.
    ///
    /// Ranges are checked but calendars are not: the 31st of February parses. Rejecting it would
    /// need month lengths and leap years, which would buy nothing — this value is compared against
    /// another one from the same generator, never turned into a duration or shown to anybody.
    fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != 20 || bytes[4] != b'-' || bytes[7] != b'-' {
            return None;
        }
        if bytes[10] != b'T' || bytes[13] != b':' || bytes[16] != b':' || bytes[19] != b'Z' {
            return None;
        }

        let number = |from: usize, to: usize| text.get(from..to)?.parse::<u16>().ok();
        let stamp = Self {
            year: number(0, 4)?,
            month: u8::try_from(number(5, 7)?).ok()?,
            day: u8::try_from(number(8, 10)?).ok()?,
            hour: u8::try_from(number(11, 13)?).ok()?,
            minute: u8::try_from(number(14, 16)?).ok()?,
            second: u8::try_from(number(17, 19)?).ok()?,
        };

        // `parse::<u16>` accepts a leading `+`, so the digits are checked rather than assumed:
        // `+2026-08-14T…` is the right length and would otherwise be read as a year.
        if !bytes
            .iter()
            .enumerate()
            .all(|(at, byte)| matches!(at, 4 | 7 | 10 | 13 | 16 | 19) || byte.is_ascii_digit())
        {
            return None;
        }

        (stamp.month <= 12
            && stamp.day <= 31
            && stamp.hour <= 23
            && stamp.minute <= 59
            && stamp.second <= 60)
            .then_some(stamp)
    }
}

/// The door the publishing pipeline comes through — roadmap task **T81a**.
///
/// [`Deserialize`] is the other one, and both reach the same private `parse`. A generator has
/// to *make* a timestamp rather than read one out of a document, and this workspace has no date
/// library to make one from — the note on [`Timestamp`] says why, and buying a civil calendar to
/// produce a format we ourselves emit would be a poor trade. So the shell's `date -u` writes the
/// text and this reads it back.
impl std::str::FromStr for Timestamp {
    type Err = crate::Error;

    fn from_str(text: &str) -> crate::Result<Self> {
        Self::parse(text).ok_or_else(|| crate::Error::Timestamp {
            text: text.to_owned(),
        })
    }
}

impl std::fmt::Display for Timestamp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

impl Serialize for Timestamp {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "{text:?} is not a UTC RFC 3339 second, e.g. 2026-08-14T06:55:12Z"
            ))
        })
    }
}

/// The artifact a target would install, and how it would run it.
///
/// Returned together rather than as an artifact alone, because the second half is a fact only the
/// selection knows: the artifact says what it *is*, and whether the machine that asked runs it
/// natively is a comparison against the target that asked. A caller that dropped it would install
/// an x86_64 build on an ARM machine without saying so, which is the one thing
/// [ADR 0023](../../../../.claude/decisions/0023-an-arm64-windows-machine-runs-the-x86_64-build.md)
/// forbids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection<'a> {
    /// What to download.
    pub artifact: &'a Artifact,

    /// Whether the machine that asked runs it natively or through its operating system's emulation.
    pub execution: Execution,
}

impl Package {
    /// The artifact `target` would install, and how it would run it.
    ///
    /// **Targets on the outside, artifacts on the inside.** Written the other way round — walk the
    /// artifacts and keep the first one that is runnable — it would compile, pass a test with one
    /// Windows artifact in it, and hand the choice between two valid ones to the order the
    /// *generator* happened to write them in. [`Target::runnable`] states a preference and this is
    /// where it is honoured.
    #[must_use]
    pub fn select(&self, target: Target) -> Option<Selection<'_>> {
        target.runnable().iter().find_map(|runnable| {
            self.artifacts
                .iter()
                .find(|artifact| artifact.os == runnable.os && artifact.arch == runnable.arch)
                .map(|artifact| Selection {
                    artifact,
                    execution: match *runnable == target {
                        true => Execution::Native,
                        false => Execution::Emulated,
                    },
                })
        })
    }
}

impl Index {
    /// The package this document has for `kind` at `version`, whatever it was built for.
    fn published(&self, kind: &str, version: &str) -> Option<&Package> {
        self.packages
            .iter()
            .find(|package| package.kind == kind && package.version == version)
    }

    /// What `target` would install for `kind` at `version`.
    #[must_use]
    pub fn select(&self, target: Target, kind: &str, version: &str) -> Option<Selection<'_>> {
        self.published(kind, version)?.select(target)
    }

    /// The same, for the machine this build runs on.
    ///
    /// [`None`] covers three different disappointments — no such package, no such version, and a
    /// version that exists but was built only for systems this one cannot execute — and the caller
    /// that needs to tell them apart walks [`Index::packages`] itself. The common caller is about to
    /// download something and only needs the one answer.
    #[must_use]
    pub fn artifact(&self, kind: &str, version: &str) -> Option<Selection<'_>> {
        self.select(Target::host()?, kind, version)
    }

    /// Every version of `kind` that `target` could install.
    ///
    /// The filter is the point: a version listed only for macOS is not a version a Windows user can
    /// install, and offering it would produce a failure at download time instead of an absence at
    /// list time. Since **T92** it resolves through [`Package::select`], so a version an ARM64
    /// Windows machine can only reach by emulation *is* listed — the alternative was that machine
    /// being shown no PHP at all, in any branch, because upstream builds none for it.
    pub fn installable_for(&self, target: Target, kind: &str) -> impl Iterator<Item = &Package> {
        self.packages
            .iter()
            .filter(move |package| package.kind == kind && package.select(target).is_some())
    }

    /// The same, for the machine this build runs on.
    pub fn installable(&self, kind: &str) -> impl Iterator<Item = &Package> {
        let host = Target::host();
        self.packages.iter().filter(move |package| {
            package.kind == kind && host.is_some_and(|target| package.select(target).is_some())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_accepts_what_the_generator_writes() {
        let stamp = Timestamp::parse("2026-08-14T06:55:12Z").expect("the published shape");
        assert_eq!(stamp.to_string(), "2026-08-14T06:55:12Z");
    }

    /// The door the publishing pipeline comes through — roadmap task **T81a**.
    #[test]
    fn timestamp_parses_from_a_string() {
        let stamp: Timestamp = "2026-09-02T11:00:00Z".parse().expect("a UTC second parses");

        assert_eq!(stamp.to_string(), "2026-09-02T11:00:00Z");
    }

    /// What `FromStr` refuses quotes what it was handed: the caller is a person who typed it.
    #[test]
    fn what_from_str_refuses_names_the_text() {
        let refused = "2026-09-02 11:00:00"
            .parse::<Timestamp>()
            .expect_err("a space is not a T");

        assert!(
            refused.to_string().contains("2026-09-02 11:00:00"),
            "the message should quote what was handed over: {refused}"
        );
    }

    #[test]
    fn timestamp_orders_by_moment_and_not_by_text() {
        let earlier = Timestamp::parse("2026-08-14T06:55:12Z").expect("valid");
        let later = Timestamp::parse("2026-09-02T00:00:00Z").expect("valid");
        assert!(earlier < later);
    }

    #[test]
    fn timestamp_refuses_every_other_rfc_3339_spelling() {
        // All of these are valid RFC 3339 and none of them compares the way a reader expects
        // against the shape above, which is the entire reason this is parsed at all.
        for text in [
            "2026-08-14T06:55:12+00:00",
            "2026-08-14T06:55:12.5Z",
            "2026-08-14 06:55:12Z",
            "2026-8-14T06:55:12Z",
            "+026-08-14T06:55:12Z",
            "",
        ] {
            assert!(
                Timestamp::parse(text).is_none(),
                "{text:?} should be refused"
            );
        }
    }

    #[test]
    fn timestamp_refuses_a_field_out_of_range() {
        assert!(Timestamp::parse("2026-13-14T06:55:12Z").is_none());
        assert!(Timestamp::parse("2026-08-14T24:55:12Z").is_none());
    }

    #[test]
    fn an_artifact_carries_where_its_binaries_are_rather_than_only_their_names() {
        let index: Index = serde_json::from_str(
            r#"{
              "schema": 1,
              "generated_at": "2026-08-14T06:55:12Z",
              "packages": [{
                "kind": "php", "version": "8.3.33", "channel": "stable",
                "artifacts": [{
                  "os": "windows", "arch": "x86_64",
                  "url": "https://example.invalid/php.zip",
                  "sha256": "00", "size": 1,
                  "provides": { "php": "php.exe", "php-cgi": "php-cgi.exe" },
                  "requires": { "vcredist": "2019" },
                  "extension_dir": "ext",
                  "extensions": { "static": ["Core"], "shared": ["curl"] }
                }]
              }]
            }"#,
        )
        .expect("the published shape parses");

        let artifact = &index.packages[0].artifacts[0];
        assert_eq!(artifact.provides["php-cgi"], "php-cgi.exe");
        assert_eq!(artifact.requires.vcredist.as_deref(), Some("2019"));
        assert_eq!(artifact.extensions.compiled_in, ["Core"]);
    }

    #[test]
    fn a_field_this_build_does_not_know_is_not_an_error() {
        // The opposite of `config.toml`'s rule, and deliberately: this document is written by us and
        // read by builds older than it, so adding a field must not break what is already deployed.
        let index: Index = serde_json::from_str(
            r#"{
              "schema": 1,
              "generated_at": "2026-08-14T06:55:12Z",
              "packages": [],
              "mirrors": ["https://example.invalid/"]
            }"#,
        )
        .expect("an unknown field is tolerated");
        assert!(index.packages.is_empty());
    }

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

    /// An index shaped like the published one where it matters: PHP is on five targets and not on
    /// ARM64 Windows, which is true of every branch upstream has ever built.
    fn php_as_published() -> Index {
        let artifact = |os: Os, arch: Arch| {
            serde_json::json!({
                "os": os.as_str(), "arch": arch.as_str(),
                "url": format!("https://example.invalid/php-{}-{}.zip", os.as_str(), arch.as_str()),
                "sha256": "00", "size": 1,
                "provides": { "php": "php.exe" }
            })
        };

        serde_json::from_value(serde_json::json!({
            "schema": 1,
            "generated_at": "2026-08-31T07:40:07Z",
            "packages": [{
                "kind": "php", "version": "8.3.33", "channel": "stable",
                "artifacts": [
                    artifact(Os::Windows, Arch::X86_64),
                    artifact(Os::Macos, Arch::X86_64),
                    artifact(Os::Macos, Arch::Aarch64),
                    artifact(Os::Linux, Arch::X86_64),
                    artifact(Os::Linux, Arch::Aarch64),
                ]
            }]
        }))
        .expect("the published shape parses")
    }

    /// The finding this task exists for — roadmap task **T92**.
    #[test]
    fn an_arm64_windows_machine_is_offered_the_x86_64_build_and_told_so() {
        let index = php_as_published();

        let chosen = index
            .select(Target::new(Os::Windows, Arch::Aarch64), "php", "8.3.33")
            .expect("the x86_64 build is what that machine can run");

        assert_eq!(chosen.artifact.arch, Arch::X86_64);
        assert_eq!(chosen.execution, Execution::Emulated);
    }

    #[test]
    fn every_other_target_is_offered_its_own_build() {
        let index = php_as_published();

        for target in TARGETS {
            let chosen = index
                .select(target, "php", "8.3.33")
                .expect("reachable from all six");

            match target == Target::new(Os::Windows, Arch::Aarch64) {
                true => assert_eq!(chosen.execution, Execution::Emulated),
                false => {
                    assert_eq!(chosen.execution, Execution::Native);
                    assert_eq!(chosen.artifact.os, target.os);
                    assert_eq!(chosen.artifact.arch, target.arch);
                }
            }
        }
    }

    /// A native build wins over one that would have to be emulated, and it wins whichever order the
    /// generator wrote the two artifacts in — which is the whole reason the search walks the
    /// preference list on the outside.
    #[test]
    fn a_native_build_wins_over_one_that_would_have_to_be_emulated() {
        for first in [false, true] {
            let mut index = php_as_published();
            let native: Artifact = serde_json::from_value(serde_json::json!({
                "os": "windows", "arch": "aarch64",
                "url": "https://example.invalid/php-windows-aarch64.zip",
                "sha256": "00", "size": 1, "provides": { "php": "php.exe" }
            }))
            .expect("an artifact");

            match first {
                true => index.packages[0].artifacts.insert(0, native),
                false => index.packages[0].artifacts.push(native),
            }

            let chosen = index
                .select(Target::new(Os::Windows, Arch::Aarch64), "php", "8.3.33")
                .expect("published");

            assert_eq!(chosen.execution, Execution::Native);
            assert_eq!(chosen.artifact.arch, Arch::Aarch64);
        }
    }

    /// `redis 7.2.15` is the shape of this: no Windows artifact at all, so no Windows machine is
    /// offered one and the emulation rule changes nothing.
    #[test]
    fn a_version_with_no_windows_build_is_offered_to_no_windows_machine() {
        let mut index = php_as_published();
        index.packages[0]
            .artifacts
            .retain(|artifact| artifact.os != Os::Windows);

        for arch in [Arch::X86_64, Arch::Aarch64] {
            let target = Target::new(Os::Windows, arch);
            assert!(index.select(target, "php", "8.3.33").is_none());
            assert_eq!(index.installable_for(target, "php").count(), 0);
        }
    }

    /// The listing follows the selection, so a version reachable only by emulation is listed.
    #[test]
    fn a_version_reachable_only_by_emulation_is_listed_as_installable() {
        let index = php_as_published();

        assert_eq!(
            index
                .installable_for(Target::new(Os::Windows, Arch::Aarch64), "php")
                .count(),
            1
        );
        assert_eq!(
            index
                .installable_for(Target::new(Os::Windows, Arch::X86_64), "node")
                .count(),
            0,
            "a kind the index has nothing for is still nothing"
        );
    }

    /// The whole of the emulation rule, asserted per target — roadmap task **T92**.
    #[test]
    fn only_an_arm64_windows_machine_can_run_something_that_is_not_its_own_build() {
        for target in TARGETS {
            let runs = target.runnable();
            assert_eq!(runs[0], target, "a machine prefers its own build, always");

            match (target.os, target.arch) {
                (Os::Windows, Arch::Aarch64) => assert_eq!(
                    runs,
                    [target, Target::new(Os::Windows, Arch::X86_64)],
                    "Windows on ARM runs an x86_64 build under the operating system's emulation"
                ),
                _ => assert_eq!(runs.len(), 1, "{target:?} runs nothing but its own build"),
            }
        }
    }

    /// Six, and the matrix in `runtime-packaging.md` is read in this order.
    #[test]
    fn the_targets_are_the_six_mixengine_ships_a_build_for() {
        assert_eq!(TARGETS[0], Target::new(Os::Windows, Arch::X86_64));
        assert_eq!(
            TARGETS
                .iter()
                .filter(|target| target.os == Os::Macos)
                .count(),
            2
        );
        for (at, target) in TARGETS.iter().enumerate() {
            assert!(
                !TARGETS[at + 1..].contains(target),
                "no target is named twice: {target:?}"
            );
        }
    }

    /// An index from before this field, and an artifact that loads nothing, are both silent.
    #[test]
    fn an_artifact_with_no_extensions_at_all_stays_empty() {
        let extensions: Extensions = serde_json::from_str("{}").expect("an empty object");

        assert!(extensions.is_empty());
        assert_eq!(serde_json::to_string(&extensions).expect("json"), "{}");
    }
}
