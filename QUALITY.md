# Quality record, cron-ledger 0.2.0

Date: 2026-11-02. Branch: `fix/cl-h1-empty-input`. No tag, no push, no publish.

Toolchain: rustc 1.99.0 (b940084d7 2026-09-28), cargo 1.99.0 (5f94df478 2026-08-27). Host: Linux. `strace` 5.16 was on `PATH`.

## Commands

| Command | Exit |
| --- | --- |
| `cargo fmt --check` | 0 |
| `cargo clippy -- -D warnings` | 0 |
| `cargo test` | 0 |
| `cargo package --allow-dirty --no-verify --list` | 0 |

`cargo clippy` here is the command above. CI (`.github/workflows/ci.yml`) also runs `cargo clippy --all-targets --locked -- -D warnings`. That wider command was not re-run for this record.

### `cargo test`

346 tests, 0 failed, 0 ignored. The strace test ran; it did not skip.

| Suite | Passed |
| --- | ---: |
| `src/lib.rs` | 238 |
| `src/main.rs` | 0 |
| `tests/cli_e2e.rs` | 15 |
| `tests/cli_report.rs` | 40 |
| `tests/crontab_golden.rs` | 5 |
| `tests/handbook_help.rs` | 2 |
| `tests/ledger_golden.rs` | 20 |
| `tests/log_golden.rs` | 11 |
| `tests/readonly_fixtures.rs` | 2 |
| `tests/readonly_strace.rs` | 1 |
| `tests/report_golden.rs` | 4 |
| `tests/systemd_golden.rs` | 8 |
| doc-tests | 0 |

### `cargo package --allow-dirty --no-verify --list`

Exit 0 before this file existed. That list had the crate sources, tests, fixtures, `man/cron-ledger.1`, `examples/`, both licenses, both READMEs, `CHANGELOG.md`, `SECURITY.md`, and `deny.toml`. It did not list `.github/`, `task.md`, or this file.

The same command after this file was added also exited 0. That list has 107 paths and includes `QUALITY.md`.

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
