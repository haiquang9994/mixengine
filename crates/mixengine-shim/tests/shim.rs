//! The shim as a user meets it: a file in `bin/`, run from a directory, with no daemon anywhere.
//!
//! Roadmap task **T25**, and the claim this suite exists for is Phase 2's milestone — `php -v` in
//! one directory and `php -v` in another are two different programs, with no shell hook installed
//! and nothing running in the background. Everything here drives the **real binary** as a child
//! process, because the two things worth proving about a shim are exactly the two an in-process
//! test cannot reach: that it reads the name it was invoked by, and that the exit code a shell sees
//! is the program's own.
//!
//! `fakeservice` stands in for every runtime here, as it does for the install tests: the real
//! artifacts are tens of megabytes each, and downloading one to find out which of two directories a
//! shim resolved would be a suite that needs the network to answer a question about a database.
//! What a fake proves here is what the shim does rather than what a runtime is: it has to do
//! what a shim asks of any program — record the environment it was handed, prove it received its
//! arguments, and exit with a status of its choosing — and `--dump-env`, `--touch` and `--exit-code`
//! are those three.
//!
//! **No `MIXENGINE_HOME` is ever set in this process.** Each case sets it on the child's own
//! `Command`, which is what `.claude/standards/testing.md` requires and what lets these run in
//! parallel: `std::env::set_var` is process-global, and two homes in one binary would overwrite each
//! other.
//!
//! The home itself is in [`harness`], shared with `overhead.rs` — T29 measures what a shim costs
//! and this file measures what it does, and the two must be asking about the same installation or
//! the budget is a number about a home nobody has.

use std::collections::BTreeMap;
use std::process::Command;

use mixengine_core::{paths, shims};
use mixengine_proto::RuntimeKind;

mod harness;

use harness::Home;

/// **Phase 2's milestone.** Two directories, one command, two different programs — and no daemon
/// was started, no socket was opened and no shell hook exists.
#[test]
fn the_same_command_runs_a_different_version_in_a_different_directory() {
    let home = Home::with(&["8.1.30", "8.3.33"]);

    let pinned = home.project("blog", Some("[runtimes]\nphp = \"8.3\"\n"));
    let plain = home.project("scratch", None);

    let ran = home.record(&pinned, &BTreeMap::new(), 7);

    // The status is the program's own, which is what makes `php -l file.php && …` a sentence about
    // PHP rather than about the shim.
    assert_eq!(ran.run.code(), 7, "{}", ran.run.stderr());
    assert!(
        ran.reached,
        "the arguments reached the program: {}",
        ran.run.stderr()
    );
    assert_eq!(
        ran.ran_from(),
        home.runtime_directory("8.3.33").join("bin"),
        "the manifest pinned 8.3"
    );

    // The directory next door pins nothing, so it gets the default — which is the first version
    // installed, not the newest.
    let ran = home.record(&plain, &BTreeMap::new(), 0);

    assert_eq!(ran.run.code(), 0, "{}", ran.run.stderr());
    assert_eq!(
        ran.ran_from(),
        home.runtime_directory("8.1.30").join("bin"),
        "nothing asked for a version, so the kind's default answered"
    );
}

/// **A home with two languages in it** — roadmap task T27, where Node.js becomes the second.
///
/// Everything above this point is one runtime kind, which cannot tell apart a shim that dispatches
/// on the *command* from one that would have worked just as well hard-wired to PHP. Here `bin/`
/// holds both, one row each, and each command resolves against its own kind's default: a `node`
/// that answers has not made `php` answer differently, and neither has heard of the other.
#[test]
fn a_second_language_in_the_same_home_resolves_on_its_own() {
    let home = Home::with(&["8.1.30", "8.3.33"]);
    let published = format!("bin/node{}", std::env::consts::EXE_SUFFIX);
    home.install(
        RuntimeKind::Node,
        "22.23.2",
        [("node".to_owned(), published)].into_iter().collect(),
    );

    // A manifest pinning PHP and saying nothing about Node, which is the ordinary shape of one.
    let project = home.project("blog", Some("[runtimes]\nphp = \"8.3\"\n"));

    let ran = home.record_command("node", &project, &BTreeMap::new(), 0);
    assert_eq!(ran.run.code(), 0, "{}", ran.run.stderr());
    assert_eq!(
        ran.ran_from(),
        home.path()
            .join("runtimes")
            .join("node")
            .join("22.23.2")
            .join("bin"),
        "node resolved against the node rows, not against the manifest's php pin"
    );

    let ran = home.record(&project, &BTreeMap::new(), 0);
    assert_eq!(
        ran.ran_from(),
        home.runtime_directory("8.3.33").join("bin"),
        "and php is still what the manifest says it is"
    );
}

/// **Two commands, one executable** — the half of T27 that Python and Ruby are the first to reach.
///
/// [`shims::Command`] has carried a `name` the user types and an `executable` the artifact publishes
/// since T25, and until now nothing could tell them apart: every published artifact happened to name
/// its executables exactly as its commands are typed, so a shim that looked the invoked name up in
/// `provides` would have passed every test in this file.
///
/// Python is where that stops. `python-build-standalone` publishes one interpreter, the index calls
/// it `python`, and `python3` — which is what most projects' scripts actually invoke — is a second
/// command for the same one. Ruby does the same with `bundler` and `bundle`. So this installs a
/// runtime whose executable is named like neither command, and runs it under both.
#[test]
fn two_commands_can_be_one_executable_named_like_neither() {
    let home = Home::with(&["8.3.33"]);

    // What the real Unix artifact publishes: a versioned file, which is a third name again.
    let published = format!("bin/python3.12{}", std::env::consts::EXE_SUFFIX);
    home.install(
        RuntimeKind::Python,
        "3.12.14",
        [("python".to_owned(), published)].into_iter().collect(),
    );

    let project = home.project("api", None);
    let expected = home
        .path()
        .join("runtimes")
        .join("python")
        .join("3.12.14")
        .join("bin");

    for command in ["python", "python3"] {
        let ran = home.record_command(command, &project, &BTreeMap::new(), 0);

        assert_eq!(ran.run.code(), 0, "{command}: {}", ran.run.stderr());
        assert!(
            ran.reached,
            "{command} reached the interpreter: {}",
            ran.run.stderr()
        );
        assert_eq!(
            ran.ran_from(),
            expected,
            "{command} runs the executable the index calls `python`, whatever the file is named"
        );
    }
}

/// **What `npm` is on Windows**, and the one thing about fronting it that is not obvious.
///
/// A Windows Node.js artifact publishes `npm` as `npm.cmd`: upstream ships `npm` as a shell script
/// for Git Bash and the `.cmd` as the thing a Windows process can start. A batch file is not a PE
/// image and `CreateProcess` refuses one — what makes this work at all is that
/// `std::process::Command` recognises the extension, goes through `cmd.exe`, hands back the batch
/// file's own exit code and escapes the arguments against `&`-style injection on the way.
///
/// That is a property of the standard library rather than of any code here, which is exactly why it
/// is worth a test: if it ever stops holding, `npm` breaks on every Windows machine and nothing
/// else in this suite would notice.
#[cfg(windows)]
#[test]
fn a_shim_fronts_a_batch_file_because_that_is_what_npm_is_on_windows() {
    let home = Home::with(&["8.3.33"]);
    home.install(
        RuntimeKind::Node,
        "22.23.2",
        [
            ("node".to_owned(), "node.exe".to_owned()),
            ("npm".to_owned(), "npm.cmd".to_owned()),
        ]
        .into_iter()
        .collect(),
    );

    let runtime = home.path().join("runtimes").join("node").join("22.23.2");
    // `%~2` strips the quoting `Command` added; `%*` is every argument as the batch file received
    // them, which is what proves they survived the trip through cmd.exe intact.
    std::fs::write(
        runtime.join("npm.cmd"),
        "@echo off\r\n> \"%~2\" echo args=%*\r\n>> \"%~2\" echo first=%PATH%\r\nexit /b 7\r\n",
    )
    .expect("a batch file");

    let project = home.project("app", None);
    let recorded = project.join("npm-said.txt");
    let ran = home.run_with(
        "npm",
        &project,
        &["--record", &recorded.display().to_string(), "install"],
        &BTreeMap::new(),
    );

    assert_eq!(
        ran.code(),
        7,
        "the batch file's own status: {}",
        ran.stderr()
    );

    let said = std::fs::read_to_string(&recorded).expect("the batch file ran and wrote its file");
    assert!(
        said.contains("--record") && said.contains("install"),
        "every argument reached it: {said}"
    );

    let path = said
        .lines()
        .find_map(|line| line.strip_prefix("first="))
        .expect("the batch file recorded its PATH");
    assert_eq!(
        std::env::split_paths(path).next().expect("a first entry"),
        runtime,
        "the runtime's own directory is ahead of everything, which is how npm.cmd finds node.exe"
    );
}

/// **The other half of that milestone** — roadmap task T26.
///
/// The case above proves a shim in `bin/` becomes the right PHP. This one proves the directory a
/// person's PATH points at has every command in it and nothing else, and that starting again does
/// not rewrite a byte — which is what a daemon start does nineteen times a second boot.
#[test]
fn bin_holds_one_command_per_row_and_a_second_pass_writes_nothing() {
    let home = Home::with(&["8.3.33"]);
    let bin = home.path().join("bin");

    for command in shims::COMMANDS {
        let file = bin.join(shims::file_name(command));
        assert!(file.is_file(), "{} is not in bin/", file.display());
    }

    // Something that answers to nothing, of the kind a renamed row leaves behind.
    let stranger = bin.join(format!("php7{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&stranger, b"from a MixEngine that is not this one").expect("a file");

    let refreshed = home.fill_bin();

    assert!(
        refreshed.written.is_empty(),
        "nothing changed, so nothing should have been copied: {:?}",
        refreshed.written
    );
    assert_eq!(
        refreshed.removed,
        vec![stranger.file_name().unwrap().to_string_lossy().into_owned()]
    );
    assert!(!stranger.exists());
    assert_eq!(refreshed.commands.len(), shims::COMMANDS.len());
}

/// Step one of the resolution order, read by the process the user invoked — which is the only
/// process that can read it.
#[test]
fn the_environment_variable_beats_the_directory_the_command_was_run_in() {
    let home = Home::with(&["8.1.30", "8.3.33"]);
    let pinned = home.project("blog", Some("[runtimes]\nphp = \"8.3\"\n"));

    let asked: BTreeMap<&str, String> = [("MIXENGINE_PHP", "8.1".to_owned())].into_iter().collect();
    let ran = home.record(&pinned, &asked, 0);

    assert_eq!(ran.run.code(), 0, "{}", ran.run.stderr());
    assert_eq!(
        ran.ran_from(),
        home.runtime_directory("8.1.30").join("bin"),
        "MIXENGINE_PHP outranks the manifest in the directory"
    );

    // And a value that is not a version is refused rather than quietly ignored: a variable that
    // does nothing looks exactly like a variable that does not work.
    let ran = home.run_with(
        "php",
        &pinned,
        &["--version"],
        &[("MIXENGINE_PHP", "eight".to_owned())]
            .into_iter()
            .collect(),
    );

    assert_eq!(ran.code(), 127);
    assert!(ran.stderr().contains("MIXENGINE_PHP"), "{}", ran.stderr());
}

/// What somebody sees when the version their project asks for is not on this machine, and the half
/// of it that tells them what to type.
#[test]
fn a_version_that_is_not_installed_names_the_command_that_would_install_it() {
    let home = Home::with(&["8.3.33"]);
    let project = home.project("legacy", Some("[runtimes]\nphp = \"8.1.30\"\n"));

    // `--version` and not the recording pair: nothing is going to run, so there is no program to
    // ask for a dump, and the flag is what a person types first when they want to know what they
    // have.
    let ran = home.run_with("php", &project, &["--version"], &BTreeMap::new());
    let said = ran.stderr();

    assert_eq!(ran.code(), 127, "{said}");
    assert!(
        said.starts_with("php:"),
        "named as the command typed: {said}"
    );
    assert!(said.contains("8.1.30"), "{said}");
    assert!(
        said.contains("mix runtime install php 8.1.30"),
        "the hint is the command to type: {said}"
    );
}

/// The binary as cargo built it, before it has been copied into `bin/` under a name that means
/// something.
#[test]
fn the_shim_run_under_its_own_name_says_what_it_answers_to() {
    let home = Home::with(&["8.3.33"]);
    let project = home.project("blog", None);

    let ran = Command::new(env!("CARGO_BIN_EXE_mixengine-shim"))
        .current_dir(&project)
        .env("MIXENGINE_HOME", home.path())
        .output()
        .expect("the shim runs");

    let said = String::from_utf8_lossy(&ran.stderr).into_owned();
    assert_eq!(ran.status.code(), Some(127), "{said}");
    assert!(said.contains("php"), "it lists what it answers to: {said}");
    assert!(said.contains("node"), "{said}");
}

/// A machine where MixEngine has never run, or a `MIXENGINE_HOME` pointing at the wrong place.
/// The shim must not create a database there — the daemon owns that file, and one written by a
/// `php -v` would be one with no schema in it at all.
#[test]
fn a_home_with_no_database_is_refused_rather_than_created() {
    let home = Home::with(&["8.3.33"]);
    let elsewhere = tempfile::tempdir().expect("a directory that is not a home");
    let project = home.project("blog", None);

    let shim = home
        .path()
        .join("bin")
        .join(format!("php{}", std::env::consts::EXE_SUFFIX));

    let ran = Command::new(&shim)
        .arg("--version")
        .current_dir(&project)
        .env("MIXENGINE_HOME", elsewhere.path())
        .output()
        .expect("the shim runs");

    let said = String::from_utf8_lossy(&ran.stderr).into_owned();
    assert_eq!(ran.status.code(), Some(127), "{said}");
    assert!(
        !elsewhere.path().join(paths::DATABASE_FILE_NAME).exists(),
        "asking must not leave a database behind: {said}"
    );
    assert!(
        said.contains(paths::DATABASE_FILE_NAME),
        "the message names the file it looked for: {said}"
    );
}
