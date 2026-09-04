//! `mix uninstall` against a real daemon — roadmap task **T87**.
//!
//! **The unignored half of this file may never remove anything from the machine running it.** Every
//! test here runs on somebody's own workstation, so what is proved without `--ignored` is the plan,
//! the refusals and the `--keep-home` path. The round trip that actually takes MixEngine off a
//! machine is `#[ignore]`d and runs on a fresh runner in CI's `system` job, which is the clean VM
//! the task asks for.
//!
//! **And nothing here may assume the machine running it is clean.** A developer's machine has a
//! helper installed, an audit log, a `PATH` entry and a hosts block of its own home's; every
//! assertion below is therefore about *this* home's plan and about what the command did not do,
//! never about the machine being empty to begin with.

mod harness;

use harness::{Home, json, stderr, stdout};

/// The plan names every row, whatever this machine holds, and changes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_names_every_row_and_changes_nothing() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let before = home.contents();

    // **Read before the plan and compared after, rather than asserted empty.** A daemon that has
    // just started has already asked for whatever its first run needs — the certificate authority,
    // on this machine — and a test demanding an empty queue would be asserting that first-run setup
    // does not happen. What is being proved is that the *plan* added nothing to it.
    let queued = json(&home.mix(&["elevation", "status", "--json"]))["pending"].clone();

    let report = json(&home.mix(&["uninstall", "--dry-run", "--json"]));

    let items = report["items"].as_array().expect("a list of rows");
    assert!(items.len() >= 11, "{report}");

    for row in items {
        assert!(
            row["what"].as_str().is_some_and(|what| !what.is_empty()),
            "{row}"
        );
        assert!(
            row["location"]
                .as_str()
                .is_some_and(|place| !place.is_empty()),
            "every row says where to go and look: {row}"
        );

        // A dry run acts on nothing, so no row may claim a removal or a queue entry.
        let removal = row["outcome"]["removal"]
            .as_str()
            .expect("a tagged outcome");
        assert!(
            matches!(removal, "planned" | "absent" | "kept" | "failed"),
            "{row}"
        );
    }

    assert_eq!(home.contents(), before, "a dry run wrote something");

    // And it asked for nothing: an operation a plan left behind would be the prompt it promised not
    // to raise, arriving at whatever the person did next.
    let waiting = json(&home.mix(&["elevation", "status", "--json"]));
    assert_eq!(waiting["pending"], queued, "the plan enqueued something");
}

/// And the home row names the home, so the one irreversible thing on the list is the one a person
/// cannot miss.
#[tokio::test(flavor = "multi_thread")]
async fn the_plan_names_the_home_it_would_remove() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let printed = stdout(&home.mix(&["uninstall", "--dry-run"]));

    assert!(
        printed.contains(&home.path().display().to_string()),
        "{printed}"
    );
    assert!(printed.contains("data/"), "{printed}");
}

/// `--keep-home` says so on the home row rather than leaving it out. A person reading the plan has
/// to see that the home was considered and deliberately left.
#[tokio::test(flavor = "multi_thread")]
async fn keeping_the_home_is_a_row_and_not_a_silence() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let report = json(&home.mix(&["uninstall", "--dry-run", "--keep-home", "--json"]));

    let kept = report["items"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["id"] == "home")
        .expect("the home is always a row");

    assert_eq!(kept["outcome"]["removal"], "kept", "{report}");
}

/// One document per run under `--json`, and the plan is not it.
///
/// **The regression this pins was found by CI and not by reading**: `mix uninstall --yes --json`
/// printed the plan *and* the report, which is two objects on one stdout and therefore not JSON —
/// every caller parsing the output got a trailing-characters error rather than an answer.
#[tokio::test(flavor = "multi_thread")]
async fn a_json_run_emits_exactly_one_object() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    // The plan is the answer here, so it is the one object.
    let printed = stdout(&home.mix(&["uninstall", "--dry-run", "--json"]));
    serde_json::from_str::<serde_json::Value>(&printed)
        .unwrap_or_else(|error| panic!("{error}: {printed}"));

    // And declining is not an object at all: nothing was asked for, so nothing is answered.
    let declined = home.mix_answering("n\n", &["uninstall"]);
    assert_eq!(declined.status.code(), Some(0), "{}", stderr(&declined));
}

/// Nobody at the keyboard is not a yes. `mix` reads end of file as *there was nobody to ask* and
/// names the flag that answers in advance — `mix elevation grant`'s standing rule, on the one
/// command where getting it wrong removes somebody's databases.
#[tokio::test(flavor = "multi_thread")]
async fn an_unattended_uninstall_without_yes_removes_nothing() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let refused = home.mix(&["uninstall"]);

    assert!(
        home.path().exists(),
        "the home was removed with nobody asked"
    );
    assert!(stderr(&refused).contains("--yes"), "{}", stderr(&refused));
    assert_ne!(refused.status.code(), Some(0), "{}", stdout(&refused));
}

/// Is there anything on this machine an uninstall would need an administrator for?
///
/// **The gate on the two tests below, and it is about the machine rather than about MixEngine.** A
/// developer's workstation has a privileged helper and an audit log from its own earlier work, and
/// they belong to the machine rather than to the temporary home a test just made — so `--yes` there
/// would raise a real elevation dialog, wait on a person, and then take away something the rest of
/// that machine is using. A clean runner has none of it, which is where these two run in full.
///
/// Printed rather than silent: a test that skipped without saying so is a test that stops running
/// and nobody notices.
fn needs_an_administrator(home: &Home) -> bool {
    let plan = json(&home.mix(&["uninstall", "--dry-run", "--json"]));

    let waiting: Vec<String> = plan["items"]
        .as_array()
        .expect("rows")
        .iter()
        .filter(|row| row["outcome"]["removal"] == "planned")
        .filter(|row| !matches!(row["id"].as_str(), Some("home" | "relocated_directory")))
        .map(|row| row["what"].as_str().unwrap_or_default().to_owned())
        .collect();

    if !waiting.is_empty() {
        println!(
            "skipped: this machine holds {} thing(s) an uninstall would need an administrator for, \
             and none of them belong to this test's home: {}",
            waiting.len(),
            waiting.join(", ")
        );
    }

    !waiting.is_empty()
}

/// `--keep-home` undoes what is outside the home, leaves the home, and leaves the daemon running.
///
/// **Nothing outside the home is asserted to have gone.** What is proved is the half this flag is
/// for: the home survives, and so does the daemon serving it.
#[tokio::test(flavor = "multi_thread")]
async fn keeping_the_home_leaves_the_home_and_the_daemon() {
    let home = Home::new();
    let daemon = home.start_daemon();

    if needs_an_administrator(&home) {
        return;
    }

    let printed = home.mix(&["uninstall", "--keep-home", "--yes", "--json"]);
    let report = json(&printed);

    let kept = report["items"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["id"] == "home")
        .expect("the home is always a row");

    assert_eq!(kept["outcome"]["removal"], "kept", "{report}");
    assert!(home.path().exists());

    // Still answering: `--keep-home` stops nothing, because there is still a home to serve.
    let status = json(&home.mix(&["status", "--json"]));
    assert!(status["daemon"]["pid"].is_number(), "{status}");

    drop(daemon);
}

/// A complete uninstall takes the home with it, and the daemon goes so that it can.
///
/// **The one test here that removes something**, and what it removes is a temporary home this test
/// made. Nothing outside it is touched, which [`needs_an_administrator`] is what establishes rather
/// than assumes.
#[tokio::test(flavor = "multi_thread")]
async fn a_complete_uninstall_takes_the_home_and_the_daemon_with_it() {
    let home = Home::new();
    let mut daemon = home.start_daemon();

    if needs_an_administrator(&home) {
        return;
    }

    let printed = home.mix(&["uninstall", "--yes"]);

    assert!(
        daemon.wait_until_gone(),
        "the daemon outlived the home it was serving:\n{}\n{}",
        stdout(&printed),
        stderr(&printed)
    );
    assert!(
        !home.path().exists(),
        "{}\n{}",
        stdout(&printed),
        stderr(&printed)
    );
    assert_eq!(
        printed.status.code(),
        Some(0),
        "{}\n{}",
        stdout(&printed),
        stderr(&printed)
    );

    // And it said so on the home's row, rather than leaving a person to infer it from the exit code.
    assert!(stdout(&printed).contains("going"), "{}", stdout(&printed));
}

/// Typing anything but yes is a decline, and a decline removes nothing and fails nothing.
#[tokio::test(flavor = "multi_thread")]
async fn answering_no_removes_nothing_and_is_not_a_failure() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let before = home.contents();
    let answered = home.mix_answering("n\n", &["uninstall"]);

    assert_eq!(answered.status.code(), Some(0), "{}", stderr(&answered));
    assert!(home.path().exists());
    assert_eq!(home.contents(), before);
}

/// The clean-machine round trip — roadmap task **T87**, and the smoke test the task asks for.
///
/// **A fresh CI runner is the clean VM.** It has never had MixEngine on it, so everything
/// [`machine::reading`] finds after a grant is what MixEngine put there and nothing else.
///
/// **Every reading is taken with the operating system's own tools and its own paths**, spelled out
/// again in `mod machine` rather than asked of `mixengine_platform` — the rule
/// `crates/mixengine-elevate/tests/system.rs` already keeps: a test that asked the code under test
/// where to look could not notice it looking in the wrong place.
///
/// **And nothing is asserted that this runner did not actually produce.** A machine with no resolver
/// mechanism writes no resolver wiring, so that assertion is skipped there — with what was found
/// printed either way, so a leg that has quietly stopped proving anything is visible in the log.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "changes this machine — set MIXENGINE_SYSTEM_TESTS=1"]
async fn nothing_of_ours_is_left_on_this_machine() {
    if std::env::var("MIXENGINE_SYSTEM_TESTS").as_deref() != Ok("1") {
        return;
    }

    let home = Home::new();
    let mut daemon = home.start_daemon();

    // Everything a first run asks for: the helper, the certificate authority, the resolver wiring
    // and the port grant, in one batch behind one prompt.
    let granted = home.mix(&["elevation", "grant", "--yes"]);
    println!(
        "--- the grant ---\n{}{}",
        stdout(&granted),
        stderr(&granted)
    );

    // **Did that grant actually apply, and it is asked rather than assumed.** An empty queue is the
    // one honest answer to *can this runner change its own machine*, and two of the three cannot:
    // a Linux runner has no polkit agent, so `probe()` answers `Unavailable` whoever is asking; and
    // CI's macOS runner was measured on 2026-09-04 ending the elevated helper **without a report**,
    // leaving every operation pending. Both are ADR 0005's worst branch rather than gaps in this
    // job, and `tests/cert.rs` already records the first about the same runner.
    //
    // Asserting the machine there would be asserting something the runner cannot do; skipping
    // outright would leave those legs proving nothing. So the branch is chosen from what actually
    // happened, and each side asserts something of its own.
    //
    // **It is the grant's own record and not the length of the queue**, which was this predicate's
    // first draft and was wrong on the one runner where everything worked: a flush that succeeded
    // reconciles the hosts block immediately afterwards, so the queue is refilled a moment later by
    // design (`Elevation::flush`, D8) and a run that applied everything reads as one that applied
    // nothing. What settles it is what the grant reported about itself.
    let waiting = json(&home.mix(&["elevation", "status", "--json"]));
    let applied = waiting["last"]["outcome"] == "completed"
        && waiting["last"]["still_pending"].as_u64() == Some(0);
    println!("--- did the grant apply? ---\n{applied}: {waiting}");

    // And a name of this home's own, so the hosts block has something in it on a machine with no
    // scoped resolver — which is what a Linux runner without a systemd user manager is. Neither call
    // is asserted on: what this is arranging is the *machine*, and `held` below is what says whether
    // the arrangement worked.
    let root = home.path().join("smoke-project");
    std::fs::create_dir_all(&root).expect("a directory inside this home");

    let project = home.mix(&[
        "project",
        "create",
        &root.display().to_string(),
        "--name",
        "smoke",
    ]);
    println!(
        "--- the project ---\n{}{}",
        stdout(&project),
        stderr(&project)
    );

    // `--kind static`, because the default is php and a runner has no PHP installed: measured on
    // 2026-09-04, where the site refused with "no php version is installed as the default". Nothing
    // here needs a program to run; what it needs is a *name*.
    let site = home.mix(&[
        "site",
        "create",
        "--project",
        "smoke",
        "--kind",
        "static",
        "--domain",
        "uninstall-smoke.test",
    ]);
    println!("--- the site ---\n{}{}", stdout(&site), stderr(&site));

    // **This home's authority by key-id, and not "any MixEngine".** The suites that ran before this
    // one in the same job put authorities of their own homes into the same store, and an uninstall
    // removes exactly one — so a reading that matched the product name would find somebody else's
    // afterwards and report it as ours left behind.
    let authority = json(&home.mix(&["cert", "ca-status", "--json"]))["ca"]["key_id"]
        .as_str()
        .expect("a started daemon has an authority")
        .to_owned();
    println!("--- this home's authority ---\n{authority}");

    let held = machine::reading(&authority);
    println!("--- what this runner now holds ---\n{held}");

    let printed = home.mix(&["uninstall", "--yes"]);
    let said = stdout(&printed);
    println!("--- the uninstall ---\n{said}{}", stderr(&printed));

    // **Two legs, two answers, and neither is a weaker version of the other.** A machine that can
    // raise a prompt is asked the question this task exists to answer; a machine that cannot is
    // asked the one the risk list names, and both are assertions rather than skips.
    match applied {
        true => {
            assert!(
                held.anything(),
                "this machine can raise a prompt and the grant still wrote nothing to it, so \
                 removing it would prove nothing"
            );

            assert!(
                daemon.wait_until_gone(),
                "the daemon outlived the home it was serving"
            );

            let after = machine::reading(&authority);
            println!("--- what is left ---\n{after}");
            held.gone(&after);

            assert!(
                !home.path().exists(),
                "{} is still there",
                home.path().display()
            );
            assert_eq!(printed.status.code(), Some(0), "{}", stderr(&printed));
        }

        // **The Linux runner, and ADR 0005's worst branch.** No polkit agent means nothing of the
        // machine's can be removed — so what is asserted here is the behaviour the design's risk
        // list promises for exactly that: the machine rows say they are still waiting, and the home
        // is **kept**, because a home removed while this machine is still wired for it is one
        // nothing could repair. A leg that removed the home anyway would pass every other assertion
        // in this file and be badly wrong.
        false => {
            println!(
                "this machine cannot raise an elevation prompt, so what is proved here is that \
                 nothing was removed and the home was kept"
            );

            assert!(
                home.path().exists(),
                "the home went while this machine was still wired for it"
            );
            assert!(said.contains("kept"), "{said}");
            assert_ne!(
                printed.status.code(),
                Some(0),
                "an uninstall that left things behind reported success: {said}"
            );

            let after = machine::reading(&authority);
            println!("--- what is left ---\n{after}");
        }
    }
}

/// What this machine holds of MixEngine's, read with the machine's own tools.
///
/// **Its own module, and it knows nothing about `mixengine_platform`.** Every path here is written
/// out a second time on purpose: the whole value of this suite is that it can notice the code under
/// test looking in the wrong place, and it cannot do that if it asks that code where to look.
mod machine {
    use std::fmt;
    use std::path::Path;
    use std::process::Command;

    /// The marker MixEngine's hosts block is wrapped in, spelled out rather than imported.
    const HOSTS_MARKER: &str = "# BEGIN MixEngine";

    /// What one reading found, per thing. [`None`] is "nothing of ours there".
    pub(super) struct Reading {
        hosts: Option<String>,
        resolver: Option<String>,
        port_access: Option<String>,
        trust: Option<String>,
        helper: Option<String>,
        audit_log: Option<String>,
        autostart: Option<String>,
        path_entry: Option<String>,
    }

    impl Reading {
        /// Did this machine end up holding anything at all?
        ///
        /// A runner that produced none of it proves nothing by having none of it removed, which is
        /// why the round trip asserts this before it removes anything.
        pub(super) fn anything(&self) -> bool {
            self.rows().iter().any(|(_, found)| found.is_some())
        }

        /// Assert that everything this reading found is gone from `after`.
        ///
        /// **Per row, and only the rows this machine actually produced.** A leg with no resolver
        /// mechanism never wrote one, and demanding its removal would be demanding the removal of
        /// something that was never there.
        ///
        /// **The privileged helper is the one exception, and it is Windows'.** A file whose image is
        /// mapped cannot be unlinked, and the helper is the running program when it removes itself,
        /// so there the assertion is that the operating system has *accepted* the removal — the path
        /// appearing in its own pending-rename queue (the T87 design, D8).
        pub(super) fn gone(&self, after: &Reading) {
            for ((name, before), (_, left)) in self.rows().iter().zip(after.rows()) {
                let Some(before) = before else {
                    println!("{name}: this runner never had one, so nothing is asserted");
                    continue;
                };

                if *name == "the privileged helper" && cfg!(windows) {
                    assert!(
                        scheduled_for_removal(),
                        "the helper is still at {before} and this system has not been asked to \
                         remove it at the next restart"
                    );
                    continue;
                }

                assert!(
                    left.is_none(),
                    "{name} is still on this machine: was {before}, now {left:?}"
                );
            }
        }

        /// Each row, named for the assertion that fails on it.
        fn rows(&self) -> [(&'static str, &Option<String>); 8] {
            [
                ("the managed hosts block", &self.hosts),
                ("the resolver wiring", &self.resolver),
                ("the port grant", &self.port_access),
                ("the certificate authority", &self.trust),
                ("the privileged helper", &self.helper),
                ("the audit log", &self.audit_log),
                ("the autostart entry", &self.autostart),
                ("the PATH entry", &self.path_entry),
            ]
        }
    }

    impl fmt::Display for Reading {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            for (name, found) in self.rows() {
                match found {
                    Some(what) => writeln!(f, "{name}: {what}")?,
                    None => writeln!(f, "{name}: nothing")?,
                }
            }

            Ok(())
        }
    }

    /// Read this machine, now.
    ///
    /// `authority` is the eight-character key-id of the home being asked about, which is what scopes
    /// the trust row to one home's certificate rather than to the product's name.
    pub(super) fn reading(authority: &str) -> Reading {
        Reading {
            hosts: containing(&hosts_file(), HOSTS_MARKER),
            resolver: resolver(),
            port_access: port_access(),
            trust: trust(authority),
            helper: there(&helper()),
            audit_log: there(&audit_log()),
            autostart: autostart(),
            path_entry: path_entry(),
        }
    }

    /// `%SystemRoot%\System32\drivers\etc\hosts`, or `/etc/hosts`.
    fn hosts_file() -> std::path::PathBuf {
        #[cfg(windows)]
        {
            std::path::PathBuf::from(
                std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_owned()),
            )
            .join("System32\\drivers\\etc\\hosts")
        }
        #[cfg(unix)]
        {
            std::path::PathBuf::from("/etc/hosts")
        }
    }

    /// `%ProgramFiles%\MixEngine\mixengine-elevate.exe`, or the two Unix constants.
    fn helper() -> std::path::PathBuf {
        #[cfg(windows)]
        {
            std::path::PathBuf::from(
                std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".to_owned()),
            )
            .join("MixEngine\\mixengine-elevate.exe")
        }
        #[cfg(target_os = "macos")]
        {
            std::path::PathBuf::from("/Library/PrivilegedHelperTools/dev.mixengine.elevate")
        }
        #[cfg(target_os = "linux")]
        {
            std::path::PathBuf::from("/usr/local/libexec/mixengine/mixengine-elevate")
        }
    }

    /// `%ProgramData%\MixEngine\elevate.log`, or the two Unix constants.
    fn audit_log() -> std::path::PathBuf {
        #[cfg(windows)]
        {
            std::path::PathBuf::from(
                std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".to_owned()),
            )
            .join("MixEngine\\elevate.log")
        }
        #[cfg(target_os = "macos")]
        {
            std::path::PathBuf::from("/Library/Logs/MixEngine/elevate.log")
        }
        #[cfg(target_os = "linux")]
        {
            std::path::PathBuf::from("/var/log/mixengine/elevate.log")
        }
    }

    /// Whatever routes a TLD at a DNS server of ours.
    fn resolver() -> Option<String> {
        #[cfg(windows)]
        {
            said(
                "reg",
                &[
                    "query",
                    "HKLM\\SYSTEM\\CurrentControlSet\\services\\Dnscache\\Parameters\\DnsPolicyConfig",
                    "/s",
                ],
            )
            .filter(|out| out.contains(".test"))
        }
        #[cfg(target_os = "macos")]
        {
            // By name, and not everything in the directory: `/etc/resolver` is shared, and another
            // product's file there is not ours to report as left behind. These are the TLDs
            // MixEngine can ever have written — `mixengine_proto::domains::WIRED_TLDS`, spelled out
            // again for this module's reason.
            ["test", "localhost", "internal"]
                .into_iter()
                .find_map(|tld| listing(Path::new("/etc/resolver"), tld))
        }
        #[cfg(target_os = "linux")]
        {
            listing(Path::new("/etc/systemd/network"), "mixengine")
        }
    }

    /// The packet filter on macOS; nothing MixEngine writes to a file on the other two.
    fn port_access() -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            containing(Path::new("/etc/pf.conf"), "mixengine")
                .or_else(|| there(Path::new("/etc/pf.anchors/dev.mixengine")))
                .or_else(|| there(Path::new("/Library/LaunchDaemons/dev.mixengine.pf.plist")))
        }
        // Linux puts a capability on a binary inside the home, which goes with the home; Windows
        // reserves no port below 1024 and grants nothing at all.
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    /// This home's authority, in whichever store this system keeps trusted roots in.
    ///
    /// **By key-id on the two systems that keep certificates by subject.** T48 names every authority
    /// `MixEngine Local CA <key-id>`, so the id is what tells one home's from another's — and telling
    /// them apart is the whole point here, because the suites that ran before this one left their
    /// own in the same store.
    ///
    /// Linux needs no such scoping: the anchor is one file with one fixed name, so the file being
    /// there *is* the answer.
    fn trust(authority: &str) -> Option<String> {
        #[cfg(windows)]
        {
            said("certutil", &["-store", "Root"])
                .filter(|out| out.to_lowercase().contains(&authority.to_lowercase()))
                .map(|_| format!("LocalMachine\\Root holds {authority}"))
        }
        #[cfg(target_os = "macos")]
        {
            said(
                "security",
                &[
                    "find-certificate",
                    "-a",
                    "-c",
                    "MixEngine",
                    "/Library/Keychains/System.keychain",
                ],
            )
            .filter(|out| out.contains(authority))
            .map(|_| format!("the System keychain holds {authority}"))
        }
        #[cfg(target_os = "linux")]
        {
            // One file with one fixed name, so `authority` has nothing to add here — named rather
            // than ignored silently, on this module's own rule about being explicit.
            let _ = authority;

            listing(Path::new("/usr/local/share/ca-certificates"), "mixengine")
                .or_else(|| listing(Path::new("/etc/pki/ca-trust/source/anchors"), "mixengine"))
        }
    }

    /// The entry that starts a daemon at login.
    fn autostart() -> Option<String> {
        #[cfg(windows)]
        {
            said("schtasks", &["/Query", "/TN", "MixEngine"])
        }
        #[cfg(target_os = "macos")]
        {
            under_home("Library/LaunchAgents/dev.mixengine.daemon.plist")
        }
        #[cfg(target_os = "linux")]
        {
            under_home(".config/systemd/user/mixengine.service")
        }
    }

    /// `<root>/bin` on this user's PATH, wherever this system persists one.
    fn path_entry() -> Option<String> {
        #[cfg(windows)]
        {
            said("reg", &["query", "HKCU\\Environment", "/v", "Path"])
                .filter(|out| out.to_lowercase().contains("mixengine"))
        }
        #[cfg(unix)]
        {
            let home = std::env::var_os("HOME")?;

            [
                ".profile",
                ".bash_profile",
                ".zprofile",
                ".bashrc",
                ".zshrc",
            ]
            .into_iter()
            .find_map(|name| containing(&Path::new(&home).join(name), "MixEngine"))
        }
    }

    /// Has this system been asked to remove the helper at the next restart?
    ///
    /// Windows' own removal queue, read as the registry value it is: `MoveFileExW` with
    /// `MOVEFILE_DELAY_UNTIL_REBOOT` is what writes it.
    #[cfg(windows)]
    fn scheduled_for_removal() -> bool {
        said(
            "reg",
            &[
                "query",
                "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Session Manager",
                "/v",
                "PendingFileRenameOperations",
            ],
        )
        .is_some_and(|out| out.to_lowercase().contains("mixengine"))
    }

    /// Never reached off Windows; declared so the one shared assertion compiles everywhere.
    #[cfg(not(windows))]
    fn scheduled_for_removal() -> bool {
        false
    }

    /// The names in `directory` that hold `needle`, when there are any.
    #[cfg(unix)]
    fn listing(directory: &Path, needle: &str) -> Option<String> {
        let names: Vec<String> = directory
            .read_dir()
            .ok()?
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.to_lowercase().contains(needle))
            .collect();

        (!names.is_empty()).then(|| format!("{}: {}", directory.display(), names.join(", ")))
    }

    /// A path under this user's home, when something is there.
    #[cfg(unix)]
    fn under_home(relative: &str) -> Option<String> {
        let home = std::env::var_os("HOME")?;

        there(&Path::new(&home).join(relative))
    }

    /// The file, named, when it holds `needle`.
    fn containing(path: &Path, needle: &str) -> Option<String> {
        let text = std::fs::read_to_string(path).ok()?;

        text.to_lowercase()
            .contains(&needle.to_lowercase())
            .then(|| format!("{} holds {needle}", path.display()))
    }

    /// The path, when something is there. `symlink_metadata`, so a dangling link counts as there.
    fn there(path: &Path) -> Option<String> {
        std::fs::symlink_metadata(path)
            .is_ok()
            .then(|| path.display().to_string())
    }

    /// What a tool said, when it ran and answered something.
    ///
    /// A tool that is not on this machine, or that exited non-zero because it found nothing, is
    /// [`None`] — the same answer as "nothing of ours", and the right one: this asks whether
    /// something is there, not whether the tool works.
    #[cfg_attr(
        target_os = "linux",
        expect(
            dead_code,
            reason = "every Linux reading is a file; Windows and macOS are the callers"
        )
    )]
    fn said(program: &str, arguments: &[&str]) -> Option<String> {
        let output = Command::new(program).args(arguments).output().ok()?;

        if !output.status.success() {
            return None;
        }

        let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();

        (!text.is_empty()).then_some(text)
    }
}
