//! Rendering a [`Ledger`] as the text and `--json` reports described in
//! the README, and deciding the process exit code from it.
//!
//! Exit codes follow the documented CLI contract:
//! **0** everything OK, **1** at least one MISSED or LATE, **3** only
//! NO_TRACE and otherwise OK. A `--fail-on` flag was left as a documented
//! future option rather than built now, since the brief marks its
//! shape as undecided.

mod json;
mod text;

pub use json::render_json;
pub use text::render_text;

use std::collections::HashMap;
use std::time::Duration;

use jiff::tz::TimeZone;
use jiff::Timestamp;

use crate::ledger::{JobKey, JobOutcome, Ledger, Status};

pub const SCHEMA_VERSION: u32 = 1;

/// Everything the report renderers need beyond the [`Ledger`] itself:
/// the audit window, and each job's human-readable schedule description
/// (kept out of the `ledger` module's own types so the ledger tests
/// and golden files don't need to change shape for a display concern).
pub struct ReportContext<'a> {
    pub since: Timestamp,
    pub until: Timestamp,
    pub tz: &'a TimeZone,
    pub tolerance: Duration,
    pub descriptions: &'a HashMap<JobKey, String>,
    /// Parse/log warnings collected during input reading, in addition to
    /// `ledger.warnings`.
    pub extra_warnings: &'a [String],
}

/// Decide the process exit code from the ledger's standard-job outcomes.
/// `REBOOT_ONLY` and `UNSUPPORTED_TRIGGER` are informational only and
/// never affect it.
pub fn exit_code(ledger: &Ledger) -> i32 {
    let mut any_missed_or_late = false;
    let mut any_no_trace = false;
    for entry in &ledger.entries {
        if let JobOutcome::Standard(outcomes) = &entry.outcome {
            for o in outcomes {
                match o.status {
                    Status::Missed | Status::Late | Status::LateCaughtUp => {
                        any_missed_or_late = true
                    }
                    Status::NoTrace => any_no_trace = true,
                    Status::Ok | Status::RebootOnly | Status::UnsupportedTrigger => {}
                }
            }
        }
    }
    if any_missed_or_late {
        1
    } else if any_no_trace {
        3
    } else {
        0
    }
}

/// Whether the text report should use ANSI color: a tty, and `NO_COLOR`
/// unset. A pure function so it is unit-testable without a real terminal.
pub fn use_color(no_color_set: bool, is_tty: bool) -> bool {
    is_tty && !no_color_set
}

/// How many of a job's earliest `MISSED` times the report shows
/// (the first 5 missed times).
const MAX_MISSED_SHOWN: usize = 5;

/// A job's outcome, reduced to the fields every renderer needs: counts,
/// the most recent observed run, and the earliest `MISSED` times.
pub(super) struct JobView<'a> {
    pub key: &'a JobKey,
    pub file: &'a str,
    pub line: usize,
    pub schedule: String,
    pub expected: usize,
    pub found: usize,
    pub missed: usize,
    pub late: usize,
    pub last_run: Option<(Timestamp, Status)>,
    pub first_missed: Vec<Timestamp>,
    pub outcome: &'a JobOutcome,
}

pub(super) fn job_views<'a>(
    ledger: &'a Ledger,
    descriptions: &'a HashMap<JobKey, String>,
) -> Vec<JobView<'a>> {
    ledger
        .entries
        .iter()
        .map(|entry| {
            let schedule = descriptions
                .get(&entry.key)
                .cloned()
                .unwrap_or_else(|| "?".to_string());
            let (expected, found, missed, late, last_run, first_missed) = match &entry.outcome {
                JobOutcome::Standard(outcomes) => {
                    let expected = outcomes.len();
                    let found = outcomes.iter().filter(|o| o.actual.is_some()).count();
                    let missed = outcomes
                        .iter()
                        .filter(|o| o.status == Status::Missed)
                        .count();
                    let late = outcomes
                        .iter()
                        .filter(|o| matches!(o.status, Status::Late | Status::LateCaughtUp))
                        .count();
                    let last_run = outcomes
                        .iter()
                        .filter_map(|o| o.actual.map(|a| (a, o.status)))
                        .max_by_key(|(a, _)| *a);
                    let first_missed = outcomes
                        .iter()
                        .filter(|o| o.status == Status::Missed)
                        .take(MAX_MISSED_SHOWN)
                        .map(|o| o.expected)
                        .collect();
                    (expected, found, missed, late, last_run, first_missed)
                }
                JobOutcome::Reboot { .. } | JobOutcome::UnsupportedTrigger => {
                    (0, 0, 0, 0, None, Vec::new())
                }
            };
            JobView {
                key: &entry.key,
                file: &entry.file,
                line: entry.line,
                schedule,
                expected,
                found,
                missed,
                late,
                last_run,
                first_missed,
                outcome: &entry.outcome,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{JobLedgerEntry, RunOutcome};
    use crate::log::ExitStatus;

    fn entry(outcome: JobOutcome) -> JobLedgerEntry {
        JobLedgerEntry {
            key: JobKey::Cron {
                user: "root".to_string(),
                command: "x".to_string(),
            },
            file: "f".to_string(),
            line: 1,
            outcome,
        }
    }

    fn run(status: Status) -> RunOutcome {
        RunOutcome {
            expected: "2026-10-20T00:00:00Z".parse().unwrap(),
            status,
            actual: None,
            late_by: None,
            exit_status: ExitStatus::Unknown,
        }
    }

    #[test]
    fn all_ok_is_exit_zero() {
        let ledger = Ledger {
            entries: vec![entry(JobOutcome::Standard(vec![run(Status::Ok)]))],
            ..Ledger::default()
        };
        assert_eq!(exit_code(&ledger), 0);
    }

    #[test]
    fn missed_or_late_is_exit_one_even_with_no_trace_present() {
        let ledger = Ledger {
            entries: vec![entry(JobOutcome::Standard(vec![
                run(Status::NoTrace),
                run(Status::Missed),
            ]))],
            ..Ledger::default()
        };
        assert_eq!(exit_code(&ledger), 1);
    }

    #[test]
    fn only_no_trace_is_exit_three() {
        let ledger = Ledger {
            entries: vec![entry(JobOutcome::Standard(vec![run(Status::NoTrace)]))],
            ..Ledger::default()
        };
        assert_eq!(exit_code(&ledger), 3);
    }

    #[test]
    fn reboot_only_and_unsupported_trigger_never_affect_exit_code() {
        let ledger = Ledger {
            entries: vec![
                entry(JobOutcome::Reboot { boots: vec![] }),
                entry(JobOutcome::UnsupportedTrigger),
            ],
            ..Ledger::default()
        };
        assert_eq!(exit_code(&ledger), 0);
    }

    #[test]
    fn color_requires_both_a_tty_and_no_color_unset() {
        assert!(use_color(false, true));
        assert!(!use_color(true, true));
        assert!(!use_color(false, false));
        assert!(!use_color(true, false));
    }
}
