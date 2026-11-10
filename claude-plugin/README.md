# Cron Ledger

This plugin helps Claude use the `cron-ledger` command-line tool. The tool compares crontab and systemd timer schedules with what actually appears in syslog or journal logs, for a time window you choose.

## Use it

Install the tool with `cargo install cron-ledger`. Ask Claude whether a cron job or systemd timer really ran. Point it at crontab files, timer units, and log files, and always set `--since`. The tool is an after-the-fact audit. It does not change schedules, start timers, or watch jobs live.

## Data

The tool reads only the files and time window you pass. It does not run `crontab -l` itself. Pass that output on standard input if you want it included. This plugin does not send data to any service and stores nothing. Use `--system-root` or explicit file flags when you do not want a scan of the real `/etc/crontab`.
