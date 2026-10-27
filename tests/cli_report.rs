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
fn empty_crontab_and_empty_syslog_report_in_english() {
    let root = empty_root();
    let crontab = root.join("empty.crontab");
    let syslog = root.join("empty.log");
    std::fs::write(&crontab, "").unwrap();
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
        .code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(stdout.starts_with("(no jobs found)\n"), "{stdout}");
    assert!(!stdout.contains("expected:"), "{stdout}");
    assert!(stdout.contains(
        "Summary: 0 OK, 0 LATE, 0 LATE (caught up), 0 MISSED, 0 NO_TRACE, 0 REBOOT_ONLY, 0 UNSUPPORTED_TRIGGER"
    ));
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
        .code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let shown = format!("{}:49:", crontab.display());
    let hidden = format!("{}:50:", crontab.display());
    assert!(stdout.starts_with("(no jobs found)\n"), "{stdout}");
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

/// A crontab or timer unit with a non-UTF-8 byte is read lossily: a
/// warning names the file, and the valid lines still count.
fn bad_utf8_crontab() -> Vec<u8> {
    let mut bytes = b"# caf".to_vec();
    bytes.push(0xe9);
    bytes.extend(b"\n0 3 * * * root /usr/local/bin/backup.sh\n");
    bytes
}

fn lossy_cmd(root: &Path) -> Command {
    let mut cmd = Command::cargo_bin("cron-ledger").unwrap();
    cmd.arg("--system-root")
        .arg(root)
        .arg("--syslog")
        .arg(fixture("ok").join("syslog"))
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--until")
        .arg("2026-10-21T00:00:00Z");
    cmd
}

fn combined_output(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn crontab_file_with_a_non_utf8_byte_is_a_warning_not_exit_two() {
    let root = empty_root();
    let path = root.join("bin.dat");
    std::fs::write(&path, bad_utf8_crontab()).unwrap();
    let assert = lossy_cmd(&root)
        .arg("--crontab")
        .arg(&path)
        .assert()
        .code(0);
    let text = combined_output(assert.get_output());
    assert!(text.contains("bin.dat"), "{text}");
    assert!(text.contains("invalid UTF-8"), "{text}");
    assert!(text.contains("backup.sh"), "{text}");
    assert!(text.contains("OK"), "{text}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn crontab_on_stdin_with_a_non_utf8_byte_is_a_warning_not_exit_two() {
    let root = empty_root();
    let assert = lossy_cmd(&root)
        .arg("--crontab")
        .arg("-")
        .write_stdin(bad_utf8_crontab())
        .assert()
        .code(0);
    let text = combined_output(assert.get_output());
    assert!(text.contains("<stdin>"), "{text}");
    assert!(text.contains("invalid UTF-8"), "{text}");
    assert!(text.contains("backup.sh"), "{text}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn timer_unit_with_a_non_utf8_byte_is_a_warning_not_exit_two() {
    let root = empty_root();
    let path = root.join("backup.timer");
    let mut bytes = b"[Unit]\nDescription=caf".to_vec();
    bytes.push(0xe9);
    bytes.extend(b"\n[Timer]\nOnCalendar=*-*-* 03:00:00\n");
    std::fs::write(&path, bytes).unwrap();
    let output = lossy_cmd(&root)
        .arg("--timer-unit")
        .arg(&path)
        .output()
        .unwrap();
    assert_ne!(output.status.code(), Some(2), "{output:?}");
    let text = combined_output(&output);
    assert!(text.contains("backup.timer"), "{text}");
    assert!(text.contains("invalid UTF-8"), "{text}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn missing_or_directory_crontab_is_still_exit_two() {
    let root = empty_root();
    lossy_cmd(&root)
        .arg("--crontab")
        .arg(root.join("does-not-exist"))
        .assert()
        .code(2);
    lossy_cmd(&root)
        .arg("--crontab")
        .arg(&root)
        .assert()
        .code(2);
    std::fs::remove_dir_all(&root).ok();
}
