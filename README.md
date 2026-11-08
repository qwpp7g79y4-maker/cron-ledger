# cron-ledger

English · [Nederlands](README.nl.md)

A free, open-source, read-only Rust CLI that reads crontabs and systemd
timers, computes when each job *should* have run in a time window, lines
that up against syslog or the journal, and reports **OK**, **LATE**,
**MISSED**, or **NO_TRACE**. It runs locally, makes no network
connections, and writes nothing.

Version **0.2.0**. License: `MIT OR Apache-2.0`.

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

## Aan de slag in 5 minuten

Install cron-ledger as in Installation and put it on your `PATH`. Run the
commands below from the repository root. They read the invented crontab and
syslog files in `examples/`: no `/var/log`, no `journalctl`, no network. The test
`tests/readme_quickstart.rs` runs these lines literally.

```sh
cron-ledger --system-root /nonexistent --crontab examples/sample.crontab --syslog examples/sample.log --since 2026-10-20T00:00:00Z --until 2026-10-21T00:00:00Z --tz UTC
cron-ledger --system-root /nonexistent --crontab examples/sample.crontab --syslog examples/sample.log --since 2026-10-20T00:00:00Z --until 2026-10-21T00:00:00Z --tz UTC --csv
```

The first command prints the report in `examples/sample.expected.txt`: one run,
status OK, exit code 0. The second prints the same run as CSV.

## Usage

```
cron-ledger [--crontab PATH|-]... [--crontab-user USER=PATH]... [--system-root DIR]
            [--timer-unit PATH]... [--timers-list PATH|-]...
            [--syslog PATH|-]... [--journal-json PATH|-]...
            --since <RFC3339|30d> [--until <RFC3339|30d>] [--tz Europe/Amsterdam]
            [--tolerance 5m] [--json | --csv] [-h | --help]
cron-ledger --diff OLD.json NEW.json
```

The manual page is [man/cron-ledger.1](man/cron-ledger.1).

`-` means stdin; at most one source across all flags may be stdin.
`--since`/`--until` take either an RFC3339 timestamp or a
`systemd.time(7)`-style duration counted back from now (`30d`, `2h`...).
Anything else is a usage error and exits 2, with
`'{value}' is not an RFC3339 timestamp or a duration like '30d'`.
Everything below uses the fixtures under `tests/fixtures/` so every
example is also exactly what the test suite runs. Except where a
command is specifically demonstrating the `--system-root` default scan,
the examples pin `--system-root /nonexistent`. An explicit `--crontab`,
`--crontab-user`, `--timer-unit`, or `--timers-list` already turns that
scan off, so the path is not read. It stays in the example so dropping
the source flag cannot pick up your own machine's `/etc/crontab` and
`/etc/cron.d/*`.
The `tests/fixtures/` paths below exist only in a clone of this
repository (`git clone`, then `cargo install --path .`). After
`cargo install cron-ledger`, pass your own files with `--crontab` and
`--syslog`.

### System crontab (`--crontab`, and the `--system-root` default scan)

Without any `--crontab`/`--crontab-user`/`--timer-unit`/`--timers-list`
flags, cron-ledger scans `--system-root DIR` (default `/`) for
`DIR/etc/crontab`, `DIR/etc/cron.d/*`, and `DIR/var/spool/cron/**` on its
own — the same layout a real system uses. Passing `--system-root` points
that scan at a snapshot instead of `/`, and the scan still runs. Passing
one of those flags selects exactly the sources you named and does not
also scan the system root. A second read of the same file would schedule the
command twice, and one log line can satisfy only one of the copies, so
the extra copy would be reported `MISSED`.

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

### Saved `systemctl list-timers` text (`--timers-list`)

cron-ledger does not run `systemctl`. Pass a text file saved from
`systemctl list-timers --all` (header row and `N timers listed.` footer
included). Only LAST, NEXT and UNIT are read. That listing has no
calendar, so this is only a check that the last run is older than
expected: when NEXT is earlier than `--until` by more than `--tolerance`,
and LAST is missing or earlier than that NEXT by more than `--tolerance`,
the timer is `MISSED` and the report's last run is the LAST column.
Otherwise that last run is `OK`. A row whose NEXT and LAST are both
`n/a` or `-` is skipped with a warning. A file with no usable row is
exit 2. The check does not use logs, so an empty log does not turn it
into `NO_TRACE`.

```sh
cron-ledger --system-root /nonexistent \
  --timers-list tests/fixtures/systemd/list-timers.txt \
  --since 2026-10-23T00:00:00Z --until 2026-10-25T00:00:00Z --tz UTC
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

Each line is one JSON object. A line that repeats a key is not evidence: it is skipped and reported as a warning, in the same way as a line that cannot be read. A repeated `MESSAGE`, `SYSLOG_IDENTIFIER`, `__REALTIME_TIMESTAMP`, `_TRANSPORT`, or any other key is enough. The line is not a run and does not extend log coverage.

A timer run is a manager line (`SYSLOG_IDENTIFIER=systemd`) whose `MESSAGE` starts with `Started `, or with `Finished ` when that activation did not already log `Started `. `Starting ` is not a run. `UNIT` names the unit, or `_SYSTEMD_UNIT` when `UNIT` is absent.

### `--json` for scripting/CI

```sh
cron-ledger --system-root /nonexistent \
  --crontab tests/fixtures/crontab/system.crontab \
  --syslog tests/fixtures/logs/debian-syslog.log \
  --since 2026-10-20T00:00:00Z --until 2026-10-21T00:00:00Z --json
```

## Example

Two invented inputs live in `examples/`, each with its expected output.
From a clone of this repository:

```sh
cron-ledger --system-root /nonexistent \
  --crontab-user ops=examples/backup.crontab \
  --syslog examples/backup.log \
  --since 2026-10-20T00:00:00Z --until 2026-10-23T00:00:00Z --tz UTC
```

The backup ran every day but 11 minutes late on 21 October (`LATE`), and
the 12:00 job ran only once (two `MISSED`), so the exit code is 1. The
full output is in `examples/backup.expected.txt`; the second example,
`examples/sample.*`, is a clean run that exits 0.
`tests/examples_golden.rs` compares both with their stored output.

## Options

| Option | Meaning |
|---|---|
| `--crontab PATH\|-` | System crontab (five fields, a user, a command). Repeatable. `-` is stdin. |
| `--crontab-user USER=PATH` | Per-user crontab for `USER` (no user column). Repeatable. |
| `--system-root DIR` | Root directory scanned for `etc/crontab`, `etc/cron.d` and `var/spool/cron` when no crontab or timer source is named. Default `/`. |
| `--timer-unit PATH` | A systemd `.timer` unit file; its `OnCalendar=` is expanded. Repeatable. |
| `--timers-list PATH\|-` | A saved `systemctl list-timers --all` listing. Repeatable. |
| `--syslog PATH\|-` | A saved syslog or cron log; `.gz` is decompressed. Repeatable. |
| `--journal-json PATH\|-` | A saved `journalctl -o json` export. Repeatable. |
| `--since <RFC3339\|30d>` | Start of the window. Required. |
| `--until <RFC3339\|30d>` | End of the window (half-open). Default: now. |
| `--tz ZONE` | IANA timezone for expansion and zone-less log lines, e.g. `Europe/Amsterdam`. Default: system zone, else UTC. |
| `--tolerance SPAN` | How late an event may be and still be `OK`. Default `5m`. |
| `--json` | Write one JSON document instead of the text report. |
| `--csv` | Write CSV, one row per audited run. Not combinable with `--json`. |
| `--diff OLD.json NEW.json` | Compare two saved `--json` reports. Besides `--quiet` and `--verbose`, it takes no other option. Exit 0 identical, 1 different, 2 input error. |
| `--output PATH` | Write the report to this file instead of standard output. An existing file is replaced. A missing parent directory exits 2 and is not created. |
| `--quiet` | Hide the usage text after an error, even on a terminal. No context line. |
| `--verbose` | Print the usage text after an error, and on success one `context:` line on stderr, even on a pipe. |
| `-h`, `--help` | Print the usage text and exit 0. |

`-` means stdin; at most one source across all options may be stdin.

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
| `2` | Usage or input/output error (bad flags, a `--since`/`--until` value that is neither an RFC3339 timestamp nor a duration, `--until` earlier than `--since`, more than one stdin source, an explicitly-named input file that can't be read, a file, a scanned crontab, a `--diff` report, or standard input larger than 268435456 bytes (256 MiB), a crontab or timer unit that is UTF-16/UTF-32 or contains a NUL byte, a log that is UTF-16/UTF-32 or at least one byte in four NUL, an argument that is not valid UTF-8, or a named source that yields no jobs (empty, only invalid lines, or a timer unit with no trigger or no valid `OnCalendar=`; an empty `--crontab -` included), a `--system-root` that is not a directory or whose scan found crontab files and could read none of them (a root with no crontab files is exit 0 with a `no crontab file found` warning), an `--since`/`--until` window whose schedule would expand past the expected-run cap, or the report cannot be written). |
| `3` | No `MISSED`/`LATE`, but at least one `NO_TRACE`. |
| `130` | SIGINT. No half-written report. An existing `--output` file is kept when the new report was not finished. Stderr is `cron-ledger: interrupted by SIGINT`. |

Each named file, each crontab found by `--system-root`, each `--diff` report, and standard input may be at most 268435456 bytes (256 MiB). A larger input exits 2 and is not loaded. The message contains `file is larger than 268435456 bytes`. A `.gz` log is also refused when the inflated text passes that size. SIGINT exits 130. A signal before the report is finished writes nothing to stdout and leaves an existing `--output` file as it was. Exit codes 0, 1, 2 and 3 are unchanged.

## What it does NOT do

- No live monitoring or alerting; it is an after-the-fact analysis of a time window.
- It cannot see runs when there are no logs. Coverage is one closed span per input file, from that file's first recognised line to its last, not one span stretched across every file. An expected time inside a file's span with no matching event is MISSED. One syslog that contains a line at 18 October 03:00 and a line at 22 October 03:00 still reports 19, 20 and 21 October as MISSED: those days sit inside that file. The same two lines in two files that do not meet, such as `s.2.gz` ending on the 18th and `s.1` starting on the 22nd, report those three days as NO_TRACE. A hole between files is not proof the job did not run; pass each rotation as its own `--syslog` (a file you concatenated yourself is one span). Expected times before the earliest line, after the latest line, and an empty log are NO_TRACE. In `--json`, `log_coverage.first` and `log_coverage.last` are the outer bounds across files. A time between them is MISSED only when it also falls inside some single file's span. The text report lists each file's span.
- It does not know whether a job **succeeded**, unless cronie's CMDEND or the journal logs the exit code.
- No anacron, fcron, Kubernetes CronJobs or macOS launchd (v1). No OnBootSec/OnUnitActiveSec.
- Arguments must be valid UTF-8; one that is not is refused with exit 2 rather than read lossily. A file under `var/spool/cron` whose name is not valid UTF-8 has no known user, so it is skipped with a warning (it may be an unaudited job) instead of being audited as user `""`.
- It does not run `crontab -l` itself; pass that output via stdin.
- It does not run `systemctl`. `--timers-list` reads a saved listing; it never executes `systemctl`.
- DST behaviour follows cronie's daemon (`cron(8)` and `src/cron.c`): a fixed time in the spring gap runs once, just after the jump; a fixed time in the repeated autumn hour runs once; a wildcard hour runs in each physical hour. `crontab(5)` says the gap never matches and the repeated hour runs twice; that page does not match the daemon. Other cron implementations may differ. cronie itself can miss or double-run a job whose `CRON_TZ` is not the system zone; cron-ledger expands each job in its own zone with the same rules.

"Read-only" and "offline" are only claimed for what `tests/readonly_fixtures.rs`,
`tests/readonly_strace.rs` and `tests/offline.rs` actually cover.

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
| An explicit `--crontab`, `--crontab-user`, `--timer-unit`, or `--timers-list` is the whole set of jobs. The `--system-root` scan does not also run, so a file under that root is not scheduled a second time. `--system-root` with none of those flags still scans that directory | `src/app.rs::explicit_crontab_is_not_counted_again_by_the_system_root_scan`, `src/app.rs::timers_list_does_not_scan_the_system_root`, `src/app.rs::system_root_default_scan_reads_etc_crontab_cron_d_and_spool` |
| `--timers-list` reads a saved `systemctl list-timers --all` listing (LAST, NEXT, UNIT only; no `systemctl` call). The LAST timestamp is the report's last run. A NEXT already due before `--until`, past `--tolerance`, whose LAST is missing or older than that NEXT by more than `--tolerance`, is `MISSED`; otherwise the last run is `OK`. `n/a`/`-` on both columns is skipped. The check does not use logs | `src/systemd/list_timers.rs::fixture_last_runs_are_parsed`, `tests/cli_report.rs::timers_list_fixture_puts_the_last_run_in_the_report` (`tests/fixtures/systemd/list-timers.txt`) |
| `--crontab-user USER=PATH` reads one user's crontab explicitly | `src/app.rs::crontab_user_flag_reads_a_per_user_crontab_file`, `src/cli.rs::crontab_user_splits_on_equals` |
| Timezone- and DST-aware schedule expansion (system tz, `--tz`, per-line `CRON_TZ=`), cronie-style spring-gap/fall-fold handling for Europe/Amsterdam and America/New_York, strictly increasing output | `src/schedule.rs` unit tests (`amsterdam_*`, `new_york_*`, `times_are_strictly_increasing_for_a_busy_schedule`, the `expand_is_always_sorted_and_in_range` property test) |
| Reads systemd `.timer` units: `OnCalendar=` (`daily`, weekday ranges, `*:0/15`, explicit tz suffix), `Persistent=`, `RandomizedDelaySec=`, `AccuracySec=`, `Unit=`, cross-checked once by hand against `systemd-analyze calendar` | `tests/systemd_golden.rs` (`tests/fixtures/systemd/{daily,weekday-business,quarter-hour,tz-suffix}.timer`, `tests/fixtures/systemd/README.md`) |
| Several timer files with the same `Unit=` keep every description, combined, and a repeated description is stored once | `src/app.rs::two_timers_sharing_a_unit_combine_their_descriptions` |
| `Persistent=true` turns a missed-then-caught-up run into `LATE (caught up)`, not `MISSED` | `tests/systemd_golden.rs::persistent_missed_run_is_late_caught_up_not_missed` (`tests/fixtures/systemd/persistent-backup.timer`) |
| A monotonic-only trigger (`OnBootSec=`, ...) is flagged `UNSUPPORTED_TRIGGER`, not a crash | `tests/systemd_golden.rs::onbootsec_is_flagged_unsupported_with_no_oncalendar` (`tests/fixtures/systemd/unsupported-trigger.timer`) |
| Parses Debian/Ubuntu syslog and RHEL/cronie `/var/log/cron` (incl. `CMDOUT`/`CMDEND`): RFC3164, RFC5424, and rsyslog's high-precision file format (a leading RFC3339 timestamp, no priority; numeric offset or `Z`). A traditional or file-format line may start with `<PRI>`. The missing RFC3164 year is inferred across a Dec 31 → Jan 1 boundary | `tests/log_golden.rs` (`tests/fixtures/logs/{debian-syslog,rhel-cron,rfc5424,rsyslog-fileformat,year-boundary}.log`), `src/log/syslog.rs` (`pri_prefixed_rfc3164_cron_line_is_read`, `pri_prefixed_rfc3339_cron_line_is_read`) |
| Parses `journalctl -o json`: cron `MESSAGE` as a string or byte array, a timer unit's `Started ...` event or a one-shot unit's `Finished ...` event (`Starting ...` is not a run; `Started ...` and `Finished ...` for the same activation count once), a boot only when a kernel banner or an entry from the start of the boot says so, and log coverage across every valid entry | `tests/log_golden.rs::journal_json_matches_golden` (`tests/fixtures/logs/journal.jsonl`), `src/log/journal.rs` boot tests, `tests/ledger_golden.rs` (`journal_entry_after_the_last_event_keeps_a_later_run_missed`, `journal_excerpt_without_boot_evidence_does_not_catch_up_a_persistent_timer`, `kernel_banner_still_catches_up_a_persistent_timer`, `oneshot_finished_line_is_a_run_not_missed`, `started_and_finished_for_one_run_is_not_counted_twice`) |
| A `journalctl -o json` line that repeats a key is skipped with a warning and is not evidence: no run is recorded and the line does not extend log coverage | `src/log/journal.rs` (`duplicate_message_key_is_not_evidence`, `repeated_key_in_a_journal_object_is_not_evidence`) |
| An explicitly-named `.gz` log rotation is decompressed transparently | `tests/log_golden.rs::explicit_gz_rotation_is_decompressed_and_parsed` (`tests/fixtures/logs/syslog.2.gz`) |
| Broken log lines are counted as warnings, never fatal | `tests/log_golden.rs::garbage_lines_are_counted_not_fatal_and_match_golden` (`tests/fixtures/logs/garbage.log`); a non-UTF-8 byte is replaced and the rest of the file is still audited (`src/app.rs::invalid_utf8_in_a_syslog_is_a_warning_and_the_audit_continues`); a line with a NUL byte is skipped with an `N line(s) with a NUL byte skipped` warning and the rest of the log is still audited, unless at least one byte in four is NUL (`nuls*4 >= len`; `src/log/source.rs::nul_share_boundary_is_one_in_four_for_a_lossy_log`, `tests/cli_report.rs::nul_in_the_middle_of_a_syslog_skips_only_that_line`, `nul_in_the_middle_of_a_gz_syslog_skips_only_that_line`, `utf16_syslog_without_a_bom_is_still_refused_for_its_nul_bytes`) |
| The text report shows at most 50 warnings, with a `… and N more` summary line; `--json`'s `warnings` array always has the full list and count | `src/report/text.rs::many_warnings_are_truncated_in_text_but_full_in_json`, `exactly_the_warning_cap_is_not_truncated` |
| A million-line syslog parses within the test's release-mode time budget | `tests/log_golden.rs::one_million_lines_parses_fast_in_release` (budget only asserted with `cargo test --release`; see `tests/fixtures/logs/README.md`) |
| Matches each expected run to the nearest event within tolerance, independently per `(user, command)` or timer unit, including duplicate commands across users and a `%`-containing command | `tests/ledger_golden.rs` (`duplicate_command_two_users_matches_independently`, `percent_command_job_matches_on_the_split_command`) |
| `@reboot` never produces expected times; it only reports found boots as `REBOOT_ONLY` | `tests/ledger_golden.rs::reboot_job_reports_found_boots_for_info_only` |
| `MISSED` is only reported inside one input file's span, from that file's first recognised line to its last. A gap inside that span with no matching event is `MISSED`, including 19–21 October between a line on the 18th and a line on the 22nd in the same file. A gap between two files, including non-adjacent `s.1` and `s.2.gz`, a gap before the first file or after the last, an empty log, and a gap that falls on the window edge between two files are `NO_TRACE`. A time equal to a file's first or last line is inside that span. `--json` `log_coverage` is the outer bounds; the text report lists each file's span | `tests/ledger_golden.rs` (`log_gap_missed_only_in_covered_days`, `interior_gap_between_first_and_last_log_line_is_missed`, `gap_between_two_log_files_is_no_trace`, `gap_on_the_window_edge_between_two_files_is_no_trace`), `src/app.rs::non_adjacent_rotation_files_are_separate_coverage_spans`, `tests/cli_report.rs::no_trace_only_scenario_exits_three` (`tests/fixtures/report/no_trace/`) |
| Exit codes 0/1/2/3 as documented above | `src/report/mod.rs` unit tests, `tests/cli_report.rs` (`ok_scenario_exits_zero`, `missed_scenario_exits_one`, `late_scenario_exits_one`, `no_trace_only_scenario_exits_three`, `missing_since_is_a_usage_error_exit_two`, `until_earlier_than_since_is_a_usage_error_exit_two`, `until_equal_to_since_still_audits`, `more_than_one_stdin_source_is_a_usage_error_exit_two`, `full_stdout_exits_with_a_write_error_instead_of_panicking`) |
| SIGINT exits 130 and leaves no half-written report. An existing `--output` file is kept when the new report was not finished, and a signal before that writes nothing to stdout. Stderr is `cron-ledger: interrupted by SIGINT` | `tests/sigint.rs::sigint_on_blocked_stdin_writes_no_report_and_exits_130` |
| A `--since`/`--until` window is capped at 5,000,000 expected runs across the whole audit. Expanding one schedule stops once that schedule is already over the cap, so a wide window is a clear exit-2 error instead of allocating every instant first. Exactly 5,000,000 runs still audits | `src/app.rs::window_over_the_expected_run_cap_is_a_usage_error_exit_two`, `src/app.rs::window_at_the_expected_run_cap_still_audits`, `src/schedule.rs::expand_bounded_stops_materialising_once_over_budget`, `src/systemd/calendar.rs::expand_bounded_stops_materialising_once_over_budget` |
| Each named file, each crontab found by `--system-root`, each `--diff` report, and standard input may be at most 268435456 bytes (256 MiB). A larger input exits 2 and is not loaded. The message contains `file is larger than 268435456 bytes` | `tests/input_limit.rs::syslog_over_the_byte_limit_exits_2` |
| `--json` emits a `schema_version: 1` report | `tests/cli_report.rs::json_flag_produces_a_schema_versioned_report`, `tests/report_golden.rs` |
| The text report has no ANSI color on a non-tty, and respects `NO_COLOR` | `src/report/mod.rs::color_requires_both_a_tty_and_no_color_unset`, `tests/report_golden.rs::color_wraps_status_words_in_ansi_codes_when_enabled`, `tests/cli_report.rs::ok_scenario_exits_zero` |
| A named job source that yields no job, or a crontab or timer unit that is UTF-16/UTF-32 or has a NUL byte, is exit 2; a UTF-8 BOM is stripped; a `--system-root` that is not a directory, or whose scan found only unreadable crontab files, is exit 2, while an empty one warns and exits 0; a timer with only invalid or empty `OnCalendar=` is exit 2, and `OnCalendar=` with no value resets the list as in systemd; a non-UTF-8 argument is refused with exit 2; a spool file with a non-UTF-8 name is skipped with a warning; control characters in a path, newlines included, are escaped on stderr | `tests/cli_report.rs` (`timer_unit_with_a_utf8_bom_keeps_its_timer_section`, `utf16_crontab_is_refused_with_exit_two`, `utf16_crontab_without_a_bom_is_refused_for_its_nul_bytes`, `nonexistent_system_root_is_a_usage_error_exit_two`, `crontab_with_only_invalid_lines_is_a_usage_error_exit_two`, `empty_stdin_crontab_is_a_usage_error_exit_two`, `empty_crontab_file_is_a_usage_error_exit_two`, `empty_system_root_scan_warns_and_exits_zero`, `system_root_scan_with_only_unreadable_files_is_a_usage_error_exit_two`, `system_root_with_one_unreadable_and_one_readable_file_still_audits`, `timer_with_only_an_invalid_oncalendar_is_a_usage_error_exit_two`, `timer_with_an_empty_oncalendar_is_a_usage_error_exit_two`, `timer_with_one_valid_and_one_invalid_oncalendar_still_audits`, `existing_non_utf8_file_name_is_refused_as_such`, `spool_file_with_a_non_utf8_name_is_skipped_with_a_warning`, `control_characters_in_a_path_are_escaped_in_error_messages`, `newline_in_a_path_cannot_forge_a_second_stderr_line`, `newline_in_a_scanned_file_name_cannot_forge_a_stderr_line`, `empty_report_with_no_jobs_and_empty_syslog_is_in_english`), `src/systemd/timer_unit.rs::empty_oncalendar_resets_the_list`, `src/report/mod.rs::neutralize_escapes_esc_newline_cr_and_tab` |
| At most one input source may read from stdin (`-`) | `src/cli.rs::at_most_one_stdin_source`, `tests/cli_report.rs::more_than_one_stdin_source_is_a_usage_error_exit_two` |
| `--since`/`--until` accept an RFC3339 timestamp, kept as given, or a `systemd.time(7)` duration counted back from now (`30d`, `2h`). Any other value is a usage error and exits 2 | `src/app.rs::resolve_timestamp_accepts_rfc3339`, `src/app.rs::resolve_timestamp_accepts_a_duration_ago`, `src/app.rs::resolve_timestamp_rejects_garbage`, `src/cli.rs::usage_shows_until_accepts_rfc3339_or_a_duration` |
| `--help` output | `tests/cli_report.rs::help_flag_matches_golden_usage_text` (`tests/golden/help.txt`), `src/cli.rs::usage_shows_until_accepts_rfc3339_or_a_duration` |
| Read-only: running against a copy of every fixture with all write bits cleared changes nothing (not a byte, not an mtime) | `tests/readonly_fixtures.rs::running_against_a_read_only_fixture_copy_changes_nothing` |
| Offline: no `socket`/`connect` syscall and no write-capable `openat`, observed under `strace` in CI (skipped, not failed, where `strace` is unavailable) | `tests/readonly_strace.rs::strace_sees_no_network_calls_and_no_write_capable_openat` |
| Offline: the sample audit matches `examples/sample.expected.txt` inside a network namespace with no outbound route | `tests/offline.rs::sample_audit_matches_expected_output_with_outgoing_connections_blocked` |
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

`cargo test --no-fail-fast` last measured 413 passed, 0 failed, 0 ignored
(257 unit tests, 156 in the integration suites), on branch `prep/w24-fout`.

## Support

cron-ledger is free to use. If it saves you time, you can support the work via [Ko-fi](https://ko-fi.com/pepai).

## License

Licensed under either of [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE) at your option.
