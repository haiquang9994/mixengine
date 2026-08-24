//! What `cert.ca_status` answers: this home's certificate authority, and nothing about the machine.
//!
//! **There is no trust-store field, and that is a decision rather than an omission.** Whether an
//! operating system trusts this certificate is a question about the operating system, answered by
//! machinery roadmap task T49 builds; a field this build could only ever fill with "unknown" would
//! be an answer nobody can act on. [`DnsStatus`](crate::DnsStatus) took the same shape for the same
//! reason (T46): report the independent facts, and refuse to collapse them into a verdict.
//!
//! **And there is nowhere here a private key could travel.**
//! `.claude/architecture/security-model.md` says the key is never copied, exported by an RPC, or
//! sent to a client, and the way that stays true is that no type below has a field to put one in.

use crate::Timestamp;

/// `cert.ca_status` takes no options, and says so in a type.
///
/// An empty struct with `deny_unknown_fields` rather than no parameters at all: a caller who
/// misspells something is told, instead of being silently given the default — the reasoning T40
/// established for every parameterless method since.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaStatusQuery {}

/// This home's certificate authority, as far as the daemon can see it.
///
/// **A struct around one enum, and the enum is flattened into it.** The wrapper is what lets T49
/// add whether the machine's stores trust this certificate without turning a tagged enum into
/// something a client has to unwrap; the `flatten` is what stops the tag — which is also called
/// `state` — from arriving nested inside a field of the same name.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CaStatus {
    /// What is there.
    #[serde(flatten)]
    pub state: CaState,
}

/// Whether this home has a usable certificate authority.
///
/// **Internally tagged**, so a client matches on a word rather than working out which fields
/// arrived.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CaState {
    /// This home has no certificate authority.
    ///
    /// Reachable even though the daemon makes one at start (the T48 design, D4): a start whose
    /// generation failed warns and carries on, and this is what the next question gets.
    ///
    /// An empty struct variant rather than a unit one, for [`Outcome`](crate::Outcome)'s reason:
    /// `deny_unknown_fields` never fires on a unit variant of an internally tagged enum, because it
    /// is read through `deserialize_any`.
    Absent {},

    /// There is one, and it parses.
    ///
    /// **Including one that has expired** — see [`Ca::days_left`].
    Present {
        /// What it is.
        ca: Ca,
    },

    /// There is something, and it cannot be used.
    Unusable {
        /// Which of the ways, as a name rather than a sentence.
        because: Unusable,
    },
}

/// The ways a certificate authority on disk can be unusable.
///
/// **Closed rather than a string**, for [`ProblemId`](crate::ProblemId)'s reason: a client that
/// matches on wording is a client that silently stops matching. Nothing here is advice — what to do
/// about each is the client's to say, and for most of them it is `mix cert ca-rotate`, which T54
/// builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unusable {
    /// The certificate is there and its private key is not.
    KeyMissing,

    /// The private key is there and the certificate is not.
    ///
    /// The state a crash between the two writes leaves, which is why the key is written first (the
    /// T48 design, D2): this is a shape that can be recognised, where a certificate whose key never
    /// arrived looks exactly like one whose key was lost.
    CertificateMissing,

    /// The private key is there and is not a private key this build can read.
    KeyUnreadable,

    /// The certificate is there and is not a certificate at all.
    CertificateUnreadable,

    /// Both are there, both parse, and the certificate is not this key's.
    ///
    /// How a home restored from a backup that caught one file and not the other comes back. Left
    /// unchecked the symptom appears much later and somewhere else, as leaf certificates that no
    /// browser trusts.
    KeyAndCertificateDisagree,
}

/// A certificate authority that exists and parses.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Ca {
    /// The distinguished name, as X.509 spells it.
    pub subject: String,

    /// SHA-256 of the certificate's DER, lowercase hex, no separators.
    ///
    /// **The number a browser shows and a trust store lists**, and therefore the only one a person
    /// can compare against anything. Not the same value as [`key_id`](Self::key_id), and the T48
    /// design's D1 is why there are two.
    pub fingerprint: String,

    /// The first eight hex characters of the SHA-256 of the public key, which is what
    /// [`subject`](Self::subject) ends with.
    ///
    /// A certificate cannot carry a hash of itself — the subject is inside the bytes the hash is
    /// over — so the name is derived from the key instead. That also makes two certificates over
    /// one key recognisable as one authority, which is what a rotation of the certificate alone
    /// would produce.
    pub key_id: String,

    /// When it starts being valid.
    pub not_before: Timestamp,

    /// When it stops.
    pub not_after: Timestamp,

    /// Whole days from now until [`not_after`](Self::not_after).
    ///
    /// **Signed, and allowed to be negative.** An expired authority is a true statement about a
    /// certificate that exists and parses, not an unusable one: what it needs is rotation, which is
    /// T54's and not this build's.
    pub days_left: i64,

    /// The certificate itself, PEM. The public half, and there is no field for the other one.
    pub certificate_pem: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The state travels as a word, and so does the reason it is unusable.
    #[test]
    fn a_state_travels_tagged_and_a_reason_travels_as_a_name() {
        let absent = serde_json::to_value(CaStatus {
            state: CaState::Absent {},
        })
        .expect("the status encodes");

        assert_eq!(absent, serde_json::json!({ "state": "absent" }));

        let unusable = serde_json::to_value(CaStatus {
            state: CaState::Unusable {
                because: Unusable::KeyAndCertificateDisagree,
            },
        })
        .expect("the status encodes");

        assert_eq!(
            unusable,
            serde_json::json!({
                "state": "unusable",
                "because": "key_and_certificate_disagree",
            })
        );
    }

    /// There is no field a private key could travel in, and this is what keeps it so.
    #[test]
    fn nothing_in_a_status_can_carry_a_private_key() {
        let encoded = serde_json::to_string(&CaStatus {
            state: CaState::Present { ca: example() },
        })
        .expect("the status encodes");

        assert!(
            !encoded.contains("PRIVATE"),
            "a status carried something that says PRIVATE: {encoded}"
        );

        // The field names are the whole of the guarantee. A future field called any of these is
        // what this test exists to make somebody argue for out loud.
        for forbidden in ["key_pem", "private_key", "key_der", "key_pkcs8"] {
            assert!(
                !encoded.contains(forbidden),
                "a status grew a {forbidden} field"
            );
        }
    }

    /// An expired authority is `Present` with a negative count, not `Unusable`.
    #[test]
    fn an_expired_authority_is_present_with_a_negative_count() {
        let encoded = serde_json::to_value(CaStatus {
            state: CaState::Present {
                ca: Ca {
                    days_left: -7,
                    ..example()
                },
            },
        })
        .expect("the status encodes");

        assert_eq!(encoded["state"], "present");
        assert_eq!(encoded["ca"]["days_left"], -7);
    }

    /// Every state survives the round trip a client actually makes.
    ///
    /// **The half the encoding tests do not reach, and the half `flatten` is most likely to break.**
    /// A `#[serde(flatten)]` field is deserialised through a buffering map rather than directly,
    /// which is where it interacts badly with tagged enums — and the daemon only ever serialises,
    /// so nothing else in this workspace would notice a status that encodes and cannot be read back.
    #[test]
    fn every_state_comes_back_as_what_went_out() {
        for state in [
            CaState::Absent {},
            CaState::Present { ca: example() },
            CaState::Unusable {
                because: Unusable::KeyMissing,
            },
            CaState::Unusable {
                because: Unusable::CertificateUnreadable,
            },
        ] {
            let sent = CaStatus { state };
            let wire = serde_json::to_string(&sent).expect("the status encodes");
            let received: CaStatus =
                serde_json::from_str(&wire).expect("a client can read what the daemon sent");

            assert_eq!(received, sent, "the status changed on the way: {wire}");
        }
    }

    /// A misspelled option is refused rather than ignored.
    #[test]
    fn an_unknown_option_is_refused_rather_than_ignored() {
        let refused: Result<CaStatusQuery, _> =
            serde_json::from_value(serde_json::json!({ "verbose": true }));

        assert!(refused.is_err(), "an unknown field was accepted");
    }

    fn example() -> Ca {
        Ca {
            subject: "CN=MixEngine Local CA 0123abcd".to_owned(),
            fingerprint: "ab".repeat(32),
            key_id: "0123abcd".to_owned(),
            not_before: Timestamp(0),
            not_after: Timestamp(1),
            days_left: 3_650,
            certificate_pem: "-----BEGIN CERTIFICATE-----\n".to_owned(),
        }
    }
}
