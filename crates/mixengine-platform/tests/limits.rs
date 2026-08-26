//! What this machine will actually enforce of a service's declared limits.
//!
//! Roadmap task **T68**. The read half: applying a limit is done to a child through
//! `mixengine_platform::process` and is asserted in `tests/process.rs`.

use mixengine_platform::{
    Enforcement, Host as _, LimitMechanism, LimitSupport, MemoryMeasure, WhenExceeded, mock,
};

/// The mock answers whatever a test set.
///
/// **Which is what makes the degraded path reachable on a machine that is not degraded.** A Linux
/// session without a delegated `cpu` controller is the case D6 is written for, and no test may
/// depend on the runner it lands on having one — or on not having one.
#[test]
fn the_mock_answers_what_a_test_configured() {
    let mut host = mock::Host::with_home("/tmp/mixengine-test");

    host.set_limit_support(LimitSupport {
        mechanism: LimitMechanism::CgroupV2,
        cpu: Enforcement::Unavailable {
            why: "this session has no delegated cpu controller".to_owned(),
        },
        memory: Enforcement::Hard {
            when: WhenExceeded::Killed,
        },
        memory_measure: MemoryMeasure::ChargedPages,
        priority: true,
        cores: 8,
    });

    let support = host.resource_control().support();

    assert!(matches!(support.cpu, Enforcement::Unavailable { .. }));
    assert!(matches!(support.memory, Enforcement::Hard { .. }));
    assert_eq!(support.cores, 8);
}

/// The real host answers for the system this test is running on.
///
/// **The three answers are deliberately different**, so what is asserted is the shape each system is
/// *allowed* to give rather than one value all three must agree on. A Linux machine with no
/// delegated subtree is a legitimate answer here and must not fail this.
#[test]
fn the_real_host_answers_for_this_system() {
    let host = mixengine_platform::host();
    let support = host.resource_control().support();

    assert!(support.cores >= 1, "a machine has at least one core");
    assert!(
        support.priority,
        "setpriority and priority classes both exist"
    );

    if cfg!(target_os = "macos") {
        assert_eq!(support.mechanism, LimitMechanism::None);
        assert_eq!(support.cpu, Enforcement::Unsupported);
        assert_eq!(support.memory, Enforcement::Unsupported);
    } else if cfg!(windows) {
        assert_eq!(support.mechanism, LimitMechanism::JobObject);
        assert_eq!(support.memory_measure, MemoryMeasure::Commit);
        assert!(matches!(support.cpu, Enforcement::Hard { .. }));
        assert!(matches!(support.memory, Enforcement::Hard { .. }));
    } else {
        assert_eq!(support.mechanism, LimitMechanism::CgroupV2);
        assert_eq!(support.memory_measure, MemoryMeasure::ChargedPages);

        // `Hard` or `Unavailable` are both correct answers here and which one this machine gives is
        // a property of how its session was started. `Unsupported` is the one that would be wrong:
        // the mechanism exists on this system whether or not this machine will lend it.
        assert_ne!(support.cpu, Enforcement::Unsupported);
        assert_ne!(support.memory, Enforcement::Unsupported);
    }
}

/// A cap that cannot be applied does not stop the service — on every system.
///
/// **The one assertion in this file that must hold everywhere**, and D6 is the whole of it: macOS
/// enforces neither field, a Linux session may lend neither controller, and in both cases the spawn
/// has to succeed. Refusing here would make a blueprint written for three systems undeployable on
/// one, which is a worse product than an uncapped service and a sentence explaining why.
#[test]
fn a_service_starts_even_where_its_cap_cannot_be_applied() {
    use mixengine_platform::process::Limits;

    let (program, args) = staying_up();

    let mut child = mixengine_platform::process::spawn_supervised(
        &program,
        &args,
        &std::env::temp_dir(),
        &std::collections::BTreeMap::new(),
        &Limits {
            cpu_percent: Some(50),
            memory_mb: Some(64),
            priority: mixengine_platform::process::Priority::Background,
        },
    )
    .expect("a service asking for a cap starts whether or not the cap can be applied");

    assert!(child.pid() > 0);

    let _ = child.stop();
}

/// A program every system has that does nothing for long enough to be asked about.
fn staying_up() -> (std::path::PathBuf, Vec<std::ffi::OsString>) {
    if cfg!(windows) {
        (
            std::path::PathBuf::from(r"C:\Windows\System32\cmd.exe"),
            vec!["/c".into(), "ping -n 60 127.0.0.1 >nul".into()],
        )
    } else {
        (
            std::path::PathBuf::from("/bin/sh"),
            vec!["-c".into(), "sleep 60".into()],
        )
    }
}
