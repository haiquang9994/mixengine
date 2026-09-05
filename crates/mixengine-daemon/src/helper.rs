//! What the privileged helper installed on this machine is, and what to say when it is older than
//! the daemon reading it — roadmap task **T88a**.
//!
//! # Asked without a prompt
//!
//! `Probe` needs no administrative token — the T40 design's D5 made it so, precisely so that the
//! operation reporting whether a process is elevated could ever report `false`. So the daemon finds
//! out what the installed helper is by *running it as an ordinary process*, with a one-operation
//! batch, and reading the header every answer already carries. No prompt is spent and nothing is
//! changed.
//!
//! **Only when a helper is installed.** On a fresh machine and in every development tree there is
//! nothing to ask, `Elevation::require_helper` enqueues `HelperInstall {}` as it always has, and
//! this costs nothing at all.
//!
//! # And what it is for
//!
//! Two things, and neither is speculative. The daemon marks each request with the lower of its own
//! protocol and the helper's, because a fixed old binary cannot be taught a newer one. And it reads
//! `supported_ops` before deciding what to say about an old helper: without it, the only way to
//! discover that the installed helper predates `helper-replace` is to enqueue one, spend a prompt,
//! and be answered `Unsupported` — which deletes the row (the T40b design, D5) and leaves a person
//! with a refusal and no sentence.

use std::path::Path;

use crate::error::ToWire as _;
use mixengine_proto::privileged::PrivilegedOp;
use mixengine_proto::{PendingOp, PendingOpId, ProtocolVersion, Timestamp};

/// How long the installed helper gets to answer a probe.
///
/// It reads two files and writes a small one. Generous enough for a cold start off a slow disk,
/// short enough that a daemon start is never held up by a binary that will not run.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// What the installed helper answered a probe with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HelperFacts {
    /// The protocol it speaks, which is the ceiling every request to it is marked at.
    pub(crate) speaks: ProtocolVersion,

    /// Which release it is.
    pub(crate) version: String,

    /// Every operation it knows, by wire name.
    pub(crate) supported_ops: Vec<String>,
}

impl HelperFacts {
    /// Does this helper know how to replace itself?
    pub(crate) fn can_replace_itself(&self) -> bool {
        self.supported_ops
            .iter()
            .any(|name| name == PrivilegedOp::HelperReplace {}.name())
    }
}

/// Run `helper` as an ordinary process and read what it says about itself.
///
/// Every failure is [`None`]: an unreadable answer, a binary that will not start, a timeout, and
/// the `EXIT_UNAVAILABLE` a helper answers while a grant holds `elevate.lock` all mean the same
/// thing to every caller, which is that this daemon does not know and will not guess.
pub(crate) async fn handshake(helper: &Path, home: &Path, elevate: &Path) -> Option<HelperFacts> {
    let directory = elevate.join(format!("probe-{}", std::process::id()));
    let facts = probe(helper, home, &directory).await;

    // On every branch, as `Elevation::grant` removes its own: a single-use directory that outlived
    // its use is what makes `response.json`'s existence a weaker anti-replay check than it is.
    let _ = tokio::fs::remove_dir_all(&directory).await;

    facts
}

/// The body of [`handshake`], so the cleanup above happens whatever this answers.
async fn probe(helper: &Path, home: &Path, directory: &Path) -> Option<HelperFacts> {
    let op = PrivilegedOp::Probe {};
    let waiting = PendingOp {
        id: PendingOpId(0),
        description: op.describe(),
        op,
        requested_at: Timestamp::from_system_time(std::time::SystemTime::now()),
    };

    // **At the floor, and not at this build's own protocol.** This is the one request sent before
    // anything is known about the peer, so it is marked with the version every build that will ever
    // exist still serves — which is what makes a handshake work against a helper older than the
    // daemon, and is the whole reason there is a floor.
    let request = mixengine_core::elevation::write_request(
        directory,
        home,
        std::slice::from_ref(&waiting),
        mixengine_proto::PROTOCOL_MINIMUM,
    )
    .ok()?;

    let ran = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        tokio::process::Command::new(helper)
            .arg(request.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status(),
    )
    .await;

    if !matches!(ran, Ok(Ok(status)) if status.success()) {
        tracing::debug!(
            helper = %helper.display(),
            "the installed privileged helper did not answer a probe"
        );
        return None;
    }

    let report = mixengine_core::elevation::read_report(&request).ok()?;

    Some(HelperFacts {
        speaks: report.version,
        version: report.elevate_version,
        supported_ops: report.supported_ops,
    })
}

/// What to tell somebody about a helper older than the daemon reading it, or [`None`].
///
/// **Two sentences and not one**, because the two situations end in different places. A helper that
/// knows `helper-replace` can be replaced by `mix elevation upgrade`, which fetches the published
/// one and asks. A helper that does not cannot replace itself at all, and offering that command
/// would be offering a refusal — what replaces one of those is an installer running as root.
///
/// A helper *newer* than this daemon is [`None`] too: that is routine, and is the reason the
/// response was written without `deny_unknown_fields` in the first place.
///
/// A version either side cannot parse is [`None`]: telling somebody to reinstall over a string
/// nobody can compare would be worse than saying nothing.
pub(crate) fn upgrade_sentence(facts: &HelperFacts, daemon: &str) -> Option<String> {
    use mixengine_proto::PackageVersion;

    let installed = PackageVersion::parse(facts.version.clone()).ok()?;
    let running = PackageVersion::parse(daemon.to_owned()).ok()?;

    if installed.cmp_precedence(&running) != std::cmp::Ordering::Less {
        return None;
    }

    if facts.can_replace_itself() {
        return Some(format!(
            "the privileged helper on this machine is {} and MixEngine is {daemon} — it goes on \
             serving everything it knows, and `mix elevation upgrade` fetches the newer one and \
             asks before installing it",
            facts.version
        ));
    }

    Some(format!(
        "the privileged helper on this machine is {}, which is from before MixEngine could replace \
         one. It goes on serving everything it knows; what replaces it is running this release's \
         installer, which puts it there as an administrator",
        facts.version
    ))
}

/// `elevation.upgrade` — fetch the published privileged helper and queue its installation.
///
/// Six steps, and the fifth is the one worth naming: **the candidate is run here, unelevated,
/// before anything is queued.** Between *"the upgrade was refused and nothing changed"* and *"this
/// machine can no longer elevate anything"* the difference is exactly that — the same argument T88
/// makes for running the staged `mixengined` before a swap, one binary further in, where the cost
/// of being wrong is higher. It is also the only thing that catches a Windows Code Integrity
/// refusal, which `.claude/features/updates.md` records as a refusal rather than a warning, judged
/// per file and again after every update.
///
/// **Nothing is installed by this call.** It leaves a row, and `elevation.grant` is what raises the
/// prompt — the only door into one, deliberately.
///
/// # Errors
///
/// The wire error of a feed that could not be read, of a download that failed, and of a candidate
/// that did not verify. Everything else is an outcome rather than an error: a machine whose helper
/// cannot replace itself is working, and what a person needs from it is a sentence.
pub(crate) async fn upgrade(
    elevation: &std::sync::Arc<crate::elevation::Elevation>,
    updates: &std::sync::Arc<crate::updates::Updates>,
    paths: &mixengine_core::paths::Paths,
) -> Result<mixengine_proto::HelperUpgrade, mixengine_proto::Error> {
    use mixengine_proto::{HelperUpgrade, HelperUpgradeOutcome};

    let queue = |outcome, installed, offered| async move {
        Ok(HelperUpgrade {
            installed,
            offered,
            outcome,
            pending: elevation.status().await?.pending,
        })
    };

    // A `.deb`, an `.rpm` or a `.pkg` put the helper where it is as root, and the same package
    // manager replaces it. Refused in words before a byte is fetched, exactly as `mix self-update`
    // refuses the binaries beside it.
    if let mixengine_core::updates::Placement::Managed { directory, because } = updates.placement()
    {
        return queue(
            HelperUpgradeOutcome::Unavailable {
                reason: mixengine_core::Error::UpdateNotWritable {
                    directory: directory.clone(),
                    because: because.clone(),
                }
                .to_string(),
            },
            None,
            None,
        )
        .await;
    }

    let Some(facts) = elevation.facts() else {
        return queue(
            HelperUpgradeOutcome::Unavailable {
                reason: "there is no privileged helper installed on this machine yet, or it would \
                         not say what it is; the next elevation prompt installs one"
                    .to_owned(),
            },
            None,
            None,
        )
        .await;
    };

    let installed = Some(facts.version.clone());

    if !facts.can_replace_itself() {
        let reason = upgrade_sentence(&facts, env!("CARGO_PKG_VERSION")).unwrap_or_else(|| {
            format!(
                "the privileged helper on this machine is {}, which is from before MixEngine could \
                 replace one; what replaces it is running this release's installer",
                facts.version
            )
        });

        return queue(
            HelperUpgradeOutcome::Unsupported { reason },
            installed,
            None,
        )
        .await;
    }

    let (offered, artifact) = updates.published_helper().await?;

    // **Newer, by precedence and not by string.** A helper of the published version is not an
    // upgrade, and neither is one this machine already has ahead of the release.
    let ordering = mixengine_proto::PackageVersion::parse(offered.clone())
        .ok()
        .zip(mixengine_proto::PackageVersion::parse(facts.version.clone()).ok())
        .map(|(published, here)| published.cmp_precedence(&here));

    if ordering != Some(std::cmp::Ordering::Greater) {
        return queue(HelperUpgradeOutcome::UpToDate, installed, Some(offered)).await;
    }

    let into = mixengine_proto::privileged::helper_candidate_dir(paths.root());
    let stamp = mixengine_core::updates::helper::stage(
        updates.installer(),
        &artifact,
        mixengine_core::updates::PUBLIC_KEY,
        &into,
    )
    .await
    .map_err(|error| error.to_wire())?;

    // The smoke test. A candidate that will not start here is one that would leave this machine
    // unable to elevate anything at all, and the way back from that is a reinstall.
    let candidate = mixengine_proto::privileged::helper_candidate(paths.root());
    if handshake(&candidate, paths.root(), &paths.run().join("elevate"))
        .await
        .is_none()
    {
        tracing::warn!(
            candidate = %candidate.display(),
            "the privileged helper this release publishes will not run on this machine"
        );

        return queue(
            HelperUpgradeOutcome::Unavailable {
                reason: format!(
                    "the privileged helper this release publishes ({}) will not run on this \
                     machine, so nothing has been queued and the one installed here is untouched",
                    stamp.version
                ),
            },
            installed,
            Some(offered),
        )
        .await;
    }

    elevation.enqueue(&PrivilegedOp::HelperReplace {}).await?;

    queue(HelperUpgradeOutcome::Staged, installed, Some(stamp.version)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(version: &str, knows_replace: bool) -> HelperFacts {
        HelperFacts {
            speaks: mixengine_proto::PROTOCOL_VERSION,
            version: version.to_owned(),
            supported_ops: if knows_replace {
                vec!["probe".to_owned(), "helper-replace".to_owned()]
            } else {
                vec!["probe".to_owned()]
            },
        }
    }

    #[test]
    fn a_helper_of_this_build_needs_nothing_said_about_it() {
        assert!(upgrade_sentence(&facts("0.2.0", true), "0.2.0").is_none());
    }

    /// A helper *newer* than the daemon is routine — the response was written without
    /// `deny_unknown_fields` for exactly that — so it is not something to report either.
    #[test]
    fn a_helper_newer_than_this_daemon_needs_nothing_said_about_it() {
        assert!(upgrade_sentence(&facts("0.3.0", true), "0.2.0").is_none());
    }

    #[test]
    fn an_older_helper_that_can_replace_itself_is_pointed_at_the_command() {
        let said = upgrade_sentence(&facts("0.1.0", true), "0.2.0").expect("a sentence");

        assert!(said.contains("0.1.0"), "{said}");
        assert!(said.contains("mix elevation upgrade"), "{said}");
    }

    /// The row `.claude/features/updates.md` describes in words: an old elevate keeps serving the
    /// operations it knows while the app asks the user to upgrade it. Without `supported_ops` the
    /// only way to find this out is to spend a prompt and be told `Unsupported`.
    #[test]
    fn an_older_helper_that_cannot_replace_itself_says_what_will() {
        let said = upgrade_sentence(&facts("0.1.0", false), "0.2.0").expect("a sentence");

        assert!(said.contains("0.1.0"), "{said}");
        assert!(said.contains("installer"), "{said}");
        assert!(
            !said.contains("mix elevation upgrade"),
            "the command cannot help here and must not be offered: {said}"
        );
    }

    /// A version neither side can parse is not a reason to tell somebody to reinstall.
    #[test]
    fn a_version_that_is_not_one_says_nothing() {
        assert!(upgrade_sentence(&facts("not a version", true), "0.2.0").is_none());
    }

    /// The operation list is read by name, and the name is the wire tag rather than a second
    /// spelling this module invented.
    #[test]
    fn a_helper_that_knows_the_operation_is_the_one_that_names_it() {
        assert!(facts("0.2.0", true).can_replace_itself());
        assert!(!facts("0.2.0", false).can_replace_itself());
    }
}
