//! Integration tests for the matcher and ledger, combining real
//! crontab parsing and schedule expansion with parsed log events.

use cron_ledger::crontab::{parse_user_crontab, ScheduleKind};
use cron_ledger::ledger::{build_ledger, JobKey, JobOutcome, JobSchedule, ScheduledJob, Status};
use cron_ledger::log::{parse_syslog_cron, EventSource, ExitStatus, LogEvent};
use cron_ledger::schedule;
use jiff::tz::TimeZone;
use jiff::Timestamp;
use std::time::Duration;

fn ts(s: &str) -> Timestamp {
    s.parse().unwrap()
}

fn cmd_event(file: &str, line: usize, when: &str, user: &str, command: &str) -> LogEvent {
    LogEvent {
        timestamp: ts(when),
        source: EventSource::CronCmd {
            user: user.to_string(),
            pid: None,
            command: command.to_string(),
        },
        file: file.to_string(),
        line,
    }
}

fn cron_job(key_user: &str, key_command: &str, expected: Vec<Timestamp>) -> ScheduledJob {
    ScheduledJob {
        key: JobKey::Cron {
            user: key_user.to_string(),
            command: key_command.to_string(),
        },
        file: "crontab".to_string(),
        line: 1,
        schedule: JobSchedule::Standard(expected),
        tolerance: Duration::from_secs(300),
        persistent: false,
    }
}

#[test]
fn duplicate_command_two_users_matches_independently() {
    // Duplicate identical commands for two users: same
    // command, different users, must not be matched against each other's
    // events.
    let expected = vec![ts("2026-10-20T03:00:00Z"), ts("2026-10-21T03:00:00Z")];
    let jobs = vec![
        cron_job("alice", "echo hi", expected.clone()),
        cron_job("bob", "echo hi", expected),
    ];
    let events = vec![
        cmd_event("syslog", 1, "2026-10-20T03:00:05Z", "alice", "echo hi"),
        cmd_event("syslog", 2, "2026-10-21T03:00:03Z", "alice", "echo hi"),
        cmd_event("syslog", 3, "2026-10-20T03:00:02Z", "bob", "echo hi"),
    ];

    let ledger = build_ledger(&jobs, &events);
    assert_eq!(ledger.entries.len(), 2);

    let JobOutcome::Standard(alice_outcomes) = &ledger.entries[0].outcome else {
        panic!("expected a standard outcome");
    };
    assert!(alice_outcomes.iter().all(|o| o.status == Status::Ok));

    let JobOutcome::Standard(bob_outcomes) = &ledger.entries[1].outcome else {
        panic!("expected a standard outcome");
    };
    assert_eq!(bob_outcomes[0].status, Status::Ok);
    // Bob's second day is genuinely missing, not Alice's event stolen or
    // ignored: the logs do cover that day (Alice's event on day 2 extends
    // global coverage there), so it is MISSED, not NO_TRACE.
    assert_eq!(bob_outcomes[1].status, Status::Missed);
}

#[test]
fn two_jobs_sharing_a_key_do_not_double_count_the_same_run() {
    // Two schedule lines for the identical (user, command) -- e.g. the
    // same job present in both /etc/crontab and a cron.d file -- must
    // not let one real execution count as a hit for both of them.
    let job_a = cron_job("root", "backup.sh", vec![ts("2026-10-20T03:00:00Z")]);
    let job_b = cron_job("root", "backup.sh", vec![ts("2026-10-20T03:02:00Z")]);
    let events = vec![
        // Unrelated events only to widen the log coverage window so
        // 03:00 reads as MISSED, not NO_TRACE.
        cmd_event("syslog", 1, "2026-10-20T00:00:00Z", "root", "unrelated"),
        cmd_event("syslog", 2, "2026-10-20T06:00:00Z", "root", "unrelated"),
        // The one real run, within tolerance of both expected times.
        cmd_event("syslog", 3, "2026-10-20T03:01:30Z", "root", "backup.sh"),
    ];

    let ledger = build_ledger(&[job_a, job_b], &events);
    assert_eq!(ledger.entries.len(), 2);

    let JobOutcome::Standard(a) = &ledger.entries[0].outcome else {
        panic!("expected a standard outcome");
    };
    let JobOutcome::Standard(b) = &ledger.entries[1].outcome else {
        panic!("expected a standard outcome");
    };
    // The matcher is still the same greedy, order-dependent heuristic
    // as a single job's (chronologically earlier expected times are
    // served first in pass 1), not an optimal assignment: job_a's
    // earlier expected time claims the run here. What matters for this
    // test is that exactly one of the two claims it, not both.
    assert_eq!(a[0].status, Status::Ok);
    assert_eq!(a[0].actual, Some(ts("2026-10-20T03:01:30Z")));
    assert_eq!(b[0].status, Status::Missed);
    assert!(b[0].actual.is_none());

    assert!(ledger
        .warnings
        .iter()
        .any(|w| w.contains("root: backup.sh") && w.contains("share this command")));
}

#[test]
fn percent_command_job_matches_on_the_split_command() {
    // A command containing %: the crontab parser already
    // splits the command from its %-separated stdin; the matcher must key
    // off that split command, not the raw crontab field.
    let content = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/crontab/percent.crontab"),
    )
    .unwrap();
    let parsed = parse_user_crontab(&content, "percent.crontab", Some("alice"));
    let cat_job = parsed
        .jobs
        .iter()
        .find(|j| j.command == "cat")
        .expect("percent.crontab has a 'cat%...' job");
    assert_eq!(cat_job.stdin.as_deref(), Some("line one\nline two"));

    let ScheduleKind::Standard(schedule) = &cat_job.schedule else {
        panic!("expected a standard schedule");
    };
    let tz = TimeZone::UTC;
    let since = ts("2026-10-20T00:00:00Z");
    let until = ts("2026-10-21T00:00:00Z");
    let expected = schedule::expand(schedule, &tz, since, until);
    assert_eq!(expected, vec![ts("2026-10-20T07:00:00Z")]);

    let job = cron_job("alice", "cat", expected);
    let events = vec![cmd_event(
        "syslog",
        1,
        "2026-10-20T07:00:01Z",
        "alice",
        "cat",
    )];

    let ledger = build_ledger(&[job], &events);
    let JobOutcome::Standard(outcomes) = &ledger.entries[0].outcome else {
        panic!("expected a standard outcome");
    };
    assert_eq!(outcomes[0].status, Status::Ok);
}

#[test]
fn escaped_percent_in_log_command_matches_the_unescaped_crontab_command() {
    // The crontab parser stores `date +%F` (its own %-splitting already
    // unescapes `\%`), but a cron log line echoes the command as written
    // in the crontab, backslash and all: `date +\%F`. The job key must
    // canonicalize that mismatch away.
    let expected = vec![ts("2026-10-20T06:00:00Z")];
    let job = cron_job("alice", "date +%F", expected);
    let events = vec![cmd_event(
        "syslog",
        1,
        "2026-10-20T06:00:00Z",
        "alice",
        r"date +\%F",
    )];

    let ledger = build_ledger(&[job], &events);
    let JobOutcome::Standard(outcomes) = &ledger.entries[0].outcome else {
        panic!("expected a standard outcome");
    };
    assert_eq!(outcomes[0].status, Status::Ok);
}

#[test]
fn log_gap_missed_only_in_covered_days() {
    // Logs cover only the last 2 of 7 days -> MISSED only
    // in the covered days, NO_TRACE (not MISSED) in the uncovered ones.
    let expected: Vec<Timestamp> = (0..7)
        .map(|day| ts(&format!("2026-10-{:02}T03:00:00Z", 20 + day)))
        .collect();
    let job = cron_job("root", "backup.sh", expected);

    // This job's own CMD never appears anywhere; coverage comes only from
    // an unrelated job's events on the last two days.
    let events = vec![
        cmd_event("syslog", 1, "2026-10-25T00:00:01Z", "root", "unrelated.sh"),
        cmd_event("syslog", 2, "2026-10-26T23:59:00Z", "root", "unrelated.sh"),
    ];

    let ledger = build_ledger(&[job], &events);
    let JobOutcome::Standard(outcomes) = &ledger.entries[0].outcome else {
        panic!("expected a standard outcome");
    };
    let statuses: Vec<Status> = outcomes.iter().map(|o| o.status).collect();
    assert_eq!(
        statuses,
        vec![
            Status::NoTrace, // day 20: before coverage
            Status::NoTrace, // day 21
            Status::NoTrace, // day 22
            Status::NoTrace, // day 23
            Status::NoTrace, // day 24
            Status::Missed,  // day 25: inside coverage, never ran
            Status::Missed,  // day 26: inside coverage, never ran
        ]
    );
}

#[test]
fn reboot_job_reports_found_boots_for_info_only() {
    let job = ScheduledJob {
        key: JobKey::Cron {
            user: "root".to_string(),
            command: "echo reboot".to_string(),
        },
        file: "crontab".to_string(),
        line: 8,
        schedule: JobSchedule::Reboot,
        tolerance: Duration::from_secs(300),
        persistent: false,
    };
    let events = vec![
        LogEvent {
            timestamp: ts("2026-10-20T06:00:00Z"),
            source: EventSource::Boot {
                boot_id: Some("boot-a".to_string()),
            },
            file: "journal.jsonl".to_string(),
            line: 1,
        },
        LogEvent {
            timestamp: ts("2026-10-21T06:00:00Z"),
            source: EventSource::Boot {
                boot_id: Some("boot-b".to_string()),
            },
            file: "journal.jsonl".to_string(),
            line: 2,
        },
    ];

    let ledger = build_ledger(&[job], &events);
    assert_eq!(
        ledger.entries[0].outcome,
        JobOutcome::Reboot {
            boots: vec![ts("2026-10-20T06:00:00Z"), ts("2026-10-21T06:00:00Z")]
        }
    );
}

#[test]
fn unsupported_trigger_timer_is_flagged_without_computation() {
    let job = ScheduledJob {
        key: JobKey::Timer {
            unit: "flaky.service".to_string(),
        },
        file: "flaky.timer".to_string(),
        line: 1,
        schedule: JobSchedule::UnsupportedTrigger,
        tolerance: Duration::from_secs(60),
        persistent: false,
    };
    let ledger = build_ledger(&[job], &[]);
    assert_eq!(ledger.entries[0].outcome, JobOutcome::UnsupportedTrigger);
}

#[test]
fn timer_matches_via_systemd_unit_start_events() {
    let job = ScheduledJob {
        key: JobKey::Timer {
            unit: "backup.service".to_string(),
        },
        file: "backup.timer".to_string(),
        line: 1,
        schedule: JobSchedule::Standard(vec![ts("2026-10-20T03:00:00Z")]),
        tolerance: Duration::from_secs(300),
        persistent: false,
    };
    let events = vec![LogEvent {
        timestamp: ts("2026-10-20T03:00:10Z"),
        source: EventSource::SystemdUnitStart {
            unit: "backup.service".to_string(),
        },
        file: "journal.jsonl".to_string(),
        line: 1,
    }];
    let ledger = build_ledger(&[job], &events);
    let JobOutcome::Standard(outcomes) = &ledger.entries[0].outcome else {
        panic!("expected a standard outcome");
    };
    assert_eq!(outcomes[0].status, Status::Ok);
}

#[test]
fn persistent_timer_missed_run_is_caught_up_late_via_boot_event() {
    // Machine off at 03:00, a `Persistent=true` timer's
    // boot catch-up run is observed in the journal at 08:12, after a
    // `Boot` event. Must be reported as `LateCaughtUp`, not plain `Late`.
    let job = ScheduledJob {
        key: JobKey::Timer {
            unit: "backup.service".to_string(),
        },
        file: "backup.timer".to_string(),
        line: 1,
        schedule: JobSchedule::Standard(vec![ts("2026-10-20T03:00:00Z")]),
        tolerance: Duration::from_secs(300),
        persistent: true,
    };
    let events = vec![
        // An unrelated event, only so the log coverage window includes
        // the missed 03:00 expected time (not just the catch-up window).
        LogEvent {
            timestamp: ts("2026-10-20T00:00:00Z"),
            source: EventSource::CronCmd {
                user: "root".to_string(),
                pid: None,
                command: "unrelated.sh".to_string(),
            },
            file: "syslog".to_string(),
            line: 1,
        },
        LogEvent {
            timestamp: ts("2026-10-20T08:12:00Z"),
            source: EventSource::Boot {
                boot_id: Some("boot-a".to_string()),
            },
            file: "journal.jsonl".to_string(),
            line: 1,
        },
        LogEvent {
            timestamp: ts("2026-10-20T08:12:05Z"),
            source: EventSource::SystemdUnitStart {
                unit: "backup.service".to_string(),
            },
            file: "journal.jsonl".to_string(),
            line: 2,
        },
    ];
    let ledger = build_ledger(&[job], &events);
    let JobOutcome::Standard(outcomes) = &ledger.entries[0].outcome else {
        panic!("expected a standard outcome");
    };
    assert_eq!(outcomes[0].status, Status::LateCaughtUp);
    assert_eq!(outcomes[0].actual, Some(ts("2026-10-20T08:12:05Z")));
}

#[test]
fn non_persistent_timer_missed_run_after_boot_stays_missed() {
    let job = ScheduledJob {
        key: JobKey::Timer {
            unit: "backup.service".to_string(),
        },
        file: "backup.timer".to_string(),
        line: 1,
        schedule: JobSchedule::Standard(vec![ts("2026-10-20T03:00:00Z")]),
        tolerance: Duration::from_secs(300),
        persistent: false,
    };
    let events = vec![
        LogEvent {
            timestamp: ts("2026-10-20T00:00:00Z"),
            source: EventSource::CronCmd {
                user: "root".to_string(),
                pid: None,
                command: "unrelated.sh".to_string(),
            },
            file: "syslog".to_string(),
            line: 1,
        },
        LogEvent {
            timestamp: ts("2026-10-20T08:12:00Z"),
            source: EventSource::Boot {
                boot_id: Some("boot-a".to_string()),
            },
            file: "journal.jsonl".to_string(),
            line: 1,
        },
    ];
    let ledger = build_ledger(&[job], &events);
    let JobOutcome::Standard(outcomes) = &ledger.entries[0].outcome else {
        panic!("expected a standard outcome");
    };
    assert_eq!(outcomes[0].status, Status::Missed);
}

#[test]
fn cmdend_without_pid_is_paired_fifo_and_carries_exit_status() {
    let job = cron_job("root", "backup.sh", vec![ts("2026-10-20T03:00:00Z")]);
    let events = vec![
        LogEvent {
            timestamp: ts("2026-10-20T03:00:01Z"),
            source: EventSource::CronCmd {
                user: "root".to_string(),
                pid: None,
                command: "backup.sh".to_string(),
            },
            file: "syslog".to_string(),
            line: 1,
        },
        LogEvent {
            timestamp: ts("2026-10-20T03:00:05Z"),
            source: EventSource::CronCmdEnd {
                user: "root".to_string(),
                pid: None,
                command: "backup.sh".to_string(),
            },
            file: "syslog".to_string(),
            line: 2,
        },
    ];
    let ledger = build_ledger(&[job], &events);
    let JobOutcome::Standard(outcomes) = &ledger.entries[0].outcome else {
        panic!("expected a standard outcome");
    };
    assert_eq!(outcomes[0].status, Status::Ok);
    assert_eq!(outcomes[0].exit_status, ExitStatus::Completed);
}

fn golden_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("ledger_{name}.json"))
}

fn assert_matches_golden<T: serde::Serialize>(name: &str, actual: &T) {
    let actual_json = serde_json::to_string_pretty(actual).unwrap() + "\n";
    if std::env::var_os("CRON_LEDGER_WRITE_GOLDEN").is_some() {
        std::fs::write(golden_path(name), &actual_json).unwrap();
        return;
    }
    let golden = std::fs::read_to_string(golden_path(name))
        .unwrap_or_else(|e| panic!("missing golden file for '{name}': {e}"));
    assert_eq!(
        actual_json, golden,
        "ledger output for scenario '{name}' no longer matches tests/golden/ledger_{name}.json"
    );
}

#[test]
fn end_to_end_crontab_and_syslog_matches_golden() {
    // A small, real end-to-end pipeline: crontab parsing -> schedule
    // expansion -> syslog parsing -> the ledger, for one OK job and one
    // job that is late.
    let crontab = "\
0 3 * * * /usr/local/bin/backup.sh
0 4 * * * /usr/local/bin/report.sh
";
    let parsed = parse_user_crontab(crontab, "crontab", Some("root"));
    let tz = TimeZone::UTC;
    let since = ts("2026-10-20T00:00:00Z");
    let until = ts("2026-10-21T00:00:00Z");

    let jobs: Vec<ScheduledJob> = parsed
        .jobs
        .iter()
        .map(|job| {
            let ScheduleKind::Standard(schedule) = &job.schedule else {
                unreachable!()
            };
            let expected = schedule::expand(schedule, &tz, since, until);
            ScheduledJob {
                key: JobKey::Cron {
                    user: job.user.clone().unwrap_or_default(),
                    command: cron_ledger::ledger::normalize_command(&job.command),
                },
                file: job.file.clone(),
                line: job.line,
                schedule: JobSchedule::Standard(expected),
                tolerance: Duration::from_secs(300),
                persistent: false,
            }
        })
        .collect();

    let log_content = "\
Oct 20 03:00:02 host CRON[1]: (root) CMD (/usr/local/bin/backup.sh)
Oct 20 04:12:00 host CRON[2]: (root) CMD (/usr/local/bin/report.sh)
";
    let parsed_log = parse_syslog_cron(log_content, "syslog", since, &tz);
    assert_eq!(parsed_log.warnings, vec![]);

    let ledger = build_ledger(&jobs, &parsed_log.events);
    assert_matches_golden("end_to_end", &ledger);
}
