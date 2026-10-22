# Changelog

All notable changes to this project are documented in this file, in the
style of [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0] - 2026-10-22

Initial release.

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
