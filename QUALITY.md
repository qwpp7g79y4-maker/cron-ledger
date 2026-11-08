# Quality record, cron-ledger 0.2.0

Date: 2026-11-08. Branch: `prep/w06-csv`. Measured on commit `3082cfa` plus the quick-start change. No tag, no push, no publish.

Toolchain: rustc 1.99.0 (b940084d7 2026-09-28), cargo 1.99.0 (5f94df478 2026-08-27). Host: Linux. `strace` 5.16 was on `PATH`.

## Commands

| Command | Exit |
| --- | --- |
| `cargo fmt --check` | 0 |
| `cargo clippy --all-targets -- -D warnings` | 0 |
| `cargo test --no-fail-fast` | 0 |

`cargo package` was not run for this record (not allowed in this phase); the earlier 0.2.0 package check is in the git history of this file.

### `cargo test`

372 tests, 0 failed, 0 ignored. The strace test ran; it did not skip.

| Suite | Passed |
| --- | ---: |
| `src/lib.rs` | 253 |
| `src/main.rs` | 0 |
| `tests/cli_e2e.rs` | 15 |
| `tests/cli_report.rs` | 40 |
| `tests/crontab_golden.rs` | 5 |
| `tests/csv_diff.rs` | 7 |
| `tests/examples_golden.rs` | 3 |
| `tests/handbook_help.rs` | 2 |
| `tests/ledger_golden.rs` | 20 |
| `tests/log_golden.rs` | 11 |
| `tests/readme_quickstart.rs` | 1 |
| `tests/readonly_fixtures.rs` | 2 |
| `tests/readonly_strace.rs` | 1 |
| `tests/report_golden.rs` | 4 |
| `tests/systemd_golden.rs` | 8 |
| doc-tests | 0 |

## Decisions that close the open points in `task.md`

The boxes in `task.md` are still empty. C1 ticks those.

1. Format. `task.md` of disk-full-doctor is at `/media/bitboi/DATA/projects/disk-full-doctor/task.md`. The cron-ledger brief now uses that checkbox form. The boxes stay empty.
2. Sources. The Cronitor page `https://cronitor.io/guides/where-are-cron-logs-stored` was read again on 2026-11-02. It says cron logs cannot tell you which jobs did not run. The X posts behind the original concept were not verified. The README does not rely on them.
3. Time crate. `jiff` 0.2.38. DST tests live in `src/schedule.rs`. No `chrono`.
4. Exit code 3 for `NO_TRACE` only is the 0.2.0 contract. There is no `--fail-on` flag.
5. DST was checked against cronie master `src/cron.c` (the `medium` and `negative` arms of `wakeupKind`) and `man/cron.8`. The schedule code was not changed. `crontab(5)` says a missing time never matches and a repeated time runs twice. That page does not match the daemon, so the code follows the daemon.
6. Strace. CI installs it with `sudo apt-get` on Linux. The test skips when `strace` is missing. Locally it ran and passed inside `cargo test`. No new GitHub Actions run was started. The last CI run on `main` for 0.1.1 (`40688803261`) completed successfully; that does not cover this branch.
7. Name. The crates.io crate `cron-ledger` is this project (0.1.0 and 0.1.1, owner `qwpp7g79y4-maker`). The GitHub repository `qwpp7g79y4-maker/cron-ledger` exists. 0.2.0 is not on crates.io yet.
8. Publish, push, and tag were not done. They wait for Pepijn.

## Not run

Fuzzing, mutation testing, `git push`, `git tag`, and `cargo publish`.

## Measured: 100 MiB input (W-26, 2026-11-08)

Release build (`cargo build --release`, commit `6cb6c5f`, tree clean). Input: a synthetic syslog of exactly 104857600 bytes (100 MiB), made in `/tmp` and not committed. It is the single line of `examples/sample.log` repeated, so the lines are almost identical; the last line has trailing spaces to reach the exact size. The tool read it without error and exited 0 with 1 OK.

Command:

```
/usr/bin/time -f "%e s %M KB" target/release/cron-ledger --system-root /nonexistent --crontab examples/sample.crontab --syslog /tmp/w26-in/big.log --since 2026-10-20T00:00:00Z --until 2026-10-21T00:00:00Z --tz UTC
```

Three runs, wall clock: 0.87 s, 0.87 s, 0.87 s. Peak memory (max resident set size): 348360 KB, 348444 KB, 348744 KB. These are measurements on one host with near-identical lines; real logs with varied lines may be slower. No throughput is claimed.
