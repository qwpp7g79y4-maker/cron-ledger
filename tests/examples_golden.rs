//! Golden test for `examples/`: each input pair is run through the binary
//! and the output must equal the stored `*.expected.txt` byte for byte.

use std::path::PathBuf;
use std::process::Command;

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run(args: &[&str]) -> (i32, String) {
    let path = option_env!("CARGO_BIN_EXE_cron_ledger")
        .or(option_env!("CARGO_BIN_EXE_cron-ledger"))
        .expect("CARGO_BIN_EXE_cron-ledger");
    let out = Command::new(path)
        .args(args)
        .current_dir(manifest())
        .output()
        .expect("spawn cron-ledger");
    (
        out.status.code().expect("exit code"),
        String::from_utf8(out.stdout).expect("utf-8 stdout"),
    )
}

fn expected(name: &str) -> String {
    let p = manifest().join("examples").join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

#[test]
fn sample_example_matches_expected_output() {
    let (code, stdout) = run(&[
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
    ]);
    assert_eq!(code, 0);
    assert_eq!(stdout, expected("sample.expected.txt"));
}

#[test]
fn backup_example_matches_expected_output() {
    let (code, stdout) = run(&[
        "--system-root",
        "/nonexistent",
        "--crontab-user",
        "ops=examples/backup.crontab",
        "--syslog",
        "examples/backup.log",
        "--since",
        "2026-10-20T00:00:00Z",
        "--until",
        "2026-10-23T00:00:00Z",
        "--tz",
        "UTC",
    ]);
    assert_eq!(code, 1);
    assert_eq!(stdout, expected("backup.expected.txt"));
}

#[test]
fn usage_errors_say_what_to_do_and_keep_exit_code_2() {
    let path = option_env!("CARGO_BIN_EXE_cron_ledger")
        .or(option_env!("CARGO_BIN_EXE_cron-ledger"))
        .expect("CARGO_BIN_EXE_cron-ledger");
    for (args, hint) in [
        (vec!["--bogus"], "run cron-ledger --help"),
        (vec![], "for example --since 30d"),
        (vec!["--since"], "run cron-ledger --help"),
        (
            vec!["--since", "30d", "--crontab", "/nonexistent/x.cron"],
            "check that the path exists",
        ),
    ] {
        let out = Command::new(path).args(&args).output().expect("spawn");
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(hint), "{args:?}: {err}");
    }
}
