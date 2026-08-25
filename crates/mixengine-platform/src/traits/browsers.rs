//! Whether the browsers on this machine trust MixEngine's own certificate authority.

use crate::Result;

/// What one NSS database says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseState {
    /// The directory, so a person can go and look at it.
    pub path: String,

    /// What put it there: `Firefox`, `Firefox (snap)`, `Chrome and Chromium`.
    pub owner: String,

    /// Whether it already holds exactly the certificate that was asked about.
    ///
    /// **Compared as exact DER bytes**, as [`TrustState::installed`](crate::TrustState) is and for
    /// its reason: a nickname match would claim another home's authority as this one's.
    pub installed: bool,

    /// Why not, or why this one could not be asked — a locked profile, a `certutil` that refused.
    pub because: Option<String>,
}

/// What this machine's browsers say about one certificate.
///
/// **Three states where [`TrustState`](crate::TrustState) has a `bool`, and the arity is the whole
/// reason this is a second trait** — the T49b design, D1. NSS is N databases, orthogonal to the
/// system store: a machine can hold the authority in `/etc/ssl/certs` and in none of its browsers,
/// which is an ordinary state rather than a contradiction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserSurvey {
    /// The tool is here, and this is what each database found says.
    ///
    /// **May be empty**, which is a machine with no browser profiles and not a failure.
    Reached {
        /// One per database, in the order they were found.
        databases: Vec<DatabaseState>,
    },

    /// `certutil` is not on this machine, so nothing was asked.
    ///
    /// **A state and not an error** — D7. It is not installed on a stock Ubuntu 24.04; it ships in
    /// `libnss3-tools`, and the reason names the package because "certutil not found" sends a
    /// person to a search engine where the package name ends the question.
    NoTool {
        /// In words, naming the package.
        because: String,
    },

    /// This is not a system MixEngine searches — D2.
    ///
    /// Windows and macOS. The reason says what MixEngine did and **not** that Firefox there reads
    /// the system store: that claim depends on `security.enterprise_roots` and is unmeasured.
    NotSearched {
        /// In words.
        because: String,
    },
}

impl BrowserSurvey {
    /// The databases that do not hold it, which is what an install has to write into.
    ///
    /// A machine with no databases and a machine with no tool both lack nothing, and they are
    /// different answers everywhere else — see the states above.
    #[must_use]
    pub fn lacking(&self) -> Vec<&DatabaseState> {
        match self {
            Self::Reached { databases } => databases.iter().filter(|one| !one.installed).collect(),
            Self::NoTool { .. } | Self::NotSearched { .. } => Vec::new(),
        }
    }
}

/// Whether Firefox and Chrome trust MixEngine's own certificate authority — roadmap task **T49b**.
///
/// **Needs no privilege, in either direction.** These databases belong to the user, which is the
/// line T49 was split on: the system stores need root and ride in the first-run elevation batch,
/// and nothing here goes through `mixengine-elevate` at all.
///
/// **This answers "is the authority in the database", not "does the browser accept it".** A browser
/// already running holds its database open and may not re-read it until restarted, and the honest
/// end-to-end check is a live handshake — `mix cert status`, T53. The same distinction
/// [`TrustStore::probe`](crate::TrustStore::probe) draws for the system store.
pub trait BrowserTrust: std::fmt::Debug + Send + Sync {
    /// What this machine's browsers say about `der`.
    ///
    /// # Errors
    ///
    /// [`Error::Os`](crate::Error::Os) when the tool itself could not be run. **Every caller treats
    /// an error as "no answer" and carries on**: this is asked at start-up, and a survey that
    /// failed must not become the thing that stops a daemon. A single database that could not be
    /// read is a [`DatabaseState::because`] and not an error.
    fn survey(&self, der: &[u8]) -> Result<BrowserSurvey>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holding(installed: bool) -> DatabaseState {
        DatabaseState {
            path: "/home/someone/.pki/nssdb".to_owned(),
            owner: "Chrome and Chromium".to_owned(),
            installed,
            because: (!installed).then(|| "this database does not hold it".to_owned()),
        }
    }

    /// A machine whose every database holds it needs nothing written.
    #[test]
    fn a_machine_that_already_holds_it_everywhere_lacks_nothing() {
        let survey = BrowserSurvey::Reached {
            databases: vec![holding(true), holding(true)],
        };

        assert!(survey.lacking().is_empty());
    }

    /// One database short is one database to write into, and the others are left alone.
    #[test]
    fn only_the_databases_that_lack_it_are_named() {
        let survey = BrowserSurvey::Reached {
            databases: vec![holding(true), holding(false)],
        };

        assert_eq!(survey.lacking().len(), 1);
    }

    /// **A machine with no databases lacks nothing**, and that is not the same claim as a machine
    /// with no tool. `mix doctor` reports the first as `Ok` and the second as a `Note` naming the
    /// package, and collapsing them would put a permanent problem on every server.
    #[test]
    fn a_machine_with_no_databases_and_one_with_no_tool_are_different_answers() {
        assert!(
            BrowserSurvey::Reached { databases: vec![] }
                .lacking()
                .is_empty()
        );
        assert!(
            BrowserSurvey::NoTool {
                because: "certutil is not installed".to_owned()
            }
            .lacking()
            .is_empty()
        );
    }
}
