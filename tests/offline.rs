//! The sample audit is the same when the process has no way off the machine.
//!
//! W-45. `unshare --user --map-root-user --net` starts a network namespace
//! whose route table is empty, so nothing can connect out. The audit reads
//! the existing sample crontab and syslog. No provider and no price.

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::Command;

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn bin() -> &'static str {
    option_env!("CARGO_BIN_EXE_cron_ledger")
        .or(option_env!("CARGO_BIN_EXE_cron-ledger"))
        .expect("CARGO_BIN_EXE_cron-ledger")
}

/// The sample audit, with outgoing connections blocked.
///
/// The proof reads `/proc/net/route` in that namespace. The binary then
/// reads `examples/sample.crontab` and `examples/sample.log` and must print
/// `examples/sample.expected.txt`.
#[test]
fn sample_audit_matches_expected_output_with_outgoing_connections_blocked() {
    // GitHub CI / pepai-worker-01 often lack user namespaces (EPERM / uid_map).
    let probe = Command::new("unshare")
        .args(["-rn", "true"])
        .output()
        .expect("unshare -rn true");
    if !probe.status.success() {
        let err = String::from_utf8_lossy(&probe.stderr);
        let low = err.to_ascii_lowercase();
        if low.contains("operation not permitted")
            || low.contains("eperm")
            || low.contains("uid_map")
            || low.contains("permission denied")
        {
            eprintln!("overgeslagen: geen user namespace");
            return;
        }
        panic!("unshare -rn true failed: {err}");
    }

    let ns = ["--user", "--map-root-user", "--net", "--"];
    let proof = Command::new("unshare")
        .args(ns)
        .args(["cat", "/proc/net/route"])
        .output()
        .expect("unshare route table");
    assert!(
        proof.status.success(),
        "unshare failed: {}",
        String::from_utf8_lossy(&proof.stderr)
    );
    let routes = String::from_utf8(proof.stdout).expect("utf-8 route table");
    assert!(
        !routes.split_whitespace().any(|field| field == "00000000"),
        "network namespace still has a route: {routes:?}"
    );

    let root = manifest();
    let out = Command::new("unshare")
        .args(ns)
        .arg(bin())
        .current_dir(&root)
        .args([
            "--system-root",
            "/nonexistent",
            "--crontab",
            "examples/sample.crontab",
            "--syslog",
            "examples/sample.log",
            "--since",
            "2026-10-20T00:00:00Z",
            "--until",
            "2026-10-21T00:00:00Z",
            "--tz",
            "UTC",
        ])
        .output()
        .expect("run cron-ledger with outgoing connections blocked");
    let want =
        std::fs::read_to_string(root.join("examples/sample.expected.txt")).expect("expected file");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8(out.stdout).expect("utf-8"), want);
}
