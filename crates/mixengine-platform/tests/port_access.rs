//! Being allowed to answer on 80 and 443, against the real machine.
//!
//! **Nothing here writes**, which is the point of the capability being read-only: probing needs no
//! privilege on any of the three systems, and that is what makes the daemon able to do it on every
//! start. The write is the privileged operation, and the tests that drive it are `#[ignore]`d
//! further down and belong to CI's `system` job.

use std::path::Path;

use mixengine_platform::{Host as _, PortAccessMethod, mock};

/// D2's table, from the machine's own side. Each system has one mechanism and does not negotiate.
#[test]
fn this_machine_says_which_mechanism_it_uses_and_which_port_to_bind() {
    let host = mixengine_platform::host();

    // This test's own binary: it exists on all three systems and holds no capability. A path that
    // is not there is an `Io` error on Linux and not an answer, which is correct — a front end whose
    // program has gone is a problem, not a machine that needs a grant.
    let binary = std::env::current_exe().expect("a test binary has a path");

    let state = host
        .port_access()
        .probe(&binary, &[80, 443])
        .expect("probing reads and cannot fail on a supported system");

    assert_eq!(state.bindings.len(), 2);

    #[cfg(windows)]
    {
        assert_eq!(state.method, PortAccessMethod::Direct);
        assert!(state.granted, "Windows reserves no ports below 1024");
        assert!(state.bindings.iter().all(|one| one.answer == one.bind));
    }

    #[cfg(target_os = "linux")]
    {
        assert_eq!(state.method, PortAccessMethod::Capability);
        assert!(state.bindings.iter().all(|one| one.answer == one.bind));
    }

    #[cfg(target_os = "macos")]
    {
        assert_eq!(state.method, PortAccessMethod::Redirect);
        assert_eq!(state.bindings[0].bind, 8080);
        assert_eq!(state.bindings[1].bind, 8443);
    }
}

/// A front end that answers on nothing the OS reserves needs nothing granted, on every system —
/// which is what keeps the producer quiet on a home that is not using 80 at all.
#[test]
fn a_front_end_that_asks_for_no_reserved_port_needs_no_grant() {
    let host = mixengine_platform::host();

    let binary = std::env::current_exe().expect("a test binary has a path");

    let state = host
        .port_access()
        .probe(&binary, &[8080])
        .expect("probing reads");

    assert!(state.granted, "{:?}", state.missing);
    assert!(state.plan(&binary).is_none());
}

/// D1's payoff: the daemon derives the operation from the method, so nothing above this crate needs
/// a `#[cfg]` to know what to ask for.
#[test]
fn the_plan_is_derived_from_the_method_and_never_from_a_target_os() {
    use mixengine_proto::privileged::{PortAccessPlan, PortAccessTarget};

    let binary = Path::new("/home/someone/.mixengine/packages/caddy/caddy");

    let direct = mock::Host::with_port_access("/tmp/m", PortAccessMethod::Direct);
    let capability = mock::Host::without_port_access(
        "/tmp/m",
        PortAccessMethod::Capability,
        "the binary holds no capability",
    );
    let redirect =
        mock::Host::without_port_access("/tmp/m", PortAccessMethod::Redirect, "no anchor");

    let of = |host: &mock::Host| {
        host.port_access()
            .probe(binary, &[80, 443])
            .unwrap()
            .plan(binary)
    };

    assert!(of(&direct).is_none(), "Windows asks for nothing");

    assert!(matches!(
        of(&capability),
        Some(PortAccessPlan::Capability { ports, .. }) if ports == vec![80, 443]
    ));

    assert!(matches!(
        of(&redirect),
        Some(PortAccessPlan::Redirect { redirects })
            if redirects.iter().map(|one| one.bind).collect::<Vec<_>>() == vec![8080, 8443]
    ));

    assert!(matches!(
        redirect
            .port_access()
            .probe(binary, &[80])
            .unwrap()
            .target(binary),
        Some(PortAccessTarget::Redirect {})
    ));
}

/// The three fixtures the daemon's own tests are written against.
#[test]
fn a_mock_host_answers_from_memory() {
    let binary = Path::new("/x/caddy");

    let granted = mock::Host::with_port_access("/tmp/m", PortAccessMethod::Capability);
    assert!(granted.port_access().probe(binary, &[80]).unwrap().granted);

    let withheld =
        mock::Host::without_port_access("/tmp/m", PortAccessMethod::Capability, "no capability");
    let state = withheld.port_access().probe(binary, &[80]).unwrap();
    assert!(!state.granted);
    assert_eq!(state.missing.as_deref(), Some("no capability"));

    let refusing = mock::Host::unable_to_probe_port_access("/tmp/m", "no /proc on this machine");
    assert!(refusing.port_access().probe(binary, &[80]).is_err());
}

/// The default mock needs nothing, so every suite in this workspace that predates T42 keeps asking
/// for no prompt.
#[test]
fn a_plain_mock_host_needs_nothing_granted() {
    let host = mock::Host::with_home("/tmp/m");

    let state = host
        .port_access()
        .probe(Path::new("/x"), &[80, 443])
        .unwrap();

    assert_eq!(state.method, PortAccessMethod::Direct);
    assert!(state.granted);
}
