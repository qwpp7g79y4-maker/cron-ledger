# Changelog

All notable changes to this project are documented in this file, in the
style of [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0] - 2026-10-24

First release. There is no earlier tag, so this lists every commit since
the first one, except commits that only update this file. The package
version stays 0.1.0.

### Fixed

- A non-UTF-8 byte in an explicitly named crontab or `.timer` file
  (or on stdin) failed the whole run with exit 2. It is now replaced
  and reported as a warning naming the file, the 1-based line and the
  byte offset (`<file>:<line>: invalid UTF-8 at byte <n>; ...`), like a
  syslog's; a missing file or a directory is still exit 2. Only the
  first invalid byte is named, and commands that differ only in the
  invalid byte read the same after replacement, so they count as one
  command.
- The crontab DST rule only treated an hour field of exactly `*` as a
  wildcard, so `0 */2 * * *` ran once in the repeated autumn hour while
  `0 */1 * * *` ran twice, and `*/15 2 * * *` was caught up after the
  spring jump. Like cronie's `MIN_STAR`/`HR_STAR`, a minute or hour
  field starting with `*` now makes the job wildcard: it runs in both
  copies of a repeated time and skips a time in the gap.
- A systemd calendar bare value or range bound outside its field's own
  range (`*:*:60`, `*:60:00`, `*-13-*`) parsed and stored a value that
  can never occur, unlike `systemd-analyze calendar`, which rejects it.
- A crontab or `OnCalendar=` date that can never occur (`0 0 31 2 *`,
  `*-02-31 00:00:00`) stored a job with 0 expected runs and no
  warning. Warn instead, without rejecting the job and without
  flagging a real but infrequent date like 29 February. (`92b0ae9`)
- A systemd calendar step wider than its field's range (`*:0/100:00`)
  parsed and expanded to minute 0 every hour, unlike
  `systemd-analyze calendar`, which rejects it. (`8adc457`)
- A systemd calendar `/step` applied to a bare `*` (`*:*/5:00`,
  `*-*-*/1`) was accepted as "every step", unlike
  `systemd-analyze calendar`, which rejects it; steps on an explicit
  start (`*:0/5:00`) still work.
- `--help` and both README synopses listed `--until` as RFC3339 only.
  `--until` takes an RFC3339 timestamp or a duration counted back from
  now, the same as `--since`. (`f900524`)
- The Dutch README no longer says the text report mixes Dutch and
  English labels. The report is English, and the Dutch exit-code table
  names an inverted `--until` as exit 2. (`643ad6e`)
- The default text report uses English labels (`expected`, `found`,
  `last run`, `Summary`, `Log coverage`, `Window`, `Tolerance`,
  `Warnings`, `… and N more`), the same language as `--help`, the
  status words, and `--json`. (`a232068`)
- Warn when a cron directory cannot be read, and note the example
  fixture paths. (`fd98b0e`)
- Count a oneshot timer run that only logs `Finished`. (`f884c3e`)
- Stop expanding a schedule once it is over the run cap. (`447dc3a`)
- Cap the number of warnings printed in the text report. (`6946727`)
- Reject `--since`/`--until` windows that expand to too many runs.
  (`e1ef0d1`)
- Ignore journal records with duplicate keys. (`762f4bc`)
- Do not scan the system root twice when `--crontab` is given.
  (`ae09b6b`)
- Do not report `MISSED` across a hole between syslog files.
  (`5519bf9`)
- Address review r2 findings B1-B3 and parser edge cases. (`91fc482`)
- Address review r1 blocking findings B1-B4. (`a618b82`)
- Reject a `--until` that is earlier than `--since` before the audit
  runs. An equal bound stays an empty audit. Timer files that name the
  same `Unit=` share one job key; their descriptions are combined, and
  a repeated description is kept once. (`3bf85d3`)
- Record a journal boot only when an entry shows it (a kernel banner,
  or any entry within two minutes of monotonic zero), and use that
  entry's own realtime. Take journal coverage from every entry with a
  usable `__REALTIME_TIMESTAMP`. Parse a leading RFC3339 token,
  including a numeric offset and `Z`, before the RFC3164 split.
  (`01901ea`)
- Point security reports at `Cargo.toml` for the version. Warn for
  `/etc/cron.d` names Debian cron skips and cronie still runs. Split
  crontab `%` the way cronie does, including an even run of
  backslashes, and take the schedule timezone from `CRON_TZ` only.
  Match jobs that share a command against one pool of runs, keep
  internal command whitespace, and bound a late match of the last
  expected time by the previous spacing. Accept a systemd unit start
  only from the manager's own message. Decode every member of a
  concatenated gzip log. Warn about a bad syslog timestamp only for a
  recognized cron line. Escape control characters in the text report.
  (`f2991cf`)
- Track each day-of-month and day-of-week field's leading `*`
  separately, so a stepped field such as `*/2` still counts as
  unrestricted for cron's OR/AND day rule. Accept whitespace around
  `=` in crontab environment lines. Canonicalize `\%` to `%` when
  matching a logged command. A persistent catch-up claims a late run
  only at or after the boot. Skip non-cron RFC5424 lines before
  checking their timestamp. (`685b458`)
- Skip `/etc/cron.d` files that cron itself ignores, so they cannot
  produce a false `MISSED`. Merge schedule descriptions when two jobs
  share a key. Prefer the journal `UNIT` field over `_SYSTEMD_UNIT`.
  Skip cron PAM session lines. Warn instead of panicking on an
  out-of-range syslog time or a span that does not fit in a
  `Duration`, and saturate when summing timer tolerances. The
  read-only tests require a completed audit. Document that
  `cargo package` is not run in CI. (`4e5b08b`)

### Changed

- Merge `fix/cronledger-fix-m1-0001` into `release/0.1.0`. (`a53632d`)
- Keep local-only file names out of `.gitignore`. (`e0febc7`)
- Run the strace check on Linux only. (`27c4f8f`)
- Keep dev-dependencies on Rust 1.85 (`proptest` 1.11), and run the
  permission-bit test on Unix only. (`d4523e7`)
- Test in CI on Linux, Windows, macOS ARM and Intel, plus MSRV 1.85
  with clippy. (`647b502`)
- Initial release of the read-only crontab and systemd-timer audit.
  (`b3ebefa`)

### Added

- Crontab parsing: the vixie-cron 5-field syntax, `@`-specials, the
  `/etc/crontab`/`cron.d` user column, per-user crontabs, `SHELL`/`PATH`/
  `MAILTO`/`TZ`/`CRON_TZ` env lines, comments, `%` line continuation, and
  the day-of-month/day-of-week OR rule. Invalid lines become a warning
  with `file:line`, never a crash.
- Timezone- and DST-aware schedule expansion (system timezone by
  default, `--tz`, or per-line `CRON_TZ=`), following cronie's
  spring-gap/fall-fold behavior.
- systemd `.timer` unit parsing: `OnCalendar=` (including `daily`,
  weekday ranges, step syntax, and an explicit trailing timezone),
  `Persistent=`, `RandomizedDelaySec=`, `AccuracySec=`, `Unit=`. A
  monotonic-only trigger (`OnBootSec=`, ...) is flagged
  `UNSUPPORTED_TRIGGER` rather than computed.
- Log parsing for Debian/Ubuntu syslog and RHEL/cronie `/var/log/cron`
  (RFC3164 and RFC5424 timestamps, missing-year inference across a
  year boundary) and `journalctl -o json`, including an explicitly
  named `.gz` log rotation.
- A matcher and ledger producing `OK`, `LATE`, `MISSED`, `NO_TRACE`,
  `REBOOT_ONLY`, and `UNSUPPORTED_TRIGGER`, respecting log coverage so
  missing logs are never mistaken for missing runs.
- Text and `--json` (`schema_version: 1`) reports, exit codes 0-3, a
  `cron-ledger` CLI (`--crontab`, `--crontab-user`, `--system-root`,
  `--timer-unit`, `--syslog`, `--journal-json`, `--since`/`--until`,
  `--tz`, `--tolerance`, `--json`), and `NO_COLOR`/non-tty handling.
- Read-only and offline proof in CI: a read-only fixture-copy test and
  an `strace` network/write-syscall check.

### Known gaps

- `--timers-list` (the text output of `systemctl list-timers --all`) is
  accepted as a flag but not implemented yet; it is ignored with a
  warning.
- No `anacron`, `fcron`, Kubernetes CronJobs, or macOS `launchd` support.
