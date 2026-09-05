//! The one failure shape every client sees, and the closed set of codes it can carry.
//!
//! Each crate below the daemon keeps its own `thiserror` enum, shaped for the code that raises it.
//! Exactly one of them crosses the wire, and this is it: a machine-readable [`ErrorCode`], a
//! message written for a person, and an optional hint naming the way out. The division of labour is
//! the whole design — **programs branch on the code, people read the message**. That is why the
//! code set is closed and the message is not: a message may be reworded in any release, and nothing
//! downstream is allowed to parse it.

use std::fmt;

/// What went wrong, in one word a program can branch on.
///
/// A closed enum, unlike `mixengine_core::Error` and its neighbours, which are `#[non_exhaustive]`
/// so they can grow a variant without breaking their callers. The trade is deliberate and points
/// the other way here: a new code is a change to the API contract, so every `match` in the CLI, in
/// the GUI and in the daemon's own mapping should stop compiling until somebody has decided what it
/// means. Growth happens in the library enums; this list is the vocabulary they are translated
/// into.
///
/// The set is the one in `.claude/architecture/daemon-and-ipc.md`; the strings are wire format and
/// never change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
// A closed set of codes is the most useful thing the published contract can hand a client, so the
// binding is a literal union rather than `string` — roadmap task T56. This crate's `Serialize` is
// hand-written, so there is no `#[serde(rename_all)]` for `ts-rs` to read and this is a second
// spelling of [`ErrorCode::as_str`]; `crates/mixengine-proto/tests/bindings.rs` asserts the two
// agree, which is what keeps them one list.
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename_all = "snake_case")
)]
pub enum ErrorCode {
    /// The entity named by the request does not exist.
    NotFound,
    /// Creating it would collide with something that already does.
    AlreadyExists,
    /// The request itself is malformed: a bad name, an out-of-range port, a file that does not
    /// parse.
    InvalidArgument,
    /// The request is well formed but fights something else that is currently true — two sites
    /// claiming one domain, a runtime being uninstalled while a pool uses it.
    Conflict,
    /// The system is not in a state where this can be done yet, and the user can get it there.
    PreconditionFailed,
    /// A port MixEngine needs is held by another process. Its own code because the answer is never
    /// "try again" but "here is who has it" (roadmap task T38).
    PortInUse,
    /// The operation needs `mixengine-elevate`, and either nobody has been asked yet or the user
    /// declined. Not a bug and not a dead end: the GUI turns it into a pending-permissions surface.
    PrivilegedRequired,
    /// This operating system genuinely cannot do it. A first-class result, never a panic — see the
    /// cross-platform rule in `CLAUDE.md`.
    UnsupportedPlatform,
    /// Something that has to be installed first is not: a runtime, an extension, a system tool.
    DependencyMissing,
    /// A program ran and failed. Distinct from [`ErrorCode::Io`]: it started and said no, which is
    /// a different problem — and a different fix — from not being able to start it.
    ProcessFailed,
    /// A file, directory or other operating-system object could not be read, written or created.
    /// The local endpoint is one of those objects, and so is an access token.
    Io,
    /// Nothing above fits, which in practice means a bug in MixEngine. The RPC layer also answers
    /// this when a request panics, rather than taking every managed service down with it.
    Internal,
}

impl ErrorCode {
    /// Every code, in the order `.claude/architecture/daemon-and-ipc.md` lists them.
    ///
    /// Exposed for the clients that render one row per code and for the tests that pin the wire
    /// strings — and load-bearing besides, because [`ErrorCode::from_wire`] reads a code back by
    /// searching it. **Adding a variant means editing four places**: [`ErrorCode::as_str`], which
    /// the compiler insists on; this array, which nothing forces and which decides whether the new
    /// code survives a round trip; `DOCUMENTED` in the tests below; and the list in
    /// `daemon-and-ipc.md` that all three answer to.
    pub const ALL: [Self; 12] = [
        Self::NotFound,
        Self::AlreadyExists,
        Self::InvalidArgument,
        Self::Conflict,
        Self::PreconditionFailed,
        Self::PortInUse,
        Self::PrivilegedRequired,
        Self::UnsupportedPlatform,
        Self::DependencyMissing,
        Self::ProcessFailed,
        Self::Io,
        Self::Internal,
    ];

    /// The string this code is written as on the wire.
    ///
    /// Spelled out rather than derived from the variant name by `serde(rename_all)`: these are a
    /// published contract, and a rename refactor should have to say so out loud instead of quietly
    /// changing what every client matches on.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::AlreadyExists => "already_exists",
            Self::InvalidArgument => "invalid_argument",
            Self::Conflict => "conflict",
            Self::PreconditionFailed => "precondition_failed",
            Self::PortInUse => "port_in_use",
            Self::PrivilegedRequired => "privileged_required",
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::DependencyMissing => "dependency_missing",
            Self::ProcessFailed => "process_failed",
            Self::Io => "io",
            Self::Internal => "internal",
        }
    }

    /// The code a wire string names, or `None` when this build has never heard of it.
    ///
    /// Not `FromStr`: the only caller wants the fallback rather than an error type, and there is
    /// nothing useful to put in one.
    #[must_use]
    pub fn from_wire(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|known| known.as_str() == code)
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl serde::Serialize for ErrorCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for ErrorCode {
    /// A code this build does not know becomes [`ErrorCode::Internal`] instead of a parse failure.
    ///
    /// The situation is a client older than the daemon it is talking to. Refusing the payload would
    /// replace the daemon's actual diagnosis — which is in `message`, the part a person reads and
    /// the part that still makes sense — with "invalid response", at exactly the moment something
    /// is already wrong. A program that meets a code it has never heard of cannot branch on it
    /// meaningfully anyway, and `internal` is the honest answer for "a failure this build cannot
    /// classify".
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = <String as serde::Deserialize<'_>>::deserialize(deserializer)?;
        Ok(Self::from_wire(&code).unwrap_or(Self::Internal))
    }
}

/// A failure, as it crosses the wire.
///
/// Built at the daemon boundary from a library error — see the mapping in `mixengine-daemon`, which
/// is the only place that decides a code and writes a hint. By the time one of these exists the
/// error chain has already been flattened into `message`, because a client is given one string and
/// cannot walk a `source()` it does not have.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Error {
    /// What kind of failure this is. Stable; branch on this.
    pub code: ErrorCode,

    /// What happened, phrased for the person reading it, causes included.
    ///
    /// Never parsed by a client and never guaranteed between releases.
    pub message: String,

    /// What to do about it, when there is something to do.
    ///
    /// The GUI renders it as a suggested action next to the message
    /// (`.claude/features/gui.md`), so it is advice and not a restatement: a hint that repeats the
    /// message makes the same sentence appear twice on screen. `None` is the right answer whenever
    /// the message already carries the way out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Error {
    /// A failure with no hint yet.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
        }
    }

    /// Attach the suggested action.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl fmt::Display for Error {
    /// The terminal rendering: the message, and the hint on a line of its own the way `cargo`
    /// prints one.
    ///
    /// This is for `mix`, for `mixengined`'s own startup failures, and for anything that puts an
    /// error into a log. The GUI never uses it — it reads the fields, because a hint there is a
    /// separate element and not a second line of text.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;

        match &self.hint {
            Some(hint) => write!(f, "\nhint: {hint}"),
            None => Ok(()),
        }
    }
}

/// The chain stops here: everything the causes said is already in `message`.
impl std::error::Error for Error {}

/// Flatten an error and its causes into the single string an [`Error`] carries.
///
/// Here rather than at one boundary because there is more than one. The daemon maps `core`,
/// `platform` and `supervisor` errors for the wire; `mix` maps the two or three `platform` failures
/// a client can meet on its own, before any daemon is involved. Both produce the [`message`] of this
/// crate's [`Error`], and both need the same guarantee about it — one sentence, causes appended,
/// nothing printed twice — so the algorithm belongs next to the field it fills rather than being
/// written out once per binary. What stays at each boundary is the part that differs: which
/// [`ErrorCode`] the failure is, and what advice to attach.
///
/// The library messages are written for this: none of them repeats its own `#[source]`, so the
/// result reads as one sentence with its causes appended — `cannot create C:\…: Access is denied.`
///
/// Every piece is trimmed on the way in, because not every cause is ours: `toml::de::Error` ends its
/// (deliberately multi-line) complaint with a newline, and an unnoticed one puts a blank line
/// between the message and the hint in a terminal and a stray `\n` at the end of a JSON string.
///
/// [`message`]: Error::message
#[must_use]
pub fn flatten(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string().trim_end().to_owned();
    let mut cause = error.source();

    while let Some(error) = cause {
        let text = error.to_string().trim_end().to_owned();

        // `#[error(transparent)]` gives a variant its inner error's message *and* keeps that error
        // as the source, so a naive walk prints the same sentence twice. A mapping that meets one
        // of those delegates to the inner error instead of coming here, but the guard costs nothing
        // and the next one added will not have to remember.
        if !message.ends_with(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }

        cause = error.source();
    }

    message
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The list in `.claude/architecture/daemon-and-ipc.md`, in its order. Both halves are the
    /// contract: if this test needs editing, a client somewhere needs editing too.
    const DOCUMENTED: [&str; 12] = [
        "not_found",
        "already_exists",
        "invalid_argument",
        "conflict",
        "precondition_failed",
        "port_in_use",
        "privileged_required",
        "unsupported_platform",
        "dependency_missing",
        "process_failed",
        "io",
        "internal",
    ];

    #[test]
    fn the_codes_are_the_documented_ones() {
        assert_eq!(ErrorCode::ALL.map(ErrorCode::as_str), DOCUMENTED);
    }

    #[test]
    fn every_code_round_trips_through_json() {
        for code in ErrorCode::ALL {
            let encoded = serde_json::to_string(&code).unwrap();
            assert_eq!(encoded, format!("\"{}\"", code.as_str()));
            assert_eq!(serde_json::from_str::<ErrorCode>(&encoded).unwrap(), code);
        }
    }

    #[test]
    fn a_code_this_build_never_heard_of_is_internal() {
        let payload = r#"{"code":"quantum_flux","message":"the daemon is newer than we are"}"#;
        let error: Error = serde_json::from_str(payload).unwrap();

        assert_eq!(error.code, ErrorCode::Internal);
        // The point of the fallback: the sentence a person needs survives.
        assert_eq!(error.message, "the daemon is newer than we are");
    }

    #[test]
    fn a_missing_hint_is_absent_from_the_wire_rather_than_null() {
        let error = Error::new(ErrorCode::Io, "cannot create /nope");

        assert_eq!(
            serde_json::to_string(&error).unwrap(),
            r#"{"code":"io","message":"cannot create /nope"}"#
        );
        assert_eq!(
            serde_json::from_str::<Error>(r#"{"code":"io","message":"cannot create /nope"}"#)
                .unwrap(),
            error
        );
    }

    #[test]
    fn a_hint_is_carried_and_printed_on_its_own_line() {
        let error = Error::new(ErrorCode::PortInUse, "port 80 is in use by nginx")
            .with_hint("stop it, or give the site another port");

        let encoded = serde_json::to_string(&error).unwrap();
        let decoded: Error = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            decoded.hint.as_deref(),
            Some("stop it, or give the site another port")
        );
        assert_eq!(
            error.to_string(),
            "port 80 is in use by nginx\nhint: stop it, or give the site another port"
        );
    }
}
