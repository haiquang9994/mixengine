//! Which inbound rules name a program, out of `netsh`' own listing — roadmap task **T76**.
//!
//! The one system of the three that has such a table, and the one where the question matters:
//! binding UDP 5353 makes Windows offer to write an every-port rule for `mixengined.exe`, and a
//! daemon cannot learn from its own database about a rule it never made.
//!
//! **Unprivileged.** `show rule` reads; it needs none of the token `crate::firewall::apply` does,
//! which is why this sits behind the `host` feature and that one behind `elevated`.

use std::ffi::OsString;
use std::path::Path;

use crate::{FirewallRules, Result};

/// This machine's inbound rules.
#[derive(Debug, Default)]
pub(crate) struct Rules;

impl FirewallRules for Rules {
    fn naming(&self, program: &Path) -> Result<Option<usize>> {
        let args: Vec<OsString> = crate::firewall::netsh::show()
            .into_iter()
            .map(Into::into)
            .collect();

        // **A non-zero exit is not a failure here**, which is why this is `output_of` and not
        // `run`: `netsh` answers "No rules match the specified criteria" and exits non-zero on a
        // machine with nothing to show — the reading `netsh::delete` already relies on — and for
        // this question that is simply zero.
        let listing = super::command::output_of("netsh", args.iter().map(AsRef::as_ref))?;

        Ok(Some(crate::firewall::netsh::counted(
            &listing,
            &program.display().to_string(),
        )))
    }
}
