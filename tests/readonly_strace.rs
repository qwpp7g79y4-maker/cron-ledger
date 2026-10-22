//! A second, independent read-only/offline check, this time from the
//! outside via `strace`. Skipped (not failed) when `strace` isn't on
//! `PATH`, since whether it runs unprivileged in CI is not
//! guaranteed.
//!
//! Linux only: the syscall names checked here are Linux syscalls, and the
//! `strace` shipped with Git for Windows is a different tool.
#![cfg(target_os = "linux")]

use std::process::Command;

fn strace_available() -> bool {
    Command::new("strace")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn strace_sees_no_network_calls_and_no_write_capable_openat() {
    if !strace_available() {
        eprintln!("strace not found on PATH; skipping");
        return;
    }

    let bin = assert_cmd::cargo::cargo_bin("cron-ledger");
    let log_path =
        std::env::temp_dir().join(format!("cron-ledger-strace-{}.log", std::process::id()));

    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let crontab_fixture = fixtures.join("crontab/system.crontab");
    let syslog_fixture = fixtures.join("logs/debian-syslog.log");
    let status = Command::new("strace")
        .args(["-f", "-e", "trace=network,openat", "-o"])
        .arg(&log_path)
        .arg(&bin)
        .arg("--system-root")
        .arg(&fixtures)
        .arg("--crontab")
        .arg(&crontab_fixture)
        .arg("--syslog")
        .arg(&syslog_fixture)
        .arg("--since")
        .arg("2020-01-01T00:00:00Z")
        .arg("--until")
        .arg("2030-01-01T00:00:00Z")
        .stdout(std::process::Stdio::null())
        .status()
        .expect("failed to launch strace");
    // `strace` exits with the traced program's own exit status. Only 0, 1
    // and 3 are the report's own documented completed-audit codes; a usage
    // error (2) or a crash means the trace below never got near a fixture.
    assert!(
        matches!(status.code(), Some(0) | Some(1) | Some(3)),
        "expected the traced audit to complete (exit 0, 1 or 3), got {:?}",
        status.code()
    );

    let log = std::fs::read_to_string(&log_path).unwrap_or_else(|e| {
        panic!(
            "strace did not produce a log at {}: {e}",
            log_path.display()
        )
    });
    std::fs::remove_file(&log_path).ok();

    // A completed-audit exit code alone doesn't prove the trace actually
    // covers the audit: confirm it contains a successful read of each
    // fixture named on the command line.
    for fixture in [&crontab_fixture, &syslog_fixture] {
        let fixture = fixture.to_str().expect("fixture path is not UTF-8");
        let read = log.lines().any(|line| {
            line.contains("openat(") && line.contains(fixture) && !line.contains("= -1")
        });
        assert!(
            read,
            "trace never shows a successful read of {fixture}:\n{log}"
        );
    }

    let mut offenders = Vec::new();
    for line in log.lines() {
        let is_network = line.contains("socket(") || line.contains("connect(");
        let is_write_open = line.contains("openat(")
            && (line.contains("O_WRONLY") || line.contains("O_RDWR") || line.contains("O_CREAT"));
        if is_network || is_write_open {
            offenders.push(line.to_string());
        }
    }
    assert!(
        offenders.is_empty(),
        "unexpected network or write-capable openat call(s):\n{}",
        offenders.join("\n")
    );
}
