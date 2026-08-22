//! A hosts file that exists only in memory.

use std::path::PathBuf;

use mixengine_proto::privileged::HostEntry;

use crate::{Error, Result};

/// The block a test says the machine is holding.
#[derive(Debug)]
pub(crate) struct Hosts {
    /// The block itself, already parsed — or why it cannot be read.
    managed: std::result::Result<Vec<HostEntry>, String>,
}

impl Default for Hosts {
    /// A machine with an empty block, which is what a fresh mock should say.
    fn default() -> Self {
        Self {
            managed: Ok(Vec::new()),
        }
    }
}

impl Hosts {
    /// A machine whose block holds `lines`, each written the way a hosts file writes one:
    /// `"127.0.0.1 blog.test"`.
    ///
    /// Parsed through [`crate::hosts::parse`] rather than taken as structs, so a fixture cannot
    /// describe a block the engine could not have produced.
    pub(crate) fn holding<'a>(lines: impl IntoIterator<Item = &'a str>) -> Self {
        let mut text = format!("{}\n", crate::hosts::BEGIN_MARKER);

        for line in lines {
            text.push_str(line);
            text.push('\n');
        }

        text.push_str(crate::hosts::END_MARKER);
        text.push('\n');

        Self {
            managed: crate::hosts::parse(&text).map_err(|error| error.to_string()),
        }
    }

    /// A machine whose hosts file cannot be read, with `reason`.
    pub(crate) fn refusing(reason: &str) -> Self {
        Self {
            managed: Err(reason.to_owned()),
        }
    }
}

impl crate::HostsFile for Hosts {
    fn path(&self) -> PathBuf {
        PathBuf::from("/mock/hosts")
    }

    fn managed(&self) -> Result<Vec<HostEntry>> {
        self.managed
            .clone()
            .map_err(|reason| Error::MalformedBlock { reason })
    }
}
