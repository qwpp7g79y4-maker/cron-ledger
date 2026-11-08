# Quality record, cron-ledger 0.2.0

Date: 2026-11-08. Branch: `prep/w24-fout`. The test table below was remeasured after the input-limit test. The 100 MiB measurement stays on commit `3d8d429`. The line-coverage measurement stays the one recorded below. No tag, no push, no publish.

### 2026-11-08 (W-46, input limit, branch prep/w24-fout)

- Each named file, each crontab found by `--system-root`, each `--diff` report, and standard input may be at most 268435456 bytes (256 MiB). A larger input exits 2 with `file is larger than 268435456 bytes` and is not loaded. A `.gz` log is refused when the inflated text passes that size. The limit is new. The measured 100 MiB input stays under it. The over-limit file is created in `/tmp` by the test and is not committed. Exit codes stay 0, 1, 2 and 3.
- `cargo test --no-fail-fast`: 412 passed / 0 failed / 0 ignored.
- `cargo fmt --check`: clean. `cargo clippy --all-targets -- -D warnings`: clean.
- Version stays 1.0.0. `rust-version` stays 1.85.

### 2026-11-08 (W-45, sample audit with outgoing connections blocked, branch prep/w24-fout)

- `tests/offline.rs`: `sample_audit_matches_expected_output_with_outgoing_connections_blocked` runs the sample audit inside `unshare --user --map-root-user --net`. That namespace has an empty route table. The report matches `examples/sample.expected.txt`. No provider and no price were added.
- `cargo test --no-fail-fast`: 411 passed / 0 failed / 0 ignored.
- `cargo fmt --check`: clean. `cargo clippy --all-targets -- -D warnings`: clean.
- Version stays 1.0.0. `rust-version` stays 1.85. Exit codes stay 0, 1, 2 and 3.

Toolchain: rustc 1.99.0 (b940084d7 2026-09-28), cargo 1.99.0 (5f94df478 2026-08-27). Host: Linux. `strace` 5.16 was on `PATH`.

## Commands

| Command | Exit |
| --- | --- |
| `cargo fmt --check` | 0 |
| `cargo clippy --all-targets -- -D warnings` | 0 |
| `cargo test --no-fail-fast` | 0 |

`cargo package` was not run for this record (not allowed in this phase); the earlier 0.2.0 package check is in the git history of this file.

### `cargo test`

412 tests, 0 failed, 0 ignored. The strace test ran; it did not skip. The outgoing-connection test ran. The over-limit test ran.

| Suite | Passed |
| --- | ---: |
| `src/lib.rs` | 256 |
| `src/main.rs` | 1 |
| `tests/cli_e2e.rs` | 15 |
| `tests/cli_report.rs` | 43 |
| `tests/crontab_golden.rs` | 5 |
| `tests/csv_diff.rs` | 7 |
| `tests/error_messages.rs` | 24 |
| `tests/examples_golden.rs` | 3 |
| `tests/handbook_help.rs` | 2 |
| `tests/input_limit.rs` | 1 |
| `tests/json_schema.rs` | 1 |
| `tests/ledger_golden.rs` | 20 |
| `tests/log_golden.rs` | 11 |
| `tests/offline.rs` | 1 |
| `tests/readme_quickstart.rs` | 1 |
| `tests/readonly_fixtures.rs` | 2 |
| `tests/readonly_strace.rs` | 1 |
| `tests/report_golden.rs` | 4 |
| `tests/systemd_golden.rs` | 8 |
| `tests/verbosity.rs` | 6 |
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

Release build (`cargo build --release`, commit `3d8d429`, branch `prep/w24-fout`, tree clean). Input: a synthetic syslog of exactly 104857600 bytes (100 MiB), made in `/tmp` and not committed. It is the single line of `examples/sample.log` repeated, so the lines are almost identical; the last line has trailing spaces to reach the exact size. The tool read it without error and exited 0 with 1 OK.

Command:

```
/usr/bin/time -f "%e s %M KB" target/release/cron-ledger --system-root /nonexistent --crontab examples/sample.crontab --syslog /tmp/w26-in/big.log --since 2026-10-20T00:00:00Z --until 2026-10-21T00:00:00Z --tz UTC
```

Three runs, wall clock: 0.87 s, 0.87 s, 0.87 s. Peak memory (max resident set size): 348360 KB, 348444 KB, 348744 KB. These are measurements on one host with near-identical lines; real logs with varied lines may be slower. No throughput is claimed.

## Measured: line coverage (W-28, 2026-11-08)

Line coverage: 96.55% (7005 lines, 242 missed), as printed in the TOTAL row of `cargo llvm-cov --ignore-run-fail`. Measured on 2026-11-08 on branch `prep/w24-fout`. Goal was above 90%; no tests were added to reach it.

Command: `PATH="$HOME/.cargo/bin:$PATH" cargo llvm-cov --ignore-run-fail`

Caveat: under `cargo llvm-cov` the test `readonly_strace::strace_sees_no_network_calls_and_no_write_capable_openat` fails, because the instrumented binary itself opens a `.profraw` file for writing, which the test flags. That is an artefact of instrumentation, so `--ignore-run-fail` was needed to get the report. In a plain `cargo test --no-fail-fast` run it passes. The existing test count above is left as it was.
