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
