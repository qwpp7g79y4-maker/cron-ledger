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
//!    claimed by this one. The chronologically last expected time has no
//!    "next" one to bound it this way, so it reuses the spacing to the
//!    *previous* expected time instead (same cadence assumed). A single
//!    expected time has no previous gap. When the caller passes the next
//!    firing of that schedule, including one outside the audit window,
//!    the late search stops there: the later slot's run is not claimed,
//!    and a short copied gap is not used in place of the real next
//!    firing. Without that hint a single expected time stays unbounded.
//!    A cap of a few tolerances would detach a systemd `Persistent=`
//!    catch-up that lands hours later; that observed start has to stay
//!    matched so the catch-up can keep its real timestamp.
//!
//! What's left over is `MISSED` (inside the log's coverage window) or
//! `NO_TRACE` (outside it).
//!
//! `runs` is sorted by timestamp, so both passes narrow their search to a
//! bounded slice (via [`<[T]>::partition_point`]) instead of scanning
//! every run for every expected time.

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

/// Largest second-count that still fits in `i64`. A tolerance past this
/// already covers every timestamp difference `abs_diff_secs` can return,
/// so clamping here is the tolerance, not a narrower one. Casting with
/// `as i64` would wrap `2^63` seconds to a negative bound and then
/// overflow the window arithmetic.
fn tolerance_limit_secs(tol: Duration) -> i64 {
    i64::try_from(tol.as_secs()).unwrap_or(i64::MAX)
}

fn within_tolerance(diff_secs: i64, tol: Duration) -> bool {
    diff_secs <= tolerance_limit_secs(tol)
}

/// Upper bound of the pass-2 late search for `expected[index]`.
///
/// The next in-window expected time always wins, tightened by this
/// entry's own next firing when the caller knows it. The last expected
/// time uses that next firing when present, and otherwise copies the gap
/// back to the previous expected time. A lone expected time with no hint
/// stays unbounded (`None`).
fn late_upper_bound(
    expected: &[(Timestamp, Duration)],
    index: usize,
    next_after: &[Option<Timestamp>],
) -> Option<Timestamp> {
    let own = next_after.get(index).copied().flatten();
    if let Some(&(next, _)) = expected.get(index + 1) {
        return Some(own.map_or(next, |bound| next.min(bound)));
    }
    if let Some(bound) = own {
        return Some(bound);
    }
    let exp = expected[index].0;
    index.checked_sub(1).and_then(|prev| {
        let cap_secs = exp.as_second().saturating_sub(expected[prev].0.as_second());
        Timestamp::from_second(exp.as_second().saturating_add(cap_secs.max(1))).ok()
    })
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
    let tagged: Vec<(Timestamp, Duration)> = expected.iter().map(|&exp| (exp, tolerance)).collect();
    match_shared_runs(&tagged, runs, coverage, &[])
}

/// Like [`match_runs`], but each expected time carries its own
/// tolerance, and the set may interleave the expected times of several
/// jobs that share one pool of observed runs (for instance, the same
/// cron command scheduled from two files) sorted into one chronological
/// sequence. Each run is still used by at most one expected time, now
/// across the whole merged set rather than per job, so one real
/// execution cannot count as a hit for more than one of them. Matching
/// stays the same greedy nearest-match heuristic as a single job's; it
/// does not attempt an optimal assignment across jobs.
pub fn match_shared_runs(
    expected: &[(Timestamp, Duration)],
    runs: &[(Timestamp, ExitStatus)],
    coverage: &LogCoverage,
    next_after: &[Option<Timestamp>],
) -> Vec<RunOutcome> {
    debug_assert!(next_after.is_empty() || next_after.len() == expected.len());
    let mut used = vec![false; runs.len()];
    let mut matched: Vec<Option<usize>> = vec![None; expected.len()];

    // Pass 1: strict tolerance, nearest unused run. `runs` is sorted, so
    // narrow to the window `[exp - tolerance, exp + tolerance]` first
    // instead of scanning every run.
    for (i, &(exp, tol)) in expected.iter().enumerate() {
        let limit = tolerance_limit_secs(tol);
        let exp_secs = exp.as_second();
        // Saturate: `exp ± limit` overflows `i64` when the tolerance is
        // anywhere near `2^63` seconds, which is a panic in debug and a
        // wrapped, effectively negative window in release.
        let lower = exp_secs.saturating_sub(limit);
        let upper = exp_secs.saturating_add(limit);
        let start = runs.partition_point(|&(ts, _)| ts.as_second() < lower);
        let end = runs.partition_point(|&(ts, _)| ts.as_second() <= upper);
        let mut best: Option<(usize, i64)> = None;
        for j in start..end {
            if used[j] {
                continue;
            }
            let diff = abs_diff_secs(exp, runs[j].0);
            if best.is_none_or(|(_, bd)| diff < bd) {
                best = Some((j, diff));
            }
        }
        if let Some((j, _)) = best {
            used[j] = true;
            matched[i] = Some(j);
        }
    }

    // Pass 2: remaining unmatched expected times look for the nearest
    // later, still-unused run before the next expected time (or, for
    // the last expected time, before a bound derived from spacing; see
    // the module doc). Candidates all have `ts > exp`, and `runs` is
    // sorted, so the first unused one in range is automatically the
    // nearest.
    for i in 0..expected.len() {
        if matched[i].is_some() {
            continue;
        }
        let (exp, _) = expected[i];
        let upper_bound = late_upper_bound(expected, i, next_after);
        let start = runs.partition_point(|&(ts, _)| ts <= exp);
        let end = match upper_bound {
            Some(next) => runs.partition_point(|&(ts, _)| ts < next),
            None => runs.len(),
        };
        if let Some(j) = (start..end).find(|&j| !used[j]) {
            used[j] = true;
            matched[i] = Some(j);
        }
    }

    expected
        .iter()
        .enumerate()
        .map(|(i, &(exp, tol))| match matched[i] {
            Some(j) => {
                let (ts, exit_status) = runs[j];
                let diff = abs_diff_secs(exp, ts);
                if within_tolerance(diff, tol) {
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
            .filter(|(_, o)| {
                o.expected < boot
                    && match o.status {
                        Status::Missed => true,
                        Status::Late => o.actual.is_none_or(|a| a >= boot),
                        _ => false,
                    }
            })
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
    fn persistent_catchup_does_not_claim_a_late_run_that_already_happened_before_boot() {
        // The job ran, just 20 minutes late, well before the boot. That
        // real (pre-boot) run must not be reinterpreted as the boot's
        // catch-up.
        let expected = vec![ts("2026-10-20T03:00:00Z")];
        let runs = vec![(ts("2026-10-20T03:20:00Z"), ExitStatus::Unknown)];
        let cov = coverage("2026-10-20T00:00:00Z", "2026-10-21T00:00:00Z");
        let mut got = match_runs(&expected, &runs, Duration::from_secs(300), &cov);
        assert_eq!(got[0].status, Status::Late);

        apply_persistent_catchup(&mut got, &[ts("2026-10-20T08:12:00Z")], true);

        assert_eq!(got[0].status, Status::Late);
        assert_eq!(got[0].actual, Some(ts("2026-10-20T03:20:00Z")));
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

    #[test]
    fn trailing_expected_times_late_search_is_bounded_by_the_previous_spacing() {
        // Daily job, second occurrence missed. A run found a full day
        // plus an hour later really belongs to some other, later
        // occurrence (possibly outside the audit window entirely, if
        // the log happens to extend past it) -- not this one. The late
        // search for the last expected time must not reach that far.
        let expected = vec![ts("2026-10-19T03:00:00Z"), ts("2026-10-20T03:00:00Z")];
        let runs = vec![(ts("2026-10-21T04:00:00Z"), ExitStatus::Unknown)];
        let cov = coverage("2026-10-19T00:00:00Z", "2026-10-22T00:00:00Z");
        let got = match_runs(&expected, &runs, Duration::from_secs(300), &cov);
        assert_eq!(got[0].status, Status::Missed);
        assert_eq!(got[1].status, Status::Missed);
        assert!(got.iter().all(|o| o.actual.is_none()));
    }

    #[test]
    fn single_expected_time_late_search_stays_unbounded() {
        // One expected time has no previous gap to copy, and this call
        // passes no next-firing hint. Capping it at a multiple of the
        // tolerance would turn this 2h-late run into MISSED, and the
        // same cap would drop the hours-later Persistent= catch-up in
        // `persistent_catchup_preserves_a_real_late_run_as_the_actual`.
        // Without a hint the search stays unbounded on purpose. The
        // schedule layer passes the real next firing so a later slot's
        // run is not claimed; see
        // `single_expected_time_does_not_claim_the_next_firing`.
        let expected = vec![ts("2026-10-20T03:00:00Z")];
        let runs = vec![(ts("2026-10-20T05:00:00Z"), ExitStatus::Unknown)]; // 2h later
        let cov = coverage("2026-10-20T00:00:00Z", "2026-10-21T00:00:00Z");
        let got = match_runs(&expected, &runs, Duration::from_secs(300), &cov);
        assert_eq!(got[0].status, Status::Late);
        assert_eq!(got[0].actual, Some(ts("2026-10-20T05:00:00Z")));
    }

    #[test]
    fn tolerance_of_two_to_the_63_seconds_stays_ok_and_does_not_panic() {
        // 1s after the expected time. A tolerance of 2^63 seconds (and
        // one past that, which does not fit in i64) used to panic in
        // debug and come back LATE in release, because the window
        // subtraction overflowed.
        let expected = vec![ts("2026-10-20T03:00:00Z")];
        let runs = vec![(ts("2026-10-20T03:00:01Z"), ExitStatus::Unknown)];
        let cov = coverage("2026-10-20T00:00:00Z", "2026-10-21T00:00:00Z");
        for secs in [1u64 << 63, u64::MAX] {
            let got = match_runs(&expected, &runs, Duration::from_secs(secs), &cov);
            assert_eq!(got[0].status, Status::Ok, "tolerance {secs}");
            assert_eq!(got[0].actual, Some(ts("2026-10-20T03:00:01Z")));
        }
    }

    #[test]
    fn single_expected_time_does_not_claim_the_next_firing() {
        // The only in-window time is 03:00 on the 20th. The run on the
        // 21st is that schedule's next firing, so it is not evidence
        // that the 20th ran late. A 2h-late run the same day still is.
        let expected = vec![(ts("2026-10-20T03:00:00Z"), Duration::from_secs(300))];
        let cov = coverage("2026-10-19T03:00:01Z", "2026-10-21T03:00:01Z");
        let next = vec![Some(ts("2026-10-21T03:00:00Z"))];

        let missed = match_shared_runs(
            &expected,
            &[(ts("2026-10-21T03:00:01Z"), ExitStatus::Unknown)],
            &cov,
            &next,
        );
        assert_eq!(missed[0].status, Status::Missed);
        assert!(missed[0].actual.is_none());

        let late = match_shared_runs(
            &expected,
            &[(ts("2026-10-20T05:00:00Z"), ExitStatus::Unknown)],
            &cov,
            &next,
        );
        assert_eq!(late[0].status, Status::Late);
        assert_eq!(late[0].actual, Some(ts("2026-10-20T05:00:00Z")));
    }

    #[test]
    fn short_previous_gap_does_not_hide_a_late_run_before_the_real_next_firing() {
        // 03:00 then 03:05. Copying the 5-minute gap would stop the late
        // search at 03:10 and call a 03:12 run MISSED. The real next
        // firing is the next day, so 03:12 is LATE for 03:05 and does
        // not belong to 03:00.
        let expected = vec![
            (ts("2026-10-20T03:00:00Z"), Duration::from_secs(300)),
            (ts("2026-10-20T03:05:00Z"), Duration::from_secs(300)),
        ];
        let runs = vec![(ts("2026-10-20T03:12:00Z"), ExitStatus::Unknown)];
        let cov = coverage("2026-10-20T02:00:00Z", "2026-10-20T04:00:00Z");
        let next_day = Some(ts("2026-10-21T03:00:00Z"));
        let got = match_shared_runs(&expected, &runs, &cov, &[next_day, next_day]);
        assert_eq!(got[0].status, Status::Missed);
        assert_eq!(got[1].status, Status::Late);
        assert_eq!(got[1].actual, Some(ts("2026-10-20T03:12:00Z")));
    }

    /// The pre-optimization reference implementation: identical matching
    /// rules (including the bounded-last-expected-time fix), but scanning
    /// every run for every expected time instead of narrowing to a
    /// bounded window first. Kept only to prove the optimized
    /// [`match_shared_runs`] above never changes the result.
    fn naive_match_shared_runs(
        expected: &[(Timestamp, Duration)],
        runs: &[(Timestamp, ExitStatus)],
        coverage: &LogCoverage,
        next_after: &[Option<Timestamp>],
    ) -> Vec<RunOutcome> {
        let mut used = vec![false; runs.len()];
        let mut matched: Vec<Option<usize>> = vec![None; expected.len()];

        for (i, &(exp, tol)) in expected.iter().enumerate() {
            let mut best: Option<(usize, i64)> = None;
            for (j, &(ts, _)) in runs.iter().enumerate() {
                if used[j] {
                    continue;
                }
                let diff = abs_diff_secs(exp, ts);
                if within_tolerance(diff, tol) && best.is_none_or(|(_, bd)| diff < bd) {
                    best = Some((j, diff));
                }
            }
            if let Some((j, _)) = best {
                used[j] = true;
                matched[i] = Some(j);
            }
        }

        for i in 0..expected.len() {
            if matched[i].is_some() {
                continue;
            }
            let (exp, _) = expected[i];
            let upper_bound = late_upper_bound(expected, i, next_after);
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
            .map(|(i, &(exp, tol))| match matched[i] {
                Some(j) => {
                    let (ts, exit_status) = runs[j];
                    let diff = abs_diff_secs(exp, ts);
                    if within_tolerance(diff, tol) {
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

    fn arb_expected() -> impl proptest::strategy::Strategy<Value = Vec<(Timestamp, Duration)>> {
        use proptest::prelude::*;
        let anchor = ts("2026-10-20T00:00:00Z");
        prop::collection::btree_set(0i64..200_000, 1..8).prop_flat_map(move |offsets| {
            let offsets: Vec<i64> = offsets.into_iter().collect();
            let n = offsets.len();
            prop::collection::vec(prop::sample::select(vec![60u64, 300, 900]), n).prop_map(
                move |tols| {
                    offsets
                        .iter()
                        .zip(tols)
                        .map(|(&off, tol)| {
                            (
                                anchor.checked_add(jiff::Span::new().seconds(off)).unwrap(),
                                Duration::from_secs(tol),
                            )
                        })
                        .collect::<Vec<_>>()
                },
            )
        })
    }

    fn arb_runs() -> impl proptest::strategy::Strategy<Value = Vec<(Timestamp, ExitStatus)>> {
        use proptest::prelude::*;
        let anchor = ts("2026-10-20T00:00:00Z");
        prop::collection::vec(0i64..200_000, 0..10).prop_map(move |mut offsets| {
            offsets.sort_unstable();
            offsets
                .into_iter()
                .map(|off| {
                    (
                        anchor.checked_add(jiff::Span::new().seconds(off)).unwrap(),
                        ExitStatus::Unknown,
                    )
                })
                .collect()
        })
    }

    proptest::proptest! {
        // The windowed, partition_point-based matcher must never disagree
        // with the naive full-scan reference, across many random
        // expected/run shapes -- the optimization in `match_shared_runs`
        // is a performance change only, never a behavior change.
        #[test]
        fn match_shared_runs_matches_the_naive_reference(
            expected in arb_expected(),
            runs in arb_runs(),
        ) {
            let cov = coverage("2026-10-19T00:00:00Z", "2026-10-23T00:00:00Z");
            let optimized = match_shared_runs(&expected, &runs, &cov, &[]);
            let naive = naive_match_shared_runs(&expected, &runs, &cov, &[]);
            proptest::prop_assert_eq!(optimized, naive);
        }
    }
}
