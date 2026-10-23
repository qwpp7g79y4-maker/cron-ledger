# cron-ledger

English · [Nederlands](README.nl.md)

A free, open-source, read-only Rust CLI that reads crontabs and systemd
timers, computes when each job *should* have run in a time window, lines
that up against syslog or the journal, and reports **OK**, **LATE**,
**MISSED**, or **NO_TRACE**. It runs locally, makes no network
connections, and writes nothing.

Version **0.1.0**. License: `MIT OR Apache-2.0`.

## Installation

```sh
cargo install cron-ledger
```

Or build from a checkout of this repository:

```sh
cargo install --path .
```

Both install the `cron-ledger` binary into `~/.cargo/bin` (or wherever
`cargo install` normally places binaries on your system).

## Usage

```
cron-ledger [--crontab PATH|-]... [--crontab-user USER=PATH]... [--system-root DIR]
            [--timer-unit PATH]... [--timers-list PATH|-]...
            [--syslog PATH|-]... [--journal-json PATH|-]...
            --since <RFC3339|30d> [--until <RFC3339>] [--tz Europe/Amsterdam]
            [--tolerance 5m] [--json]
```

`-` means stdin; at most one source across all flags may be stdin.
`--since`/`--until` take either an RFC3339 timestamp or a
`systemd.time(7)`-style duration counted back from now (`30d`, `2h`...).
Everything below uses the fixtures under `tests/fixtures/` so every
example is also exactly what the test suite runs. Except where a
command is specifically demonstrating the `--system-root` default scan,
the examples pin `--system-root /nonexistent` — a path that doesn't
exist, so the scan silently finds nothing — to keep them from also
picking up your own machine's real `/etc/crontab` and `/etc/cron.d/*`.

### System crontab (`--crontab`, and the `--system-root` default scan)

Without any `--crontab`/`--crontab-user`/`--timer-unit` flags, cron-ledger
scans `--system-root DIR` (default `/`) for `DIR/etc/crontab`,
`DIR/etc/cron.d/*`, and `DIR/var/spool/cron/**` on its own — the same
layout a real system uses:

```sh
cron-ledger --system-root /path/to/a/system/snapshot \
  --syslog /var/log/syslog \
  --since 7d
```

To point at one file explicitly instead (works the same for a snapshot
or the real `/etc/crontab`):

```sh
cron-ledger --system-root /nonexistent \
  --crontab tests/fixtures/crontab/system.crontab \
  --syslog tests/fixtures/logs/debian-syslog.log \
  --since 2026-10-20T00:00:00Z --until 2026-10-21T00:00:00Z
```

### Per-user crontab (`--crontab-user`)

```sh
cron-ledger --system-root /nonexistent \
  --crontab-user alice=/var/spool/cron/crontabs/alice \
  --syslog /var/log/syslog \
  --since 30d
```

### systemd timer units (`--timer-unit`)

```sh
cron-ledger --system-root /nonexistent \
  --timer-unit tests/fixtures/systemd/persistent-backup.timer \
  --journal-json tests/fixtures/logs/journal.jsonl \
  --since 2026-10-20T00:00:00Z --until 2026-10-21T00:00:00Z
```

### Debian/Ubuntu or RHEL/cronie syslog (`--syslog`), including a `.gz` rotation

```sh
cron-ledger --system-root /nonexistent \
  --crontab tests/fixtures/crontab/system.crontab \
  --syslog tests/fixtures/logs/rhel-cron.log \
  --syslog tests/fixtures/logs/syslog.2.gz \
  --since 2026-10-20T00:00:00Z --until 2026-10-21T00:00:00Z
```

### `journalctl -o json` output (`--journal-json`)

```sh
journalctl -o json --since "7 days ago" > /tmp/journal.jsonl
cron-ledger --system-root /nonexistent \
  --crontab /etc/crontab --journal-json /tmp/journal.jsonl --since 7d
```

### `--json` for scripting/CI

```sh
cron-ledger --system-root /nonexistent \
  --crontab tests/fixtures/crontab/system.crontab \
  --syslog tests/fixtures/logs/debian-syslog.log \
  --since 2026-10-20T00:00:00Z --until 2026-10-21T00:00:00Z --json
```

## Statuses

| Status | Meaning |
|---|---|
| `OK` | A matching log event was found within tolerance. |
| `LATE` | A matching event was found, but later than the tolerance (for a systemd timer with `Persistent=true`, a run caught up after boot is `LATE (caught up)`, not `MISSED`). |
| `MISSED` | No matching event, and the logs genuinely cover this instant: the run appears to be missing. |
| `NO_TRACE` | No matching event, and the logs don't cover this instant — absence of evidence, not evidence of absence. |
| `REBOOT_ONLY` | A `@reboot` job: no expected run times are computed, only the boots found in the logs, for information. |
| `UNSUPPORTED_TRIGGER` | A systemd timer with only a monotonic trigger (`OnBootSec=`, `OnUnitActiveSec=`, ...) this project cannot compute expected times for in v1. |

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Everything is `OK` (ignoring `REBOOT_ONLY`/`UNSUPPORTED_TRIGGER`, which are informational only). |
| `1` | At least one `MISSED` or `LATE`. |
| `2` | Usage or input/output error (bad flags, `--until` earlier than `--since`, more than one stdin source, an explicitly-named input file that can't be read, or the report cannot be written). |
| `3` | No `MISSED`/`LATE`, but at least one `NO_TRACE`. |

## What it does NOT do

- No live monitoring or alerting; it is an after-the-fact analysis of a time window.
- It cannot see runs when there are no logs. Missing or rotated logs give NO_TRACE, not proof that the job did not run.
- It does not know whether a job **succeeded**, unless cronie's CMDEND or the journal logs the exit code.
- No anacron, fcron, Kubernetes CronJobs or macOS launchd (v1). No OnBootSec/OnUnitActiveSec.
- It does not run `crontab -l` itself; pass that output via stdin.
- DST behaviour follows cronie; other cron implementations may differ.

One more, specific to the current build: `--timers-list` (the text output
of `systemctl list-timers --all`) is accepted as a CLI flag but not yet
implemented — any path given to it is ignored with a warning, not parsed.
"Read-only" and "offline" are only claimed for what `tests/readonly_fixtures.rs`
and `tests/readonly_strace.rs` actually cover.

## Alternatives

- **Cronitor**, **healthchecks.io** (also self-hostable, open source), **Dead Man's Snitch**: heartbeat monitoring. Better for live alerts, but every job needs a ping added up front. Cronitor itself notes that cron logs don't show which jobs did *not* run: https://cronitor.io/guides/where-are-cron-logs-stored
- `systemctl list-timers`: last and next run for timers, not for crontabs.
- `cronic`/`chronic`: mail on failure, not on a missing run.
- Advice: use cron-ledger to find gaps after the fact, and a heartbeat service for alerting going forward.

## Claim → test/fixture

Every claim above is backed by an existing test or fixture in this repo;
none of the numbers in this README are unmeasured benchmark badges.

| Claim | Test / fixture |
|---|---|
| Parses the 5-field vixie-cron syntax (lists, ranges, steps, `*/n`, month/weekday names, `7`=Sunday), `@`-specials, the user column, `SHELL`/`PATH`/`MAILTO`/`TZ`/`CRON_TZ` env lines, comments, and `%` line continuation | `tests/crontab_golden.rs` (`tests/fixtures/crontab/{basic,system,specials,percent}.crontab`) |
| Invalid crontab lines become a `file:line` warning, not a crash | `tests/crontab_golden.rs::invalid_crontab_matches_golden` (`tests/fixtures/crontab/invalid.crontab`) |
| vixie-cron's day-of-month/day-of-week OR rule when both fields are restricted | `src/crontab/model.rs` `matches_day` unit tests |
| `--system-root` default scan reads `etc/crontab`, `etc/cron.d/*`, and `var/spool/cron/**` | `src/app.rs::system_root_default_scan_reads_etc_crontab_cron_d_and_spool` |
| `--crontab-user USER=PATH` reads one user's crontab explicitly | `src/app.rs::crontab_user_flag_reads_a_per_user_crontab_file`, `src/cli.rs::crontab_user_splits_on_equals` |
| Timezone- and DST-aware schedule expansion (system tz, `--tz`, per-line `CRON_TZ=`), cronie-style spring-gap/fall-fold handling for Europe/Amsterdam and America/New_York, strictly increasing output | `src/schedule.rs` unit tests (`amsterdam_*`, `new_york_*`, `times_are_strictly_increasing_for_a_busy_schedule`, the `expand_is_always_sorted_and_in_range` property test) |
| Reads systemd `.timer` units: `OnCalendar=` (`daily`, weekday ranges, `*:0/15`, explicit tz suffix), `Persistent=`, `RandomizedDelaySec=`, `AccuracySec=`, `Unit=`, cross-checked once by hand against `systemd-analyze calendar` | `tests/systemd_golden.rs` (`tests/fixtures/systemd/{daily,weekday-business,quarter-hour,tz-suffix}.timer`, `tests/fixtures/systemd/README.md`) |
| Several timer files with the same `Unit=` keep every description, combined, and a repeated description is stored once | `src/app.rs::two_timers_sharing_a_unit_combine_their_descriptions` |
| `Persistent=true` turns a missed-then-caught-up run into `LATE (caught up)`, not `MISSED` | `tests/systemd_golden.rs::persistent_missed_run_is_late_caught_up_not_missed` (`tests/fixtures/systemd/persistent-backup.timer`) |
| A monotonic-only trigger (`OnBootSec=`, ...) is flagged `UNSUPPORTED_TRIGGER`, not a crash | `tests/systemd_golden.rs::onbootsec_is_flagged_unsupported_with_no_oncalendar` (`tests/fixtures/systemd/unsupported-trigger.timer`) |
| Parses Debian/Ubuntu syslog and RHEL/cronie `/var/log/cron` (incl. `CMDOUT`/`CMDEND`): RFC3164, RFC5424, and rsyslog's high-precision file format (a leading RFC3339 timestamp, no priority; numeric offset or `Z`), and infers the missing RFC3164 year across a Dec 31 → Jan 1 boundary | `tests/log_golden.rs` (`tests/fixtures/logs/{debian-syslog,rhel-cron,rfc5424,rsyslog-fileformat,year-boundary}.log`) |
| Parses `journalctl -o json`: cron `MESSAGE` as a string or byte array, a timer unit's `Started ...` event, a boot only when a kernel banner or an entry from the start of the boot says so, and log coverage across every valid entry | `tests/log_golden.rs::journal_json_matches_golden` (`tests/fixtures/logs/journal.jsonl`), `src/log/journal.rs` boot tests, `tests/ledger_golden.rs` (`journal_entry_after_the_last_event_keeps_a_later_run_missed`, `journal_excerpt_without_boot_evidence_does_not_catch_up_a_persistent_timer`, `kernel_banner_still_catches_up_a_persistent_timer`) |
| An explicitly-named `.gz` log rotation is decompressed transparently | `tests/log_golden.rs::explicit_gz_rotation_is_decompressed_and_parsed` (`tests/fixtures/logs/syslog.2.gz`) |
| Broken log lines are counted as warnings, never fatal | `tests/log_golden.rs::garbage_lines_are_counted_not_fatal_and_match_golden` (`tests/fixtures/logs/garbage.log`); a non-UTF-8 byte is replaced and the rest of the file is still audited (`src/app.rs::invalid_utf8_in_a_syslog_is_a_warning_and_the_audit_continues`) |
| A million-line syslog parses within the test's release-mode time budget | `tests/log_golden.rs::one_million_lines_parses_fast_in_release` (budget only asserted with `cargo test --release`; see `tests/fixtures/logs/README.md`) |
| Matches each expected run to the nearest event within tolerance, independently per `(user, command)` or timer unit, including duplicate commands across users and a `%`-containing command | `tests/ledger_golden.rs` (`duplicate_command_two_users_matches_independently`, `percent_command_job_matches_on_the_split_command`) |
| `@reboot` never produces expected times; it only reports found boots as `REBOOT_ONLY` | `tests/ledger_golden.rs::reboot_job_reports_found_boots_for_info_only` |
| `MISSED` is only reported inside the span the logs actually cover; a gap or empty logs become `NO_TRACE`/a coverage warning, never a false `MISSED` | `tests/ledger_golden.rs::log_gap_missed_only_in_covered_days`, `tests/cli_report.rs::no_trace_only_scenario_exits_three` (`tests/fixtures/report/no_trace/`) |
| Exit codes 0/1/2/3 as documented above | `src/report/mod.rs` unit tests, `tests/cli_report.rs` (`ok_scenario_exits_zero`, `missed_scenario_exits_one`, `late_scenario_exits_one`, `no_trace_only_scenario_exits_three`, `missing_since_is_a_usage_error_exit_two`, `until_earlier_than_since_is_a_usage_error_exit_two`, `until_equal_to_since_still_audits`, `more_than_one_stdin_source_is_a_usage_error_exit_two`, `full_stdout_exits_with_a_write_error_instead_of_panicking`) |
| `--json` emits a `schema_version: 1` report | `tests/cli_report.rs::json_flag_produces_a_schema_versioned_report`, `tests/report_golden.rs` |
| The text report has no ANSI color on a non-tty, and respects `NO_COLOR` | `src/report/mod.rs::color_requires_both_a_tty_and_no_color_unset`, `tests/report_golden.rs::color_wraps_status_words_in_ansi_codes_when_enabled`, `tests/cli_report.rs::ok_scenario_exits_zero` |
| At most one input source may read from stdin (`-`) | `src/cli.rs::at_most_one_stdin_source`, `tests/cli_report.rs::more_than_one_stdin_source_is_a_usage_error_exit_two` |
| `--help` output | `tests/cli_report.rs::help_flag_matches_golden_usage_text` (`tests/golden/help.txt`) |
| Read-only: running against a copy of every fixture with all write bits cleared changes nothing (not a byte, not an mtime) | `tests/readonly_fixtures.rs::running_against_a_read_only_fixture_copy_changes_nothing` |
| Offline: no `socket`/`connect` syscall and no write-capable `openat`, observed under `strace` in CI (skipped, not failed, where `strace` is unavailable) | `tests/readonly_strace.rs::strace_sees_no_network_calls_and_no_write_capable_openat` |
| No network crate (`reqwest`, `hyper`, `ureq`, `curl`) anywhere in the dependency tree | `.github/workflows/ci.yml`'s `readonly-static-check` and `deny` jobs, `deny.toml` |

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo package
```

The first three run in CI (`.github/workflows/ci.yml`) along with `cargo
audit` and `cargo deny check`. Run `cargo package` yourself before a
release; CI does not run it.

## Support

cron-ledger is free to use. If it saves you time, you can support the work via [Ko-fi](https://ko-fi.com/pepai).

## License

Licensed under either of [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE) at your option.
