//! Cross-checks this project's `OnCalendar=` expansion against real
//! `systemd-analyze calendar` output, captured once by hand; see
//! `tests/fixtures/systemd/README.md` for the exact commands and output.

use cron_ledger::systemd::{
    evaluate_missed_run, evaluate_missed_runs, parse_calendar, parse_timer_unit, MissedRunStatus,
};
use jiff::tz::TimeZone;
use jiff::Timestamp;
use std::time::Duration;

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/systemd")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// The exact instant captured in tests/fixtures/systemd/README.md
// immediately before running `systemd-analyze calendar`.
const REFERENCE_NOW: &str = "2026-10-22T06:28:15Z";

fn amsterdam_iterations(on_calendar: &str, count: usize) -> Vec<String> {
    let cal = parse_calendar(on_calendar).unwrap();
    let tz = TimeZone::get("Europe/Amsterdam").unwrap();
    let since: Timestamp = REFERENCE_NOW.parse().unwrap();
    // Wide enough window to comfortably cover `count` iterations for every
    // fixture used below (the sparsest is one per day).
    let until = since + jiff::Span::new().hours(10 * 24);
    let got = cron_ledger::systemd::expand_calendar(&cal, &tz, since, until).unwrap();
    got.into_iter()
        .take(count)
        .map(|t| {
            t.to_zoned(tz.clone())
                .strftime("%a %Y-%m-%d %H:%M:%S %Z")
                .to_string()
        })
        .collect()
}

#[test]
fn daily_matches_systemd_analyze() {
    let unit = parse_timer_unit(&fixture("daily.timer"), "daily.timer");
    let got = amsterdam_iterations(&unit.on_calendar[0], 3);
    assert_eq!(
        got,
        vec![
            "Fri 2026-10-23 00:00:00 CEST",
            "Sat 2026-10-24 00:00:00 CEST",
            "Sun 2026-10-25 00:00:00 CEST",
        ]
    );
}

#[test]
fn weekday_business_matches_systemd_analyze() {
    let unit = parse_timer_unit(&fixture("weekday-business.timer"), "weekday-business.timer");
    let got = amsterdam_iterations(&unit.on_calendar[0], 3);
    assert_eq!(
        got,
        vec![
            "Thu 2026-10-22 09:00:00 CEST",
            "Fri 2026-10-23 09:00:00 CEST",
            "Mon 2026-10-26 09:00:00 CET",
        ]
    );
}

#[test]
fn quarter_hour_matches_systemd_analyze() {
    let unit = parse_timer_unit(&fixture("quarter-hour.timer"), "quarter-hour.timer");
    let got = amsterdam_iterations(&unit.on_calendar[0], 3);
    assert_eq!(
        got,
        vec![
            "Thu 2026-10-22 08:30:00 CEST",
            "Thu 2026-10-22 08:45:00 CEST",
            "Thu 2026-10-22 09:00:00 CEST",
        ]
    );
}

#[test]
fn tz_suffix_matches_systemd_analyze() {
    let unit = parse_timer_unit(&fixture("tz-suffix.timer"), "tz-suffix.timer");
    let got = amsterdam_iterations(&unit.on_calendar[0], 3);
    assert_eq!(
        got,
        vec![
            "Fri 2026-10-23 03:00:00 CEST",
            "Sat 2026-10-24 03:00:00 CEST",
            "Sun 2026-10-25 03:00:00 CET",
        ]
    );
}

#[test]
fn persistent_missed_run_is_late_caught_up_not_missed() {
    let unit = parse_timer_unit(
        &fixture("persistent-backup.timer"),
        "persistent-backup.timer",
    );
    assert!(unit.persistent);
    assert_eq!(unit.unit.as_deref(), Some("backup.service"));

    // The machine was off from before 03:00 until 08:12 the same day.
    let expected: Timestamp = "2026-10-20T03:00:00Z".parse().unwrap();
    let boot: Timestamp = "2026-10-20T08:12:00Z".parse().unwrap();
    let status = evaluate_missed_run(expected, boot, unit.persistent);
    assert_eq!(
        status,
        MissedRunStatus::LateCaughtUp {
            by: Duration::from_secs(5 * 3600 + 12 * 60)
        }
    );
}

#[test]
fn persistent_backup_three_missed_nightly_runs_only_last_caught_up() {
    // Same persistent-backup.timer, but the machine was off for three
    // whole nightly 03:00 elapses before booting at 08:12 on the third
    // day. Per systemd.timer(5) ("triggered... at least once"), only
    // that most recent miss is caught up; the earlier two stay MISSED,
    // not LATE.
    let unit = parse_timer_unit(
        &fixture("persistent-backup.timer"),
        "persistent-backup.timer",
    );
    assert!(unit.persistent);

    let expected: Vec<Timestamp> = [
        "2026-10-18T03:00:00Z",
        "2026-10-19T03:00:00Z",
        "2026-10-20T03:00:00Z",
    ]
    .iter()
    .map(|s| s.parse().unwrap())
    .collect();
    let boot: Timestamp = "2026-10-20T08:12:00Z".parse().unwrap();

    let got = evaluate_missed_runs(&expected, boot, unit.persistent);
    assert_eq!(
        got,
        vec![
            MissedRunStatus::Missed,
            MissedRunStatus::Missed,
            MissedRunStatus::LateCaughtUp {
                by: Duration::from_secs(5 * 3600 + 12 * 60)
            },
        ]
    );
}

#[test]
fn non_persistent_missed_run_stays_missed() {
    // Same scenario, but without Persistent=true.
    let content = "[Timer]\nOnCalendar=*-*-* 03:00:00\n";
    let unit = parse_timer_unit(content, "f");
    let expected: Timestamp = "2026-10-20T03:00:00Z".parse().unwrap();
    let boot: Timestamp = "2026-10-20T08:12:00Z".parse().unwrap();
    assert_eq!(
        evaluate_missed_run(expected, boot, unit.persistent),
        MissedRunStatus::Missed
    );
}

#[test]
fn onbootsec_is_flagged_unsupported_with_no_oncalendar() {
    let unit = parse_timer_unit(
        &fixture("unsupported-trigger.timer"),
        "unsupported-trigger.timer",
    );
    assert!(unit.on_calendar.is_empty());
    assert_eq!(unit.unsupported_triggers, vec!["OnBootSec".to_string()]);
}
