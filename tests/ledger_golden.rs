//! Integration tests for the matcher and ledger, combining real
//! crontab parsing and schedule expansion with parsed log events.

use cron_ledger::crontab::{parse_user_crontab, ScheduleKind};
use cron_ledger::ledger::{
    build_ledger, build_ledger_with_coverage, JobKey, JobOutcome, JobSchedule, ScheduledJob, Status,
};
use cron_ledger::log::{parse_journal_json, parse_syslog_cron, EventSource, ExitStatus, LogEvent};
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
        next_after_window: None,
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
    // ignored: the logs do cover that day (Alice's event on day 2 is in
    // the same file and extends that file's span), so it is MISSED, not
    // NO_TRACE.
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
fn interior_gap_between_first_and_last_log_line_is_missed() {
    // Both lines are in one file. The days between sit inside that
    // file's span, so a run with no event there is MISSED. The day
    // before the first line and the day after the last line are outside
    // the span and stay NO_TRACE. One file is one closed range: a quiet
    // stretch between two lines of the same file is not a hole between
    // logs. A hole between two files is a different case; see
    // `gap_between_two_log_files_is_no_trace`.
    let expected: Vec<Timestamp> = (17..=23)
        .map(|day| ts(&format!("2026-10-{day:02}T03:00:00Z")))
        .collect();
    let job = cron_job("root", "/bin/backup.sh", expected);
    let events = vec![
        cmd_event(
            "gap.log",
            1,
            "2026-10-18T03:00:01Z",
            "root",
            "/bin/backup.sh",
        ),
        cmd_event(
            "gap.log",
            2,
            "2026-10-22T03:00:01Z",
            "root",
            "/bin/backup.sh",
        ),
    ];
    let ledger = build_ledger(&[job], &events);
    let JobOutcome::Standard(outcomes) = &ledger.entries[0].outcome else {
        panic!("expected a standard outcome");
    };
    let statuses: Vec<Status> = outcomes.iter().map(|o| o.status).collect();
    assert_eq!(
        statuses,
        vec![
            Status::NoTrace, // 17th: before the first line
            Status::Ok,      // 18th
            Status::Missed,  // 19th: inside the span, no event
            Status::Missed,  // 20th
            Status::Missed,  // 21st
            Status::Ok,      // 22nd
            Status::NoTrace, // 23rd: after the last line
        ]
    );
}

#[test]
fn gap_between_two_log_files_is_no_trace() {
    // The same instants as `interior_gap_between_first_and_last_log_line_is_missed`,
    // but each line lives in its own file. The days between the files
    // are not inside either span. A missing rotation is not proof the
    // job did not run, so those days are NO_TRACE. Days inside one
    // file's own span, with no matching command, stay MISSED.
    let expected: Vec<Timestamp> = (17..=23)
        .map(|day| ts(&format!("2026-10-{day:02}T03:00:00Z")))
        .collect();
    let job = cron_job("root", "/bin/backup.sh", expected);
    let events = vec![
        cmd_event("s.2.gz", 1, "2026-10-18T00:00:01Z", "root", "/bin/other.sh"),
        cmd_event("s.2.gz", 2, "2026-10-19T23:00:00Z", "root", "/bin/other.sh"),
        cmd_event("s.1", 1, "2026-10-22T03:00:01Z", "root", "/bin/backup.sh"),
    ];
    let ledger = build_ledger(&[job], &events);
    let JobOutcome::Standard(outcomes) = &ledger.entries[0].outcome else {
        panic!("expected a standard outcome");
    };
    let statuses: Vec<Status> = outcomes.iter().map(|o| o.status).collect();
    assert_eq!(
        statuses,
        vec![
            Status::NoTrace, // 17th: before the first file
            Status::Missed,  // 18th: inside s.2.gz, backup never ran
            Status::Missed,  // 19th: inside s.2.gz
            Status::NoTrace, // 20th: hole between the two files
            Status::NoTrace, // 21st
            Status::Ok,      // 22nd: the line in s.1
            Status::NoTrace, // 23rd: after the last file
        ]
    );
    assert_eq!(ledger.log_coverage.spans().len(), 2);
}

#[test]
fn gap_on_the_window_edge_between_two_files_is_no_trace() {
    // Window [19T03:00:00, 20T03:00:00). The only expected run is the
    // window start. One line sits before the window and one sits after
    // it (20T03:00:01 is past an exclusive `until` of 20T03:00:00).
    let expected = vec![ts("2026-10-19T03:00:00Z")];

    let split = vec![
        cmd_event("s.2.gz", 1, "2026-10-18T03:00:01Z", "root", "/bin/other.sh"),
        cmd_event("s.1", 1, "2026-10-20T03:00:01Z", "root", "/bin/other.sh"),
    ];
    let split_ledger = build_ledger(
        &[cron_job("root", "/bin/backup.sh", expected.clone())],
        &split,
    );
    let JobOutcome::Standard(split_outcomes) = &split_ledger.entries[0].outcome else {
        panic!("expected a standard outcome");
    };
    assert_eq!(
        split_outcomes[0].status,
        Status::NoTrace,
        "19T03:00:00 is the window edge and falls in the hole between the files"
    );

    // The same two lines in one file still cover the window edge.
    let one_file = vec![
        cmd_event("syslog", 1, "2026-10-18T03:00:01Z", "root", "/bin/other.sh"),
        cmd_event("syslog", 2, "2026-10-20T03:00:01Z", "root", "/bin/other.sh"),
    ];
    let one_ledger = build_ledger(
        &[cron_job("root", "/bin/backup.sh", expected.clone())],
        &one_file,
    );
    let JobOutcome::Standard(one_outcomes) = &one_ledger.entries[0].outcome else {
        panic!("expected a standard outcome");
    };
    assert_eq!(one_outcomes[0].status, Status::Missed);

    // A line exactly on the window start covers that instant.
    let on_edge = vec![cmd_event(
        "only.log",
        1,
        "2026-10-19T03:00:00Z",
        "root",
        "/bin/other.sh",
    )];
    let edge_ledger = build_ledger(&[cron_job("root", "/bin/backup.sh", expected)], &on_edge);
    let JobOutcome::Standard(edge_outcomes) = &edge_ledger.entries[0].outcome else {
        panic!("expected a standard outcome");
    };
    assert_eq!(edge_outcomes[0].status, Status::Missed);
}

fn journal_line(ts_us: i64, extra: &str) -> String {
    format!(r#"{{"__REALTIME_TIMESTAMP":"{ts_us}",{extra}}}"#)
}

#[test]
fn journal_entry_after_the_last_event_keeps_a_later_run_missed() {
    // Recognised CMD at 03:00, then a valid but unrecognised sshd line at
    // 09:00. 06:00 sits inside that span and has no run of its own.
    let t_cmd = ts("2026-10-20T03:00:00Z").as_microsecond();
    let t_sshd = ts("2026-10-20T09:00:00Z").as_microsecond();
    let t_bad_body = ts("2026-10-20T09:30:00Z").as_microsecond();
    let content = format!(
        "{}\n{}\nnot json at all\n{{\"SYSLOG_IDENTIFIER\":\"sshd\",\"MESSAGE\":\"no timestamp\"}}\n{}\n",
        journal_line(
            t_cmd,
            r#""SYSLOG_IDENTIFIER":"CROND","MESSAGE":"(root) CMD (other.sh)""#
        ),
        journal_line(
            t_sshd,
            r#""SYSLOG_IDENTIFIER":"sshd","MESSAGE":"Disconnected from user""#
        ),
        journal_line(
            t_bad_body,
            r#""SYSLOG_IDENTIFIER":"CROND","MESSAGE":"not a cron body""#
        ),
    );
    let parsed = parse_journal_json(&content, "journal.jsonl");
    assert_eq!(parsed.events.len(), 1, "the sshd line is not a cron event");
    assert_eq!(parsed.warnings.len(), 3, "invalid lines stay warnings");
    // The bad cron body is a real entry and does extend the span. The
    // invalid JSON line and the entry with no timestamp do not.
    assert_eq!(
        parsed.coverage_timestamps,
        vec![
            ts("2026-10-20T03:00:00Z"),
            ts("2026-10-20T09:00:00Z"),
            ts("2026-10-20T09:30:00Z"),
        ]
    );

    let job = cron_job("root", "/bin/job.sh", vec![ts("2026-10-20T06:00:00Z")]);
    let ledger = build_ledger_with_coverage(&[job], &parsed.events, parsed.coverage_timestamps);
    let JobOutcome::Standard(outcomes) = &ledger.entries[0].outcome else {
        panic!("expected a standard outcome");
    };
    assert_eq!(
        outcomes[0].status,
        Status::Missed,
        "coverage stopped at the last recognised event ({:?})",
        ledger.log_coverage
    );
}

#[test]
fn journal_excerpt_without_boot_evidence_does_not_catch_up_a_persistent_timer() {
    // 00:00 is a real cron line (coverage start). 08:00 is the first line
    // of this `_BOOT_ID` and is an ordinary sshd record eight hours after
    // monotonic zero, not a boot. A persistent timer due at 03:00 with no
    // `Started` line must not become LATE (caught up) at 08:00.
    let t0 = ts("2026-10-20T00:00:00Z").as_microsecond();
    let t8 = ts("2026-10-20T08:00:00Z").as_microsecond();
    let content = format!(
        "{}\n{}\n",
        journal_line(
            t0,
            r#""SYSLOG_IDENTIFIER":"CROND","MESSAGE":"(root) CMD (other.sh)""#
        ),
        journal_line(
            t8,
            r#""_BOOT_ID":"abc","__MONOTONIC_TIMESTAMP":"28800000000","SYSLOG_IDENTIFIER":"sshd","MESSAGE":"Disconnected from user""#
        ),
    );
    let parsed = parse_journal_json(&content, "journal.jsonl");
    assert!(
        parsed
            .events
            .iter()
            .all(|e| !matches!(e.source, EventSource::Boot { .. })),
        "excerpt invented a boot: {:?}",
        parsed.events
    );

    let reboot = ScheduledJob {
        key: JobKey::Cron {
            user: "root".to_string(),
            command: "echo reboot".to_string(),
        },
        file: "crontab".to_string(),
        line: 1,
        schedule: JobSchedule::Reboot,
        tolerance: Duration::from_secs(300),
        persistent: false,
        next_after_window: None,
    };
    let timer = ScheduledJob {
        key: JobKey::Timer {
            unit: "backup.service".to_string(),
        },
        file: "backup.timer".to_string(),
        line: 1,
        schedule: JobSchedule::Standard(vec![ts("2026-10-20T03:00:00Z")]),
        tolerance: Duration::from_secs(300),
        persistent: true,
        next_after_window: None,
    };
    let ledger =
        build_ledger_with_coverage(&[reboot, timer], &parsed.events, parsed.coverage_timestamps);
    assert_eq!(
        ledger.entries[0].outcome,
        JobOutcome::Reboot { boots: vec![] },
        "@reboot listed an invented boot time"
    );
    let JobOutcome::Standard(outcomes) = &ledger.entries[1].outcome else {
        panic!("expected a standard outcome");
    };
    // 03:00 is inside the journal span (00:00 through 08:00) and nothing
    // started the unit. MISSED, not a catch-up at the sshd line.
    assert_eq!(outcomes[0].status, Status::Missed);
    assert_eq!(outcomes[0].actual, None);
}

#[test]
fn kernel_banner_still_catches_up_a_persistent_timer() {
    // An sshd line of this boot comes first and is not the boot. The
    // kernel banner ten minutes later is. The timer's `Started` line
    // just after that stays LATE (caught up), at the real start time.
    let t0 = ts("2026-10-20T00:00:00Z").as_microsecond();
    let t_sshd = ts("2026-10-20T08:00:00Z").as_microsecond();
    let t_boot = ts("2026-10-20T08:10:00Z").as_microsecond();
    let t_start = ts("2026-10-20T08:12:00Z").as_microsecond();
    let content = format!(
        "{}\n{}\n{}\n{}\n",
        journal_line(
            t0,
            r#""SYSLOG_IDENTIFIER":"CROND","MESSAGE":"(root) CMD (other.sh)""#
        ),
        journal_line(
            t_sshd,
            r#""_BOOT_ID":"boot-a","__MONOTONIC_TIMESTAMP":"28800000000","SYSLOG_IDENTIFIER":"sshd","MESSAGE":"Disconnected from user""#
        ),
        journal_line(
            t_boot,
            r#""_BOOT_ID":"boot-a","__MONOTONIC_TIMESTAMP":"29400000000","SYSLOG_IDENTIFIER":"kernel","MESSAGE":"Linux version 6.1.0 (builder@host) (gcc) #1 SMP""#
        ),
        journal_line(
            t_start,
            r#""_BOOT_ID":"boot-a","SYSLOG_IDENTIFIER":"systemd","_SYSTEMD_UNIT":"init.scope","UNIT":"backup.service","MESSAGE":"Started Nightly backup.""#
        ),
    );
    let parsed = parse_journal_json(&content, "journal.jsonl");
    let reboot = ScheduledJob {
        key: JobKey::Cron {
            user: "root".to_string(),
            command: "echo reboot".to_string(),
        },
        file: "crontab".to_string(),
        line: 1,
        schedule: JobSchedule::Reboot,
        tolerance: Duration::from_secs(300),
        persistent: false,
        next_after_window: None,
    };
    let timer = ScheduledJob {
        key: JobKey::Timer {
            unit: "backup.service".to_string(),
        },
        file: "backup.timer".to_string(),
        line: 1,
        schedule: JobSchedule::Standard(vec![ts("2026-10-20T03:00:00Z")]),
        tolerance: Duration::from_secs(300),
        persistent: true,
        next_after_window: None,
    };
    let ledger =
        build_ledger_with_coverage(&[reboot, timer], &parsed.events, parsed.coverage_timestamps);
    assert_eq!(
        ledger.entries[0].outcome,
        JobOutcome::Reboot {
            boots: vec![ts("2026-10-20T08:10:00Z")]
        }
    );
    let JobOutcome::Standard(outcomes) = &ledger.entries[1].outcome else {
        panic!("expected a standard outcome");
    };
    assert_eq!(outcomes[0].status, Status::LateCaughtUp);
    assert_eq!(outcomes[0].actual, Some(ts("2026-10-20T08:12:00Z")));
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
        next_after_window: None,
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
        next_after_window: None,
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
        next_after_window: None,
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
fn oneshot_finished_line_is_a_run_not_missed() {
    // A Type=oneshot timer service logs "Starting …" and "Finished …"
    // and never "Started …". The finish line is the run. The start
    // announcement is not.
    let t_cover = ts("2026-10-20T02:00:00Z").as_microsecond();
    let t_starting = ts("2026-10-20T03:00:01Z").as_microsecond();
    let t_finished = ts("2026-10-20T03:00:05Z").as_microsecond();
    let content = format!(
        "{}\n{}\n{}\n",
        journal_line(
            t_cover,
            r#""SYSLOG_IDENTIFIER":"sshd","MESSAGE":"Disconnected from user""#
        ),
        journal_line(
            t_starting,
            r#""SYSLOG_IDENTIFIER":"systemd","_SYSTEMD_UNIT":"init.scope","UNIT":"backup.service","MESSAGE":"Starting Nightly backup...""#
        ),
        journal_line(
            t_finished,
            r#""SYSLOG_IDENTIFIER":"systemd","_SYSTEMD_UNIT":"init.scope","UNIT":"backup.service","MESSAGE":"Finished Nightly backup.""#
        ),
    );
    let parsed = parse_journal_json(&content, "journal.jsonl");
    let starts: Vec<_> = parsed
        .events
        .iter()
        .filter(|e| {
            matches!(
                &e.source,
                EventSource::SystemdUnitStart { unit } if unit == "backup.service"
            )
        })
        .collect();
    assert_eq!(
        starts.len(),
        1,
        "Starting is not a second run: {:?}",
        parsed.events
    );
    assert_eq!(starts[0].timestamp, ts("2026-10-20T03:00:05Z"));

    let job = ScheduledJob {
        key: JobKey::Timer {
            unit: "backup.service".to_string(),
        },
        file: "backup.timer".to_string(),
        line: 1,
        schedule: JobSchedule::Standard(vec![ts("2026-10-20T03:00:00Z")]),
        tolerance: Duration::from_secs(300),
        persistent: false,
        next_after_window: None,
    };
    let ledger = build_ledger_with_coverage(&[job], &parsed.events, parsed.coverage_timestamps);
    let JobOutcome::Standard(outcomes) = &ledger.entries[0].outcome else {
        panic!("expected a standard outcome");
    };
    assert_eq!(
        outcomes[0].status,
        Status::Ok,
        "oneshot Finished line was {:?}, not a run",
        outcomes[0].status
    );
    assert_eq!(outcomes[0].actual, Some(ts("2026-10-20T03:00:05Z")));
}

#[test]
fn started_and_finished_for_one_run_is_not_counted_twice() {
    // Type=simple and Type=forking can log "Started " and, when the
    // process exits, "Finished " for the same activation. The next
    // expected time must stay MISSED: the finish line is not a second run.
    let t_cover = ts("2026-10-20T02:00:00Z").as_microsecond();
    let t_starting = ts("2026-10-20T03:00:01Z").as_microsecond();
    let t_started = ts("2026-10-20T03:00:02Z").as_microsecond();
    let t_finished = ts("2026-10-20T03:00:40Z").as_microsecond();
    let t_tail = ts("2026-10-20T03:02:00Z").as_microsecond();
    let content = format!(
        "{}\n{}\n{}\n{}\n{}\n",
        journal_line(
            t_cover,
            r#""SYSLOG_IDENTIFIER":"sshd","MESSAGE":"Disconnected from user""#
        ),
        journal_line(
            t_starting,
            r#""SYSLOG_IDENTIFIER":"systemd","_SYSTEMD_UNIT":"init.scope","UNIT":"backup.service","MESSAGE":"Starting Nightly backup...""#
        ),
        journal_line(
            t_started,
            r#""SYSLOG_IDENTIFIER":"systemd","_SYSTEMD_UNIT":"init.scope","UNIT":"backup.service","MESSAGE":"Started Nightly backup.""#
        ),
        journal_line(
            t_finished,
            r#""SYSLOG_IDENTIFIER":"systemd","_SYSTEMD_UNIT":"init.scope","UNIT":"backup.service","MESSAGE":"Finished Nightly backup.""#
        ),
        journal_line(
            t_tail,
            r#""SYSLOG_IDENTIFIER":"sshd","MESSAGE":"Disconnected from user""#
        ),
    );
    let parsed = parse_journal_json(&content, "journal.jsonl");
    let starts: Vec<_> = parsed
        .events
        .iter()
        .filter(|e| {
            matches!(
                &e.source,
                EventSource::SystemdUnitStart { unit } if unit == "backup.service"
            )
        })
        .collect();
    assert_eq!(
        starts.len(),
        1,
        "Started and Finished counted separately: {:?}",
        starts
    );
    assert_eq!(starts[0].timestamp, ts("2026-10-20T03:00:02Z"));

    let job = ScheduledJob {
        key: JobKey::Timer {
            unit: "backup.service".to_string(),
        },
        file: "backup.timer".to_string(),
        line: 1,
        schedule: JobSchedule::Standard(vec![
            ts("2026-10-20T03:00:00Z"),
            ts("2026-10-20T03:01:00Z"),
        ]),
        tolerance: Duration::from_secs(300),
        persistent: false,
        next_after_window: None,
    };
    let ledger = build_ledger_with_coverage(&[job], &parsed.events, parsed.coverage_timestamps);
    let JobOutcome::Standard(outcomes) = &ledger.entries[0].outcome else {
        panic!("expected a standard outcome");
    };
    assert_eq!(outcomes[0].status, Status::Ok);
    assert_eq!(outcomes[0].actual, Some(ts("2026-10-20T03:00:02Z")));
    assert_eq!(outcomes[1].status, Status::Missed);
    assert!(outcomes[1].actual.is_none());
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
        next_after_window: None,
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
    // Both lines belong to one file, so 03:00 sits inside that span.
    // A boot does not catch up a non-persistent timer: the run stays
    // MISSED. Separate files would leave 03:00 in the hole between
    // them, which is NO_TRACE and would not exercise this case.
    let job = ScheduledJob {
        key: JobKey::Timer {
            unit: "backup.service".to_string(),
        },
        file: "backup.timer".to_string(),
        line: 1,
        schedule: JobSchedule::Standard(vec![ts("2026-10-20T03:00:00Z")]),
        tolerance: Duration::from_secs(300),
        persistent: false,
        next_after_window: None,
    };
    let events = vec![
        LogEvent {
            timestamp: ts("2026-10-20T00:00:00Z"),
            source: EventSource::CronCmd {
                user: "root".to_string(),
                pid: None,
                command: "unrelated.sh".to_string(),
            },
            file: "journal.jsonl".to_string(),
            line: 1,
        },
        LogEvent {
            timestamp: ts("2026-10-20T08:12:00Z"),
            source: EventSource::Boot {
                boot_id: Some("boot-a".to_string()),
            },
            file: "journal.jsonl".to_string(),
            line: 2,
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
                next_after_window: None,
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
