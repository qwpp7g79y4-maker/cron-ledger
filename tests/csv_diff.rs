//! End-to-end tests for `--csv` and `--diff` on the built binary.
//! Inputs are the committed report fixtures and files in a temp dir.

use assert_cmd::Command;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/report")
        .join(name)
}

fn scratch(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("cron-ledger-csvdiff-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn audit(scenario: &str, root: &Path, extra: &[&str]) -> Command {
    audit_until(scenario, root, "2026-10-22T00:00:00Z", extra)
}

fn audit_until(scenario: &str, root: &Path, until: &str, extra: &[&str]) -> Command {
    let dir = fixture(scenario);
    let mut cmd = Command::cargo_bin("cron-ledger").unwrap();
    cmd.arg("--system-root")
        .arg(root)
        .arg("--tz")
        .arg("UTC")
        .arg("--crontab")
        .arg(dir.join("crontab"))
        .arg("--syslog")
        .arg(dir.join("syslog"))
        .args(["--since", "2026-10-20T00:00:00Z", "--until", until])
        .args(extra);
    cmd
}

fn stdout_of(cmd: &mut Command, code: i32) -> String {
    let out = cmd.assert().code(code).get_output().clone();
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn csv_has_header_and_one_row_per_run() {
    let root = scratch("csv");
    let csv = stdout_of(&mut audit("missed", &root, &["--csv"]), 1);
    let mut lines = csv.lines();
    assert_eq!(
        lines.next().unwrap(),
        "job,file,line,expected,status,actual,late_by_secs,exit_status"
    );
    let rows: Vec<&str> = lines.collect();
    assert_eq!(rows.len(), 2, "two days, two expected runs: {csv}");
    for row in rows {
        assert!(row.starts_with("root: /usr/local/bin/backup.sh,"), "{row}");
        assert_eq!(row.split(',').count(), 8, "{row}");
    }
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn csv_ok_scenario_exit_code_is_unchanged() {
    let root = scratch("csvok");
    let csv = stdout_of(
        &mut audit_until("ok", &root, "2026-10-21T00:00:00Z", &["--csv"]),
        0,
    );
    assert!(csv.contains(",Ok,"), "{csv}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn csv_and_json_together_exit_two() {
    let root = scratch("both");
    let out = audit("ok", &root, &["--csv", "--json"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(String::from_utf8_lossy(&out.stderr).contains("--csv"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn diff_of_a_report_with_itself_is_clean() {
    let root = scratch("same");
    let json = stdout_of(&mut audit("missed", &root, &["--json"]), 1);
    let file = root.join("a.json");
    std::fs::write(&file, &json).unwrap();
    let mut cmd = Command::cargo_bin("cron-ledger").unwrap();
    let out = stdout_of(cmd.arg("--diff").arg(&file).arg(&file), 0);
    assert_eq!(out, "no differences\n");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn diff_reports_a_status_change_with_exit_one() {
    let root = scratch("chg");
    let ok = stdout_of(
        &mut audit_until("ok", &root, "2026-10-21T00:00:00Z", &["--json"]),
        0,
    );
    let missed = stdout_of(
        &mut audit_until("missed", &root, "2026-10-21T00:00:00Z", &["--json"]),
        1,
    );
    let (a, b) = (root.join("ok.json"), root.join("missed.json"));
    std::fs::write(&a, ok).unwrap();
    std::fs::write(&b, missed).unwrap();
    let mut cmd = Command::cargo_bin("cron-ledger").unwrap();
    let out = stdout_of(cmd.arg("--diff").arg(&a).arg(&b), 1);
    assert!(
        out.contains("Ok -> Missed") || out.contains("-> Missed"),
        "{out}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn diff_with_unreadable_or_invalid_input_exits_two() {
    let root = scratch("bad");
    let junk = root.join("junk.json");
    std::fs::write(&junk, "not json").unwrap();
    let mut cmd = Command::cargo_bin("cron-ledger").unwrap();
    cmd.arg("--diff").arg(&junk).arg(&junk).assert().code(2);
    let mut cmd = Command::cargo_bin("cron-ledger").unwrap();
    cmd.arg("--diff")
        .arg(root.join("missing.json"))
        .arg(&junk)
        .assert()
        .code(2);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn version_flags_still_work() {
    for flag in ["--version", "-V"] {
        let mut cmd = Command::cargo_bin("cron-ledger").unwrap();
        let out = stdout_of(cmd.arg(flag), 0);
        assert!(out.starts_with("cron-ledger 1.0.0"), "{out}");
        assert!(out.ends_with('\n'), "{out}");
    }
}
