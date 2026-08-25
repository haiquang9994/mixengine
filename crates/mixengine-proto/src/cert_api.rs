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

use crate::{SiteRef, Timestamp};

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

    /// Whether this machine trusts it — roadmap task **T49a**.
    ///
    /// **Beside [`Ca`] rather than inside it**, because whether a machine trusts a certificate is a
    /// fact about the machine and `Ca` describes the certificate. A home whose authority is absent
    /// or damaged still has a machine with or without a store, and this still has something true to
    /// say about it.
    ///
    /// T48 left this out on purpose, recording that a field it could only have filled with
    /// "unknown" is not an answer. It is answerable now.
    pub trust: Trust,

    /// What Firefox and Chrome say — roadmap task **T49b**.
    ///
    /// **Beside [`trust`](Self::trust) and not inside it.** They are separate questions with
    /// separate answers: `trust` is one store and one `bool`, and this is N databases that are
    /// orthogonal to it. A machine can hold the authority in `/etc/ssl/certs` and in none of its
    /// browsers, which is an ordinary state rather than a contradiction.
    pub browsers: Browsers,
}

/// What the browsers on this machine say about MixEngine's authority.
///
/// **Four states, mirroring [`Trust`]'s** — and, as there, every branch that could not ask says why
/// rather than reporting `false`. A client renders what the daemon returns.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Browsers {
    /// The tool is here, and this is what each database said.
    ///
    /// **An empty list is a machine with no browser profiles**, which is a normal server and not a
    /// failure — and a different answer from [`NoTool`](Self::NoTool), which is a machine that may
    /// well have browsers nobody could ask about.
    Reached {
        /// One per database found.
        databases: Vec<BrowserDatabase>,
    },

    /// `certutil` is not installed, so nothing was asked.
    ///
    /// The reason names `libnss3-tools`, because "certutil not found" sends a person to a search
    /// engine where the package name ends the question.
    NoTool {
        /// In words, naming the package.
        because: String,
    },

    /// Not a system MixEngine searches.
    ///
    /// Windows and macOS. The reason says what MixEngine did rather than what Firefox reads there:
    /// that depends on `security.enterprise_roots` and is not measured.
    NotSearched {
        /// In words.
        because: String,
    },

    /// The search itself failed.
    ///
    /// As [`Trust::Unknown`]: a read that said nothing must not be rendered as "no".
    Unknown {
        /// What went wrong.
        because: String,
    },
}

/// One NSS database, and whether it holds the authority.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BrowserDatabase {
    /// The directory, so a person can go and look at it.
    pub path: String,

    /// What put it there: `Firefox`, `Firefox (snap)`, `Chrome and Chromium`.
    ///
    /// **Which browser and not just which path**, because what a person does about a database that
    /// lacks it is restart the browser that owns it.
    pub owner: String,

    /// Whether it holds exactly this authority.
    pub installed: bool,

    /// Why not, or why this one could not be asked.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub because: Option<String>,
}

/// Whether a site has a usable certificate — roadmap task **T50**.
///
/// **[`CaState`]'s vocabulary, reused deliberately.** The ways a key and a certificate on disk can
/// disagree do not depend on which of the two they are, so [`Unusable`] is the same closed enum
/// rather than a second one that would drift from it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CertState {
    /// This site has no certificate.
    Absent {},

    /// There is one, and it parses.
    ///
    /// **Including one that has expired** — see [`SiteCert::days_left`].
    Present {
        /// What it is.
        cert: SiteCert,
    },

    /// There is something, and it cannot be used.
    Unusable {
        /// Which of the ways, as a name rather than a sentence.
        because: Unusable,
    },
}

/// One site's certificate.
///
/// **There is no `certificate_pem` and there is no field a private key could travel in.** [`Ca`]
/// carries its PEM because a client installs it; nothing installs a leaf, so the field would be
/// surface with no caller — and `.claude/architecture/security-model.md`'s guarantee is easier to
/// keep on a type with fewer fields.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SiteCert {
    /// The distinguished name, as X.509 spells it.
    pub subject: String,

    /// Every name it covers, in the order it covers them.
    pub sans: Vec<String>,

    /// The authority that signed it, as X.509 spells the name.
    ///
    /// **The fourth question the issuer asks is a comparison of this against the authority's own
    /// subject** — the T50 design, D6 — which is free because T48 put the key's identity into that
    /// name. On the wire rather than derived only where it is needed, because `mix cert status`
    /// (T53) has to be able to show a person *which* authority signed a certificate they cannot get
    /// a padlock for.
    pub issuer: String,

    /// SHA-256 of the certificate's DER, lowercase hex, no separators.
    pub fingerprint: String,

    /// When it starts being valid.
    pub not_before: Timestamp,

    /// When it stops.
    pub not_after: Timestamp,

    /// Whole days from now until [`not_after`](Self::not_after).
    ///
    /// **Signed, and allowed to be negative**, for [`Ca::days_left`]'s reason: an expired
    /// certificate is a true statement about one that exists and parses.
    pub days_left: i64,
}

/// `cert.issue` — give a site the certificate its names need, or every site one.
///
/// **It names a site and never a list of domains.** `.claude/features/tls.md` specified
/// `{ domains }`; that would put in the client the decision of *what a certificate covers*, which is
/// business logic, and `.claude/CLAUDE.md`'s first rule is that a client only renders what the
/// daemon returns. The daemon reads the site's domains from its own rows.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertIssue {
    /// One site, or **every site with HTTPS declared** when it is absent.
    ///
    /// The absent form is what the daemon's own producer calls and what `mix cert issue` runs with
    /// no argument, so the automatic path and the manual one are one piece of code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<SiteRef>,
}

/// What `cert.issue` did, per site.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CertIssueReport {
    /// One entry per site considered, in primary-domain order.
    pub sites: Vec<SiteCertOutcome>,
}

/// One site, and what happened to its certificate.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SiteCertOutcome {
    /// The site, by its primary domain.
    pub domain: String,

    /// What was done.
    pub outcome: IssueOutcome,

    /// What is on disk afterwards.
    pub state: CertState,
}

/// The three things `cert.issue` can do to one site.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum IssueOutcome {
    /// A key and a certificate were written.
    Issued {},

    /// What was already there answered every question, so nothing was written.
    Reused {},

    /// Nothing was written because nothing was asked for: this site declares no HTTPS.
    ///
    /// **Not a refusal, and the distinction is not cosmetic** — roadmap task **T52**. A refusal is
    /// MixEngine failing at something it was asked to do; this is MixEngine correctly doing
    /// nothing. T52's renewal loop announces every failure it finds, so with these two under one
    /// name it would announce one per plaintext site, once an hour, for as long as the daemon runs
    /// — and `crate::sites::Sites::now_has_a_certificate` was already logging `the site has no
    /// certificate yet` about a site that never wanted one.
    NotWanted {
        /// In words.
        because: String,
    },

    /// Nothing was written, and this is why: no usable authority, or no domains at all.
    ///
    /// **A per-site outcome and not a failed call.** One site that cannot be issued for must not
    /// take the answer for the others with it, which is the same shape T49b's `BrowserChange` has.
    ///
    /// A site with no domains stays here rather than moving to [`Self::NotWanted`] with T52: it is
    /// a row that should not exist, and calling it "not wanted" would hide it.
    Refused {
        /// In words.
        because: String,
    },
}

/// Whether this machine holds MixEngine's certificate authority in its own trust store.
///
/// **This says "is it in the store", not "does a browser trust it".** Firefox and Chrome on Linux
/// read NSS databases and not the system store at all — that is T49b — and a browser already running
/// may not re-read a store it has cached. The honest end-to-end answer is a live TLS handshake,
/// which is `mix cert status`' job (T53).
/// **A nested object rather than a flattened one, unlike [`CaState`] above.** Two reasons, and both
/// are about this type rather than about taste: `Trust::NotInstalled` and `CaState::Unusable` both
/// spell their reason `because`, so flattening would let one silently overwrite the other on the one
/// screen that has to say what is wrong; and the two are about different subjects — the certificate,
/// and the machine — where `CaStatus`' single field was about one. A client reads
/// `status.trust.state`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Trust {
    /// This machine holds it.
    Installed {
        /// Which store, in words a person can go and look in.
        store: String,
    },

    /// This machine has a store MixEngine knows how to write, and it does not hold it.
    NotInstalled {
        /// Which store, and why it is not there — `mix doctor` reports the same sentence.
        because: String,
    },

    /// This machine has no system trust store MixEngine knows how to write.
    ///
    /// **A supported machine, not a failure**, exactly as `ResolverMethod::None` is: Linux without
    /// either anchors directory keeps working over HTTP, and its browsers are reached through NSS
    /// (T49b) rather than through a system store at all.
    NoStore {
        /// Which of those it is, in words.
        because: String,
    },

    /// The store could not be read.
    ///
    /// A real outcome and the only honest thing to print when it happens — a read that failed has
    /// said nothing, and rendering that as "not installed" would be the client inventing an answer.
    Unknown {
        /// What went wrong reading it.
        because: String,
    },
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

    /// A machine that has a store and does not hold it — the shape a fresh home answers with, and
    /// the one every test below is about something other than.
    fn untrusted() -> Trust {
        Trust::NotInstalled {
            because: "this machine does not hold MixEngine's authority".to_owned(),
        }
    }

    /// A machine MixEngine does not search for browser databases, which is what a Windows or macOS
    /// answer looks like and what every test below is about something other than.
    fn unsearched() -> Browsers {
        Browsers::NotSearched {
            because: "MixEngine does not search browser certificate databases here".to_owned(),
        }
    }

    /// The state travels as a word, and so does the reason it is unusable.
    #[test]
    fn a_state_travels_tagged_and_a_reason_travels_as_a_name() {
        let absent = serde_json::to_value(CaStatus {
            state: CaState::Absent {},
            trust: untrusted(),
            browsers: unsearched(),
        })
        .expect("the status encodes");

        assert_eq!(
            absent,
            serde_json::json!({
                "state": "absent",
                "trust": {
                    "state": "not_installed",
                    "because": "this machine does not hold MixEngine's authority",
                },
                // T49b's field, and it is nested for `trust`'s reason: this one spells its reason
                // `because` too, so a third flattened sentence would overwrite one of the first two.
                "browsers": {
                    "state": "not_searched",
                    "because": "MixEngine does not search browser certificate databases here",
                },
            })
        );

        let unusable = serde_json::to_value(CaStatus {
            state: CaState::Unusable {
                because: Unusable::KeyAndCertificateDisagree,
            },
            trust: Trust::NoStore {
                because: "no anchors directory here".to_owned(),
            },
            browsers: unsearched(),
        })
        .expect("the status encodes");

        assert_eq!(
            unusable,
            serde_json::json!({
                "state": "unusable",
                "because": "key_and_certificate_disagree",
                // **Nested, and this is the case that decided it.** Both halves spell their reason
                // `because`; flattened, an unusable authority on a machine with no store would lose
                // one of the two sentences and nothing would say which.
                "trust": { "state": "no_store", "because": "no anchors directory here" },
                "browsers": {
                    "state": "not_searched",
                    "because": "MixEngine does not search browser certificate databases here",
                },
            })
        );
    }

    /// There is no field a private key could travel in, and this is what keeps it so.
    #[test]
    fn nothing_in_a_status_can_carry_a_private_key() {
        let encoded = serde_json::to_string(&CaStatus {
            state: CaState::Present { ca: example() },
            trust: untrusted(),
            browsers: unsearched(),
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
            trust: untrusted(),
            browsers: unsearched(),
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
            for trust in [
                Trust::Installed {
                    store: "this machine's Trusted Root Certification Authorities".to_owned(),
                },
                Trust::NotInstalled {
                    because: "the store does not hold it".to_owned(),
                },
                Trust::NoStore {
                    because: "no anchors directory on this machine".to_owned(),
                },
                Trust::Unknown {
                    because: "security exited 1".to_owned(),
                },
            ] {
                let sent = CaStatus {
                    state: state.clone(),
                    trust,
                    browsers: unsearched(),
                };
                let wire = serde_json::to_string(&sent).expect("the status encodes");
                let received: CaStatus =
                    serde_json::from_str(&wire).expect("a client can read what the daemon sent");

                assert_eq!(received, sent, "the status changed on the way: {wire}");
            }
        }
    }

    /// **A machine with browsers and one with none are different answers**, and both survive the
    /// trip. The path and the owner both travel, because what a person does about a database that
    /// lacks it is open that path and restart that browser.
    #[test]
    fn what_the_browsers_say_travels_beside_what_the_machine_says() {
        let sent = CaStatus {
            state: CaState::Present { ca: example() },
            trust: untrusted(),
            browsers: Browsers::Reached {
                databases: vec![BrowserDatabase {
                    path: "/home/someone/.pki/nssdb".to_owned(),
                    owner: "Chrome and Chromium".to_owned(),
                    installed: false,
                    because: Some(
                        "/home/someone/.pki/nssdb does not hold this authority".to_owned(),
                    ),
                }],
            },
        };

        let wire = serde_json::to_value(&sent).expect("the status encodes");

        assert_eq!(wire["browsers"]["state"], "reached");
        assert_eq!(
            wire["browsers"]["databases"][0]["owner"],
            "Chrome and Chromium"
        );

        let received: CaStatus =
            serde_json::from_value(wire).expect("a client can read what the daemon sent");
        assert_eq!(received, sent);
    }

    /// **The three that name no database still say why**, and each is a different sentence: no
    /// tool, not this system, and a scan that failed are three things a person would do three
    /// different things about — install a package, nothing, and read a log.
    #[test]
    fn every_browsers_state_carries_its_own_reason() {
        for browsers in [
            Browsers::Reached { databases: vec![] },
            Browsers::NoTool {
                because: "certutil is not installed — it ships in libnss3-tools".to_owned(),
            },
            Browsers::NotSearched {
                because: "MixEngine does not search browser databases on Windows".to_owned(),
            },
            Browsers::Unknown {
                because: "certutil did not answer within 30 seconds".to_owned(),
            },
        ] {
            let sent = CaStatus {
                state: CaState::Absent {},
                trust: untrusted(),
                browsers,
            };
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

    /// The three outcomes travel as words, and only the refusal carries a reason.
    #[test]
    fn an_issue_report_travels_tagged_and_only_a_refusal_says_why() {
        let report = CertIssueReport {
            sites: vec![
                SiteCertOutcome {
                    domain: "blog.test".to_owned(),
                    outcome: IssueOutcome::Issued {},
                    state: CertState::Absent {},
                },
                SiteCertOutcome {
                    domain: "shop.test".to_owned(),
                    outcome: IssueOutcome::Refused {
                        because: "this home has no usable certificate authority".to_owned(),
                    },
                    state: CertState::Absent {},
                },
            ],
        };

        let wire = serde_json::to_value(&report).expect("it encodes");

        assert_eq!(wire["sites"][0]["outcome"]["outcome"], "issued");
        assert_eq!(wire["sites"][1]["outcome"]["outcome"], "refused");
        assert!(wire["sites"][0]["outcome"].get("because").is_none());

        let back: CertIssueReport =
            serde_json::from_value(wire).expect("a client can read what the daemon sent");
        assert_eq!(back, report);
    }

    /// A request naming no site is the every-site form, and it is the default rather than a
    /// separate method.
    #[test]
    fn an_issue_request_may_name_no_site_at_all() {
        let every: CertIssue = serde_json::from_value(serde_json::json!({}))
            .expect("an empty object is the every-site form");

        assert_eq!(every.site, None);

        let one: CertIssue =
            serde_json::from_value(serde_json::json!({ "site": { "domain": "blog.test" } }))
                .expect("a named site");

        assert_eq!(one.site, Some(SiteRef::Domain("blog.test".to_owned())));
    }

    /// **A site's certificate carries no key either**, which is the same guarantee `CaStatus` makes
    /// and the reason `SiteCert` has no `certificate_pem` to hide one behind.
    #[test]
    fn nothing_in_a_site_certificate_can_carry_a_private_key() {
        let encoded = serde_json::to_string(&CertState::Present {
            cert: SiteCert {
                subject: "CN=blog.test".to_owned(),
                sans: vec!["blog.test".to_owned()],
                issuer: "CN=MixEngine Local CA 0123abcd".to_owned(),
                fingerprint: "ab".repeat(32),
                not_before: Timestamp(0),
                not_after: Timestamp(1),
                days_left: 90,
            },
        })
        .expect("the state encodes");

        assert!(!encoded.contains("PRIVATE"), "{encoded}");
        for forbidden in ["key_pem", "private_key", "key_der", "key_pkcs8"] {
            assert!(
                !encoded.contains(forbidden),
                "a site certificate grew a {forbidden} field"
            );
        }
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
