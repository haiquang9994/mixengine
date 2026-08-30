//! Linux' half of T74: `ufw` or `firewalld` where one is running, and an honest shrug otherwise.

use mixengine_proto::privileged::FirewallPlan;

use crate::firewall::unix_tools::Tool;
use crate::firewall::{Applied, unix_tools};

/// Open the plan's ports through whichever firewall is running.
///
/// **Detected rather than configured.** A machine with neither running is not misconfigured and is
/// not asked to install one: nothing is blocking the port there, and
/// [`Applied::Unmanaged`] says so with the command to run if something turns out to be.
pub(crate) fn apply(plan: &FirewallPlan) -> crate::Result<Applied> {
    let Some(tool) = running() else {
        let (reason, manual) = unix_tools::unmanaged(&plan.ports);

        return Ok(Applied::Unmanaged { reason, manual });
    };

    // Whole state on a tool with no whole-state verb: every port MixEngine could have opened is
    // closed first, then the plan's are opened. The closing list is the plan's own ports plus
    // nothing else — this build never opened a port that was not in some plan, and a port a *user*
    // opened by hand is not ours to close, which is why nothing here enumerates the rule set.
    for &port in &plan.ports {
        run(tool, &arguments(tool, port, false))?;
    }

    for &port in &plan.ports {
        run(tool, &arguments(tool, port, true))?;
    }

    if plan.ports.is_empty() {
        return Ok(Applied::Written {
            detail: "closed the ports MixEngine had opened".to_owned(),
        });
    }

    let ports = plan
        .ports
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(", ");

    Ok(Applied::Written {
        detail: format!("allowed inbound TCP {ports} through {}", name(tool)),
    })
}

/// Which firewall is running here, if either.
///
/// `ufw status` says `Status: active`, and `firewall-cmd --state` says `running`. Both are asked
/// rather than assumed from a binary being installed: a `ufw` that is installed and inactive
/// filters nothing, and adding rules to it would be writing into a firewall nobody is enforcing.
fn running() -> Option<Tool> {
    if says("ufw", &["status".to_owned()], "active") {
        return Some(Tool::Ufw);
    }

    if says("firewall-cmd", &["--state".to_owned()], "running") {
        return Some(Tool::Firewalld);
    }

    None
}

/// Whether running `command` prints something containing `wanted`.
fn says(command: &'static str, args: &[String], wanted: &str) -> bool {
    std::process::Command::new(command)
        .args(args)
        .output()
        .is_ok_and(|output| {
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains(wanted)
        })
}

/// The arguments for one port under one tool.
fn arguments(tool: Tool, port: u16, allow: bool) -> Vec<String> {
    match tool {
        Tool::Ufw => unix_tools::ufw(port, allow),
        Tool::Firewalld => unix_tools::firewalld(port, allow),
    }
}

/// The program each tool is driven through.
fn name(tool: Tool) -> &'static str {
    match tool {
        Tool::Ufw => "ufw",
        Tool::Firewalld => "firewall-cmd",
    }
}

/// Run one of the two tools, failing loudly.
fn run(tool: Tool, args: &[String]) -> crate::Result<()> {
    let command = name(tool);

    let output = std::process::Command::new(command)
        .args(args)
        .output()
        .map_err(|source| crate::Error::Os {
            action: "run this machine's firewall tool",
            source,
        })?;

    // A close of a port that was never open is the ordinary first call of a whole-state apply, and
    // both tools exit non-zero for it. Only an *open* that failed is a failure worth reporting.
    let opening = args
        .iter()
        .any(|arg| arg.contains("allow") || arg.contains("--add-port"));

    if !output.status.success() && opening {
        return Err(crate::Error::Command {
            command: "firewall",
            path: None,
            status: output.status.to_string(),
            output: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    Ok(())
}
