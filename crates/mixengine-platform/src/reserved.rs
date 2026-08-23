//! `netsh`'s excluded port ranges, as text — roadmap task **T47a**.
//!
//! **Pure and compiled on all three systems**, for [`crate::resolver::directory`]'s reason: the half
//! of a per-OS mechanism that is a decision about text is tested everywhere, and only the call that
//! can be made nowhere else stays behind a `#[cfg]`.

use crate::PortRange;

/// Every range in `netsh int ipv4 show excludedportrange` output.
///
/// **Two integers on a line and nothing else.** The header, the rule of dashes under it and the
/// footnote about administered exclusions are all skipped by that rule rather than by matching their
/// wording — which is what keeps a localised Windows from reading as a machine with no reservations
/// at all. Every word of that output is translated; the numbers are not.
#[allow(
    dead_code,
    reason = "called by Windows' reader only; compiled on all three so its tests run on all three"
)]
pub(crate) fn parse(output: &str) -> Vec<PortRange> {
    output
        .lines()
        .filter_map(|line| {
            let mut numbers = line
                .split_whitespace()
                .map_while(|word| word.parse::<u16>().ok());

            let start = numbers.next()?;
            let end = numbers.next()?;

            (start <= end).then_some(PortRange { start, end })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from `netsh int ipv4 show excludedportrange protocol=tcp` on a machine running
    /// Hyper-V. The asterisk marks an administered exclusion and is part of the output.
    const SAMPLE: &str = "\r\n\
Protocol tcp Port Exclusion Ranges\r\n\
\r\n\
Start Port    End Port\r\n\
----------    --------\r\n\
      1024        1123\r\n\
      1124        1223\r\n\
     50000       50059     *\r\n\
\r\n\
* - Administered port exclusions.\r\n";

    #[test]
    fn every_range_is_read_and_nothing_else_is() {
        let ranges = parse(SAMPLE);

        assert_eq!(
            ranges,
            vec![
                PortRange {
                    start: 1024,
                    end: 1123
                },
                PortRange {
                    start: 1124,
                    end: 1223
                },
                PortRange {
                    start: 50_000,
                    end: 50_059
                },
            ]
        );
    }

    /// A machine with nothing reserved prints the header and no rows, which is not a failure.
    #[test]
    fn a_machine_that_reserves_nothing_reads_as_nothing() {
        let empty = "\r\nProtocol tcp Port Exclusion Ranges\r\n\r\n\
                     Start Port    End Port\r\n----------    --------\r\n\r\n";

        assert!(parse(empty).is_empty());
    }

    /// The rule of dashes under the header is two words to anything that only looks for digits, and
    /// this is the line that would turn it into a range covering everything.
    #[test]
    fn the_rule_under_the_header_is_not_a_range() {
        assert!(parse("----------    --------\r\n").is_empty());
    }

    /// A line with one number is a line with no range on it.
    #[test]
    fn one_number_is_not_a_range() {
        assert!(parse("      1024\r\n").is_empty());
    }

    #[test]
    fn a_range_knows_which_ports_it_holds() {
        let range = PortRange {
            start: 1024,
            end: 1123,
        };

        assert!(range.holds(1024));
        assert!(range.holds(1123));
        assert!(!range.holds(1023));
        assert!(!range.holds(1124));
    }
}
