//! Combining a schedule-expanded job list with parsed log events into a
//! ledger: one entry per job, each either a set of matched run outcomes,
//! a `@reboot` job's found boots, or a timer flagged `UNSUPPORTED_TRIGGER`.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use jiff::Timestamp;
use serde::Serialize;

use crate::log::{EventSource, ExitStatus, LogEvent};

use super::job_key::JobKey;
use super::matcher::{apply_persistent_catchup, match_shared_runs, LogCoverage, RunOutcome};

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
    /// Notes from building the ledger. Includes a shared-command
    /// assignment warning when several jobs normalize to one key, and,
    /// when there were no log events at all, the note that every
    /// standard job is `NO_TRACE` rather than `MISSED`.
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

fn job_key_label(key: &JobKey) -> String {
    match key {
        JobKey::Cron { user, command } => format!("{user}: {command}"),
        JobKey::Timer { unit } => unit.clone(),
    }
}

/// Build the ledger: match every job's expected run times against the
/// events that identify it, using the logs' overall coverage (earliest to
/// latest log line, across every event regardless of which job it
/// belongs to) to tell `MISSED` apart from `NO_TRACE`.
///
/// Jobs that normalize to the same [`JobKey`] (the same cron command
/// scheduled more than once, say from two files) are matched together
/// against one shared pool of observed runs, via [`match_shared_runs`],
/// so a single real execution cannot be counted as a hit for more than
/// one of them. That shared match is still the same greedy
/// nearest-match heuristic as a single job's, not an optimal
/// assignment, so which schedule line claims a given run among several
/// sharing a key is approximate; a warning notes when this happens.
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

    // Group standard-schedule jobs by key, so jobs sharing a key draw
    // from one shared pool of observed runs instead of each
    // independently matching against the same log lines.
    let mut groups: HashMap<&JobKey, Vec<usize>> = HashMap::new();
    for (idx, job) in jobs.iter().enumerate() {
        if matches!(job.schedule, JobSchedule::Standard(_)) {
            groups.entry(&job.key).or_default().push(idx);
        }
    }

    let mut ordered: Vec<(&JobKey, &Vec<usize>)> = groups
        .iter()
        .map(|(key, indices)| (*key, indices))
        .collect();
    // HashMap order is not stable. Warnings follow the earliest job
    // that uses the key, which is the order the jobs were given in.
    ordered.sort_by_key(|(_, indices)| indices.first().copied().unwrap_or(usize::MAX));

    let mut standard_outcomes: HashMap<usize, Vec<RunOutcome>> = HashMap::new();
    let mut warnings = Vec::new();
    for (key, indices) in ordered {
        let mut runs = match key {
            JobKey::Cron { .. } => cron_events
                .get(key)
                .map(|evs| pair_cmd_with_end(evs))
                .unwrap_or_default(),
            JobKey::Timer { unit } => timer_starts
                .get(unit)
                .map(|starts| starts.iter().map(|&ts| (ts, ExitStatus::Unknown)).collect())
                .unwrap_or_default(),
        };
        runs.sort_by_key(|(ts, _)| *ts);

        // Tag each job's own expected times with where they came from,
        // so the merged match can be split back out per job afterwards,
        // in that job's own order.
        let mut tagged: Vec<(Timestamp, Duration, usize, usize)> = Vec::new();
        for &job_idx in indices {
            let JobSchedule::Standard(expected) = &jobs[job_idx].schedule else {
                unreachable!("grouped only jobs with a Standard schedule")
            };
            for (pos, &exp) in expected.iter().enumerate() {
                tagged.push((exp, jobs[job_idx].tolerance, job_idx, pos));
            }
        }
        tagged.sort_by_key(|&(ts, _, job_idx, pos)| (ts, job_idx, pos));

        let merged: Vec<(Timestamp, Duration)> =
            tagged.iter().map(|&(ts, tol, ..)| (ts, tol)).collect();
        let merged_outcomes = match_shared_runs(&merged, &runs, &log_coverage);

        let mut per_job: HashMap<usize, Vec<(usize, RunOutcome)>> = HashMap::new();
        for (&(_, _, job_idx, pos), outcome) in tagged.iter().zip(merged_outcomes) {
            per_job.entry(job_idx).or_default().push((pos, outcome));
        }
        for (job_idx, mut positioned) in per_job {
            positioned.sort_by_key(|&(pos, _)| pos);
            standard_outcomes.insert(job_idx, positioned.into_iter().map(|(_, o)| o).collect());
        }

        if indices.len() > 1 {
            let what = match key {
                JobKey::Cron { .. } => "command",
                JobKey::Timer { .. } => "unit",
            };
            warnings.push(format!(
                "{}: {} scheduled jobs share this {what}; one observed run is given to at \
                 most one of them by the same greedy matcher used for a single job, so \
                 which schedule line claims a run can be approximate",
                job_key_label(key),
                indices.len()
            ));
        }
    }

    let entries = jobs
        .iter()
        .enumerate()
        .map(|(idx, job)| {
            let outcome = match &job.schedule {
                JobSchedule::Reboot => JobOutcome::Reboot {
                    boots: boots.clone(),
                },
                JobSchedule::UnsupportedTrigger => JobOutcome::UnsupportedTrigger,
                JobSchedule::Standard(_) => {
                    let mut outcomes = standard_outcomes.remove(&idx).unwrap_or_default();
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

    if log_coverage.is_empty() {
        warnings.push(
            "no log events found in the window: every expected run is reported as NO_TRACE, \
             never MISSED"
                .to_string(),
        );
    }

    Ledger {
        entries,
        log_coverage,
        warnings,
    }
}
