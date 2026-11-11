//! `--next` prints the next run in an explicit timezone.
//!
//! Every instant below is written in the test. The machine clock is not
//! read, and no host name is invented: the inputs are `examples/` and
//! `tests/fixtures/`.

use std::path::PathBuf;
use std::process::Command;

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run(args: &[&str]) -> (i32, String, String) {
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
        String::from_utf8(out.stderr).expect("utf-8 stderr"),
    )
}

fn preview(args: &[&str]) -> String {
    let (code, stdout, stderr) = run(args);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout: {stdout}");
    assert!(stderr.is_empty(), "{stderr}");
    stdout
}

#[test]
fn sample_example_next_run_in_utc_and_amsterdam() {
    let utc = preview(&[
        "--system-root",
        "/nonexistent",
        "--crontab",
        "examples/sample.crontab",
        "--next",
        "2026-10-20T03:00:00Z",
        "--tz",
        "UTC",
    ]);
    assert_eq!(
        utc,
        "\
audit: /usr/local/bin/ledger-sample (examples/sample.crontab:1) [15 4 * * *]
  next run: 2026-10-20T04:15:00+00:00[UTC]

----
At: 2026-10-20T03:00:00+00:00[UTC] (tz UTC)
"
    );

    let amsterdam = preview(&[
        "--system-root",
        "/nonexistent",
        "--crontab",
        "examples/sample.crontab",
        "--next",
        "2026-10-20T03:00:00Z",
        "--tz",
        "Europe/Amsterdam",
    ]);
    assert_eq!(
        amsterdam,
        "\
audit: /usr/local/bin/ledger-sample (examples/sample.crontab:1) [15 4 * * *]
  next run: 2026-10-21T04:15:00+02:00[Europe/Amsterdam]

----
At: 2026-10-20T05:00:00+02:00[Europe/Amsterdam] (tz Europe/Amsterdam)
"
    );
    assert_ne!(utc, amsterdam);
}

#[test]
fn backup_example_spring_gap_differs_between_utc_and_amsterdam() {
    let utc = preview(&[
        "--system-root",
        "/nonexistent",
        "--crontab-user",
        "ops=examples/backup.crontab",
        "--next",
        "2026-03-29T00:00:00Z",
        "--tz",
        "UTC",
    ]);
    assert_eq!(
        utc,
        "\
ops: /usr/local/bin/nightly-backup (examples/backup.crontab:3) [30 2 * * *]
  next run: 2026-03-29T02:30:00+00:00[UTC]

ops: /usr/local/bin/other-job (examples/backup.crontab:4) [0 12 * * *]
  next run: 2026-03-29T12:00:00+00:00[UTC]

----
At: 2026-03-29T00:00:00+00:00[UTC] (tz UTC)
"
    );

    let amsterdam = preview(&[
        "--system-root",
        "/nonexistent",
        "--crontab-user",
        "ops=examples/backup.crontab",
        "--next",
        "2026-03-29T00:00:00Z",
        "--tz",
        "Europe/Amsterdam",
    ]);
    assert_eq!(
        amsterdam,
        "\
ops: /usr/local/bin/nightly-backup (examples/backup.crontab:3) [30 2 * * *]
  next run: 2026-03-29T03:00:00+02:00[Europe/Amsterdam]

ops: /usr/local/bin/other-job (examples/backup.crontab:4) [0 12 * * *]
  next run: 2026-03-29T12:00:00+02:00[Europe/Amsterdam]

----
At: 2026-03-29T01:00:00+01:00[Europe/Amsterdam] (tz Europe/Amsterdam)
"
    );
}

#[test]
fn timer_with_an_explicit_zone_is_shown_in_utc_and_amsterdam() {
    let utc = preview(&[
        "--system-root",
        "/nonexistent",
        "--timer-unit",
        "tests/fixtures/systemd/tz-suffix.timer",
        "--next",
        "2026-10-20T00:00:00Z",
        "--tz",
        "UTC",
    ]);
    assert_eq!(
        utc,
        "\
tz-suffix.service (tests/fixtures/systemd/tz-suffix.timer:0) [*-*-* 03:00:00 Europe/Amsterdam]
  next run: 2026-10-20T01:00:00+00:00[UTC]

----
At: 2026-10-20T00:00:00+00:00[UTC] (tz UTC)
"
    );

    let amsterdam = preview(&[
        "--system-root",
        "/nonexistent",
        "--timer-unit",
        "tests/fixtures/systemd/tz-suffix.timer",
        "--next",
        "2026-10-20T00:00:00Z",
        "--tz",
        "Europe/Amsterdam",
    ]);
    assert_eq!(
        amsterdam,
        "\
tz-suffix.service (tests/fixtures/systemd/tz-suffix.timer:0) [*-*-* 03:00:00 Europe/Amsterdam]
  next run: 2026-10-20T03:00:00+02:00[Europe/Amsterdam]

----
At: 2026-10-20T02:00:00+02:00[Europe/Amsterdam] (tz Europe/Amsterdam)
"
    );
}

#[test]
fn system_fixture_shows_cron_tz_in_the_requested_zone() {
    let stdout = preview(&[
        "--system-root",
        "/nonexistent",
        "--crontab",
        "tests/fixtures/crontab/system.crontab",
        "--next",
        "2026-10-20T03:00:00Z",
        "--tz",
        "Europe/Amsterdam",
    ]);
    assert_eq!(
        stdout,
        "\
root: /usr/local/bin/backup.sh (tests/fixtures/crontab/system.crontab:6) [0 3 * * *]
  next run: 2026-10-21T03:00:00+02:00[Europe/Amsterdam]

alice: /usr/local/bin/alice-job.sh (tests/fixtures/crontab/system.crontab:8) [30 2 * * *]
  next run: 2026-10-20T08:30:00+02:00[Europe/Amsterdam]

----
At: 2026-10-20T05:00:00+02:00[Europe/Amsterdam] (tz Europe/Amsterdam)
"
    );
}

#[test]
fn monotonic_timer_has_no_next_run() {
    let stdout = preview(&[
        "--system-root",
        "/nonexistent",
        "--timer-unit",
        "tests/fixtures/systemd/unsupported-trigger.timer",
        "--next",
        "2026-10-20T03:00:00Z",
        "--tz",
        "UTC",
    ]);
    assert_eq!(
        stdout,
        "\
unsupported-trigger.service (tests/fixtures/systemd/unsupported-trigger.timer:0) [unsupported: OnBootSec]
  next run: -

----
At: 2026-10-20T03:00:00+00:00[UTC] (tz UTC)
"
    );
}

#[test]
fn a_named_crontab_does_not_pick_up_the_host_root() {
    let pinned = preview(&[
        "--system-root",
        "/nonexistent",
        "--crontab",
        "examples/sample.crontab",
        "--next",
        "2026-10-20T03:00:00Z",
        "--tz",
        "UTC",
    ]);
    let host_root = preview(&[
        "--system-root",
        "/",
        "--crontab",
        "examples/sample.crontab",
        "--next",
        "2026-10-20T03:00:00Z",
        "--tz",
        "UTC",
    ]);
    assert_eq!(host_root, pinned);
}
