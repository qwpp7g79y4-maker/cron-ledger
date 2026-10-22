//! Combining a schedule-expanded job list with parsed log events into a
//! ledger: one entry per job, each either a set of matched run outcomes,
//! a `@reboot` job's found boots, or a timer flagged `UNSUPPORTED_TRIGGER`.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use jiff::Timestamp;
use serde::Serialize;

use crate::log::{EventSource, ExitStatus, LogEvent};

use super::job_key::JobKey;
use super::matcher::{apply_persistent_catchup, match_runs, LogCoverage, RunOutcome};

/// What a job's schedule resolved to, before matching against logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobSchedule {
    /// Expected run times in `[since, until)`, sorted and deduplicated.
    Standard(Vec<Timestamp>),
    /// `@reboot`: no expected times to match against.
    Reboot,
    /// A systemd timer with only monotonic triggers this project cannot
    /// compute expected times for.
    UnsupportedTrigger,
}

/// One job ready to be matched: its identity, where it came from, and its
/// resolved schedule and tolerance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledJob {
    pub key: JobKey,
    pub file: String,
    pub line: usize,
    pub schedule: JobSchedule,
    pub tolerance: Duration,
    /// A systemd timer's `Persistent=`; always `false` for cron jobs,
    /// which have no such concept. Drives `LATE (caught up)` via
    /// [`apply_persistent_catchup`].
    pub persistent: bool,
}

/// What a job's ledger entry ended up as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum JobOutcome {
    Standard(Vec<RunOutcome>),
    /// All boots found in the given events, for info only; not matched
    /// against any schedule or tolerance.
    Reboot {
        boots: Vec<Timestamp>,
    },
    UnsupportedTrigger,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobLedgerEntry {
    pub key: JobKey,
    pub file: String,
    pub line: usize,
    pub outcome: JobOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Ledger {
    pub entries: Vec<JobLedgerEntry>,
    pub log_coverage: LogCoverage,
    /// Non-empty only when there were no log events at all: every
    /// standard job's expected times all become `NO_TRACE`, never
    /// `MISSED`.
    pub warnings: Vec<String>,
}

/// Pair each `CronCmd` with the nearest following `CronCmdEnd` for the
/// same job (by pid if both have one, else first-in-first-out), to
/// recover what exit-status evidence (if any) exists for each run.
/// `events` must already be filtered to a single job's key.
fn pair_cmd_with_end(events: &[&LogEvent]) -> Vec<(Timestamp, ExitStatus)> {
    let mut records: Vec<(Timestamp, ExitStatus)> = Vec::new();
    let mut pending_by_pid: HashMap<u32, usize> = HashMap::new();
    let mut pending_fifo: VecDeque<usize> = VecDeque::new();

    for event in events {
        match &event.source {
            EventSource::CronCmd { pid, .. } => {
                let idx = records.len();
                records.push((event.timestamp, ExitStatus::Unknown));
                match pid {
                    Some(p) => {
                        pending_by_pid.insert(*p, idx);
                    }
                    None => pending_fifo.push_back(idx),
                }
            }
            EventSource::CronCmdEnd { pid, .. } => {
                let idx = match pid {
                    Some(p) => pending_by_pid.remove(p),
                    None => pending_fifo.pop_front(),
                };
                if let Some(idx) = idx {
                    records[idx].1 = ExitStatus::Completed;
                }
            }
            _ => {}
        }
    }

    records
}

/// Build the ledger: match every job's expected run times against the
/// events that identify it, using the logs' overall coverage (earliest to
/// latest log line, across every event regardless of which job it
/// belongs to) to tell `MISSED` apart from `NO_TRACE`.
pub fn build_ledger(jobs: &[ScheduledJob], events: &[LogEvent]) -> Ledger {
    let log_coverage = LogCoverage::from_timestamps(events.iter().map(|e| e.timestamp));

    let mut cron_events: HashMap<JobKey, Vec<&LogEvent>> = HashMap::new();
    let mut timer_starts: HashMap<String, Vec<Timestamp>> = HashMap::new();
    let mut boots: Vec<Timestamp> = Vec::new();

    for event in events {
        match &event.source {
            EventSource::CronCmd { user, command, .. }
            | EventSource::CronCmdEnd { user, command, .. } => {
                let key = JobKey::Cron {
                    user: user.clone(),
                    command: super::job_key::normalize_command(command),
                };
                cron_events.entry(key).or_default().push(event);
            }
            EventSource::CronCmdOut { .. } => {}
            EventSource::SystemdUnitStart { unit } => {
                timer_starts
                    .entry(unit.clone())
                    .or_default()
                    .push(event.timestamp);
            }
            EventSource::Boot { .. } => boots.push(event.timestamp),
        }
    }
    boots.sort();

    let entries = jobs
        .iter()
        .map(|job| {
            let outcome = match &job.schedule {
                JobSchedule::Reboot => JobOutcome::Reboot {
                    boots: boots.clone(),
                },
                JobSchedule::UnsupportedTrigger => JobOutcome::UnsupportedTrigger,
                JobSchedule::Standard(expected) => {
                    let runs = match &job.key {
                        JobKey::Cron { .. } => cron_events
                            .get(&job.key)
                            .map(|evs| pair_cmd_with_end(evs))
                            .unwrap_or_default(),
                        JobKey::Timer { unit } => timer_starts
                            .get(unit)
                            .map(|starts| {
                                starts.iter().map(|&ts| (ts, ExitStatus::Unknown)).collect()
                            })
                            .unwrap_or_default(),
                    };
                    let mut runs = runs;
                    runs.sort_by_key(|(ts, _)| *ts);
                    let mut outcomes = match_runs(expected, &runs, job.tolerance, &log_coverage);
                    apply_persistent_catchup(&mut outcomes, &boots, job.persistent);
                    JobOutcome::Standard(outcomes)
                }
            };
            JobLedgerEntry {
                key: job.key.clone(),
                file: job.file.clone(),
                line: job.line,
                outcome,
            }
        })
        .collect();

    let warnings = if log_coverage.is_empty() {
        vec![
            "no log events found in the window: every expected run is reported as NO_TRACE, \
             never MISSED"
                .to_string(),
        ]
    } else {
        Vec::new()
    };

    Ledger {
        entries,
        log_coverage,
        warnings,
    }
}
