//! Golden tests for the report renderers (text and `--json`), built
//! directly against hand-constructed ledgers (same style as
//! `ledger_golden.rs`), so these do not depend on the full CLI pipeline.

use cron_ledger::ledger::{
    JobKey, JobLedgerEntry, JobOutcome, Ledger, LogCoverage, RunOutcome, Status,
};
use cron_ledger::log::ExitStatus;
use cron_ledger::report::{render_json, render_text, ReportContext};
use jiff::tz::TimeZone;
use jiff::Timestamp;
use std::collections::HashMap;
use std::time::Duration;

fn ts(s: &str) -> Timestamp {
    s.parse().unwrap()
}

fn run(expected: &str, status: Status, actual: Option<&str>, late_by: Option<u64>) -> RunOutcome {
    RunOutcome {
        expected: ts(expected),
        status,
        actual: actual.map(ts),
        late_by: late_by.map(Duration::from_secs),
        exit_status: ExitStatus::Unknown,
    }
}

fn golden_path(name: &str, ext: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("report_{name}.{ext}"))
}

fn assert_matches_golden(name: &str, ext: &str, actual: &str) {
    let actual = if actual.ends_with('\n') {
        actual.to_string()
    } else {
        format!("{actual}\n")
    };
    if std::env::var_os("CRON_LEDGER_WRITE_GOLDEN").is_some() {
        std::fs::write(golden_path(name, ext), &actual).unwrap();
        return;
    }
    let golden = std::fs::read_to_string(golden_path(name, ext))
        .unwrap_or_else(|e| panic!("missing golden file for '{name}.{ext}': {e}"));
    assert_eq!(
        actual, golden,
        "report output for scenario '{name}' no longer matches tests/golden/report_{name}.{ext}"
    );
}

fn ok_ledger() -> (Ledger, HashMap<JobKey, String>) {
    let key = JobKey::Cron {
        user: "root".to_string(),
        command: "/usr/local/bin/backup.sh".to_string(),
    };
    let ledger = Ledger {
        entries: vec![JobLedgerEntry {
            key: key.clone(),
            file: "crontab".to_string(),
            line: 1,
            outcome: JobOutcome::Standard(vec![run(
                "2026-10-20T03:00:00Z",
                Status::Ok,
                Some("2026-10-20T03:00:02Z"),
                None,
            )]),
        }],
        log_coverage: LogCoverage::from_timestamps([ts("2026-10-20T03:00:02Z")]),
        warnings: Vec::new(),
    };
    let mut descriptions = HashMap::new();
    descriptions.insert(key, "0 3 * * *".to_string());
    (ledger, descriptions)
}

fn mixed_ledger() -> (Ledger, HashMap<JobKey, String>) {
    let backup_key = JobKey::Cron {
        user: "root".to_string(),
        command: "/usr/local/bin/backup.sh".to_string(),
    };
    let mut backup_outcomes = vec![
        run(
            "2026-10-20T03:00:00Z",
            Status::Ok,
            Some("2026-10-20T03:00:02Z"),
            None,
        ),
        run(
            "2026-10-21T03:00:00Z",
            Status::Late,
            Some("2026-10-21T03:12:00Z"),
            Some(12 * 60),
        ),
    ];
    for day in 22..29 {
        backup_outcomes.push(run(
            &format!("2026-10-{day}T03:00:00Z"),
            Status::Missed,
            None,
            None,
        ));
    }
    backup_outcomes.push(run("2026-10-30T03:00:00Z", Status::NoTrace, None, None));
    backup_outcomes.push(run("2026-10-31T03:00:00Z", Status::NoTrace, None, None));

    let timer_key = JobKey::Timer {
        unit: "backup.service".to_string(),
    };
    let reboot_key = JobKey::Cron {
        user: "root".to_string(),
        command: "/usr/local/bin/on-boot.sh".to_string(),
    };
    let flaky_key = JobKey::Timer {
        unit: "flaky.service".to_string(),
    };

    let ledger = Ledger {
        entries: vec![
            JobLedgerEntry {
                key: backup_key.clone(),
                file: "crontab".to_string(),
                line: 3,
                outcome: JobOutcome::Standard(backup_outcomes),
            },
            JobLedgerEntry {
                key: timer_key.clone(),
                file: "backup.timer".to_string(),
                line: 0,
                outcome: JobOutcome::Standard(vec![run(
                    "2026-10-20T03:00:00Z",
                    Status::Ok,
                    Some("2026-10-20T03:00:10Z"),
                    None,
                )]),
            },
            JobLedgerEntry {
                key: reboot_key.clone(),
                file: "crontab".to_string(),
                line: 9,
                outcome: JobOutcome::Reboot {
                    boots: vec![ts("2026-10-20T06:00:00Z"), ts("2026-10-25T06:00:00Z")],
                },
            },
            JobLedgerEntry {
                key: flaky_key.clone(),
                file: "flaky.timer".to_string(),
                line: 0,
                outcome: JobOutcome::UnsupportedTrigger,
            },
        ],
        log_coverage: LogCoverage::from_timestamps([
            ts("2026-10-20T03:00:02Z"),
            ts("2026-10-29T03:00:00Z"),
        ]),
        warnings: Vec::new(),
    };

    let mut descriptions = HashMap::new();
    descriptions.insert(backup_key, "0 3 * * *".to_string());
    descriptions.insert(timer_key, "OnCalendar=*-*-* 03:00:00".to_string());
    descriptions.insert(reboot_key, "@reboot".to_string());
    descriptions.insert(flaky_key, "unsupported: OnBootSec".to_string());
    (ledger, descriptions)
}

fn no_trace_ledger() -> (Ledger, HashMap<JobKey, String>) {
    let key = JobKey::Cron {
        user: "root".to_string(),
        command: "/usr/local/bin/ghost.sh".to_string(),
    };
    let ledger = Ledger {
        entries: vec![JobLedgerEntry {
            key: key.clone(),
            file: "crontab".to_string(),
            line: 1,
            outcome: JobOutcome::Standard(vec![run(
                "2026-10-20T03:00:00Z",
                Status::NoTrace,
                None,
                None,
            )]),
        }],
        log_coverage: LogCoverage::default(),
        warnings: vec![
            "no log events found in the window: every expected run is reported as NO_TRACE, \
             never MISSED"
                .to_string(),
        ],
    };
    let mut descriptions = HashMap::new();
    descriptions.insert(key, "0 3 * * *".to_string());
    (ledger, descriptions)
}

fn ctx<'a>(tz: &'a TimeZone, descriptions: &'a HashMap<JobKey, String>) -> ReportContext<'a> {
    ReportContext {
        since: ts("2026-10-20T00:00:00Z"),
        until: ts("2026-11-01T00:00:00Z"),
        tz,
        tolerance: Duration::from_secs(300),
        descriptions,
        extra_warnings: &[],
    }
}

#[test]
fn ok_scenario_matches_golden() {
    let (ledger, descriptions) = ok_ledger();
    let tz = TimeZone::UTC;
    let c = ctx(&tz, &descriptions);
    assert_matches_golden("ok", "txt", &render_text(&ledger, &c, false));
    assert_matches_golden("ok", "json", &render_json(&ledger, &c));
}

#[test]
fn mixed_scenario_matches_golden_and_truncates_missed_times_to_five() {
    let (ledger, descriptions) = mixed_ledger();
    let tz = TimeZone::UTC;
    let c = ctx(&tz, &descriptions);
    let text = render_text(&ledger, &c, false);
    assert_matches_golden("mixed", "txt", &text);
    assert_matches_golden("mixed", "json", &render_json(&ledger, &c));

    // Seven MISSED expected times exist (Oct 22-28); only the first five
    // are shown.
    assert!(text.contains("2026-10-26T03:00:00"));
    assert!(!text.contains("2026-10-27T03:00:00"));
}

#[test]
fn no_trace_scenario_matches_golden_and_exits_three() {
    let (ledger, descriptions) = no_trace_ledger();
    let tz = TimeZone::UTC;
    let c = ctx(&tz, &descriptions);
    assert_matches_golden("no_trace", "txt", &render_text(&ledger, &c, false));
    assert_matches_golden("no_trace", "json", &render_json(&ledger, &c));
    assert_eq!(cron_ledger::report::exit_code(&ledger), 3);
}

#[test]
fn color_wraps_status_words_in_ansi_codes_when_enabled() {
    let (ledger, descriptions) = ok_ledger();
    let tz = TimeZone::UTC;
    let c = ctx(&tz, &descriptions);
    let colored = render_text(&ledger, &c, true);
    assert!(colored.contains("\x1b["));
    let plain = render_text(&ledger, &c, false);
    assert!(!plain.contains("\x1b["));
}
