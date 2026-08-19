//! Version strings and the constraints that select between them.
//!
//! Not runtime-specific and never was: a [`PackageVersion`] is upstream's own string, validated
//! because it becomes a path component, and `packages/<name>/<version>/` needs that as much as
//! `runtimes/<kind>/<version>/` does. The `Runtime` prefix these types carried until T31a described
//! the only caller there happened to be, not what they are about.
//!
//! # The version *grammar* lives here, and the resolution *order* does not
//!
//! T24 added [`VersionConstraint`] and [`PackageVersion::cmp_precedence`] beside the identifier they
//! are about. They are here rather than in `mixengine_core::resolve` because a constraint travels on
//! the wire — `runtime.resolve` takes one — and this crate validates what it carries, on the same
//! reasoning that made a version a validated type in the first place: `^8.3` arriving as a `String`
//! would be refused somewhere further in, with the caller's request already half honoured.
//!
//! What is *not* here is which of four sources a constraint came from. That order — flag,
//! `mixengine.toml`, project record, default — reads a file and a table, which makes it domain logic
//! and `mixengine_core::resolve`'s.

use std::cmp::Ordering;
use std::fmt;

/// Which release channel a published version belongs to.
///
/// The index's own three, mirrored here rather than shared with it: the document's vocabulary
/// belongs to `mixengine_core::index`, which is a description of a *file we publish*, and this one
/// belongs to the wire. They agree today and the conversion between them is one `match` in `core`,
/// which is the price of the two being able to move apart — a channel added to the index for the
/// publishing pipeline's own purposes should not become an API change by accident.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PackageChannel {
    /// Offered by default.
    Stable,
    /// A release candidate. Behind a setting.
    Rc,
    /// A beta. Behind a setting.
    Beta,
}

impl PackageChannel {
    /// The word this is stored and published as.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Rc => "rc",
            Self::Beta => "beta",
        }
    }

    /// Read one back, or [`None`] for a word this build does not know.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "stable" => Some(Self::Stable),
            "rc" => Some(Self::Rc),
            "beta" => Some(Self::Beta),
            _ => None,
        }
    }
}

impl fmt::Display for PackageChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Upstream's version string, exactly as upstream writes it.
///
/// **Validated because it is a path component**, on [`ServiceId`](crate::ServiceId)'s reasoning: an
/// install lands in `runtimes/<kind>/<version>/` or `packages/<name>/<version>/`, so a value
/// carrying a separator, a `..` or a
/// trailing dot is not a lookup that fails — it is a write somewhere nobody meant. The charset is
/// narrow enough that none of those can be spelled at all, which is why the installer can `join`
/// this rather than escape it.
///
/// **Not normalised**, which is what keeps it an identifier: `8.3.33` is the string a user pinned in
/// `mixengine.toml` and the string the index published, and rewriting it here would stop the two
/// matching — the directory on disk is named after it.
///
/// **Two orders, and the derived one is not the version order.** `Ord` is the string's, kept because
/// this type is a `BTreeMap` key in the daemon and a map wants a cheap total order it can rely on.
/// Which of two versions is *newer* is [`cmp_precedence`](Self::cmp_precedence), and it is a
/// different answer: `8.10.0` is newer than `8.9.0` and sorts before it as text. Anything choosing a
/// version for somebody wants the second one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct PackageVersion(String);

impl PackageVersion {
    /// The longest a version may be, in bytes.
    ///
    /// Not a filesystem limit. The longest thing upstream has ever published is on the order of
    /// `8.5.0RC1-dev`, and a value approaching this is somebody sending a sentence.
    pub const MAX_LEN: usize = 32;

    /// The characters a version may contain besides ASCII letters and digits.
    ///
    /// `+` because semantic versioning's build metadata uses it and every target allows it in a
    /// filename; `~` is deliberately absent, because PHP's own ini parser refuses one in an unquoted
    /// value and a path containing it would fail silently at the worst possible moment — the bug
    /// T20a found on a runner and wrote up in the packaging notes.
    const PUNCTUATION: [char; 4] = ['.', '-', '_', '+'];

    /// Read a version, refusing anything that could not be a directory name.
    ///
    /// # Errors
    ///
    /// [`VersionError`] naming what is wrong with the value, phrased for whoever typed it.
    pub fn parse(value: impl Into<String>) -> Result<Self, VersionError> {
        let value = value.into();

        let reject = |reason: &str| {
            Err(VersionError {
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

        // **A version begins with a digit**, which is where most of the safety comes from rather
        // than from a list of refusals: `.`, `..`, `-rf` and every name Windows reserves (`CON`,
        // `AUX`, `NUL`) are all excluded by this one rule, and every version any of these four
        // upstreams has ever published satisfies it.
        if !value.starts_with(|first: char| first.is_ascii_digit()) {
            return reject("it does not begin with a digit");
        }
        if value.ends_with('.') {
            // Windows strips a trailing dot from a directory name, so `8.3.` and `8.3` would be the
            // same directory while being different rows.
            return reject("it ends with a dot");
        }
        if let Some(bad) = value
            .chars()
            .find(|c| !c.is_ascii_alphanumeric() && !Self::PUNCTUATION.contains(c))
        {
            return reject(&format!("it contains {bad:?}"));
        }

        Ok(Self(value))
    }

    /// The string, for a path, a query or a message.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Which of two versions is newer, as upstream means it rather than as ASCII does.
    ///
    /// **Not `Ord`**, deliberately: see the note on this type. Overriding the derived order would
    /// make `BTreeMap<PackageVersion, _>` silently change what it keys on, and a comparison this
    /// expensive hidden behind `<` is one nobody would think about.
    ///
    /// Three rules, and each one exists because an upstream this manages publishes something that
    /// needs it:
    ///
    /// - **Numeric segments compare as numbers**, so `8.10.0` is newer than `8.9.0`. A missing
    ///   segment is zero: `8.3` and `8.3.0` are the same release.
    /// - **A release is newer than its own pre-releases**, so `8.5.0` beats `8.5.0RC1` — PHP
    ///   publishes both, and the string order puts them the wrong way round.
    /// - **Build metadata is not a version.** Semantic versioning says `+build4` never decides
    ///   precedence, and two artifacts differing only there are the same release built twice.
    #[must_use]
    pub fn cmp_precedence(&self, other: &Self) -> Ordering {
        precedence(&self.0).cmp(&precedence(&other.0))
    }
}

/// One version, taken apart far enough to be compared with another.
///
/// Borrowed rather than owned — every string it points into outlives the comparison — so ordering a
/// list of versions allocates nothing but the segment vectors.
#[derive(Debug)]
struct Precedence<'a> {
    /// The leading numeric segments, most significant first.
    core: Vec<u64>,

    /// Whatever followed them, with build metadata already dropped: `RC1`, `-rc1`, `-dev`, or
    /// nothing at all.
    tail: &'a str,
}

impl Precedence<'_> {
    /// Whether two versions are the same numbered release, `8.3` and `8.3.0` included.
    ///
    /// The pre-release rule needs this rather than `==` on the vectors: a missing segment is a zero
    /// everywhere else in this file, and one place where it is a difference would be the place the
    /// rule fails.
    fn same_release(&self, other: &Self) -> bool {
        (0..self.core.len().max(other.core.len()))
            .all(|index| self.segment(index) == other.segment(index))
    }

    /// One segment, with the zeros nobody wrote.
    fn segment(&self, index: usize) -> u64 {
        self.core.get(index).copied().unwrap_or(0)
    }
}

impl Ord for Precedence<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Over the longer of the two, so `8.3` and `8.3.0` compare equal rather than by length.
        for index in 0..self.core.len().max(other.core.len()) {
            let ordering = self.segment(index).cmp(&other.segment(index));
            if ordering != Ordering::Equal {
                return ordering;
            }
        }

        match (self.tail.is_empty(), other.tail.is_empty()) {
            (true, true) => Ordering::Equal,
            // The release is newer than anything leading up to it.
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => alphanumeric(self.tail, other.tail),
        }
    }
}

impl PartialOrd for Precedence<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// Written out rather than derived: `Ord` says `8.3` and `8.3.0` are one release and a derive would
// say they are two — an `Eq` disagreeing with its own `cmp` is a comparison that means one thing in
// a `sort` and another in an `==`.
impl PartialEq for Precedence<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Precedence<'_> {}

/// Take a version string apart: leading numbers, then everything else.
///
/// Written by hand rather than by splitting on `.`, because `8.5.0RC1` has to become `8 · 5 · 0` and
/// `RC1` — the suffix arrives *attached* to the last number, with no separator, which is how PHP
/// spells a release candidate and what a naive `split('.')` gets wrong.
///
/// A segment longer than a `u64` saturates instead of failing. Nothing upstream publishes a
/// twenty-digit segment, and a comparison is not the place to refuse one: the value was already
/// accepted as a version, and this function's caller is choosing between versions rather than
/// validating them.
fn precedence(version: &str) -> Precedence<'_> {
    // Semantic versioning: build metadata never participates in precedence.
    let version = version.split('+').next().unwrap_or(version);

    let mut core = Vec::new();
    let mut rest = version;

    loop {
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            break;
        }

        core.push(rest[..digits].parse::<u64>().unwrap_or(u64::MAX));
        rest = &rest[digits..];

        // Another segment only if a dot separates one, and only if a digit follows it: the `.` in
        // `1.2.3-rc.1` belongs to the pre-release, not to the version.
        match rest.strip_prefix('.') {
            Some(next) if next.starts_with(|c: char| c.is_ascii_digit()) => rest = next,
            _ => break,
        }
    }

    Precedence { core, tail: rest }
}

/// Compare two pre-release tails the way a person reads them.
///
/// Digit runs as numbers and everything else as text, so `RC10` is newer than `RC2` — which plain
/// ASCII gets backwards, and which is the whole reason this is not a `str` comparison. Case is
/// folded because `RC1` and `rc1` are the same release written by two publishers.
fn alphanumeric(left: &str, right: &str) -> Ordering {
    /// The leading run of characters on the same side of the digit/not-digit line.
    fn run(text: &str, digits: bool) -> &str {
        let length = text
            .find(|c: char| c.is_ascii_digit() != digits)
            .unwrap_or(text.len());

        &text[..length]
    }

    let (mut left, mut right) = (left, right);

    while !left.is_empty() && !right.is_empty() {
        let digits = left.starts_with(|c: char| c.is_ascii_digit());
        if digits != right.starts_with(|c: char| c.is_ascii_digit()) {
            // A number and a word at the same position: the number is the smaller, as semantic
            // versioning has it — `1` before `alpha`.
            return match digits {
                true => Ordering::Less,
                false => Ordering::Greater,
            };
        }

        let (mine, theirs) = (run(left, digits), run(right, digits));

        let ordering = match digits {
            true => mine
                .parse::<u64>()
                .unwrap_or(u64::MAX)
                .cmp(&theirs.parse::<u64>().unwrap_or(u64::MAX)),
            false => mine.to_ascii_lowercase().cmp(&theirs.to_ascii_lowercase()),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }

        left = &left[mine.len()..];
        right = &right[theirs.len()..];
    }

    // One ran out: the shorter tail is the earlier release (`rc1` before `rc1a`).
    left.len().cmp(&right.len())
}

impl fmt::Display for PackageVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for PackageVersion {
    /// Validating, for [`ServiceId`](crate::ServiceId)'s reason: a version that cannot be a
    /// directory name fails later, further from the cause, in the middle of an install.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize<'_>>::deserialize(deserializer)?;

        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// What is wrong with a version somebody typed.
///
/// Its own type rather than a variant of [`SpecError`](crate::SpecError): that one belongs to the
/// service-spec vocabulary, and a version is refused in places — a command line, a wire request —
/// that have nothing to do with a spec.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{value:?} is not a version: {reason}")]
pub struct VersionError {
    /// What was offered.
    pub value: String,
    /// Why it was refused, as a sentence that completes "it …".
    pub reason: String,
}

/// A version somebody asked for, which may name more than one.
///
/// What a `--version` flag, a `MIXENGINE_PHP`, a `mixengine.toml` pin and a project record all
/// carry. Three forms, which is what
/// [runtime-versions.md](../../../../.claude/features/runtime-versions.md) promises:
///
/// | Written | Means |
/// | --- | --- |
/// | `8.3.12`, `8.3`, `8` | **a prefix.** Every installed version whose segments begin with these |
/// | `^8.3` | **a caret.** From `8.3.0` up to, but not including, `9.0.0` |
/// | `8.5.0RC1` | **that release exactly**, pre-release and all |
///
/// One rule holds the first two together and is worth stating out loud, because it is the surprise
/// otherwise: **a constraint with no pre-release in it never selects a pre-release.** `8.5` does not
/// mean `8.5.0RC1`, and neither does `^8.5` — somebody who wants a release candidate names it, which
/// is exactly how it is spelled on the third row.
///
/// **Resolved against installed versions and never against downloadable ones.** That is the feature
/// spec's word, and the reason is that the alternative is a `cd` into a directory silently
/// downloading eighty megabytes. What a constraint matching nothing produces is a message naming the
/// install command.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct VersionConstraint(String);

impl VersionConstraint {
    /// The longest a constraint may be: a version, plus the `^`.
    pub const MAX_LEN: usize = PackageVersion::MAX_LEN + 1;

    /// Read a constraint, refusing anything that is not one of the three forms.
    ///
    /// The value after any `^` is held to [`PackageVersion::parse`]'s own rules rather than to
    /// looser ones. A constraint is not a path component, so it does not have to be — but a range
    /// whose characters could not name a version could not select one either, and two charsets
    /// would be two places to argue about `~`.
    ///
    /// # Errors
    ///
    /// [`VersionError`] naming what is wrong with the value, phrased for whoever typed it.
    pub fn parse(value: impl Into<String>) -> Result<Self, VersionError> {
        let value = value.into();

        if value.len() > Self::MAX_LEN {
            return Err(VersionError {
                reason: format!("it is longer than {} characters", Self::MAX_LEN),
                value,
            });
        }

        // The `^` is this type's whole vocabulary beyond a version's, so it is stripped here and the
        // rest is held to the rule that already exists.
        let bare = value.strip_prefix('^').unwrap_or(&value);
        PackageVersion::parse(bare).map_err(|error| VersionError {
            value: value.clone(),
            reason: error.reason,
        })?;

        Ok(Self(value))
    }

    /// The exact version this names, when it names exactly one.
    ///
    /// [`None`] for every range, and for a bare `8.3` — which *looks* exact and is not, since it
    /// selects whatever `8.3.x` is installed. What this is for is the hint on a failed resolution:
    /// only a constraint naming one version can be turned into the `mix runtime install` command
    /// that would satisfy it.
    #[must_use]
    pub fn exact(&self) -> Option<PackageVersion> {
        if self.0.starts_with('^') {
            return None;
        }

        let taken = precedence(&self.0);
        // Three numbers, or anything carrying a pre-release, is a single release. One or two
        // numbers is a series.
        if taken.core.len() < 3 && taken.tail.is_empty() {
            return None;
        }

        PackageVersion::parse(self.0.clone()).ok()
    }

    /// Whether an installed version answers this.
    #[must_use]
    pub fn matches(&self, version: &PackageVersion) -> bool {
        let (caret, text) = match self.0.strip_prefix('^') {
            Some(rest) => (true, rest),
            None => (false, self.0.as_str()),
        };

        let asked = precedence(text);
        let have = precedence(version.as_str());

        // A pre-release is only ever selected by a constraint that names one, and then only within
        // its own release. Without this, `^8.3` on a machine holding 8.4.0RC1 resolves to a release
        // candidate nobody asked for.
        if !have.tail.is_empty() && (asked.tail.is_empty() || !asked.same_release(&have)) {
            return false;
        }

        if caret {
            return have >= asked && have < ceiling(&asked);
        }

        // A prefix: as many segments as were written have to agree, and a tail that was written has
        // to be the same release. `8.3` accepts `8.3.33`; `8.3.33` accepts only itself.
        (0..asked.core.len()).all(|index| asked.segment(index) == have.segment(index))
            && asked.tail.eq_ignore_ascii_case(have.tail)
    }

    /// The string, as it was written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The first release a caret constraint no longer accepts.
///
/// Semantic versioning's rule rather than "the next major", and the difference is only visible below
/// 1.0: a caret allows anything that does not change the leftmost **non-zero** segment, so `^0.12`
/// stops at `0.13` and not at `1.0`. Node's own 0.x line is why that is not a hypothetical — its
/// minor releases were breaking ones, and a `^0.12` that accepted `0.13` would be honouring the
/// letter of the rule against everything the person meant.
fn ceiling(asked: &Precedence<'_>) -> Precedence<'static> {
    let mut core = asked.core.clone();
    let bumped = core.iter().position(|segment| *segment != 0).unwrap_or(0);

    core.truncate(bumped + 1);
    core[bumped] = core[bumped].saturating_add(1);

    Precedence { core, tail: "" }
}

impl fmt::Display for VersionConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<PackageVersion> for VersionConstraint {
    /// Every version is a constraint naming itself, which is what an installed version pinned by a
    /// project record arrives as.
    fn from(version: PackageVersion) -> Self {
        Self(version.0)
    }
}

impl<'de> serde::Deserialize<'de> for VersionConstraint {
    /// Validating, for [`PackageVersion`]'s reason one step further out: a range that is not one is
    /// a request that would otherwise be refused in the middle of being honoured.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize<'_>>::deserialize(deserializer)?;

        Self::parse(value).map_err(serde::de::Error::custom)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_round_trips_through_the_word_it_is_stored_as() {
        for channel in [
            PackageChannel::Stable,
            PackageChannel::Rc,
            PackageChannel::Beta,
        ] {
            assert_eq!(PackageChannel::parse(channel.as_str()), Some(channel));
        }
    }

    #[test]
    fn the_versions_these_upstreams_actually_publish_are_accepted() {
        for version in [
            "8.3.33",
            "7.0.33",
            "8.5.0RC1",
            "20.11.0",
            "3.12.1",
            "3.3.6",
            "1.2.3+build4",
        ] {
            assert_eq!(
                PackageVersion::parse(version).expect(version).as_str(),
                version
            );
        }
    }

    /// The whole reason this type validates: every one of these is a write somewhere nobody meant.
    #[test]
    fn a_version_that_could_leave_its_own_directory_is_refused() {
        for version in [
            "",
            "..",
            ".",
            "../../etc/passwd",
            "8.3/../..",
            "8.3\\33",
            "8.3.",
            "-rf",
            "CON",
            "8.3 33",
            "8.3.33-longer-than-anything-upstream-has-ever-published",
        ] {
            assert!(
                PackageVersion::parse(version).is_err(),
                "{version:?} should be refused"
            );
        }
    }

    fn constraint(text: &str) -> VersionConstraint {
        VersionConstraint::parse(text).unwrap_or_else(|error| panic!("{text:?}: {error}"))
    }

    /// The order this whole task exists to get right: as numbers, not as text.
    #[test]
    fn a_version_is_newer_than_another_the_way_upstream_means_it() {
        let newest = |versions: [&str; 2]| {
            let [left, right] = versions.map(|text| PackageVersion::parse(text).expect(text));
            match left.cmp_precedence(&right) {
                Ordering::Greater => left.as_str().to_owned(),
                _ => right.as_str().to_owned(),
            }
        };

        // The one an alphabetical sort gets backwards, which is why `records` does not use one.
        assert_eq!(newest(["8.9.0", "8.10.0"]), "8.10.0");
        assert_eq!(newest(["8.3.33", "8.3.4"]), "8.3.33");
        assert_eq!(newest(["7.4.33", "8.0.0"]), "8.0.0");
        // A release beats its own candidates, and the candidates are in the order they were cut.
        assert_eq!(newest(["8.5.0RC1", "8.5.0"]), "8.5.0");
        assert_eq!(newest(["8.5.0RC2", "8.5.0RC10"]), "8.5.0RC10");
        assert_eq!(newest(["8.5.0alpha1", "8.5.0beta1"]), "8.5.0beta1");

        let (short, padded) = (
            PackageVersion::parse("8.3").expect("a version"),
            PackageVersion::parse("8.3.0").expect("a version"),
        );
        assert_eq!(
            short.cmp_precedence(&padded),
            Ordering::Equal,
            "a segment nobody wrote is a zero"
        );

        // Build metadata is not a version: semantic versioning says so, and two artifacts differing
        // only there are one release built twice.
        let (plain, built) = (
            PackageVersion::parse("1.2.3").expect("a version"),
            PackageVersion::parse("1.2.3+build4").expect("a version"),
        );
        assert_eq!(plain.cmp_precedence(&built), Ordering::Equal);
    }

    /// The three forms the feature spec promises, against the versions these upstreams publish.
    #[test]
    fn a_constraint_selects_the_versions_it_names_and_no_others() {
        for (asked, version, expected) in [
            // Prefix, one segment at a time.
            ("8", "8.3.33", true),
            ("8", "7.4.33", false),
            ("8.3", "8.3.33", true),
            ("8.3", "8.30.1", false),
            ("8.3", "8.4.0", false),
            ("8.3.33", "8.3.33", true),
            ("8.3.33", "8.3.34", false),
            // Caret: up to the next major, and no further.
            ("^8.3", "8.3.0", true),
            ("^8.3", "8.4.12", true),
            ("^8.3", "8.2.20", false),
            ("^8.3", "9.0.0", false),
            // Below 1.0 the caret stops at the minor, which is Node's own 0.x line.
            ("^0.12", "0.12.18", true),
            ("^0.12", "0.13.0", false),
            // A pre-release is only ever selected by a constraint that names one.
            ("8.5", "8.5.0RC1", false),
            ("^8.5", "8.5.0RC1", false),
            ("8.5.0RC1", "8.5.0RC1", true),
            ("8.5.0rc1", "8.5.0RC1", true),
            ("8.5.0RC1", "8.5.0", false),
            ("^8.5.0RC1", "8.5.0RC2", true),
            ("^8.5.0RC1", "8.6.0RC1", false),
        ] {
            let version = PackageVersion::parse(version).expect(version);
            assert_eq!(
                constraint(asked).matches(&version),
                expected,
                "{asked} against {version}"
            );
        }
    }

    /// Only a constraint naming one release can become the `mix runtime install` a failed
    /// resolution suggests — which is the whole reason this exists.
    #[test]
    fn a_constraint_says_whether_it_names_exactly_one_version() {
        assert_eq!(
            constraint("8.3.33").exact().map(|v| v.as_str().to_owned()),
            Some("8.3.33".to_owned())
        );
        assert_eq!(
            constraint("8.5.0RC1")
                .exact()
                .map(|v| v.as_str().to_owned()),
            Some("8.5.0RC1".to_owned())
        );

        for range in ["8", "8.3", "^8.3", "^8.3.33"] {
            assert_eq!(
                constraint(range).exact(),
                None,
                "{range} names a series rather than a release"
            );
        }
    }

    #[test]
    fn a_constraint_that_could_not_name_a_version_is_refused() {
        for text in ["", "^", "~8.3", ">=8.3", "8.3 || 8.4", "^../escape", "php"] {
            assert!(
                VersionConstraint::parse(text).is_err(),
                "{text:?} should be refused"
            );
        }

        let constraint: VersionConstraint =
            serde_json::from_str(r#""^8.3""#).expect("a caret is a constraint");
        assert_eq!(constraint.as_str(), "^8.3");
        assert_eq!(serde_json::to_string(&constraint).unwrap(), r#""^8.3""#);
        serde_json::from_str::<VersionConstraint>(r#""~8.3""#)
            .expect_err("validated on the way in, like every identifier here");
    }

    /// Refused on the way in as well as on construction — the wire is where an untrusted one
    /// arrives.
    #[test]
    fn a_version_is_validated_when_it_is_read_off_the_wire() {
        let error = serde_json::from_str::<PackageVersion>(r#""../escape""#)
            .expect_err("not a version")
            .to_string();

        assert!(error.contains("does not begin with a digit"), "{error}");
    }
}
