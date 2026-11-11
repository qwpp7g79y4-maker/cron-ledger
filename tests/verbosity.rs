//! `--quiet` and `--verbose`. A pipe prints no usage text after an error.
//! `--verbose` adds that text, and on success one context line. Exit codes stay.

use std::process::{Command, Stdio};

fn run(args: &[&str]) -> (i32, String, String) {
    let path = option_env!("CARGO_BIN_EXE_cron_ledger")
        .or(option_env!("CARGO_BIN_EXE_cron-ledger"))
        .expect("CARGO_BIN_EXE_cron_ledger");
    let output = Command::new(path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

const SAMPLE: &[&str] = &[
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
];

#[test]
fn help_lists_quiet_and_verbose() {
    let (code, stdout, stderr) = run(&["--help"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("--quiet"), "{stdout}");
    assert!(stdout.contains("--verbose"), "{stdout}");
    assert!(!stdout.contains("--threshold"));
}

#[test]
fn pipe_and_quiet_omit_the_usage_verbose_prints_it() {
    let (code, stdout, stderr) = run(&["--tz", "UTC"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert_eq!(stderr.lines().count(), 1, "{stderr:?}");
    assert!(stderr.starts_with("cron-ledger: "), "{stderr}");
    assert!(stderr.contains("for example --since 30d"), "{stderr}");
    assert!(!stderr.contains("[--crontab"), "{stderr}");

    let (qcode, qout, qerr) = run(&["--quiet", "--tz", "UTC"]);
    assert_eq!(qcode, 2, "{qerr}");
    assert!(qout.is_empty());
    assert_eq!(qerr, stderr, "quiet matches a pipe");

    let (vcode, vout, verr) = run(&["--verbose", "--tz", "UTC"]);
    assert_eq!(vcode, 2, "{verr}");
    assert!(vout.is_empty());
    assert!(
        verr.lines().next().unwrap().starts_with("cron-ledger: "),
        "{verr}"
    );
    assert!(verr.contains("--since <RFC3339|30d>"), "{verr}");
}

#[test]
fn quiet_with_verbose_stays_exit_2() {
    let (code, stdout, stderr) = run(&["--quiet", "--verbose"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("cannot be combined"), "{stderr}");
    assert_ne!(code, 1, "no threshold exit");
}

#[test]
fn verbose_context_stays_off_stdout() {
    let (code, stdout, stderr) = run(SAMPLE);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.is_empty(), "{stderr}");

    let mut verbose = vec!["--verbose"];
    verbose.extend_from_slice(SAMPLE);
    let (vcode, vout, verr) = run(&verbose);
    assert_eq!(vcode, 0, "{verr}");
    assert_eq!(vout, stdout, "verbose does not change the report");
    assert_eq!(verr, "context: jobs 1, warnings 0\n", "{verr:?}");

    let mut quiet = vec!["--quiet"];
    quiet.extend_from_slice(SAMPLE);
    let (qcode, qout, qerr) = run(&quiet);
    assert_eq!(qcode, 0, "{qerr}");
    assert_eq!(qout, stdout);
    assert!(qerr.is_empty(), "{qerr}");
}

#[test]
fn flags_leave_the_missed_exit_at_1() {
    let base = [
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
    ];
    let (code, stdout, stderr) = run(&base);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    for flag in ["--quiet", "--verbose"] {
        let mut args = vec![flag];
        args.extend_from_slice(&base);
        let (flag_code, flag_out, flag_err) = run(&args);
        assert_eq!(flag_code, 1, "{flag} {flag_err}");
        assert_eq!(flag_out, stdout);
        assert_ne!(flag_code, 2);
    }
}

#[test]
fn diff_error_stays_one_line_and_exit_2() {
    for args in [
        &["--diff", "no-old.json", "no-new.json"][..],
        &["--quiet", "--diff", "no-old.json", "no-new.json"][..],
        &["--verbose", "--diff", "no-old.json", "no-new.json"][..],
    ] {
        let (code, stdout, stderr) = run(args);
        assert_eq!(code, 2, "{args:?} {stderr}");
        assert!(stdout.is_empty(), "{args:?}");
        assert_eq!(stderr.lines().count(), 1, "{args:?} {stderr:?}");
        assert!(
            stderr.contains("check that the path exists"),
            "{args:?} {stderr}"
        );
        assert!(!stderr.contains("context:"), "{args:?} {stderr}");
        assert_ne!(code, 1);
    }
}
