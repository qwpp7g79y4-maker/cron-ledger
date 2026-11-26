---
name: cron-ledger
description: Audit whether crontab jobs and systemd timers actually ran, by comparing schedules with syslog or journal evidence. Use when the user mentions a missed cron job, a systemd timer that did not fire, crontab vs logs, or cron-ledger.
---

# cron-ledger

Use the local `cron-ledger` CLI (version 0.1.1). It is a read-only, after-the-fact audit of crontab and systemd-timer schedules against syslog or journal evidence. It does not change schedules, start units, or monitor jobs live.

## Install

If `cron-ledger` is not on `PATH`:

```sh
cargo install cron-ledger
```

This tool has no `--version` flag. Help is printed with `--help` or `-h`. Do not invent flags. The usage text from `--help` is:

```
cron-ledger [--crontab PATH|-]... [--crontab-user USER=PATH]... [--system-root DIR]
            [--timer-unit PATH]... [--timers-list PATH|-]...
            [--syslog PATH|-]... [--journal-json PATH|-]...
            --since <RFC3339|30d> [--until <RFC3339|30d>] [--tz Europe/Amsterdam]
            [--tolerance 5m] [--json]
```

`--since` is required. Values are an RFC3339 timestamp or a duration counted back from now, such as `30d` or `2h`. `-` means stdin; at most one source across all flags may be stdin. Default `--tolerance` is `5m`. Default `--system-root` is `/`.

## Flags

- `--crontab PATH|-` — system crontab file (repeatable)
- `--crontab-user USER=PATH` — per-user crontab
- `--system-root DIR` — root for the default scan (`DIR/etc/crontab`, `DIR/etc/cron.d/*`, `DIR/var/spool/cron/**`)
- `--timer-unit PATH` — systemd timer unit file (repeatable)
- `--timers-list PATH|-` — accepted, but not yet implemented: the path is ignored with a warning
- `--syslog PATH|-` — Debian/Ubuntu or RHEL/cronie syslog, including a `.gz` rotation (repeatable)
- `--journal-json PATH|-` — `journalctl -o json` output, one JSON object per line (repeatable)
- `--since` / `--until` — window
- `--tz` — timezone, for example `Europe/Amsterdam`
- `--tolerance` — how late a run may be and still count as `OK`
- `--json` — machine-readable report
- `-h`, `--help`

Without `--crontab`, `--crontab-user`, or `--timer-unit`, the tool scans `--system-root` (default `/`) itself. Passing one of those three flags selects only the named files and turns the default scan off. When you only want the files you named, pass `--system-root /nonexistent` as the README examples do, so a dropped flag cannot pick up the real `/etc/crontab`.

The tool does not run `crontab -l` itself; pass that output via stdin.

## Examples from the project README

Scan a snapshot plus a syslog for the last seven days:

```sh
cron-ledger --system-root /path/to/a/system/snapshot --syslog /var/log/syslog --since 7d
```

Point at one crontab and one syslog with a fixed window (fixture paths exist only in a clone):

```sh
cron-ledger --system-root /nonexistent \
  --crontab tests/fixtures/crontab/system.crontab \
  --syslog tests/fixtures/logs/debian-syslog.log \
  --since 2026-10-20T00:00:00Z --until 2026-10-21T00:00:00Z
```

Per-user crontab:

```sh
cron-ledger --system-root /nonexistent \
  --crontab-user alice=/var/spool/cron/crontabs/alice \
  --syslog /var/log/syslog \
  --since 30d
```

Saved journal:

```sh
journalctl -o json --since "7 days ago" > /tmp/journal.jsonl
cron-ledger --system-root /nonexistent \
  --crontab /etc/crontab --journal-json /tmp/journal.jsonl --since 7d
```

## Statuses and exit codes

Statuses: `OK`, `LATE`, `MISSED`, `NO_TRACE`, `REBOOT_ONLY`, `UNSUPPORTED_TRIGGER`.

- 0 — everything is `OK` (ignoring informational `REBOOT_ONLY` / `UNSUPPORTED_TRIGGER`)
- 1 — at least one `MISSED` or `LATE`
- 2 — usage or input/output error
- 3 — no `MISSED`/`LATE`, but at least one `NO_TRACE`

Absence of a log line is not always a missed run: if the logs do not cover that instant, the status is `NO_TRACE`.
