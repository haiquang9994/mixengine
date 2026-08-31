//! Windows' half of T74: one inbound rule, named, on the private profile.

use std::ffi::OsStr;

use mixengine_proto::privileged::FirewallPlan;

use crate::firewall::{Applied, netsh};

/// Delete whatever is there under this label, then add the plan's ports if it names any.
///
/// **Delete-then-add and not a diff**, which is what makes a whole-state plan idempotent on a tool
/// whose `add rule` appends: the rule set after two identical plans is the rule set after one.
///
/// The cost is that this cannot answer [`Applied::Unchanged`] by comparison the way the resolver
/// does — `netsh` has no reliable machine-readable read of one rule, and parsing its localised
/// human output to save a write nobody can measure would be the more fragile of the two. A second
/// identical share therefore reports what it wrote, and the machine ends up in the same state
/// either way.
pub(crate) fn apply(plan: &FirewallPlan) -> crate::Result<Applied> {
    // Deleting a name that is not there exits non-zero and says so; that is the ordinary first
    // call, not a failure, so its result is deliberately dropped.
    let removal = netsh::delete(&plan.label);
    drop(run(&removal));

    if plan.ports.is_empty() {
        return Ok(Applied::Written {
            detail: format!("removed the firewall rule named \"{}\"", plan.label),
        });
    }

    run(&netsh::add(&plan.label, &plan.ports))?;

    let ports = plan
        .ports
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(", ");

    Ok(Applied::Written {
        detail: format!(
            "allowed inbound TCP {ports} on the private profile, as \"{}\"",
            plan.label
        ),
    })
}

/// Run `netsh` with these arguments.
fn run(args: &[String]) -> crate::Result<String> {
    super::command::run("netsh", None, args.iter().map(OsStr::new))
}
