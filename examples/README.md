# Examples

Invented text fixtures for `cron-ledger`. They are not a live crontab and not a copy of a system log. The tool only reads them. Run the command from the repository root.

## Sample audit

`examples/sample.crontab` is one system crontab line (04:15 every day, user `audit`). `examples/sample.log` is one syslog line for that command, one second later. The window is 20 October 2026 in UTC, so the schedule expands to that single run. The line is inside the default tolerance, so the run is OK.

```
cron-ledger --system-root /nonexistent --crontab examples/sample.crontab --syslog examples/sample.log --since 2026-10-20T00:00:00Z --until 2026-10-21T00:00:00Z --tz UTC
```

Expected exit code: 0
