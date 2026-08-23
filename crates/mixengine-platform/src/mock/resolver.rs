//! Resolver wiring that exists only in memory.

use crate::{Error, ResolverMethod, ResolverState, Result};

/// What a test says this machine routes.
#[derive(Debug)]
pub(crate) struct Resolver {
    answer: std::result::Result<Answer, String>,
}

/// The two fields a fixture sets. `missing` is derived rather than given: a machine that routes
/// less than was asked has one honest sentence to say about it, and letting a fixture write a
/// different one would let a test assert against a machine none of the three could be.
#[derive(Debug, Clone)]
struct Answer {
    method: ResolverMethod,
    wired: Vec<String>,
}

impl Default for Resolver {
    /// A machine with no scoped mechanism, routing nothing.
    ///
    /// **Which is what every suite written before T45 should keep looking like**: they were written
    /// against a home in `hosts_only`, they assert the hosts entries that mode produces, and a
    /// default that quietly wired them would rewrite what they are about.
    fn default() -> Self {
        Self {
            answer: Ok(Answer {
                method: ResolverMethod::None,
                wired: Vec::new(),
            }),
        }
    }
}

impl Resolver {
    /// A machine using `method` that already routes `wired`.
    pub(crate) fn routing(method: ResolverMethod, wired: &[&str]) -> Self {
        Self {
            answer: Ok(Answer {
                method,
                wired: wired.iter().map(|tld| (*tld).to_owned()).collect(),
            }),
        }
    }

    /// A machine that cannot say, with `reason`.
    pub(crate) fn refusing(reason: &str) -> Self {
        Self {
            answer: Err(reason.to_owned()),
        }
    }

    /// The fixture's answer, or the error it was built to give.
    fn answer(&self) -> Result<Answer> {
        self.answer
            .clone()
            .map_err(|reason| Error::UnsupportedPlatform {
                capability: "ResolverConfig",
                reason,
            })
    }
}

impl crate::ResolverConfig for Resolver {
    fn method(&self) -> Result<ResolverMethod> {
        Ok(self.answer()?.method)
    }

    fn probe(&self, tlds: &[&str], _port: u16) -> Result<ResolverState> {
        let answer = self.answer()?;

        // Only what was asked about, so a fixture built with three TLDs answers a caller that asked
        // about one the way a real machine would.
        let wired: Vec<String> = tlds
            .iter()
            .filter(|tld| answer.wired.iter().any(|one| one == *tld))
            .map(|tld| (*tld).to_owned())
            .collect();

        let missing = (wired.len() < tlds.len())
            .then(|| "this machine does not route every managed TLD here".to_owned());

        Ok(ResolverState {
            method: answer.method,
            wired,
            missing,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::Host as _;

    use super::*;

    /// A mock that answers a wired machine, which is the fixture the daemon's own tests need and
    /// which no machine in CI can be asked to be for a unit test.
    #[test]
    fn a_mock_answers_what_it_was_built_with() {
        let host = crate::mock::Host::with_resolver(
            "/mixengine",
            ResolverMethod::Nrpt,
            &["test", "localhost"],
        );

        let state = host
            .resolver()
            .probe(&["test", "localhost"], 53)
            .expect("a state");

        assert_eq!(state.method, ResolverMethod::Nrpt);
        assert_eq!(state.wired, vec!["test".to_owned(), "localhost".to_owned()]);
        assert_eq!(state.plan(&["test", "localhost"], 53), None);
    }

    /// A machine that routes less than was asked says so, and the planner asks for the whole state.
    #[test]
    fn a_partly_wired_mock_reports_the_gap() {
        let host = crate::mock::Host::with_resolver("/mixengine", ResolverMethod::Nrpt, &["test"]);

        let state = host
            .resolver()
            .probe(&["test", "internal"], 53)
            .expect("a state");

        assert_eq!(state.wired, vec!["test".to_owned()]);
        assert!(state.missing.is_some());
        assert!(state.plan(&["test", "internal"], 53).is_some());
    }

    /// The default is a machine with no mechanism, so every suite written before T45 keeps the mode
    /// it was written against.
    #[test]
    fn the_default_mock_routes_nothing_and_has_no_mechanism() {
        let host = crate::mock::Host::with_home("/mixengine");

        let state = host.resolver().probe(&["test"], 53_535).expect("a state");

        assert_eq!(state.method, ResolverMethod::None);
        assert!(state.wired.is_empty());
        assert_eq!(state.plan(&["test"], 53_535), None);
    }

    /// A machine that cannot be asked. Every caller treats this as "no answer" and carries on.
    #[test]
    fn a_mock_that_cannot_be_read_says_so() {
        let host = crate::mock::Host::unable_to_read_resolver("/mixengine", "no resolvectl here");

        assert!(host.resolver().probe(&["test"], 53_535).is_err());
        assert!(host.resolver().method().is_err());
    }
}
