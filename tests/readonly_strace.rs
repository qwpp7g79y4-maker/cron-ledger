//! A second, independent read-only/offline check, this time from the
//! outside via `strace`. Skipped (not failed) when `strace` isn't on
//! `PATH`, since whether it runs unprivileged in CI is not
//! guaranteed.

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
    let status = Command::new("strace")
        .args(["-f", "-e", "trace=network,openat", "-o"])
        .arg(&log_path)
        .arg(&bin)
        .arg("--system-root")
        .arg(&fixtures)
        .arg("--crontab")
        .arg(fixtures.join("crontab/system.crontab"))
        .arg("--syslog")
        .arg(fixtures.join("logs/debian-syslog.log"))
        .arg("--since")
        .arg("2020-01-01T00:00:00Z")
        .arg("--until")
        .arg("2030-01-01T00:00:00Z")
        .stdout(std::process::Stdio::null())
        .status()
        .expect("failed to launch strace");
    // `strace` exits with the traced program's own exit status (0-3 here,
    // per the report's exit codes), not strace's own success; only the
    // trace log's contents are under test.
    let _ = status;

    let log = std::fs::read_to_string(&log_path).unwrap_or_else(|e| {
        panic!(
            "strace did not produce a log at {}: {e}",
            log_path.display()
        )
    });
    std::fs::remove_file(&log_path).ok();

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
