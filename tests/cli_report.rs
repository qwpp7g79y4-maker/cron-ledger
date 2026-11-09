//! CLI-level exit-code tests, driving the real `cron-ledger` binary
//! via `assert_cmd`, one per exit-code scenario from the README:
//! 0 (all OK), 1 (MISSED/LATE present), 2 (usage/parse error), 3 (only
//! NO_TRACE).
//!
//! Every invocation pins `--system-root` to an empty, isolated directory
//! so the real host's `/etc/crontab` etc. (which may or may not exist,
//! and is outside this project's control) never leaks into the result.

#[cfg(target_os = "linux")]
use assert_cmd::prelude::CommandCargoExt;
use assert_cmd::Command;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/report")
        .join(name)
}

fn thread_id_hash() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::thread::current().id().hash(&mut hasher);
    hasher.finish()
}

fn empty_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cron-ledger-cli-test-root-{}-{}",
        std::process::id(),
        thread_id_hash()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn base_cmd(scenario: &str, root: &Path, until: &str) -> Command {
    let dir = fixture(scenario);
    let mut cmd = Command::cargo_bin("cron-ledger").unwrap();
    cmd.arg("--system-root")
        .arg(root)
        .arg("--crontab")
        .arg(dir.join("crontab"))
        .arg("--syslog")
        .arg(dir.join("syslog"))
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--until")
        .arg(until);
    cmd
}

fn output_cmd(root: &Path, report: &Path) -> Command {
    let mut cmd = base_cmd("ok", root, "2026-10-21T00:00:00Z");
    cmd.arg("--output").arg(report);
    cmd
}

#[test]
fn output_flag_writes_a_new_file() {
    let root = empty_root();
    let report = root.join("report.txt");
    assert!(!report.exists());
    let written = output_cmd(&root, &report).assert().code(0);
    let written_out = written.get_output();
    assert!(written_out.stdout.is_empty(), "stdout must stay empty");
    assert!(written_out.stderr.is_empty(), "stderr must stay empty");
    let on_stdout = base_cmd("ok", &root, "2026-10-21T00:00:00Z")
        .assert()
        .code(0);
    let expected = on_stdout.get_output().stdout.clone();
    assert!(!expected.is_empty());
    assert_eq!(std::fs::read(&report).unwrap(), expected);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn output_flag_overwrites_an_existing_file() {
    let root = empty_root();
    let report = root.join("report.txt");
    std::fs::write(&report, b"stale report that must be replaced\n").unwrap();
    let written = output_cmd(&root, &report).assert().code(0);
    assert!(written.get_output().stdout.is_empty());
    let on_stdout = base_cmd("ok", &root, "2026-10-21T00:00:00Z")
        .assert()
        .code(0);
    let expected = on_stdout.get_output().stdout.clone();
    let body = std::fs::read(&report).unwrap();
    assert_eq!(body, expected);
    assert!(!String::from_utf8_lossy(&body).contains("stale"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn output_flag_missing_directory_exits_two() {
    let root = empty_root();
    let missing_dir = root.join("no-such-dir");
    let report = missing_dir.join("report.txt");
    let failed = output_cmd(&root, &report).assert().code(2);
    let output = failed.get_output();
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot write report"), "stderr: {stderr}");
    assert!(!report.exists());
    assert!(
        !missing_dir.exists(),
        "a missing parent directory must not be created"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn ok_scenario_exits_zero() {
    let root = empty_root();
    let assert = base_cmd("ok", &root, "2026-10-21T00:00:00Z")
        .assert()
        .code(0);
    let output = assert.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("OK"));
    assert!(
        !stdout.contains('\x1b'),
        "no tty: output must not contain ANSI escapes"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn missed_scenario_exits_one() {
    let root = empty_root();
    let assert = base_cmd("missed", &root, "2026-10-22T00:00:00Z")
        .assert()
        .code(1);
    let output = assert.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("MISSED"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn late_scenario_exits_one() {
    let root = empty_root();
    let assert = base_cmd("late", &root, "2026-10-21T00:00:00Z")
        .assert()
        .code(1);
    let output = assert.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("LATE"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn persistent_timer_catchup_scenario_exits_one_as_late_caught_up() {
    // A Persistent=true timer missed its 03:00 run
    // while the machine was down, then caught up at boot (08:12). The CLI
    // must report this as "LATE (caught up)", not MISSED or plain LATE.
    let root = empty_root();
    let dir = fixture("persistent");
    let assert = Command::cargo_bin("cron-ledger")
        .unwrap()
        .arg("--system-root")
        .arg(&root)
        .arg("--timer-unit")
        .arg(dir.join("backup.timer"))
        .arg("--journal-json")
        .arg(dir.join("journal.jsonl"))
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--until")
        .arg("2026-10-21T00:00:00Z")
        .assert()
        .code(1);
    let output = assert.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("LATE (caught up)"));
    // All statuses appear in the zero-count summary line regardless, but
    // no job block should report a bare MISSED status or count.
    assert!(stdout.contains("missed: 0"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn no_trace_only_scenario_exits_three() {
    let root = empty_root();
    let assert = base_cmd("no_trace", &root, "2026-10-21T00:00:00Z")
        .assert()
        .code(3);
    let output = assert.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("NO_TRACE"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn until_earlier_than_since_is_a_usage_error_exit_two() {
    // A daily job exists. An inverted window expands to no expected
    // runs, so without this check the audit exits 0 and looks clean.
    let root = empty_root();
    let dir = fixture("ok");
    let assert = Command::cargo_bin("cron-ledger")
        .unwrap()
        .arg("--system-root")
        .arg(&root)
        .arg("--crontab")
        .arg(dir.join("crontab"))
        .arg("--tz")
        .arg("UTC")
        .arg("--since")
        .arg("2026-10-21T00:00:00Z")
        .arg("--until")
        .arg("2026-10-20T00:00:00Z")
        .assert()
        .code(2);
    let output = assert.get_output();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "--until (2026-10-20T00:00:00Z) is earlier than --since (2026-10-21T00:00:00Z)"
        ),
        "stderr: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "an inverted window must not print an audit"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn until_equal_to_since_still_audits() {
    // An equal bound is an empty window, not a usage error.
    let root = empty_root();
    let dir = fixture("ok");
    let assert = Command::cargo_bin("cron-ledger")
        .unwrap()
        .arg("--system-root")
        .arg(&root)
        .arg("--crontab")
        .arg(dir.join("crontab"))
        .arg("--tz")
        .arg("UTC")
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--until")
        .arg("2026-10-20T00:00:00Z")
        .assert()
        .code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(stdout.contains("expected: 0"), "{stdout}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn missing_since_is_a_usage_error_exit_two() {
    let root = empty_root();
    Command::cargo_bin("cron-ledger")
        .unwrap()
        .arg("--system-root")
        .arg(&root)
        .assert()
        .code(2);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn more_than_one_stdin_source_is_a_usage_error_exit_two() {
    let root = empty_root();
    Command::cargo_bin("cron-ledger")
        .unwrap()
        .arg("--system-root")
        .arg(&root)
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--crontab")
        .arg("-")
        .arg("--syslog")
        .arg("-")
        .assert()
        .code(2);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn json_flag_produces_a_schema_versioned_report() {
    let root = empty_root();
    let assert = base_cmd("ok", &root, "2026-10-21T00:00:00Z")
        .arg("--json")
        .assert()
        .code(0);
    let output = assert.get_output();
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["exit_code"], 0);
    std::fs::remove_dir_all(&root).ok();
}

/// `--help` output is part of the README's documented interface,
/// so it gets the same golden-file protection as the reports do.
#[test]
fn help_flag_matches_golden_usage_text() {
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/help.txt");
    let expected = std::fs::read_to_string(&golden).unwrap();
    assert_cmd::Command::cargo_bin("cron-ledger")
        .unwrap()
        .arg("--help")
        .assert()
        .code(0)
        .stdout(expected);
}

/// `/dev/full` exists on Linux. The other CI targets have no equivalent.
#[cfg(target_os = "linux")]
#[test]
fn full_stderr_still_exits_two_instead_of_panicking() {
    let full = std::fs::File::options()
        .write(true)
        .open("/dev/full")
        .expect("open /dev/full");
    let output = std::process::Command::cargo_bin("cron-ledger")
        .expect("cron-ledger binary")
        .arg("--bogus")
        .stderr(full)
        .output()
        .expect("spawn cron-ledger");
    assert_eq!(output.status.code(), Some(2), "{:?}", output.status);
}

/// `/dev/full` exists on Linux. The other CI targets have no equivalent.
#[cfg(target_os = "linux")]
#[test]
fn full_stdout_exits_with_a_write_error_instead_of_panicking() {
    let root = empty_root();
    let dir = fixture("ok");
    let full = std::fs::File::options()
        .write(true)
        .open("/dev/full")
        .expect("open /dev/full");
    let output = std::process::Command::cargo_bin("cron-ledger")
        .expect("cron-ledger binary")
        .arg("--system-root")
        .arg(&root)
        .arg("--crontab")
        .arg(dir.join("crontab"))
        .arg("--syslog")
        .arg(dir.join("syslog"))
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--until")
        .arg("2026-10-21T00:00:00Z")
        .stdout(full)
        .output()
        .expect("spawn cron-ledger");
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        !err.contains("panicked"),
        "stdout write failure must not panic: {err}"
    );
    assert_eq!(output.status.code(), Some(2), "{err}");
    assert!(
        err.contains("cannot write report"),
        "stderr should name the write failure: {err}"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// Structural labels the text report used to print in Dutch. Status words
/// (`OK`, `MISSED`, …) were already English and are not in this list.
fn assert_no_dutch_report_labels(stdout: &str) {
    for label in [
        "verwacht:",
        "gevonden:",
        "laatste run:",
        "gemiste tijden",
        "Samenvatting:",
        "Logdekking:",
        "geen logs",
        "Venster:",
        "Tolerantie:",
        "Waarschuwingen:",
        "(geen jobs gevonden)",
        "boots gevonden:",
        "… en ",
    ] {
        assert!(
            !stdout.contains(label),
            "Dutch report label {label:?} still in:\n{stdout}"
        );
    }
}

/// The system-crontab example from the README, which the status review
/// ran and got back a Dutch text report for. `--tz` is pinned to the zone
/// that review printed (`Europe/Amsterdam`); without it the same files
/// audit in the host zone and the timestamps move.
#[test]
fn reviewed_system_crontab_text_report_is_english() {
    let root = empty_root();
    let assert = Command::cargo_bin("cron-ledger")
        .unwrap()
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .arg("--system-root")
        .arg("/nonexistent")
        .arg("--crontab")
        .arg("tests/fixtures/crontab/system.crontab")
        .arg("--syslog")
        .arg("tests/fixtures/logs/debian-syslog.log")
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--until")
        .arg("2026-10-21T00:00:00Z")
        .arg("--tz")
        .arg("Europe/Amsterdam")
        .assert()
        .code(1);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let expected = "\
root: /usr/local/bin/backup.sh (tests/fixtures/crontab/system.crontab:6) [0 3 * * *]
  expected: 1, found: 1, missed: 0, late: 0
  last run: 2026-10-20T03:00:01+02:00[Europe/Amsterdam] (OK)
  missed times: -

alice: /usr/local/bin/alice-job.sh (tests/fixtures/crontab/system.crontab:8) [30 2 * * *]
  expected: 1, found: 0, missed: 1, late: 0
  last run: -
  first missed times: 2026-10-20T08:30:00+02:00[Europe/Amsterdam]

----
Summary: 1 OK, 0 LATE, 0 LATE (caught up), 1 MISSED, 0 NO_TRACE, 0 REBOOT_ONLY, 0 UNSUPPORTED_TRIGGER
Log coverage: 2026-10-20T03:00:01+02:00[Europe/Amsterdam] .. 2026-10-22T03:00:00+02:00[Europe/Amsterdam]
Window: 2026-10-20T02:00:00+02:00[Europe/Amsterdam] .. 2026-10-21T02:00:00+02:00[Europe/Amsterdam] (tz Europe/Amsterdam)
Tolerance: 300s
";
    assert_eq!(stdout, expected);
    assert_no_dutch_report_labels(&stdout);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn too_many_broken_lines_truncate_the_english_report() {
    // One empty log warning plus 60 unparseable crontab lines is past the
    // 50-line text cap. The overflow line has to stay English.
    let root = empty_root();
    let crontab = root.join("big.crontab");
    let syslog = root.join("empty.log");
    let mut body = String::new();
    for i in 0..60 {
        body.push_str(&format!("not a valid cron line {i}\n"));
    }
    // A named crontab with no job at all is a usage error, so one valid
    // line follows the 60 broken ones (their line numbers stay 1..=60).
    body.push_str("0 3 * * * root /bin/backup.sh\n");
    std::fs::write(&crontab, body).unwrap();
    std::fs::write(&syslog, "").unwrap();
    let assert = Command::cargo_bin("cron-ledger")
        .unwrap()
        .arg("--system-root")
        .arg(&root)
        .arg("--crontab")
        .arg(&crontab)
        .arg("--syslog")
        .arg(&syslog)
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--until")
        .arg("2026-10-21T00:00:00Z")
        .arg("--tz")
        .arg("UTC")
        .assert()
        .code(3);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let shown = format!("{}:49:", crontab.display());
    let hidden = format!("{}:50:", crontab.display());
    assert_eq!(stdout.matches("\n  - ").count(), 50, "{stdout}");
    assert!(
        stdout.contains("not enough schedule fields, or missing command"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&shown),
        "last shown parse warning missing:\n{stdout}"
    );
    assert!(
        !stdout.contains(&hidden),
        "warning past the cap was printed:\n{stdout}"
    );
    assert!(stdout.contains("… and 11 more\n"), "{stdout}");
    assert!(stdout.contains("Warnings:\n"), "{stdout}");
    assert!(stdout.contains("Summary: 0 OK,"), "{stdout}");
    assert!(stdout.contains("Log coverage: no logs\n"), "{stdout}");
    assert_no_dutch_report_labels(&stdout);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn broken_crontab_and_syslog_still_report_in_english() {
    let root = empty_root();
    let crontab = root.join("broken.crontab");
    let syslog = root.join("broken.log");
    let mut crontab_body = String::from("0 3 * * * root /usr/local/bin/backup.sh");
    crontab_body.push('\u{1b}');
    crontab_body.push_str(
        "\nthis is not a valid cron line at all\n\
         60 0 * * * root /bin/bad-minute\n\
         @notaspecial root /bin/nope\n",
    );
    std::fs::write(&crontab, crontab_body).unwrap();
    let mut syslog_bytes =
        b"Oct 20 03:00:01 web1 CRON[1]: (root) CMD (/usr/local/bin/backup.sh".to_vec();
    syslog_bytes.push(0x1b);
    syslog_bytes.extend_from_slice(
        b")\nOct 20 03:05:00 web1 CRON[2]: no parens here at all\n\
NotAMonth 99 99:99:99 web1 CRON[4]: (root) CMD (/usr/local/bin/backup.sh)\n\xff\n",
    );
    std::fs::write(&syslog, syslog_bytes).unwrap();
    let assert = Command::cargo_bin("cron-ledger")
        .unwrap()
        .arg("--system-root")
        .arg(&root)
        .arg("--crontab")
        .arg(&crontab)
        .arg("--syslog")
        .arg(&syslog)
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--until")
        .arg("2026-10-21T00:00:00Z")
        .arg("--tz")
        .arg("UTC")
        .assert()
        .code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        stdout.contains("expected: 1, found: 1, missed: 0, late: 0"),
        "{stdout}"
    );
    assert!(stdout.contains("last run:"), "{stdout}");
    assert!(stdout.contains("(OK)"), "{stdout}");
    assert!(stdout.contains("\\u{1b}"), "{stdout}");
    assert!(
        !stdout.contains('\u{1b}'),
        "raw ESC from the command leaked into the report:\n{stdout}"
    );
    assert!(
        stdout.contains("minute: 'this' is not a number"),
        "{stdout}"
    );
    assert!(
        stdout.contains("minute: '60' (60) is out of range 0-59"),
        "{stdout}"
    );
    assert!(
        stdout.contains("unknown special string '@notaspecial'"),
        "{stdout}"
    );
    assert!(
        stdout.contains("line has no '(user) ACTION (arg)' body"),
        "{stdout}"
    );
    assert!(
        stdout.contains("'NotAMonth' is not a month name"),
        "{stdout}"
    );
    assert!(stdout.contains("invalid UTF-8 at byte "), "{stdout}");
    assert!(stdout.contains(
        "Summary: 1 OK, 0 LATE, 0 LATE (caught up), 0 MISSED, 0 NO_TRACE, 0 REBOOT_ONLY, 0 UNSUPPORTED_TRIGGER"
    ));
    assert!(stdout.contains("Warnings:\n"), "{stdout}");
    assert!(!stdout.contains("… and "), "{stdout}");
    assert_no_dutch_report_labels(&stdout);
    std::fs::remove_dir_all(&root).ok();
}

const H1_WINDOW: [&str; 6] = [
    "--since",
    "2026-10-20T00:00:00Z",
    "--until",
    "2026-10-21T00:00:00Z",
    "--tz",
    "UTC",
];

const H1_OTHER_LOG: &str = "Oct 20 12:00:00 h CRON[1]: (root) CMD (/bin/other)\n";

fn h1_cmd(root: &Path) -> Command {
    let mut cmd = Command::cargo_bin("cron-ledger").unwrap();
    cmd.arg("--system-root").arg(root).args(H1_WINDOW);
    cmd
}

fn h1_stderr(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&assert.get_output().stderr).into_owned()
}

#[test]
fn timer_unit_with_a_utf8_bom_keeps_its_timer_section() {
    let root = empty_root();
    let timer = root.join("bomt.timer");
    let log = root.join("other.log");
    std::fs::write(
        &timer,
        b"\xef\xbb\xbf[Timer]\nOnCalendar=*-*-* 03:00:00\nUnit=d.service\n",
    )
    .unwrap();
    std::fs::write(&log, H1_OTHER_LOG).unwrap();
    let assert = h1_cmd(&root)
        .arg("--timer-unit")
        .arg(&timer)
        .arg("--syslog")
        .arg(&log)
        .assert()
        .code(3);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(stdout.contains("expected: 1"), "{stdout}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn utf16_crontab_is_refused_with_exit_two() {
    let root = empty_root();
    let crontab = root.join("utf16.cron");
    let mut bytes = vec![0xff, 0xfe];
    for unit in "0 3 * * * root /bin/backup.sh\n".encode_utf16() {
        bytes.extend(unit.to_le_bytes());
    }
    std::fs::write(&crontab, bytes).unwrap();
    let assert = h1_cmd(&root)
        .arg("--crontab")
        .arg(&crontab)
        .assert()
        .code(2);
    let stderr = h1_stderr(&assert);
    assert!(stderr.contains("utf16.cron"), "{stderr}");
    assert!(stderr.contains("UTF-16"), "{stderr}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn utf16_crontab_without_a_bom_is_refused_for_its_nul_bytes() {
    let root = empty_root();
    let crontab = root.join("nobom.cron");
    let mut bytes = Vec::new();
    for unit in "0 3 * * * root /bin/backup.sh\n".encode_utf16() {
        bytes.extend(unit.to_le_bytes());
    }
    std::fs::write(&crontab, bytes).unwrap();
    let assert = h1_cmd(&root)
        .arg("--crontab")
        .arg(&crontab)
        .assert()
        .code(2);
    assert!(h1_stderr(&assert).contains("NUL byte"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn nonexistent_system_root_is_a_usage_error_exit_two() {
    let root = empty_root();
    let log = root.join("other.log");
    std::fs::write(&log, H1_OTHER_LOG).unwrap();
    let missing = root.join("does-not-exist");
    let assert = h1_cmd(&missing).arg("--syslog").arg(&log).assert().code(2);
    let stderr = h1_stderr(&assert);
    assert!(stderr.contains("--system-root"), "{stderr}");
    assert!(stderr.contains("does-not-exist"), "{stderr}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn crontab_with_only_invalid_lines_is_a_usage_error_exit_two() {
    let root = empty_root();
    let crontab = root.join("inv.cron");
    let log = root.join("other.log");
    std::fs::write(&crontab, "garbage line here\n").unwrap();
    std::fs::write(&log, H1_OTHER_LOG).unwrap();
    let assert = h1_cmd(&root)
        .arg("--crontab")
        .arg(&crontab)
        .arg("--syslog")
        .arg(&log)
        .assert()
        .code(2);
    let stderr = h1_stderr(&assert);
    assert!(stderr.contains("inv.cron: no jobs found"), "{stderr}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn empty_stdin_crontab_is_a_usage_error_exit_two() {
    let root = empty_root();
    let assert = h1_cmd(&root)
        .arg("--crontab")
        .arg("-")
        .write_stdin("")
        .assert()
        .code(2);
    let stderr = h1_stderr(&assert);
    assert!(stderr.contains("<stdin>: no jobs found"), "{stderr}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn empty_crontab_file_is_a_usage_error_exit_two() {
    let root = empty_root();
    let crontab = root.join("empty.crontab");
    std::fs::write(&crontab, "").unwrap();
    let assert = h1_cmd(&root)
        .arg("--crontab")
        .arg(&crontab)
        .assert()
        .code(2);
    assert!(h1_stderr(&assert).contains("empty.crontab: no jobs found"));
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_argument_is_a_usage_error_not_a_panic() {
    use std::os::unix::ffi::OsStrExt;
    let assert = Command::cargo_bin("cron-ledger")
        .unwrap()
        .arg("--crontab")
        .arg(std::ffi::OsStr::from_bytes(b"ok.cron\xff"))
        .args(["--since", "1d"])
        .assert()
        .code(2);
    assert!(h1_stderr(&assert).contains("ok.cron"));
}

#[cfg(target_os = "linux")]
#[test]
fn existing_non_utf8_file_name_is_refused_as_such() {
    use std::os::unix::ffi::OsStrExt;
    let root = empty_root();
    let name = std::ffi::OsStr::from_bytes(b"ok.cron\xff");
    let path = root.join(name);
    std::fs::write(&path, "0 3 * * * root /bin/backup.sh\n").unwrap();
    let assert = Command::cargo_bin("cron-ledger")
        .unwrap()
        .arg("--crontab")
        .arg(&path)
        .args(["--since", "1d"])
        .assert()
        .code(2);
    let stderr = h1_stderr(&assert);
    assert!(stderr.contains("not valid UTF-8"), "{stderr}");
    assert!(!stderr.contains("No such file"), "{stderr}");
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(unix)]
#[test]
fn control_characters_in_a_path_are_escaped_in_error_messages() {
    let root = empty_root();
    let crontab = root.join("e\x1b[31m\x1b]0;pwn\x07.cron");
    std::fs::write(&crontab, "").unwrap();
    let assert = h1_cmd(&root)
        .arg("--crontab")
        .arg(&crontab)
        .assert()
        .code(2);
    let stderr = h1_stderr(&assert);
    assert!(
        stderr.contains("e\\u{1b}[31m\\u{1b}]0;pwn\\u{7}.cron: no jobs found"),
        "{stderr}"
    );
    assert!(
        !stderr.contains('\x1b') && !stderr.contains('\x07'),
        "{stderr:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(unix)]
#[test]
fn newline_in_a_path_cannot_forge_a_second_stderr_line() {
    let root = empty_root();
    let crontab = root.join("nope\ncron-ledger: all 12 jobs OK.cron");
    std::fs::write(&crontab, "").unwrap();
    let assert = h1_cmd(&root)
        .arg("--crontab")
        .arg(&crontab)
        .assert()
        .code(2);
    let stderr = h1_stderr(&assert);
    assert_eq!(stderr.lines().count(), 1, "{stderr:?}");
    assert!(
        stderr.contains("nope\\u{a}cron-ledger: all 12 jobs OK"),
        "{stderr:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(unix)]
#[test]
fn newline_in_a_scanned_file_name_cannot_forge_a_stderr_line() {
    let root = empty_root();
    let d = root.join("var/spool/cron/crontabs");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("a\ncron-ledger: all 12 jobs OK"), b"\0\0").unwrap();
    let assert = h1_cmd(&root).assert().code(2);
    let stderr = h1_stderr(&assert);
    let lines: Vec<&str> = stderr.lines().collect();
    assert!(
        lines[0].starts_with("cron-ledger: --system-root"),
        "{stderr:?}"
    );
    assert!(lines.len() >= 2, "{stderr:?}");
    for l in &lines[1..] {
        assert!(l.starts_with("  - "), "forged line {l:?} in {stderr:?}");
    }
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(target_os = "linux")]
#[test]
fn spool_file_with_a_non_utf8_name_is_skipped_with_a_warning() {
    use std::os::unix::ffi::OsStrExt;
    let root = empty_root();
    let d = root.join("var/spool/cron/crontabs");
    std::fs::create_dir_all(&d).unwrap();
    let name = std::ffi::OsStr::from_bytes(b"al\xffice");
    std::fs::write(d.join(name), "0 12 * * * /bin/other\n").unwrap();
    let log = root.join("other.log");
    std::fs::write(&log, H1_OTHER_LOG).unwrap();
    let assert = h1_cmd(&root).arg("--syslog").arg(&log).assert();
    let out = assert.get_output();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        !stdout.contains("/bin/other"),
        "no job for user \"\": {stdout}"
    );
    assert!(
        stdout.contains("file name is not valid UTF-8, so its user is unknown; skipped"),
        "{stdout}"
    );
    std::fs::remove_dir_all(&root).ok();
}

fn timer_run(root: &Path, body: &str) -> assert_cmd::assert::Assert {
    let timer = root.join("g.timer");
    std::fs::write(&timer, body).unwrap();
    let log = root.join("other.log");
    std::fs::write(&log, H1_OTHER_LOG).unwrap();
    h1_cmd(root)
        .arg("--timer-unit")
        .arg(&timer)
        .arg("--syslog")
        .arg(&log)
        .assert()
}

#[test]
fn timer_with_only_an_invalid_oncalendar_is_a_usage_error_exit_two() {
    let root = empty_root();
    let assert = timer_run(&root, "[Timer]\nOnCalendar=garbage\n").code(2);
    let stderr = h1_stderr(&assert);
    assert!(stderr.contains("g.timer: no jobs found"), "{stderr}");
    assert!(stderr.contains("garbage"), "{stderr}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn timer_with_an_empty_oncalendar_is_a_usage_error_exit_two() {
    let root = empty_root();
    timer_run(&root, "[Timer]\nOnCalendar=\n").code(2);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn timer_with_one_valid_and_one_invalid_oncalendar_still_audits() {
    let root = empty_root();
    let assert = timer_run(
        &root,
        "[Timer]\nOnCalendar=garbage\nOnCalendar=*-*-* 03:00:00\n",
    )
    .code(3);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(stdout.contains("garbage"), "{stdout}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn empty_report_with_no_jobs_and_empty_syslog_is_in_english() {
    let root = empty_root();
    std::fs::create_dir_all(root.join("etc")).unwrap();
    std::fs::write(root.join("etc/crontab"), "# no jobs\n").unwrap();
    let syslog = root.join("empty.log");
    std::fs::write(&syslog, "").unwrap();
    let assert = h1_cmd(&root).arg("--syslog").arg(&syslog).assert().code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(stdout.starts_with("(no jobs found)\n"), "{stdout}");
    assert!(!stdout.contains("expected:"), "{stdout}");
    assert!(
        stdout.contains(
            "Summary: 0 OK, 0 LATE, 0 LATE (caught up), 0 MISSED, 0 NO_TRACE, 0 REBOOT_ONLY, 0 UNSUPPORTED_TRIGGER"
        ),
        "{stdout}"
    );
    assert!(stdout.contains("Log coverage: no logs\n"), "{stdout}");
    assert!(stdout.contains("Window: "), "{stdout}");
    assert!(stdout.contains("Tolerance: 300s\n"), "{stdout}");
    assert!(
        stdout.contains("Warnings:\n  - no log events found in the window:"),
        "{stdout}"
    );
    assert_no_dutch_report_labels(&stdout);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn empty_system_root_scan_warns_and_exits_zero() {
    let root = empty_root();
    let assert = h1_cmd(&root).assert().code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(stdout.contains("(no jobs found)"), "{stdout}");
    assert!(stdout.contains("no crontab file found"), "{stdout}");
    std::fs::remove_dir_all(&root).ok();
}

// Windows reports system-root paths with different separators; unix only.
#[cfg(unix)]
#[test]
fn system_root_scan_with_only_unreadable_files_is_a_usage_error_exit_two() {
    let root = empty_root();
    std::fs::create_dir_all(root.join("etc/cron.d")).unwrap();
    std::fs::write(root.join("etc/crontab"), b"\0\0").unwrap();
    let mut utf16 = vec![0xff, 0xfe];
    for unit in "0 3 * * * root /bin/backup.sh\n".encode_utf16() {
        utf16.extend(unit.to_le_bytes());
    }
    std::fs::write(root.join("etc/cron.d/u16"), utf16).unwrap();
    let assert = h1_cmd(&root).assert().code(2);
    let stderr = h1_stderr(&assert);
    assert!(stderr.contains("no jobs found"), "{stderr}");
    assert!(stderr.contains("etc/crontab"), "{stderr}");
    assert!(stderr.contains("etc/cron.d/u16"), "{stderr}");
    std::fs::remove_dir_all(&root).ok();
}

// Same path-separator assumption as the unreadable-only scan; unix only.
#[cfg(unix)]
#[test]
fn system_root_with_one_unreadable_and_one_readable_file_still_audits() {
    let root = empty_root();
    std::fs::create_dir_all(root.join("etc/cron.d")).unwrap();
    std::fs::write(root.join("etc/crontab"), b"\0\0").unwrap();
    std::fs::write(root.join("etc/cron.d/ok"), "0 12 * * * root /bin/other\n").unwrap();
    let log = root.join("other.log");
    std::fs::write(&log, H1_OTHER_LOG).unwrap();
    let assert = h1_cmd(&root).arg("--syslog").arg(&log).assert().code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(stdout.contains("etc/crontab"), "{stdout}");
    assert!(stdout.contains("expected: 1"), "{stdout}");
    std::fs::remove_dir_all(&root).ok();
}

const H1_NUL_LOG: &[u8] = b"Oct 20 12:00:00 h CRON[1]: (root) CMD (/bin/other)\n\
Oct 20 13:00:00 h CRON[2]: (root) CMD (/bin/x\0\0\0)\n\
Oct 20 14:00:00 h CRON[3]: (root) CMD (/bin/after)\n";

fn nul_log_run(root: &Path, log: &Path) -> (assert_cmd::assert::Assert, String) {
    let crontab = root.join("nul.cron");
    std::fs::write(
        &crontab,
        "0 12 * * * root /bin/other\n0 14 * * * root /bin/after\n",
    )
    .unwrap();
    let assert = h1_cmd(root)
        .arg("--crontab")
        .arg(&crontab)
        .arg("--syslog")
        .arg(log)
        .assert();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    (assert, stdout)
}

#[test]
fn nul_in_the_middle_of_a_syslog_skips_only_that_line() {
    let root = empty_root();
    let log = root.join("nul.log");
    std::fs::write(&log, H1_NUL_LOG).unwrap();
    let (assert, stdout) = nul_log_run(&root, &log);
    assert.code(0);
    assert!(stdout.contains("2 OK"), "{stdout}");
    assert!(
        stdout.contains("1 line(s) with a NUL byte skipped"),
        "{stdout}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn nul_in_the_middle_of_a_gz_syslog_skips_only_that_line() {
    use std::io::Write;
    let root = empty_root();
    let log = root.join("nul.log.gz");
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(H1_NUL_LOG).unwrap();
    std::fs::write(&log, encoder.finish().unwrap()).unwrap();
    let (assert, stdout) = nul_log_run(&root, &log);
    assert.code(0);
    assert!(stdout.contains("2 OK"), "{stdout}");
    // The warning is what tells the gz path from the lossy read: without
    // the NUL handling the line is kept (lossily) and nothing is reported.
    assert!(
        stdout.contains("1 line(s) with a NUL byte skipped"),
        "{stdout}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn utf16_syslog_without_a_bom_is_still_refused_for_its_nul_bytes() {
    let root = empty_root();
    let log = root.join("u16.log");
    let mut bytes = Vec::new();
    for unit in String::from_utf8_lossy(H1_OTHER_LOG.as_bytes()).encode_utf16() {
        bytes.extend(unit.to_le_bytes());
    }
    std::fs::write(&log, bytes).unwrap();
    let (assert, _) = nul_log_run(&root, &log);
    let stderr = h1_stderr(&assert.code(2));
    assert!(stderr.contains("u16.log"), "{stderr}");
    assert!(stderr.contains("NUL"), "{stderr}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn timers_list_fixture_puts_the_last_run_in_the_report() {
    let list = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/systemd/list-timers.txt");
    let assert = Command::cargo_bin("cron-ledger")
        .unwrap()
        .arg("--system-root")
        .arg("/nonexistent/cron-ledger-timers-list-root")
        .arg("--timers-list")
        .arg(&list)
        .arg("--since")
        .arg("2026-10-23T00:00:00Z")
        .arg("--until")
        .arg("2026-10-25T00:00:00Z")
        .arg("--tz")
        .arg("UTC")
        .assert()
        .code(1);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        stdout.contains("list-timers LAST 2026-10-24T00:00:00Z NEXT 2026-10-25T00:00:00Z"),
        "{stdout}"
    );
    assert!(
        stdout.contains("last run: 2026-10-24T00:00:00+00:00[UTC] (OK)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("last run: 2026-10-23T06:00:00+00:00[UTC] (MISSED)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("first missed times: 2026-10-24T12:00:00+00:00[UTC]"),
        "{stdout}"
    );
    assert!(stdout.contains("found: 0, missed: 1"), "{stdout}");
    assert!(!stdout.contains("idle.timer ("), "{stdout}");
    assert!(!stdout.contains("pending.timer ("), "{stdout}");
    assert!(stdout.contains("give nothing to check"), "{stdout}");
    assert!(!stdout.contains("not implemented"), "{stdout}");
    assert!(!stdout.contains("no log events found"), "{stdout}");
    assert!(!stdout.contains('\x1b'), "{stdout}");
}
