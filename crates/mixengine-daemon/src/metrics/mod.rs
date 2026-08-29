//! Measuring what is running, and keeping a day of it — roadmap task **T71**.
//!
//! Three parts, and the split is by what each one needs to know. [`minutes`] is arithmetic over a
//! series of readings and knows nothing about clocks, stores or clients. `watchers` is how many
//! clients have `GET /metrics` open, which is the whole of what decides the rate. `sampler` is the
//! loop that puts the two together and is the only part that touches the machine or the database.
//!
//! **The reading itself is not here.** What a process group costs is
//! [`ProcessMetrics`](mixengine_platform::ProcessMetrics), in the platform crate with the rest of
//! what this machine is asked, so that every question above can be answered from invented numbers.
//!
//! Design: `docs/superpowers/specs/2026-08-30-t71-metrics-history-design.md`.

pub(crate) mod minutes;
pub(crate) mod sampler;
pub(crate) mod watchers;
