//! Matching a job's expected run times against its observed run instants.
//!
//! Matching happens in two passes, both greedy and each observed instant
//! used at most once:
//!
//! 1. Every expected time is paired with its nearest *unused* observed
//!    instant that falls within tolerance (`OK`).
//! 2. Every expected time still unpaired is paired with its nearest
//!    *unused* observed instant that falls strictly after it and before
//!    the next expected time (`LATE`) — bounding the search this way
//!    stops a late run from being stolen by whichever expected time
//!    happens to ask for it first, and stops a run far enough away that
//!    it's really evidence for the *next* expected time from being
//!    claimed by this one.
//!
//! What's left over is `MISSED` (inside the log's coverage window) or
//! `NO_TRACE` (outside it).

use crate::log::ExitStatus;
use jiff::Timestamp;
use serde::Serialize;
use std::time::Duration;

use super::status::Status;

/// The timespan the logs actually cover: from the first log line to the
/// last, regardless of which job (or no job at all) it was evidence for.
/// Empty if there were no log lines at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct LogCoverage {
    pub first: Option<Timestamp>,
    pub last: Option<Timestamp>,
}

impl LogCoverage {
    pub fn from_timestamps(timestamps: impl IntoIterator<Item = Timestamp>) -> Self {
        let mut first = None;
        let mut last = None;
        for ts in timestamps {
            first = Some(first.map_or(ts, |f: Timestamp| f.min(ts)));
            last = Some(last.map_or(ts, |l: Timestamp| l.max(ts)));
        }
        LogCoverage { first, last }
    }

    pub fn is_empty(&self) -> bool {
        self.first.is_none()
    }

    /// Whether `t` falls inside `[first, last]`. Always `false` when the
    /// logs are empty, by construction — never `true` on an empty
    /// coverage window.
    pub fn covers(&self, t: Timestamp) -> bool {
        match (self.first, self.last) {
            (Some(f), Some(l)) => t >= f && t <= l,
            _ => false,
        }
    }
}

/// The outcome of matching one expected run time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RunOutcome {
    pub expected: Timestamp,
    pub status: Status,
    pub actual: Option<Timestamp>,
    pub late_by: Option<Duration>,
    pub exit_status: ExitStatus,
}

fn abs_diff_secs(a: Timestamp, b: Timestamp) -> i64 {
    (a.as_second() - b.as_second()).abs()
}

/// Match `expected` (sorted, strictly increasing) against `runs` (sorted
/// by timestamp; this job's own observed run instants with whatever exit
/// status evidence was found for each), within `tolerance`, reporting
/// `MISSED`/`NO_TRACE` for the rest according to `coverage`.
pub fn match_runs(
    expected: &[Timestamp],
    runs: &[(Timestamp, ExitStatus)],
    tolerance: Duration,
    coverage: &LogCoverage,
) -> Vec<RunOutcome> {
    let tolerance_secs = tolerance.as_secs() as i64;
    let mut used = vec![false; runs.len()];
    let mut matched: Vec<Option<usize>> = vec![None; expected.len()];

    // Pass 1: strict tolerance, nearest unused run.
    for (i, &exp) in expected.iter().enumerate() {
        let mut best: Option<(usize, i64)> = None;
        for (j, &(ts, _)) in runs.iter().enumerate() {
            if used[j] {
                continue;
            }
            let diff = abs_diff_secs(exp, ts);
            if diff <= tolerance_secs && best.is_none_or(|(_, bd)| diff < bd) {
                best = Some((j, diff));
            }
        }
        if let Some((j, _)) = best {
            used[j] = true;
            matched[i] = Some(j);
        }
    }

    // Pass 2: remaining unmatched expected times look for the nearest
    // later, still-unused run before the next expected time.
    for i in 0..expected.len() {
        if matched[i].is_some() {
            continue;
        }
        let exp = expected[i];
        let upper_bound = expected.get(i + 1).copied();
        let mut best: Option<(usize, i64)> = None;
        for (j, &(ts, _)) in runs.iter().enumerate() {
            if used[j] || ts <= exp {
                continue;
            }
            if upper_bound.is_some_and(|next| ts >= next) {
                continue;
            }
            let diff = abs_diff_secs(exp, ts);
            if best.is_none_or(|(_, bd)| diff < bd) {
                best = Some((j, diff));
            }
        }
        if let Some((j, _)) = best {
            used[j] = true;
            matched[i] = Some(j);
        }
    }

    expected
        .iter()
        .enumerate()
        .map(|(i, &exp)| match matched[i] {
            Some(j) => {
                let (ts, exit_status) = runs[j];
                let diff = abs_diff_secs(exp, ts);
                if diff <= tolerance_secs {
                    RunOutcome {
                        expected: exp,
                        status: Status::Ok,
                        actual: Some(ts),
                        late_by: None,
                        exit_status,
                    }
                } else {
                    RunOutcome {
                        expected: exp,
                        status: Status::Late,
                        actual: Some(ts),
                        late_by: Some(Duration::from_secs(diff.unsigned_abs())),
                        exit_status,
                    }
                }
            }
            None => RunOutcome {
                expected: exp,
                status: if coverage.covers(exp) {
                    Status::Missed
                } else {
                    Status::NoTrace
                },
                actual: None,
                late_by: None,
                exit_status: ExitStatus::Unknown,
            },
        })
        .collect()
}

/// Upgrade outcomes caused by a systemd `Persistent=` boot catch-up from
/// generic `MISSED`/`LATE` to `LateCaughtUp`. Per `systemd.timer(5)`, at
/// most one missed run is triggered "immediately" at the next boot: the
/// chronologically last expected run still unresolved before that boot.
/// `boots` need not be sorted; each boot is allowed to catch up at most
/// one outcome, reusing [`crate::systemd::evaluate_missed_runs`] so this
/// matches `Persistent=`'s own classification rather than a separate copy
/// of it. A no-op when `persistent` is false (every cron job's case) or
/// there are no boots.
pub fn apply_persistent_catchup(
    outcomes: &mut [RunOutcome],
    boots: &[Timestamp],
    persistent: bool,
) {
    if !persistent || boots.is_empty() {
        return;
    }
    let mut boots = boots.to_vec();
    boots.sort();

    for boot in boots {
        let candidates: Vec<(usize, Timestamp)> = outcomes
            .iter()
            .enumerate()
            .filter(|(_, o)| matches!(o.status, Status::Missed | Status::Late) && o.expected < boot)
            .map(|(i, o)| (i, o.expected))
            .collect();
        if candidates.is_empty() {
            continue;
        }
        let expected_times: Vec<Timestamp> = candidates.iter().map(|&(_, e)| e).collect();
        let statuses = crate::systemd::evaluate_missed_runs(&expected_times, boot, persistent);
        let caught_up = statuses.iter().enumerate().find_map(|(i, s)| match s {
            crate::systemd::MissedRunStatus::LateCaughtUp { by } => Some((i, *by)),
            crate::systemd::MissedRunStatus::Missed => None,
        });
        if let Some((pos, by)) = caught_up {
            let (idx, _) = candidates[pos];
            let outcome = &mut outcomes[idx];
            if outcome.actual.is_none() {
                outcome.actual = Some(boot);
                outcome.late_by = Some(by);
            }
            outcome.status = Status::LateCaughtUp;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn coverage(first: &str, last: &str) -> LogCoverage {
        LogCoverage {
            first: Some(ts(first)),
            last: Some(ts(last)),
        }
    }

    #[test]
    fn exact_match_within_tolerance_is_ok() {
        let expected = vec![ts("2026-10-20T03:00:00Z")];
        let runs = vec![(ts("2026-10-20T03:00:01Z"), ExitStatus::Unknown)];
        let cov = coverage("2026-10-20T00:00:00Z", "2026-10-21T00:00:00Z");
        let got = match_runs(&expected, &runs, Duration::from_secs(300), &cov);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].status, Status::Ok);
        assert_eq!(got[0].actual, Some(ts("2026-10-20T03:00:01Z")));
    }

    #[test]
    fn run_later_than_tolerance_is_late_not_missed() {
        // 12 minutes late, 5 minute tolerance -> LATE.
        let expected = vec![ts("2026-10-20T03:00:00Z")];
        let runs = vec![(ts("2026-10-20T03:12:00Z"), ExitStatus::Unknown)];
        let cov = coverage("2026-10-20T00:00:00Z", "2026-10-21T00:00:00Z");
        let got = match_runs(&expected, &runs, Duration::from_secs(300), &cov);
        assert_eq!(got[0].status, Status::Late);
        assert_eq!(got[0].late_by, Some(Duration::from_secs(12 * 60)));
    }

    #[test]
    fn no_run_within_coverage_is_missed() {
        let expected = vec![ts("2026-10-20T03:00:00Z")];
        let runs: Vec<(Timestamp, ExitStatus)> = vec![];
        let cov = coverage("2026-10-20T00:00:00Z", "2026-10-21T00:00:00Z");
        let got = match_runs(&expected, &runs, Duration::from_secs(300), &cov);
        assert_eq!(got[0].status, Status::Missed);
    }

    #[test]
    fn no_run_outside_coverage_is_no_trace() {
        let expected = vec![ts("2026-10-20T03:00:00Z")];
        let runs: Vec<(Timestamp, ExitStatus)> = vec![];
        // Logs only cover a later window.
        let cov = coverage("2026-10-22T00:00:00Z", "2026-10-23T00:00:00Z");
        let got = match_runs(&expected, &runs, Duration::from_secs(300), &cov);
        assert_eq!(got[0].status, Status::NoTrace);
    }

    #[test]
    fn empty_logs_are_always_no_trace_never_missed() {
        let expected = vec![ts("2026-10-20T03:00:00Z"), ts("2026-10-21T03:00:00Z")];
        let runs: Vec<(Timestamp, ExitStatus)> = vec![];
        let cov = LogCoverage::default();
        assert!(cov.is_empty());
        let got = match_runs(&expected, &runs, Duration::from_secs(300), &cov);
        assert!(got.iter().all(|o| o.status == Status::NoTrace));
    }

    #[test]
    fn late_run_does_not_steal_the_next_expected_times_match() {
        // Two daily runs; the first is 20 minutes late, the second is
        // exactly on time. The late run for day 1 must not claim day 2's
        // event, and vice versa.
        let expected = vec![ts("2026-10-20T03:00:00Z"), ts("2026-10-21T03:00:00Z")];
        let runs = vec![
            (ts("2026-10-20T03:20:00Z"), ExitStatus::Unknown),
            (ts("2026-10-21T03:00:00Z"), ExitStatus::Unknown),
        ];
        let cov = coverage("2026-10-20T00:00:00Z", "2026-10-22T00:00:00Z");
        let got = match_runs(&expected, &runs, Duration::from_secs(300), &cov);
        assert_eq!(got[0].status, Status::Late);
        assert_eq!(got[0].actual, Some(ts("2026-10-20T03:20:00Z")));
        assert_eq!(got[1].status, Status::Ok);
        assert_eq!(got[1].actual, Some(ts("2026-10-21T03:00:00Z")));
    }

    #[test]
    fn exit_status_is_carried_through_from_the_matched_run() {
        let expected = vec![ts("2026-10-20T03:00:00Z")];
        let runs = vec![(ts("2026-10-20T03:00:00Z"), ExitStatus::Completed)];
        let cov = coverage("2026-10-20T00:00:00Z", "2026-10-21T00:00:00Z");
        let got = match_runs(&expected, &runs, Duration::from_secs(300), &cov);
        assert_eq!(got[0].exit_status, ExitStatus::Completed);
    }

    #[test]
    fn persistent_catchup_upgrades_the_most_recent_missed_run_before_boot() {
        // Three missed nights, machine boots at 08:12
        // on the third day; only the last of the three is LATE (caught
        // up), the earlier two stay MISSED.
        let expected = vec![
            ts("2026-10-18T03:00:00Z"),
            ts("2026-10-19T03:00:00Z"),
            ts("2026-10-20T03:00:00Z"),
        ];
        let runs: Vec<(Timestamp, ExitStatus)> = vec![];
        let cov = coverage("2026-10-18T00:00:00Z", "2026-10-21T00:00:00Z");
        let mut got = match_runs(&expected, &runs, Duration::from_secs(300), &cov);
        assert!(got.iter().all(|o| o.status == Status::Missed));

        let boot = ts("2026-10-20T08:12:00Z");
        apply_persistent_catchup(&mut got, &[boot], true);

        assert_eq!(got[0].status, Status::Missed);
        assert_eq!(got[1].status, Status::Missed);
        assert_eq!(got[2].status, Status::LateCaughtUp);
        assert_eq!(got[2].actual, Some(boot));
        assert_eq!(
            got[2].late_by,
            Some(Duration::from_secs(5 * 3600 + 12 * 60))
        );
    }

    #[test]
    fn persistent_catchup_is_a_no_op_without_persistent() {
        let expected = vec![ts("2026-10-20T03:00:00Z")];
        let cov = coverage("2026-10-20T00:00:00Z", "2026-10-21T00:00:00Z");
        let mut got = match_runs(&expected, &[], Duration::from_secs(300), &cov);
        apply_persistent_catchup(&mut got, &[ts("2026-10-20T08:12:00Z")], false);
        assert_eq!(got[0].status, Status::Missed);
    }

    #[test]
    fn persistent_catchup_preserves_a_real_late_run_as_the_actual() {
        // The catch-up run *is* observed in the logs (e.g. a journal
        // "Started ..." line), later than the boot that triggered it:
        // the real observed instant and lateness are kept, only the
        // status is upgraded.
        let expected = vec![ts("2026-10-20T03:00:00Z")];
        let runs = vec![(ts("2026-10-20T08:14:00Z"), ExitStatus::Unknown)];
        let cov = coverage("2026-10-20T00:00:00Z", "2026-10-21T00:00:00Z");
        let mut got = match_runs(&expected, &runs, Duration::from_secs(300), &cov);
        assert_eq!(got[0].status, Status::Late);

        apply_persistent_catchup(&mut got, &[ts("2026-10-20T08:12:00Z")], true);

        assert_eq!(got[0].status, Status::LateCaughtUp);
        assert_eq!(got[0].actual, Some(ts("2026-10-20T08:14:00Z")));
        assert_eq!(
            got[0].late_by,
            Some(Duration::from_secs(5 * 3600 + 14 * 60))
        );
    }
}
