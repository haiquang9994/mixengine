//! Being allowed to answer on 80 and 443, against the real machine.
//!
//! **Nothing here writes**, which is the point of the capability being read-only: probing needs no
//! privilege on any of the three systems, and that is what makes the daemon able to do it on every
//! start. The write is the privileged operation, and the tests that drive it are `#[ignore]`d
//! further down and belong to CI's `system` job.

use std::path::Path;

use mixengine_platform::{Host as _, PortAccessMethod, PortBinding, mock};

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

// The write side. Everything below either needs no token at all — because the answer is a refusal —
// or is `#[ignore]`d and belongs to CI's `system` job.

/// D2, from the refusing side. Each system has one mechanism, and being handed the other one is an
/// `Unsupported` rather than a silent success — which is the difference between a machine that says
/// it cannot do something and one that says it did.
#[cfg(feature = "elevated")]
#[test]
fn the_plan_this_system_does_not_use_is_refused_by_name() {
    use mixengine_platform::port_access;
    use mixengine_proto::privileged::{PortAccessPlan, PortAccessTarget, PortRedirect};

    let capability = PortAccessPlan::Capability {
        binary: std::path::PathBuf::from("/nonexistent/caddy"),
        ports: vec![80],
    };
    let redirect = PortAccessPlan::Redirect {
        redirects: vec![PortRedirect {
            answer: 80,
            bind: 8080,
        }],
    };

    // Windows grants nothing at all: both directions of both plans are the same answer.
    #[cfg(windows)]
    for error in [
        port_access::apply(&capability).unwrap_err(),
        port_access::apply(&redirect).unwrap_err(),
        port_access::revoke(&PortAccessTarget::Redirect {}).unwrap_err(),
    ] {
        assert!(
            matches!(
                &error,
                mixengine_platform::Error::UnsupportedPlatform { capability, .. }
                    if *capability == "PortAccess"
            ),
            "{error}"
        );
    }

    #[cfg(target_os = "linux")]
    {
        let error = port_access::apply(&redirect).unwrap_err();
        assert!(
            error.to_string().contains("capability"),
            "the refusal says what this system does instead: {error}"
        );
        assert!(port_access::revoke(&PortAccessTarget::Redirect {}).is_err());
    }

    #[cfg(target_os = "macos")]
    {
        let error = port_access::apply(&capability).unwrap_err();
        assert!(
            error.to_string().contains("packet filter"),
            "the refusal says what this system does instead: {error}"
        );
        assert!(
            port_access::revoke(&PortAccessTarget::Capability {
                binary: std::path::PathBuf::from("/nonexistent/caddy")
            })
            .is_err()
        );
    }

    let _ = (&capability, &redirect);
}

/// Linux, against the kernel: the attribute is written, read back through the ordinary capability,
/// lost when the file is overwritten, and taken away again.
///
/// **The loss is the half that matters.** D11's whole argument for granting a capability on a
/// user-writable file is that the kernel clears it on any write, and D7's whole argument for probing
/// on every start is that the loss is then detectable — this is where both are asserted rather than
/// remembered.
#[cfg(all(target_os = "linux", feature = "elevated"))]
#[test]
#[ignore = "writes an extended attribute, which needs CAP_SETFCAP; run in CI's system job"]
fn a_capability_is_granted_read_back_lost_to_a_write_and_revoked() {
    use mixengine_platform::port_access::{self, Change};
    use mixengine_proto::privileged::{PortAccessPlan, PortAccessTarget};

    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join("front-end");
    std::fs::write(&binary, b"#!/bin/sh\nexit 0\n").unwrap();

    let host = mixengine_platform::host();
    let plan = PortAccessPlan::Capability {
        binary: binary.clone(),
        ports: vec![80, 443],
    };

    assert!(
        !host.port_access().probe(&binary, &[80]).unwrap().granted,
        "a file nobody has granted anything holds nothing"
    );

    let written = port_access::apply(&plan)
        .unwrap_or_else(|error| panic!("this needs an administrative token: {error}"));
    assert!(matches!(written, Change::Written { .. }), "{written:?}");

    assert!(
        host.port_access().probe(&binary, &[80]).unwrap().granted,
        "the ordinary capability reads it back, which is what makes probing on every start free"
    );

    // D1's payoff: the second call is a comparison, not a judgement.
    assert_eq!(port_access::apply(&plan).unwrap(), Change::Unchanged);

    // What an update does — measured, and the reason T88b is closed by this task.
    std::fs::write(&binary, b"#!/bin/sh\nexit 1\n").unwrap();

    let state = host.port_access().probe(&binary, &[80]).unwrap();
    assert!(!state.granted, "the kernel did not clear the capability");
    assert!(state.missing.is_some());

    port_access::apply(&plan).unwrap();
    let target = PortAccessTarget::Capability {
        binary: binary.clone(),
    };

    assert!(matches!(
        port_access::revoke(&target).unwrap(),
        Change::Written { .. }
    ));
    assert_eq!(port_access::revoke(&target).unwrap(), Change::Unchanged);
    assert!(!host.port_access().probe(&binary, &[80]).unwrap().granted);
}

/// macOS, against the packet filter: a redirect is installed, a server on 8080 is reached through
/// `http://127.0.0.1/`, and the machine's own `/etc/pf.conf` comes back byte for byte.
///
/// **The plist is checked as a file and not by rebooting**, which is the honest limit of what a
/// runner can prove — D3 and D9. `pfctl -e` is run here because the boot job is what would run it on
/// a real machine, and the test undoes whatever it changed about pf's enabled state.
#[cfg(all(target_os = "macos", feature = "elevated"))]
#[test]
#[ignore = "edits /etc/pf.conf and enables the packet filter; run in CI's system job"]
fn a_redirect_is_installed_reaches_a_server_on_8080_and_leaves_the_machine_as_it_was() {
    use std::io::{Read as _, Write as _};

    use mixengine_platform::port_access::{self, Change};
    use mixengine_proto::privileged::{PortAccessPlan, PortAccessTarget, PortRedirect};

    let conf = std::path::Path::new("/etc/pf.conf");
    let before = std::fs::read_to_string(conf).expect("macOS ships one");
    let was_enabled = pf_is_enabled();

    let plan = PortAccessPlan::Redirect {
        redirects: vec![PortRedirect {
            answer: 80,
            bind: 8080,
        }],
    };

    let written = port_access::apply(&plan)
        .unwrap_or_else(|error| panic!("this needs an administrative token: {error}"));
    assert!(matches!(written, Change::Written { .. }), "{written:?}");
    assert_eq!(port_access::apply(&plan).unwrap(), Change::Unchanged);

    assert!(std::path::Path::new("/Library/LaunchDaemons/dev.mixengine.pf.plist").exists());
    assert!(
        mixengine_platform::host()
            .port_access()
            .probe(std::path::Path::new("/unused"), &[80])
            .unwrap()
            .granted
    );

    // What the boot job does. It cannot be tested by rebooting a hosted runner, so this is the one
    // step the plist stands in for — see D3.
    let enabled = std::process::Command::new("/sbin/pfctl")
        .args(["-e", "-f", "/etc/pf.conf"])
        .output()
        .expect("pfctl is on every macOS machine");
    assert!(
        enabled.status.success(),
        "{}",
        String::from_utf8_lossy(&enabled.stderr)
    );

    let listener = std::net::TcpListener::bind("127.0.0.1:8080").expect("8080 is free on a runner");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the redirected connection");
        let mut request = [0u8; 64];
        let _ = stream.read(&mut request);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi")
            .expect("the reply");
    });

    let mut through =
        std::net::TcpStream::connect("127.0.0.1:80").expect("pf sent 80 to the server on 8080");
    through.write_all(b"GET / HTTP/1.0\r\n\r\n").unwrap();
    let mut answer = String::new();
    through.read_to_string(&mut answer).unwrap();
    server.join().expect("the server thread");

    assert!(answer.contains("200 OK"), "{answer}");

    assert!(matches!(
        port_access::revoke(&PortAccessTarget::Redirect {}).unwrap(),
        Change::Written { .. }
    ));

    assert_eq!(
        std::fs::read_to_string(conf).unwrap(),
        before,
        "the machine's own /etc/pf.conf did not come back"
    );
    assert!(!std::path::Path::new("/etc/pf.anchors/mixengine").exists());
    assert!(!std::path::Path::new("/Library/LaunchDaemons/dev.mixengine.pf.plist").exists());

    // Leave pf as this test found it. The operation deliberately does not — D3 — but a test that
    // changed a machine-wide switch and walked away would be one.
    let _ = std::process::Command::new("/sbin/pfctl")
        .args(["-f", "/etc/pf.conf"])
        .output();
    if !was_enabled {
        let _ = std::process::Command::new("/sbin/pfctl").arg("-d").output();
    }
}

/// `pfctl -s info` says `Status: Enabled` or `Status: Disabled`, and nothing else answers it: the
/// daemon cannot ask, because `/dev/pf` belongs to root — which is D9's whole point, and why this
/// helper is in a test rather than in the probe.
#[cfg(all(target_os = "macos", feature = "elevated"))]
fn pf_is_enabled() -> bool {
    std::process::Command::new("/sbin/pfctl")
        .arg("-s")
        .arg("info")
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains("Status: Enabled"))
        .unwrap_or(false)
}

/// The mapping alone, with nothing read and no binary named — which is what lets a `Generator`
/// built once for the life of the daemon carry it.
///
/// `probe` answers the same table, because it is the one that calls this. A second expression
/// would be a second answer to "what does this system make a front end bind".
#[test]
fn the_bind_mapping_is_a_pure_value_and_probe_agrees_with_it() {
    let host = mixengine_platform::host();
    let binary = std::env::current_exe().expect("a test binary has a path");

    let bindings = host.port_access().bindings(&[80, 443, 8080]);

    assert_eq!(bindings.len(), 3);
    assert_eq!(bindings[0].answer, 80);
    assert_eq!(bindings[1].answer, 443);
    assert_eq!(
        bindings[2],
        PortBinding {
            answer: 8080,
            bind: 8080
        },
        "a port the OS does not reserve is never mapped on any system"
    );

    #[cfg(target_os = "macos")]
    {
        assert_eq!(bindings[0].bind, 8080);
        assert_eq!(bindings[1].bind, 8443);
    }

    #[cfg(not(target_os = "macos"))]
    assert!(bindings.iter().all(|one| one.answer == one.bind));

    let probed = host
        .port_access()
        .probe(&binary, &[80, 443, 8080])
        .expect("probing reads");

    assert_eq!(probed.bindings, bindings);
}

/// The mock maps the way the system each method belongs to does, so a fixture cannot describe a
/// machine none of the three could be — and it answers this without being asked to probe.
#[test]
fn the_mock_maps_by_the_method_it_was_given() {
    let redirect = mock::Host::with_port_access("/tmp/m", PortAccessMethod::Redirect);
    let direct = mock::Host::with_port_access("/tmp/m", PortAccessMethod::Direct);

    let mapped = redirect.port_access().bindings(&[80, 443, 3000]);

    assert_eq!(mapped[0].bind, 8080);
    assert_eq!(mapped[1].bind, 8443);
    assert_eq!(mapped[2].bind, 3000);

    assert!(
        direct
            .port_access()
            .bindings(&[80, 443])
            .iter()
            .all(|one| one.answer == one.bind)
    );
}
