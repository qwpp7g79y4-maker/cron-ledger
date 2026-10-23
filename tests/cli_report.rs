//! CLI-level exit-code tests, driving the real `cron-ledger` binary
//! via `assert_cmd`, one per exit-code scenario from the README:
//! 0 (all OK), 1 (MISSED/LATE present), 2 (usage/parse error), 3 (only
//! NO_TRACE).
//!
//! Every invocation pins `--system-root` to an empty, isolated directory
//! so the real host's `/etc/crontab` etc. (which may or may not exist,
//! and is outside this project's control) never leaks into the result.

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
    assert!(stdout.contains("verwacht: 0"), "{stdout}");
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
