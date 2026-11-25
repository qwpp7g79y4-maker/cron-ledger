//! End-to-end tests that start the `cron-ledger` binary
//! (`CARGO_BIN_EXE_cron-ledger` via `assert_cmd`).
//!
//! `--system-root` is always a temp directory created for the test, or a
//! path under that directory that is never created. Nothing here reads
//! `/`, `/etc/crontab`, or this machine's journal.

use assert_cmd::Command;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn thread_id_hash() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::thread::current().id().hash(&mut hasher);
    hasher.finish()
}

fn temp_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cron-ledger-e2e-{}-{}",
        std::process::id(),
        thread_id_hash()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

/// Isolated binary. `--tz UTC` so the process does not consult the
/// host timezone. The root is not scanned when a named job source is set.
fn bin(root: &Path) -> Command {
    let mut cmd = Command::cargo_bin("cron-ledger").unwrap();
    cmd.arg("--system-root").arg(root).arg("--tz").arg("UTC");
    cmd
}

fn text(output: &std::process::Output) -> (String, String) {
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn assert_no_panic(stderr: &str) {
    assert!(
        !stderr.contains("panicked"),
        "stderr must not contain a panic: {stderr}"
    );
}

#[test]
fn invalid_since_exits_two_with_the_readme_message() {
    // README: anything that is neither RFC3339 nor a duration exits 2
    // with this sentence. `2026-10-20` is a date with no time, so it is
    // not RFC3339, and the `-` is not a duration unit.
    let root = temp_root();
    for value in ["yesterday", "2026-10-20"] {
        let assert = bin(&root).arg("--since").arg(value).assert().code(2);
        let output = assert.get_output();
        let (stdout, stderr) = text(output);
        let message = format!("'{value}' is not an RFC3339 timestamp or a duration like '30d'");
        assert!(stderr.contains(&message), "stderr: {stderr}");
        assert_no_panic(&stderr);
        assert!(stdout.is_empty(), "stdout: {stdout}");
    }
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn duration_since_with_only_reboot_exits_zero() {
    // The other half of `<RFC3339|30d>`: a duration counted back from now.
    // `@reboot` does not expand, so the result does not depend on `now`.
    let root = temp_root();
    let cron = root.join("reboot.cron");
    std::fs::write(&cron, "@reboot /bin/true\n").unwrap();
    let assert = bin(&root)
        .arg("--crontab-user")
        .arg(format!("root={}", cron.display()))
        .arg("--since")
        .arg("2h")
        .assert()
        .code(0);
    let (stdout, stderr) = text(assert.get_output());
    assert!(stdout.contains("REBOOT_ONLY"), "{stdout}");
    assert_no_panic(&stderr);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn syslog_on_stdin_audits_the_ok_fixture() {
    let root = temp_root();
    let syslog = std::fs::read_to_string(fixture("report/ok/syslog")).unwrap();
    let assert = bin(&root)
        .arg("--crontab")
        .arg(fixture("report/ok/crontab"))
        .arg("--syslog")
        .arg("-")
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--until")
        .arg("2026-10-21T00:00:00Z")
        .write_stdin(syslog)
        .assert()
        .code(0);
    let (stdout, stderr) = text(assert.get_output());
    assert!(stdout.contains("OK"), "{stdout}");
    assert_no_panic(&stderr);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn specials_fixture_reports_reboot_only() {
    let root = temp_root();
    let cron = fixture("crontab/specials.crontab");
    let assert = bin(&root)
        .arg("--crontab-user")
        .arg(format!("alice={}", cron.display()))
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--until")
        .arg("2026-10-21T00:00:00Z")
        .assert()
        .code(3);
    let (stdout, stderr) = text(assert.get_output());
    assert!(stdout.contains("REBOOT_ONLY"), "{stdout}");
    assert_no_panic(&stderr);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn unknown_cron_tz_warns_and_still_audits() {
    let root = temp_root();
    let cron = root.join("tz.cron");
    std::fs::write(&cron, "CRON_TZ=Not/AZone\n0 3 * * * /bin/true\n").unwrap();
    let assert = bin(&root)
        .arg("--crontab-user")
        .arg(format!("root={}", cron.display()))
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--until")
        .arg("2026-10-21T00:00:00Z")
        .assert()
        .code(3);
    let (stdout, stderr) = text(assert.get_output());
    assert!(stdout.contains("unknown timezone 'Not/AZone'"), "{stdout}");
    assert_no_panic(&stderr);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn empty_crontab_user_exits_two() {
    let root = temp_root();
    let missing = root.join("no-such-crontab");
    let assert = bin(&root)
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--crontab-user")
        .arg(format!("={}", missing.display()))
        .assert()
        .code(2);
    let (stdout, stderr) = text(assert.get_output());
    assert!(
        stderr.contains("--crontab-user expects USER=PATH"),
        "{stderr}"
    );
    assert_no_panic(&stderr);
    assert!(stdout.is_empty(), "{stdout}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn reboot_line_without_a_command_is_rejected() {
    let root = temp_root();
    let cron = root.join("reboot-only.cron");
    // System crontab form needs a user and a command after `@reboot`.
    std::fs::write(&cron, "@reboot\n").unwrap();
    let assert = bin(&root)
        .arg("--crontab")
        .arg(&cron)
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--until")
        .arg("2026-10-21T00:00:00Z")
        .assert()
        .code(2);
    let (stdout, stderr) = text(assert.get_output());
    assert!(stderr.contains("missing user or command"), "{stderr}");
    assert_no_panic(&stderr);
    assert!(stdout.is_empty(), "{stdout}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn syslog_edge_lines_warn_and_do_not_panic() {
    // 2025 is not a leap year, so `Feb 29` cannot be built in the year
    // inferred from the June line and the parser tries the next year.
    let root = temp_root();
    let cron = root.join("job.cron");
    std::fs::write(&cron, "0 3 * * * root /usr/local/bin/backup.sh\n").unwrap();
    let log = root.join("edges.log");
    std::fs::write(
        &log,
        "\
Jun  1 03:00:02 host CRON[1]: (root) CMD (/usr/local/bin/backup.sh)
Feb 29 03:00:00 host CRON[7]: (root) CMD (/usr/local/bin/backup.sh)
Oct xx 03:00:00 host CRON[2]: (root) CMD (/usr/local/bin/backup.sh)
Oct 20 3 host CRON[3]: (root) CMD (/usr/local/bin/backup.sh)
Oct 20 aa:bb:cc host CRON[4]: (root) CMD (/usr/local/bin/backup.sh)
NotAMonth 20 03:00:00 host CRON[5]: (root) CMD (/usr/local/bin/backup.sh)
<134>1 not-a-time host CRON 9 - - (root) CMD (/usr/local/bin/backup.sh)
<134>1 2026-10-20T03:00:02Z host CRON 9 - [meta] (root) CMD (/usr/local/bin/backup.sh)
<134>1 2026-10-20T03:00:02Z host CRON 9 - - (root) FOO (/usr/local/bin/backup.sh)
<not-a-pri>1 2026-10-20T03:00:02Z host CRON 9 - - (root) CMD (/usr/local/bin/backup.sh)
2026-10-20T03:00:02Z
2026-10-20T03:00:02Z host CRON[8]: not a body
",
    )
    .unwrap();
    let assert = bin(&root)
        .arg("--crontab")
        .arg(&cron)
        .arg("--syslog")
        .arg(&log)
        .arg("--since")
        .arg("2025-06-01T00:00:00Z")
        .arg("--until")
        .arg("2025-06-02T00:00:00Z")
        .assert()
        .code(0);
    let (stdout, stderr) = text(assert.get_output());
    assert!(stdout.contains("OK"), "{stdout}");
    for needle in [
        "'xx' is not a day of month",
        "is not HH:MM:SS",
        "'NotAMonth' is not a month name",
        "bad RFC5424 timestamp",
        "RFC5424 structured data other than '-' is not supported",
        "has no '(user) ACTION (arg)' body",
    ] {
        assert!(stdout.contains(needle), "missing {needle} in {stdout}");
    }
    assert_no_panic(&stderr);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn journal_odd_lines_warn_and_a_matching_cmd_is_ok() {
    let root = temp_root();
    let cron = root.join("job.cron");
    std::fs::write(&cron, "0 3 * * * root /usr/local/bin/backup.sh\n").unwrap();
    let journal = root.join("journal.jsonl");
    std::fs::write(
        &journal,
        r#"true
-1
1
1.5
"x"
null
[1,2]
{"__REALTIME_TIMESTAMP":"1792465200000000","SYSLOG_IDENTIFIER":1}
{"__REALTIME_TIMESTAMP":"1792465200000000","SYSLOG_IDENTIFIER":"kernel","MESSAGE":1}
{"__REALTIME_TIMESTAMP":"1792465200000000","SYSLOG_IDENTIFIER":"cron","MESSAGE":true}
{"__REALTIME_TIMESTAMP":1792465200000000,"SYSLOG_IDENTIFIER":"CROND","_PID":"1","MESSAGE":"(root) CMDOUT (ignored)"}
{"__REALTIME_TIMESTAMP":"1792465201000000","SYSLOG_IDENTIFIER":"CROND","_PID":"1","MESSAGE":"(root) CMDEND (/usr/local/bin/backup.sh)"}
{"__REALTIME_TIMESTAMP":"1792465200000000","SYSLOG_IDENTIFIER":"CROND","_PID":"1","MESSAGE":"(root) CMD (/usr/local/bin/backup.sh)"}
"#,
    )
    .unwrap();
    let assert = bin(&root)
        .arg("--crontab")
        .arg(&cron)
        .arg("--journal-json")
        .arg(&journal)
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--until")
        .arg("2026-10-21T00:00:00Z")
        .assert()
        .code(0);
    let (stdout, stderr) = text(assert.get_output());
    assert!(stdout.contains("OK"), "{stdout}");
    assert!(
        stdout.contains("journal entry is not a JSON object"),
        "{stdout}"
    );
    assert!(
        stdout.contains("SYSLOG_IDENTIFIER is not a single string"),
        "{stdout}"
    );
    assert!(stdout.contains("cron entry has no MESSAGE"), "{stdout}");
    assert_no_panic(&stderr);
    std::fs::remove_dir_all(&root).ok();
}

fn timers_table(rows: &[(&str, &str, &str)]) -> String {
    let mut out = format!("{:<46}{:<46}{}\n", "NEXT", "LAST", "UNIT");
    for (next, last, unit) in rows {
        out.push_str(&format!("{next:<46}{last:<46}{unit}\n"));
    }
    out
}

#[test]
fn timers_list_edge_rows_keep_the_valid_timer() {
    let root = temp_root();
    let list = root.join("timers.txt");
    let next = "Sun 2026-10-25 00:00:00 UTC";
    let last = "Sat 2026-10-24 00:00:00 UTC";
    std::fs::write(
        &list,
        timers_table(&[
            (next, last, "logrotate.timer"),
            (next, last, "logrotate.timer"),
            (next, last, "-"),
            ("nope", last, "bad.timer"),
            ("Mon 2026-10-25 00:00:00 UTC", last, "mismatch.timer"),
            ("Sun 2026-10-25 00:00:00 +05", last, "plus.timer"),
            ("Sun 2026-10-25 00:00:00 +02:00", last, "colon.timer"),
            ("Sun 2026-10-25 00:00:00 +0530", last, "compact.timer"),
            ("Sun 2026-10-25 00:00:00 +25:00", last, "huge.timer"),
            ("Sun 2026-10-25 00:00:00 +", last, "sign.timer"),
            ("Sun 2026-10-25 00:00:00 +2:00", last, "short.timer"),
            ("Sun 2026-10-25 00:00:00 +0a", last, "digit.timer"),
            ("Sun 2026-10-25 00:00:00 XYZ", last, "zone.timer"),
        ]) + "Pass --all to see loaded but inactive timers, too.\n",
    )
    .unwrap();
    let assert = bin(&root)
        .arg("--timers-list")
        .arg(&list)
        .arg("--since")
        .arg("2026-10-23T00:00:00Z")
        .arg("--until")
        .arg("2026-10-25T00:00:00Z")
        .assert()
        .code(1);
    let (stdout, stderr) = text(assert.get_output());
    // `+05` is five hours ahead of UTC, so that NEXT is already due.
    assert!(stdout.contains("logrotate.timer"), "{stdout}");
    assert!(stdout.contains("plus.timer"), "{stdout}");
    assert!(stdout.contains("MISSED"), "{stdout}");
    assert!(stdout.contains("duplicate unit"), "{stdout}");
    assert!(stdout.contains("timer row has no UNIT"), "{stdout}");
    assert!(stdout.contains("is not a systemctl timestamp"), "{stdout}");
    assert!(stdout.contains("does not match"), "{stdout}");
    assert_no_panic(&stderr);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn timers_list_with_only_a_header_exits_two() {
    let root = temp_root();
    let list = root.join("empty-timers.txt");
    std::fs::write(&list, "NEXT LAST UNIT\n").unwrap();
    let assert = bin(&root)
        .arg("--timers-list")
        .arg(&list)
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--until")
        .arg("2026-10-21T00:00:00Z")
        .assert()
        .code(2);
    let (stdout, stderr) = text(assert.get_output());
    assert!(stderr.contains("no jobs found"), "{stderr}");
    assert_no_panic(&stderr);
    assert!(stdout.is_empty(), "{stdout}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn timer_unit_on_stdin_is_named_stdin_service() {
    let root = temp_root();
    let assert = bin(&root)
        .arg("--timer-unit")
        .arg("-")
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--until")
        .arg("2026-10-21T00:00:00Z")
        .write_stdin("[Timer]\nOnCalendar=daily\n")
        .assert()
        .code(3);
    let (stdout, stderr) = text(assert.get_output());
    assert!(stdout.contains("stdin.service"), "{stdout}");
    assert!(stdout.contains("NO_TRACE"), "{stdout}");
    assert_no_panic(&stderr);
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(target_os = "linux")]
#[test]
fn cron_d_skips_a_non_utf8_file_name() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let root = temp_root();
    let cron_d = root.join("etc/cron.d");
    std::fs::create_dir_all(&cron_d).unwrap();
    std::fs::write(
        cron_d.join(OsString::from_vec(vec![0xff, 0xfe])),
        "0 4 * * * root /bin/hidden\n",
    )
    .unwrap();
    std::fs::write(cron_d.join("ok"), "0 12 * * * root /bin/shown\n").unwrap();
    let assert = bin(&root)
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--until")
        .arg("2026-10-21T00:00:00Z")
        .assert()
        .code(3);
    let (stdout, stderr) = text(assert.get_output());
    assert!(stdout.contains("/bin/shown"), "{stdout}");
    assert!(!stdout.contains("/bin/hidden"), "{stdout}");
    assert_no_panic(&stderr);
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(unix)]
#[test]
fn unreadable_cron_d_directory_is_a_warning() {
    let root = temp_root();
    std::fs::create_dir_all(root.join("etc")).unwrap();
    std::fs::write(root.join("etc/crontab"), "0 12 * * * root /bin/shown\n").unwrap();
    let cron_d = root.join("etc/cron.d");
    std::fs::create_dir(&cron_d).unwrap();
    let mut perms = std::fs::metadata(&cron_d).unwrap().permissions();
    perms.set_mode(0o000);
    std::fs::set_permissions(&cron_d, perms).unwrap();
    // Restored even when an assertion panics, so the temp dir can be removed.
    let _restore = RestoreMode(cron_d.clone());

    let assert = bin(&root)
        .arg("--since")
        .arg("2026-10-20T00:00:00Z")
        .arg("--until")
        .arg("2026-10-21T00:00:00Z")
        .assert()
        .code(3);
    let (stdout, stderr) = text(assert.get_output());
    assert!(stdout.contains("/bin/shown"), "{stdout}");
    assert!(stdout.contains("cron.d"), "{stdout}");
    assert_no_panic(&stderr);
    drop(_restore);
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(unix)]
struct RestoreMode(PathBuf);

#[cfg(unix)]
impl Drop for RestoreMode {
    fn drop(&mut self) {
        if let Ok(meta) = std::fs::metadata(&self.0) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(&self.0, perms);
        }
    }
}

/// `/dev/full` exists on Linux. Writing `--help` there must be exit 2,
/// the same write-error path as a report that cannot be printed.
#[cfg(target_os = "linux")]
#[test]
fn help_to_a_full_stdout_exits_two_instead_of_panicking() {
    use assert_cmd::cargo::CommandCargoExt;

    let full = std::fs::File::options()
        .write(true)
        .open("/dev/full")
        .expect("open /dev/full");
    let output = std::process::Command::cargo_bin("cron-ledger")
        .expect("cron-ledger binary")
        .arg("--help")
        .stdout(full)
        .output()
        .expect("spawn cron-ledger");
    assert_eq!(output.status.code(), Some(2), "{:?}", output.status);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot write report"), "{stderr}");
    assert_no_panic(&stderr);
}

/// Standard output closed at the start (`>&-`) is exit 2 with a message,
/// not a report that goes nowhere. Rust ignores EBADF on stdout, so without a
/// check the run ends with its audit code. `/dev/null` is a real, open file
/// and stays exit 0.
#[cfg(unix)]
#[test]
fn closed_stdout_at_the_start_is_exit_two_and_dev_null_stays_zero() {
    use assert_cmd::cargo::CommandCargoExt;

    let bin = std::process::Command::cargo_bin("cron-ledger")
        .expect("cron-ledger binary")
        .get_program()
        .to_owned();
    let closed = std::process::Command::new("sh")
        .arg("-c")
        .arg("exec \"$0\" --help >&-")
        .arg(&bin)
        .output()
        .expect("spawn sh");
    let stderr = String::from_utf8_lossy(&closed.stderr);
    assert_eq!(closed.status.code(), Some(2), "{stderr}");
    assert!(stderr.starts_with("cron-ledger: "), "{stderr}");
    assert!(stderr.contains("standard output is closed"), "{stderr}");
    assert_no_panic(&stderr);

    let null = std::process::Command::new(&bin)
        .arg("--help")
        .stdout(
            std::fs::File::options()
                .write(true)
                .open("/dev/null")
                .unwrap(),
        )
        .output()
        .expect("spawn cron-ledger");
    assert_eq!(null.status.code(), Some(0), "{:?}", null.status);
}

/// `--output` writes a regular file. A named pipe at that path is not
/// replaced by a regular file: exit 2, and the pipe is still a pipe.
#[cfg(unix)]
#[test]
fn output_does_not_replace_a_special_file() {
    use std::os::unix::fs::FileTypeExt;

    let dir = temp_root().join("output-fifo");
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("report.fifo");
    let _ = std::fs::remove_file(&fifo);
    let made = std::process::Command::new("mkfifo").arg(&fifo).status();
    if !matches!(made, Ok(status) if status.success()) {
        eprintln!("mkfifo unavailable; skipped");
        return;
    }
    let assert = Command::cargo_bin("cron-ledger")
        .unwrap()
        .args(["--next", "2026-10-20T00:00:00Z", "--tz", "UTC", "--crontab"])
        .arg(fixture("crontab/basic.crontab"))
        .arg("--output")
        .arg(&fifo)
        .timeout(std::time::Duration::from_secs(10))
        .assert();
    let output = assert.get_output();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("cannot write report"), "{stderr}");
    let kind = std::fs::symlink_metadata(&fifo).unwrap().file_type();
    assert!(kind.is_fifo(), "the pipe was replaced: {kind:?}");
    let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
    assert_eq!(leftovers.len(), 1, "temp file left behind: {leftovers:?}");
}
