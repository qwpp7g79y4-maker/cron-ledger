//! Next-run preview. The caller supplies the instant. Nothing here reads
//! the machine clock, and nothing here invents a host.

use std::fmt::Write as _;

use jiff::tz::TimeZone;
use jiff::Timestamp;

use crate::crontab::{CronJob, ScheduleKind};
use crate::report::neutralize_controls;
use crate::schedule;

/// How many warning lines the preview prints before one summary line.
const MAX_WARNINGS_SHOWN: usize = 50;

/// One job in a next-run preview.
pub struct NextJob {
    /// `user: command`, or the timer unit name.
    pub label: String,
    /// Crontab or unit path, as named by the caller.
    pub file: String,
    /// 1-based crontab line. Timer units use 0, as the audit report does.
    pub line: usize,
    /// Schedule text in brackets.
    pub schedule: String,
    /// First firing at or after the caller's instant. `None` for
    /// `@reboot`, a monotonic timer, or a date that never occurs.
    pub next: Option<Timestamp>,
}

/// Rendered preview plus the counts a `--verbose` context line reports.
pub struct NextPreview {
    /// Full text, ending in a newline.
    pub text: String,
    /// Jobs printed.
    pub jobs: usize,
    /// Warnings collected, including ones the text truncates.
    pub warnings: usize,
}

/// First firing of `job` at or after `at`, in `tz`.
///
/// `tz` is the schedule zone: `CRON_TZ` when the job has one, otherwise
/// the explicit `--tz`. A plain `TZ=` line is not a schedule zone.
pub fn next_cron(job: &CronJob, tz: &TimeZone, at: Timestamp) -> Option<Timestamp> {
    match &job.schedule {
        ScheduleKind::Reboot => None,
        ScheduleKind::Standard(schedule) => schedule::first_on_or_after(schedule, tz, at),
    }
}

fn fmt_ts(ts: Timestamp, tz: &TimeZone) -> String {
    ts.to_zoned(tz.clone()).to_string()
}

/// Text preview. `at` is the caller's instant, printed in `tz`.
pub fn render(jobs: &[NextJob], at: Timestamp, tz: &TimeZone, warnings: &[String]) -> NextPreview {
    let mut out = String::new();
    if jobs.is_empty() {
        out.push_str("(no jobs found)\n\n");
    } else {
        for job in jobs {
            let _ = writeln!(
                out,
                "{} ({}:{}) [{}]",
                neutralize_controls(&job.label),
                neutralize_controls(&job.file),
                job.line,
                neutralize_controls(&job.schedule),
            );
            match job.next {
                Some(ts) => {
                    let _ = writeln!(out, "  next run: {}", fmt_ts(ts, tz));
                }
                None => {
                    let _ = writeln!(out, "  next run: -");
                }
            }
            out.push('\n');
        }
    }
    out.push_str("----\n");
    let _ = writeln!(
        out,
        "At: {} (tz {})",
        fmt_ts(at, tz),
        tz.iana_name().unwrap_or("?"),
    );

    if !warnings.is_empty() {
        out.push_str("Warnings:\n");
        let shown = warnings.len().min(MAX_WARNINGS_SHOWN);
        for w in &warnings[..shown] {
            let _ = writeln!(out, "  - {}", neutralize_controls(w));
        }
        let hidden = warnings.len() - shown;
        if hidden > 0 {
            let _ = writeln!(out, "  … and {hidden} more");
        }
    }

    NextPreview {
        text: out,
        jobs: jobs.len(),
        warnings: warnings.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crontab::{parse_system_crontab, parse_user_crontab};

    fn zone(name: &str) -> TimeZone {
        TimeZone::get(name).unwrap()
    }

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn sample_example_crosses_midnight_in_amsterdam_and_not_in_utc() {
        // The instant is this test's. examples/sample.crontab is 04:15.
        let parsed = parse_system_crontab(
            include_str!("../examples/sample.crontab"),
            "examples/sample.crontab",
        );
        let job = &parsed.jobs[0];
        let instant = at("2026-10-20T03:00:00Z");
        let utc = zone("UTC");
        let amsterdam = zone("Europe/Amsterdam");

        let next_utc = next_cron(job, &utc, instant).unwrap();
        let next_amsterdam = next_cron(job, &amsterdam, instant).unwrap();
        assert_eq!(fmt_ts(next_utc, &utc), "2026-10-20T04:15:00+00:00[UTC]");
        assert_eq!(
            fmt_ts(next_amsterdam, &amsterdam),
            "2026-10-21T04:15:00+02:00[Europe/Amsterdam]"
        );

        let shown = render(
            &[NextJob {
                label: "audit: /usr/local/bin/ledger-sample".to_string(),
                file: "examples/sample.crontab".to_string(),
                line: 1,
                schedule: "15 4 * * *".to_string(),
                next: Some(next_amsterdam),
            }],
            instant,
            &amsterdam,
            &[],
        );
        assert!(shown
            .text
            .contains("2026-10-21T04:15:00+02:00[Europe/Amsterdam]"));
        assert!(shown
            .text
            .contains("At: 2026-10-20T05:00:00+02:00[Europe/Amsterdam] (tz Europe/Amsterdam)"));
    }

    #[test]
    fn backup_example_spring_gap_runs_once_just_after_the_amsterdam_jump() {
        // examples/backup.crontab is 02:30, inside the 2026-03-29 gap.
        let parsed = parse_user_crontab(
            include_str!("../examples/backup.crontab"),
            "examples/backup.crontab",
            Some("ops"),
        );
        let backup = &parsed.jobs[0];
        let instant = at("2026-03-29T00:00:00Z");
        let utc = zone("UTC");
        let amsterdam = zone("Europe/Amsterdam");
        assert_eq!(
            fmt_ts(next_cron(backup, &utc, instant).unwrap(), &utc),
            "2026-03-29T02:30:00+00:00[UTC]"
        );
        assert_eq!(
            fmt_ts(next_cron(backup, &amsterdam, instant).unwrap(), &amsterdam),
            "2026-03-29T03:00:00+02:00[Europe/Amsterdam]"
        );
    }

    #[test]
    fn system_fixture_keeps_cron_tz_and_ignores_plain_tz() {
        // tests/fixtures/crontab/system.crontab: TZ=UTC is not the schedule
        // zone. CRON_TZ=America/New_York is, for the second job only.
        let parsed = parse_system_crontab(
            include_str!("../tests/fixtures/crontab/system.crontab"),
            "tests/fixtures/crontab/system.crontab",
        );
        assert_eq!(parsed.jobs.len(), 2);
        let instant = at("2026-10-20T03:00:00Z");
        let amsterdam = zone("Europe/Amsterdam");
        let new_york = zone("America/New_York");

        assert!(parsed.jobs[0].effective_tz().is_none());
        assert_eq!(parsed.jobs[1].effective_tz(), Some("America/New_York"));

        let first = next_cron(&parsed.jobs[0], &amsterdam, instant).unwrap();
        let second = next_cron(&parsed.jobs[1], &new_york, instant).unwrap();
        assert_eq!(
            fmt_ts(first, &amsterdam),
            "2026-10-21T03:00:00+02:00[Europe/Amsterdam]"
        );
        assert_eq!(
            fmt_ts(second, &amsterdam),
            "2026-10-20T08:30:00+02:00[Europe/Amsterdam]"
        );
    }
}
