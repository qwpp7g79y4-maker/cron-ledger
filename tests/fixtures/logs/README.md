# Log fixtures

Anonymized, hand-written examples of the syslog and journal formats this
project parses — not copied from any real system's logs.

- `debian-syslog.log` — Debian/Ubuntu `/var/log/syslog`: `CRON[pid]: (user)
  CMD (cmd)`, with one unrelated `sshd` line to check that non-cron syslog
  traffic is skipped silently rather than counted as garbage.
- `rhel-cron.log` — RHEL/cronie `/var/log/cron`: `CROND[pid]:` with the
  full `CMD`/`CMDOUT`/`CMDEND` sequence for one run.
- `rfc5424.log` — the same cron lines framed as RFC5424 (full ISO8601
  timestamp, structured `APP-NAME`/`PROCID` fields) instead of RFC3164.
- `year-boundary.log` — RFC3164 has no year; this checks that a run just
  before midnight on Dec 31 and one just after midnight on Jan 1 land in
  consecutive years, not the same one.
- `garbage.log` — a mix of well-formed cron lines and several kinds of
  broken ones (unrecognized month, no `(user) ACTION (arg)` body), to
  check that broken lines are counted as warnings, never fatal.
- `journal.jsonl` — `journalctl -o json` output: a cron `CMD` with
  `MESSAGE` as a plain string, one with `MESSAGE` as a byte array, a
  `_BOOT_ID` boot marker, and a systemd timer-activated unit's `Starting`
  (ignored) and `Started` (an event) messages.
- `syslog.2.gz` — one of `debian-syslog.log`'s lines, gzip-compressed, to
  check that an explicitly-named `.gz` rotation is read transparently.

The "1,000,000 lines in under 5 seconds" performance goal from the task
brief is only meaningfully measured in a release build; see
`one_million_lines_parses_fast_in_release` in `tests/log_golden.rs` — it
always runs and asserts the event count, but only asserts the 5s budget
when `cfg!(debug_assertions)` is false. Run `cargo test --release
one_million_lines` to check the actual number.
