//! W-24: every hard error says what went wrong and what to do next, and
//! exits 2 (the existing exit code; nothing is renumbered). One test per
//! error path of the binary.

use assert_cmd::Command;
use std::path::PathBuf;

/// A fresh temp dir per test name, so parallel tests never share files.
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cron-ledger-w24-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Run with `args` (tz fixed to UTC, host root never scanned unless the
/// test names one) and return stderr; asserts exit 2, empty stdout, no panic.
fn fails(args: &[&str]) -> String {
    let out = Command::cargo_bin("cron-ledger")
        .unwrap()
        .args(args)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(2), "stderr: {stderr}");
    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(stderr.starts_with("cron-ledger: "), "{stderr}");
    stderr
}

fn has(stderr: &str, what: &str, next: &str) {
    assert!(stderr.contains(what), "missing what {what:?}: {stderr}");
    assert!(
        stderr.contains(next),
        "missing next step {next:?}: {stderr}"
    );
}

fn p(path: &std::path::Path) -> String {
    path.to_str().unwrap().to_string()
}

#[test]
fn missing_since() {
    let e = fails(&["--tz", "UTC"]);
    has(&e, "--since is required", "for example --since 30d");
}

#[test]
fn flag_without_value() {
    let e = fails(&["--since"]);
    has(&e, "--since requires a value", "cron-ledger --help");
}

#[test]
fn unknown_argument() {
    let e = fails(&["--since", "30d", "--bogus"]);
    has(&e, "unknown argument '--bogus'", "cron-ledger --help");
}

#[test]
fn crontab_user_without_equals() {
    let e = fails(&["--since", "30d", "--crontab-user", "alice"]);
    has(
        &e,
        "expects USER=PATH, got 'alice'",
        "--crontab-user alice=/path/to/crontab",
    );
}

#[test]
fn bad_tolerance() {
    let e = fails(&["--since", "30d", "--tolerance", "soon"]);
    has(&e, "--tolerance:", "such as 90s or 5min");
}

#[test]
fn json_and_csv_together() {
    let e = fails(&["--since", "30d", "--json", "--csv"]);
    has(
        &e,
        "--json and --csv cannot be combined",
        "pick one output format",
    );
}

#[test]
fn two_stdin_sources() {
    let e = fails(&["--since", "30d", "--crontab", "-", "--syslog", "-"]);
    has(
        &e,
        "at most one source may read from stdin",
        "pass their paths",
    );
}

#[test]
fn diff_with_another_option() {
    let e = fails(&["--diff", "a.json", "b.json", "--json"]);
    has(
        &e,
        "--diff takes exactly two report files",
        "cron-ledger --diff OLD.json NEW.json",
    );
}

#[test]
fn diff_unreadable_file() {
    let d = dir("diff-unreadable");
    let missing = p(&d.join("missing.json"));
    let e = fails(&["--diff", &missing, &missing]);
    has(
        &e,
        "cannot read",
        "check that the path exists and is readable",
    );
}

#[test]
fn diff_not_json() {
    let d = dir("diff-notjson");
    let f = d.join("bad.json");
    std::fs::write(&f, "not json").unwrap();
    let f = p(&f);
    let e = fails(&["--diff", &f, &f]);
    has(&e, "is not a JSON report", "written by cron-ledger --json");
}

#[test]
fn diff_wrong_schema_version() {
    let d = dir("diff-schema");
    let f = d.join("v2.json");
    std::fs::write(&f, r#"{"schema_version":2,"jobs":[]}"#).unwrap();
    let f = p(&f);
    let e = fails(&["--diff", &f, &f]);
    has(&e, "schema_version Some(2), expected 1", "regenerate it");
}

#[test]
fn diff_report_without_jobs() {
    let d = dir("diff-nojobs");
    let f = d.join("nojobs.json");
    std::fs::write(&f, r#"{"schema_version":1}"#).unwrap();
    let f = p(&f);
    let e = fails(&["--diff", &f, &f]);
    has(&e, "has no \"jobs\" list", "written by cron-ledger --json");
}

#[test]
fn unparsable_since() {
    let d = dir("since");
    let e = fails(&["--since", "yesterday", "--system-root", &p(&d)]);
    has(
        &e,
        "'yesterday' is not an RFC3339 timestamp",
        "try --since 30d",
    );
}

#[test]
fn since_out_of_range() {
    let d = dir("since-range");
    let e = fails(&["--since", "9999999999999d", "--system-root", &p(&d)]);
    has(&e, "is out of range", "use a shorter duration");
}

#[test]
fn unknown_timezone() {
    let d = dir("tz");
    let e = fails(&[
        "--since",
        "30d",
        "--tz",
        "Not/AZone",
        "--system-root",
        &p(&d),
    ]);
    has(
        &e,
        "unknown timezone 'Not/AZone'",
        "such as Europe/Amsterdam or UTC",
    );
}

#[test]
fn until_before_since() {
    let d = dir("until");
    let e = fails(&["--since", "30d", "--until", "60d", "--system-root", &p(&d)]);
    has(
        &e,
        "is earlier than --since",
        "swap them or choose a later --until",
    );
}

#[test]
fn system_root_not_a_directory() {
    let d = dir("root");
    let missing = p(&d.join("nope"));
    let e = fails(&["--since", "30d", "--system-root", &missing]);
    has(&e, "not a directory", "or drop --system-root");
}

#[test]
fn crontab_unreadable() {
    let d = dir("cron-unreadable");
    let missing = p(&d.join("missing.cron"));
    let e = fails(&["--since", "30d", "--crontab", &missing]);
    has(
        &e,
        "cannot read",
        "check that the path exists and is readable",
    );
}

#[test]
fn crontab_not_utf8() {
    let d = dir("cron-utf16");
    let f = d.join("utf16.cron");
    std::fs::write(&f, [0xff, 0xfe, b'*', 0]).unwrap();
    let e = fails(&["--since", "30d", "--crontab", &p(&f)]);
    has(&e, "only UTF-8 is supported", "iconv -f UTF-16 -t UTF-8");
}

#[test]
fn crontab_without_jobs() {
    let d = dir("cron-empty");
    let f = d.join("empty.cron");
    std::fs::write(&f, "# nothing\n").unwrap();
    let e = fails(&["--since", "30d", "--crontab", &p(&f)]);
    has(
        &e,
        "empty.cron: no jobs found",
        "at least one schedule line",
    );
}

#[test]
fn timer_unit_without_trigger() {
    let d = dir("timer-empty");
    let f = d.join("x.timer");
    std::fs::write(&f, "[Unit]\nDescription=x\n").unwrap();
    let e = fails(&["--since", "30d", "--timer-unit", &p(&f)]);
    has(&e, "x.timer: no jobs found", "pass a .timer unit file");
}

#[test]
fn timer_unit_with_invalid_calendar() {
    let d = dir("timer-badcal");
    let f = d.join("y.timer");
    std::fs::write(&f, "[Timer]\nOnCalendar=not a calendar\n").unwrap();
    let e = fails(&["--since", "30d", "--timer-unit", &p(&f)]);
    has(
        &e,
        "y.timer: no jobs found (no valid OnCalendar=",
        "fix the OnCalendar= value",
    );
}

#[test]
fn timers_list_without_header() {
    let d = dir("timers-list");
    let f = d.join("list.txt");
    std::fs::write(&f, "hello\n").unwrap();
    let e = fails(&["--since", "30d", "--timers-list", &p(&f)]);
    has(&e, "list.txt: no jobs found", "systemctl list-timers --all");
}

#[cfg(unix)]
#[test]
fn argument_not_utf8() {
    use std::os::unix::ffi::OsStrExt;
    let out = Command::cargo_bin("cron-ledger")
        .unwrap()
        .arg(std::ffi::OsStr::from_bytes(b"--since\xff"))
        .output()
        .unwrap();
    let e = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(2), "{e}");
    has(&e, "is not valid UTF-8", "needs UTF-8 paths and arguments");
}
