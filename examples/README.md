# Examples

Invented text fixtures for `cron-ledger`. They are not a live crontab and not a copy of a system log. The tool only reads them. Run the command from the repository root.

## Sample audit

`examples/sample.crontab` is one system crontab line (04:15 every day, user `audit`). `examples/sample.log` is one syslog line for that command, one second later. The window is 20 October 2026 in UTC, so the schedule expands to that single run. The line is inside the default tolerance, so the run is OK.

```
cron-ledger --system-root /nonexistent --crontab examples/sample.crontab --syslog examples/sample.log --since 2026-10-20T00:00:00Z --until 2026-10-21T00:00:00Z --tz UTC
```

Expected exit code: 0

The expected output is in `examples/sample.expected.txt`.

## Backup audit (LATE and MISSED)

`examples/backup.crontab` is a per-user crontab for the user `ops`: a backup at 02:30 and another job at 12:00, both daily. `examples/backup.log` is invented syslog. The backup ran on all three days, but 11 minutes late on 21 October, which is past the default tolerance of 5 minutes. The 12:00 job ran only on 22 October, so the other two runs are MISSED. The command exits 1 because not everything is OK. The expected output is in `examples/backup.expected.txt`; `tests/examples_golden.rs` compares both examples with their stored output.

```
cron-ledger --system-root /nonexistent --crontab-user ops=examples/backup.crontab --syslog examples/backup.log --since 2026-10-20T00:00:00Z --until 2026-10-23T00:00:00Z --tz UTC
```

Expected exit code: 1
