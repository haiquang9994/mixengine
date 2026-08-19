//! Performing a first-run ritual — roadmap task **T33**.
//!
//! `mixengine-core` says *what* has to happen once ([`generate::first_run`]); this says *when*, *in
//! what order*, and *with what credentials*. It is in the daemon because both of the things it needs
//! are: the OS keyring, and the job system a long operation reports its progress through.
//!
//! # The order, and why it is that one
//!
//! 1. **Read the markers.** A directory that is already ready costs one `read_dir` and nothing else,
//!    which is what makes this affordable at the top of every start.
//! 2. **Store the credentials.** *Before* anything is created, so a machine with no credential store
//!    fails while there is nothing to clean up — rather than half-way through, leaving a data
//!    directory whose root password exists nowhere.
//! 3. **Write the started marker.** From here on, this directory is one we may clear.
//! 4. **Run the steps in order.** A step that fails ends the ritual; nothing after it runs, and the
//!    started marker is what lets the next attempt clean up rather than refuse.
//! 5. **Write the ready marker**, holding the package version that performed it.
//!
//! [`generate::first_run`]: mixengine_core::generate::first_run

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use mixengine_core::generate::first_run::{self, DataDirectory, FirstRun, SecretSpec, Step};
use mixengine_platform::{Host, KEYRING_SERVICE};
use mixengine_proto::{Error, ErrorCode, JobKind, JobOutcome, Millis};

use crate::error::ToWire as _;
use crate::jobs::{JobHandle, Jobs};

/// What a first-run job is called in `mix job list`.
///
/// A validated string rather than an enum variant, which is [`JobKind`]'s own design: a new kind of
/// work needs no change to the protocol, and a client that does not know this one still renders the
/// row.
const KIND: &str = "service.first_run";

/// How much longer than the work itself the job is waited for.
///
/// The steps carry their own deadlines and the last thing each of them does is kill the program it
/// ran, so this covers only what is around them — two keyring writes, two marker files, and a
/// machine that is compiling something else at the same time.
const SLACK: Millis = Millis(60_000);

/// Do it, if it has not been done. Returns once the data directory is ready to be started against.
///
/// # Errors
///
/// A wire error a client renders: a machine with no credential store, a data directory that is not
/// ours, a step that failed. Every one of them leaves the service unstarted, and the durable account
/// of what went wrong is the job row — which holds the step and its output, and outlives this call.
pub(super) async fn ensure(
    host: &Arc<dyn Host>,
    jobs: &Arc<Jobs>,
    plan: &FirstRun,
) -> Result<(), Error> {
    if !needed(plan.data()).await? {
        return Ok(());
    }

    let kind = JobKind::parse(KIND).expect("a constant that is a valid job kind");
    let patience = patience_for(plan);

    let (host, work) = (Arc::clone(host), plan.clone());
    let started = jobs
        .begin(&kind, move |handle| async move {
            perform(&host, &work, &handle).await
        })
        .await?;

    let finished = jobs.wait(started.id, patience).await?;

    match finished.outcome {
        Some(JobOutcome::Succeeded { .. }) => Ok(()),
        Some(JobOutcome::Failed { error }) => Err(error),

        // Cancelled, or still running when the wait gave up. Both leave the started marker behind,
        // so the next attempt clears the directory rather than refusing it.
        other => Err(Error::new(
            ErrorCode::Internal,
            format!(
                "the first run of this service did not finish; `mix job status {}` has the account \
                 of it ({other:?})",
                started.id
            ),
        )),
    }
}

/// Whether there is anything to do, and a refusal for a directory that is not ours.
///
/// Takes the directory rather than the plan, because the directory is the whole of the question —
/// and because a `FirstRun` can only be built by `mixengine-core`'s generator, which would put this
/// decision out of reach of a test.
async fn needed(data: &Path) -> Result<bool, Error> {
    match first_run::inspect(data)
        .await
        .map_err(|error| error.to_wire())?
    {
        DataDirectory::Ready { .. } => Ok(false),
        DataDirectory::Empty => Ok(true),

        // Ours, and half-done. **The only case anything is ever deleted in**, and the marker is why:
        // what is cleared is a directory carrying our own in-progress evidence and nothing else.
        DataDirectory::Unfinished => {
            tracing::warn!(
                data = %data.display(),
                "this data directory holds a first run that did not finish; clearing it and \
                 starting again"
            );

            first_run::clear(data)
                .await
                .map_err(|error| error.to_wire())?;

            Ok(true)
        }

        DataDirectory::Foreign => Err(Error::new(
            ErrorCode::PreconditionFailed,
            format!(
                "{} has contents and was not created by MixEngine, so nothing here will bootstrap \
                 over it",
                data.display()
            ),
        )
        .with_hint("point this service at an empty directory, or move that one aside")),
    }
}

/// How long the job is given: the sum of what its steps ask for, and a little around them.
///
/// Read off the plan rather than written down, because the steps are the recipe's and a constant
/// here would be a second opinion about how long a bootstrap takes.
fn patience_for(plan: &FirstRun) -> Millis {
    // An empty map is what a plan whose steps cannot be built has, and the wait then covers the
    // failure that is about to be reported rather than nothing at all.
    let steps = plan.steps(BTreeMap::new()).unwrap_or_default();
    let asked: u64 = steps.iter().map(|step| step.timeout.0).sum();

    Millis(asked.saturating_add(SLACK.0))
}

/// Generate every credential this ritual declares and put it in the OS keyring.
///
/// **Before the disk is touched.** A machine with no credential store — a headless Linux, a WSL
/// session, CI without a secret service — fails here, with nothing created and a message naming what
/// is missing. It does **not** fall back to a file: the root password has exactly one home, and a
/// second one would be a plaintext credential on disk where this project has never had one.
///
/// Whoever meets this on a headless Linux sees the wrong error until **T15b** lands — `secrets.rs`
/// maps only `NoStorageAccess` to `UnsupportedPlatform`, and a session with no provider at all
/// currently arrives as `Error::Secret`.
async fn store_the_secrets(
    host: &Arc<dyn Host>,
    secrets: &'static [SecretSpec],
    address: impl Fn(&str) -> String,
) -> Result<BTreeMap<String, String>, Error> {
    let mut generated = BTreeMap::new();

    for spec in secrets {
        let secret =
            mixengine_platform::generate_secret(spec.length).map_err(|error| error.to_wire())?;
        let (host, key, value) = (Arc::clone(host), address(spec.key), secret.clone());

        // The keyring blocks, and on Linux it blocks on a D-Bus round trip to a daemon that may be
        // prompting the user to unlock their keyring. `.claude/standards/rust.md`'s rule for
        // anything that can hang.
        tokio::task::spawn_blocking(move || {
            host.keyring().set_secret(KEYRING_SERVICE, &key, &value)
        })
        .await
        .map_err(|_| {
            Error::new(
                ErrorCode::Internal,
                "the task storing this service's credential did not finish".to_owned(),
            )
        })?
        .map_err(|error| error.to_wire())?;

        generated.insert(spec.key.to_owned(), secret);
    }

    Ok(generated)
}

/// The ritual itself, inside the job.
async fn perform(
    host: &Arc<dyn Host>,
    plan: &FirstRun,
    handle: &JobHandle,
) -> Result<serde_json::Value, Error> {
    handle
        .progress(0, "generating this service's credentials")
        .await;

    let secrets = store_the_secrets(host, plan.secrets(), |key| plan.secret_address(key)).await?;
    let steps = plan.steps(secrets).map_err(|error| error.to_wire())?;

    first_run::mark_started(plan.data())
        .await
        .map_err(|error| error.to_wire())?;

    for (index, step) in steps.iter().enumerate() {
        if handle.is_cancelled() {
            // Any error at all is enough: `Jobs::begin` reports work that failed while its token was
            // cancelled as cancelled, so the outcome on the row says so without a code for it here.
            return Err(Error::new(
                ErrorCode::Internal,
                "the first run of this service was cancelled".to_owned(),
            ));
        }

        // Capped below a hundred, so the last step never reports finished before the marker that
        // says it is finished has been written.
        let percent = u8::try_from(index * 90 / steps.len().max(1)).unwrap_or(90);
        handle.progress(percent, step.label.clone()).await;

        run(step).await?;
    }

    first_run::mark_ready(plan.data(), plan.version())
        .await
        .map_err(|error| error.to_wire())?;

    handle.progress(100, "this service is ready to start").await;

    Ok(serde_json::json!({
        "data_dir": plan.data(),
        "version": plan.version(),
    }))
}

/// Run one step, and fail the ritual if it did not succeed.
///
/// **What it printed goes into the failure and nowhere else.** A step's output can hold a bootstrap
/// server's own SQL error, which is the only thing that says why a data directory would not be made
/// — `mariadb-install-db`'s own summary never contains it, and the runner it ran on is thrown away.
async fn run(step: &Step) -> Result<(), Error> {
    let args: Vec<std::ffi::OsString> = step.args.iter().map(std::ffi::OsString::from).collect();
    let patience = step.timeout.as_duration();

    let ran = match &step.stdin {
        None => {
            mixengine_platform::process::run_once(
                &step.program,
                &args,
                &step.cwd,
                &step.env,
                patience,
            )
            .await
        }
        Some(input) => {
            mixengine_platform::process::run_once_with_input(
                &step.program,
                &args,
                &step.cwd,
                &step.env,
                patience,
                input,
            )
            .await
        }
    }
    .map_err(|error| error.to_wire())?;

    if ran.succeeded() {
        return Ok(());
    }

    Err(Error::new(
        ErrorCode::Internal,
        format!(
            "{} failed while {}: {}{}",
            step.program.display(),
            step.label,
            ran.exit().map_or_else(
                || "it ran out of time".to_owned(),
                |exit| format!("it exited with {exit}")
            ),
            ran.complaint()
                .map_or_else(String::new, |said| format!(" — {said}")),
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A [`FirstRun`] can only be built by `mixengine-core`'s generator, so what is provable here is
    /// the two halves that need no plan: the refusal a foreign directory earns, and the ordering
    /// that keeps a machine with no credential store from creating one.
    ///
    /// The whole ritual against a real server is `crates/mixengine-cli/tests/mariadb.rs`.
    fn host() -> Arc<dyn Host> {
        Arc::new(mixengine_platform::mock::Host::with_home(
            std::env::temp_dir(),
        ))
    }

    /// Storing the credentials creates nothing on disk.
    ///
    /// **The ordering is the assertion.** The design's promise is that a machine with no credential
    /// store fails while nothing has been created, and that promise is only worth anything if the
    /// step before the first directory is made is this one.
    #[tokio::test]
    async fn storing_a_credential_creates_no_data_directory() {
        const SECRETS: &[SecretSpec] = &[SecretSpec {
            key: "root",
            length: 32,
        }];

        let home = tempfile::tempdir().expect("a directory");
        let data = home.path().join("data").join("mariadb").join("main");

        let stored = store_the_secrets(&host(), SECRETS, |key| format!("mariadb@main/{key}"))
            .await
            .expect("a mock keyring takes what it is given");

        assert_eq!(stored.len(), 1, "{stored:?}");
        assert_eq!(
            stored["root"].len(),
            32,
            "the length a recipe declared is the length it got"
        );
        assert!(
            !data.exists(),
            "storing a credential created a data directory"
        );
    }

    /// A data directory that is not ours is refused, and is still there afterwards.
    ///
    /// `.claude/features/services.md` says a half-finished data directory is cleaned; this is the
    /// assertion that keeps it from also meaning *MixEngine deletes a database it did not create*.
    #[tokio::test]
    async fn a_foreign_data_directory_is_refused_and_left_alone() {
        let home = tempfile::tempdir().expect("a directory");
        let data = home.path().join("data");

        std::fs::create_dir_all(&data).expect("a directory");
        std::fs::write(data.join("ibdata1"), b"somebody's database").expect("contents");

        let refused = needed(&data).await.expect_err("that directory is not ours");

        assert_eq!(refused.code, ErrorCode::PreconditionFailed, "{refused:?}");
        assert!(
            refused.message.contains("was not created by MixEngine"),
            "{refused:?}"
        );
        assert!(
            data.join("ibdata1").is_file(),
            "the refusal removed something"
        );
    }

    /// One we began and did not finish is cleared rather than refused, and then there is work to do.
    #[tokio::test]
    async fn a_half_finished_data_directory_is_cleared_and_done_again() {
        let home = tempfile::tempdir().expect("a directory");
        let data = home.path().join("data");

        first_run::mark_started(&data).await.expect("the marker");
        std::fs::write(data.join("ibdata1"), b"half a database").expect("contents");

        assert!(needed(&data).await.expect("ours to clear"));
        assert!(
            !data.join("ibdata1").exists(),
            "the half-finished directory was reused rather than cleared"
        );
    }

    /// And one that finished is left alone, with nothing to do.
    #[tokio::test]
    async fn a_finished_data_directory_is_not_bootstrapped_again() {
        let home = tempfile::tempdir().expect("a directory");
        let data = home.path().join("data");

        first_run::mark_started(&data).await.expect("the marker");
        first_run::mark_ready(&data, "11.4.9")
            .await
            .expect("and the second");

        assert!(!needed(&data).await.expect("a readable directory"));
    }
}
