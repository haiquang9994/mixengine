//! The JSON-RPC 2.0 envelope every call travels in.
//!
//! The spec is followed rather than approximated, because the reason
//! `.claude/architecture/daemon-and-ipc.md` chose HTTP in the first place was off-the-shelf clients:
//! a `jsonrpc` member on both sides, an integer `error.code`, `id` echoed back on every response and
//! omitted only by a notification, batches as arrays.
//!
//! **Two error codes, and they are not competing.** JSON-RPC insists `error.code` is an integer, and
//! its useful values are the five reserved ones — a client library can tell "you sent nonsense" from
//! "the server broke" without knowing anything about MixEngine. That is all it can tell, which is
//! why every error also carries [`ErrorData`] with the [`ErrorCode`] from
//! [`crate::Error`]: the integer is for generic tooling, the string is what `mix` and the GUI
//! actually branch on. The message is written once, in the standard `message` member, and not
//! repeated inside `data`.

use serde_json::Value;

use crate::{Error, ErrorCode};

/// The methods this build answers, named once so a client and the daemon cannot drift apart.
///
/// Namespaced `namespace.verb` as `.claude/architecture/daemon-and-ipc.md` requires. The list grows
/// with the phase that implements each namespace; a method not in it is answered with
/// [`RpcCode::METHOD_NOT_FOUND`].
pub mod method {
    /// Everything the daemon knows about itself and its home. See [`DaemonStatus`](crate::DaemonStatus).
    pub const DAEMON_STATUS: &str = "daemon.status";

    /// Build and protocol version alone — the cheap half of [`DAEMON_STATUS`], for the handshake a
    /// client does before it decides whether it can talk to this daemon at all.
    pub const DAEMON_VERSION: &str = "daemon.version";
}

/// The `"jsonrpc": "2.0"` member.
///
/// A unit type rather than a `String`, so that a payload claiming another version fails to
/// deserialise instead of being served as if it had said 2.0, and so that the daemon cannot forget
/// to write it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Version;

impl Version {
    /// The only value this member is ever allowed to have.
    pub const STR: &'static str = "2.0";
}

impl serde::Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(Self::STR)
    }
}

impl<'de> serde::Deserialize<'de> for Version {
    /// Deserialised as an owned `String` rather than a borrowed `&str` on purpose: the daemon reads
    /// a body into a [`Value`] first — a batch and a single call are told apart before either is
    /// decoded — and a `&str` cannot borrow out of one, so the borrowing version would fail on
    /// every request that arrived the way they all actually arrive.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let version = String::deserialize(deserializer)?;

        if version == Self::STR {
            Ok(Self)
        } else {
            Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Str(&version),
                &Self::STR,
            ))
        }
    }
}

/// What a client called its request, echoed back untouched on the response that answers it.
///
/// The spec allows a string, a number or null, and says null is discouraged — so null is not
/// representable here, and a client of this crate cannot accidentally send one. It stays legal on a
/// *response*, which is why [`Response::id`] is an `Option<Id>` and this enum is not: an answer
/// carries a null id when the request's own id could not be read, and when the request spelled its
/// id `null` and is echoed the way it asked to be.
///
/// Numbers are `i64` rather than `f64`: the spec discourages fractional ids, and a client that sends
/// one gets `invalid_request` instead of an id that comes back subtly different from what it sent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum Id {
    /// A numeric id, the form nearly every client uses.
    Number(i64),
    /// A string id.
    Text(String),
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(number) => write!(f, "{number}"),
            Self::Text(text) => f.write_str(text),
        }
    }
}

/// One call.
///
/// `params` stays a [`Value`] until the method is known, because only the handler knows what shape
/// it expects — decoding it here would mean one enormous enum of every request type in the API, and
/// an unknown method would fail as a parse error rather than as `method_not_found`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Request {
    /// Always `"2.0"`.
    pub jsonrpc: Version,

    /// `namespace.verb` — see [`method`].
    pub method: String,

    /// The method's arguments, if it takes any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,

    /// What to echo on the answer.
    ///
    /// `None` here means either of two things the spec keeps apart — an absent `id`, which is a
    /// notification, and `"id":null`, which is a request that still has to be answered — because
    /// `Option<Id>` reads both as the same value. Anything that has to tell them apart looks at the
    /// undecoded JSON, which is why the daemon decides that before it decodes a call and why
    /// [`Request::is_notification`] says out loud that it cannot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
}

impl Request {
    /// A call that expects an answer.
    pub fn new(method: impl Into<String>, params: Option<Value>, id: Id) -> Self {
        Self {
            jsonrpc: Version,
            method: method.into(),
            params,
            id: Some(id),
        }
    }

    /// Whether the caller asked for an answer, as far as a decoded request can tell.
    ///
    /// **Not enough on its own to decide whether to answer.** A notification is a request with no
    /// `id` *member*; `"id":null` is a request with one, and the spec does not let it mean silence.
    /// Both arrive here as `None`, so a server reads the raw JSON — see [`Request::id`] — and this
    /// is only the shorthand for a client asking what it built.
    #[must_use]
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// One answer.
///
/// Exactly one of `result` and `error` is present, which is why [`Outcome`] is an enum and not two
/// `Option` fields: the invalid state — both, or neither — is simply not constructible.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Response {
    /// Always `"2.0"`.
    pub jsonrpc: Version,

    /// The result, or the failure.
    #[serde(flatten)]
    pub outcome: Outcome,

    /// The id of the request this answers.
    ///
    /// `None` serialises as `null`, which the spec requires for a body so malformed that the id
    /// could not be read out of it — and which is also what a request that spelled its own id
    /// `null` gets back, since an answer echoes the id it was given rather than improving on it.
    pub id: Option<Id>,
}

impl Response {
    /// An answer carrying a result.
    ///
    /// `id` is an `Option` for the same reason [`Response::failure`]'s is: a request may have
    /// spelled its id `null`, and it is answered rather than corrected.
    pub fn success(id: Option<Id>, result: Value) -> Self {
        Self {
            jsonrpc: Version,
            outcome: Outcome::Success { result },
            id,
        }
    }

    /// An answer carrying a failure.
    pub fn failure(id: Option<Id>, error: RpcError) -> Self {
        Self {
            jsonrpc: Version,
            outcome: Outcome::Failure { error },
            id,
        }
    }
}

/// The half of a [`Response`] that is either a result or an error, never both.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum Outcome {
    /// The method ran. `result` is whatever that method documents; `null` when it returns nothing.
    Success {
        /// The method's return value.
        result: Value,
    },
    /// The method did not run, or ran and failed.
    Failure {
        /// Why.
        error: RpcError,
    },
}

/// The integer in a JSON-RPC `error` member.
///
/// A transparent newtype with constants rather than a closed enum, unlike
/// [`ErrorCode`]. The reasoning that made *that* one closed does not apply here:
/// nothing branches on this number — [`ErrorData::code`] is what a client matches on — so an
/// unfamiliar value has nothing to be matched against and is simply carried through instead of
/// being flattened into a wrong one.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct RpcCode(pub i32);

impl RpcCode {
    /// The body is not JSON.
    pub const PARSE_ERROR: Self = Self(-32700);

    /// The body is JSON but not a request: a missing `method`, a `jsonrpc` that is not `"2.0"`, an
    /// empty batch.
    pub const INVALID_REQUEST: Self = Self(-32600);

    /// No such method in this build. Also what an older client meets when it calls something a
    /// newer daemon has, so the message names the method rather than only the failure.
    pub const METHOD_NOT_FOUND: Self = Self(-32601);

    /// The method exists and its `params` are the wrong shape.
    pub const INVALID_PARAMS: Self = Self(-32602);

    /// A bug in the daemon, including a handler that panicked.
    pub const INTERNAL_ERROR: Self = Self(-32603);

    /// Everything MixEngine itself refuses: a site that does not exist, a port already held, an
    /// operation this OS cannot do. Inside the `-32000..=-32099` range the spec reserves for
    /// implementation-defined errors, and deliberately one value rather than a range — the
    /// distinction a client needs is in [`ErrorData::code`], and mirroring twelve codes onto twelve
    /// integers would create a second vocabulary to keep in step with the first.
    pub const APPLICATION_ERROR: Self = Self(-32000);
}

/// The `error` member of a failed [`Response`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RpcError {
    /// For generic JSON-RPC tooling. See [`RpcCode`].
    pub code: RpcCode,

    /// What happened, phrased for the person reading it, causes included. The same string
    /// [`Error::message`] carries, and written exactly once.
    pub message: String,

    /// MixEngine's own classification of the failure, which is what clients branch on.
    ///
    /// Always present, including on the protocol-level failures above, so a client never has to
    /// handle "an error with no code".
    pub data: ErrorData,
}

impl RpcError {
    /// A failure MixEngine itself produced, at [`RpcCode::APPLICATION_ERROR`].
    ///
    /// This is the conversion for everything that comes out of the daemon's `ToWire` mapping — the
    /// method was found, its params parsed, and the work then failed for a reason the user can
    /// usually act on.
    #[must_use]
    pub fn application(error: Error) -> Self {
        Self::at(RpcCode::APPLICATION_ERROR, error)
    }

    /// A failure at a chosen JSON-RPC code — the protocol-level ones, which happen before any
    /// method has run.
    #[must_use]
    pub fn at(code: RpcCode, error: Error) -> Self {
        Self {
            code,
            message: error.message,
            data: ErrorData {
                code: error.code,
                hint: error.hint,
            },
        }
    }

    /// Back into the error shape the rest of MixEngine speaks.
    ///
    /// Clients use this the moment they have a response: below this point nothing knows or cares
    /// that the call arrived over JSON-RPC.
    #[must_use]
    pub fn into_error(self) -> Error {
        Error {
            code: self.data.code,
            message: self.message,
            hint: self.data.hint,
        }
    }
}

/// What MixEngine adds to a JSON-RPC error: the stable code, and the way out where there is one.
///
/// The message is *not* here — it is the standard `message` member one level up, and duplicating it
/// would put the same sentence on screen twice.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ErrorData {
    /// The code from [`crate::Error`]. Branch on this.
    pub code: ErrorCode,

    /// The suggested action, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_is_the_json_the_spec_describes() {
        let request = Request::new(method::DAEMON_STATUS, None, Id::Text("status-1".to_owned()));

        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"jsonrpc":"2.0","method":"daemon.status","id":"status-1"}"#
        );
    }

    #[test]
    fn a_request_without_an_id_is_a_notification() {
        let request: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"daemon.status"}"#).unwrap();

        assert!(request.is_notification());
        // Absent rather than null: a notification that serialised `"id":null` would be a request
        // the spec discourages, and one every conforming server would answer.
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"jsonrpc":"2.0","method":"daemon.status"}"#
        );
    }

    #[test]
    fn a_null_id_decodes_to_the_same_none_an_absent_one_does() {
        // The reason a server cannot decide "notification" from a decoded request: `"id":null` is
        // a request the spec expects an answer to, and it is indistinguishable here. Pinned in this
        // crate so that a later change to `Id` — a `Null` variant, say — has to face this test
        // rather than quietly change what the daemon treats as silence.
        let null: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"daemon.status","id":null}"#)
                .unwrap();

        assert_eq!(null.id, None);
        assert!(
            null.is_notification(),
            "which is exactly why it is not enough"
        );
    }

    #[test]
    fn another_protocol_version_is_refused_rather_than_assumed() {
        let error =
            serde_json::from_str::<Request>(r#"{"jsonrpc":"1.0","method":"daemon.status"}"#)
                .expect_err("1.0 is not this protocol");

        assert!(error.to_string().contains("2.0"), "{error}");
    }

    #[test]
    fn a_result_and_an_error_are_the_same_shape_apart_from_which_member_is_there() {
        let success =
            Response::success(Some(Id::Number(7)), serde_json::json!({"version": "0.1.0"}));
        assert_eq!(
            serde_json::to_string(&success).unwrap(),
            r#"{"jsonrpc":"2.0","result":{"version":"0.1.0"},"id":7}"#
        );

        let failure = Response::failure(
            Some(Id::Number(7)),
            RpcError::application(Error::new(ErrorCode::NotFound, "no such site: blog.test")),
        );
        assert_eq!(
            serde_json::to_string(&failure).unwrap(),
            r#"{"jsonrpc":"2.0","error":{"code":-32000,"message":"no such site: blog.test","data":{"code":"not_found"}},"id":7}"#
        );
    }

    #[test]
    fn a_response_round_trips_into_the_outcome_it_carries() {
        let encoded = r#"{"jsonrpc":"2.0","result":null,"id":1}"#;
        let response: Response = serde_json::from_str(encoded).unwrap();

        assert!(matches!(
            response.outcome,
            Outcome::Success {
                result: Value::Null
            }
        ));
        assert_eq!(serde_json::to_string(&response).unwrap(), encoded);
    }

    #[test]
    fn an_unreadable_request_is_answered_with_a_null_id() {
        let response = Response::failure(
            None,
            RpcError::at(
                RpcCode::PARSE_ERROR,
                Error::new(ErrorCode::InvalidArgument, "the request body is not JSON"),
            ),
        );

        // The one place a null id is correct, and the reason `Response::id` is an `Option` while
        // `Id` itself has no null variant.
        assert!(
            serde_json::to_string(&response)
                .unwrap()
                .ends_with(r#","id":null}"#)
        );
    }

    #[test]
    fn an_error_becomes_the_one_the_rest_of_mixengine_speaks() {
        let original = Error::new(ErrorCode::PortInUse, "port 80 is in use by nginx")
            .with_hint("stop it, or give the site another port");

        let recovered = RpcError::application(original.clone()).into_error();

        assert_eq!(recovered, original);
    }

    #[test]
    fn the_message_is_written_once_and_not_repeated_inside_data() {
        let error = RpcError::application(Error::new(ErrorCode::Io, "cannot create /nope"));
        let encoded = serde_json::to_string(&error).unwrap();

        assert_eq!(
            encoded.matches("cannot create /nope").count(),
            1,
            "{encoded}"
        );
    }

    #[test]
    fn an_unfamiliar_numeric_code_survives_the_trip() {
        // A daemon newer than this client, using a value from the implementation-defined range that
        // this build has no constant for. Nothing branches on the integer, so it is carried rather
        // than rounded off to one we do know.
        let decoded: RpcError = serde_json::from_str(
            r#"{"code":-32050,"message":"something new","data":{"code":"conflict"}}"#,
        )
        .unwrap();

        assert_eq!(decoded.code, RpcCode(-32050));
        assert_eq!(decoded.data.code, ErrorCode::Conflict);
    }
}
