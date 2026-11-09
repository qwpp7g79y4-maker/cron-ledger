//! One byte over the documented input limit is exit 2.
//! The file is sparse, lives in `/tmp`, and is removed when the test ends.
//! It is not a repository file.
//!
//! Windows has no `/tmp` sparse-file path used here; unix only.

#![cfg(unix)]

use std::time::Duration;

use assert_cmd::Command;

#[test]
fn syslog_over_the_byte_limit_exits_2() {
    const LIMIT: u64 = 268435456;
    struct Gone(std::path::PathBuf);
    impl Drop for Gone {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let path = Gone(std::path::PathBuf::from(format!(
        "/tmp/cron-ledger-w46-over-limit-{}.log",
        std::process::id()
    )));
    let file = std::fs::File::create(&path.0).expect("create over-limit file in /tmp");
    file.set_len(LIMIT + 1)
        .expect("sparse length one byte over the limit");
    drop(file);

    let shown = path.0.to_str().expect("path is utf-8");
    let assert = Command::cargo_bin("cron-ledger")
        .expect("cron-ledger binary")
        .timeout(Duration::from_secs(5))
        .args([
            "--quiet",
            "--system-root",
            "/nonexistent",
            "--crontab",
            "examples/sample.crontab",
            "--syslog",
            shown,
            "--since",
            "2026-10-20T00:00:00Z",
            "--until",
            "2026-10-21T00:00:00Z",
            "--tz",
            "UTC",
        ])
        .assert()
        .failure()
        .code(2)
        .stdout("");
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains(&format!("file is larger than {LIMIT} bytes")),
        "{stderr}"
    );
    assert!(!stderr.contains("panicked"), "{stderr}");
}
