# systemd timer fixtures: manual cross-check

The `OnCalendar=` golden test (`tests/systemd_golden.rs`) checks this
project's own calendar expansion against real `systemd-analyze calendar`
output, captured once by hand on this development machine
(`systemd 249`, system timezone `Europe/Amsterdam`) and hard-coded into the
test as the expected values. This is not re-run automatically in CI; if the
calendar grammar changes, re-capture it the same way and update the test.

Reference instant used as the expansion window's `since`:

```
$ date -u +%Y-%m-%dT%H:%M:%SZ
2026-10-22T06:28:15Z
```

Captured output immediately after, for each fixture's `OnCalendar=` value:

```
$ systemd-analyze calendar --iterations=3 "daily"
    Next elapse: Fri 2026-10-23 00:00:00 CEST
       Iter. #2: Sat 2026-10-24 00:00:00 CEST
       Iter. #3: Sun 2026-10-25 00:00:00 CEST

$ systemd-analyze calendar --iterations=3 "Mon..Fri *-*-* 09:00:00"
    Next elapse: Thu 2026-10-22 09:00:00 CEST
       Iter. #2: Fri 2026-10-23 09:00:00 CEST
       Iter. #3: Mon 2026-10-26 09:00:00 CET

$ systemd-analyze calendar --iterations=3 "*:0/15"
    Next elapse: Thu 2026-10-22 08:30:00 CEST
       Iter. #2: Thu 2026-10-22 08:45:00 CEST
       Iter. #3: Thu 2026-10-22 09:00:00 CEST

$ systemd-analyze calendar --iterations=3 "*-*-* 03:00:00 Europe/Amsterdam"
    Next elapse: Fri 2026-10-23 03:00:00 CEST
       Iter. #2: Sat 2026-10-24 03:00:00 CEST
       Iter. #3: Sun 2026-10-25 03:00:00 CET
```

`systemd-analyze calendar` on this systemd version does support
`--base-time=TIMESTAMP` (see `systemd-analyze calendar --help`); the
captures above were taken without it, computed relative to the current
wall clock at the moment each command ran. The test pins `since` to the
exact instant captured above instead of "now" so the comparison stays
reproducible on later runs, not because the flag is unavailable.

Note the third iteration of `Mon..Fri *-*-* 09:00:00` jumps from Friday
2026-10-23 straight to Monday 2026-10-26 (skipping the weekend) and changes
offset from CEST to CET — that's Amsterdam's 2026-10-25 DST fall-back
landing in between, confirmed by `zdump -v Europe/Amsterdam` separately
(see `src/schedule.rs`'s own DST tests for the cronie-style schedule
expander; this systemd one does not need special gap/fold handling here
because none of the three iterations lands on the transition day itself).

`persistent-backup.timer`'s `Persistent=true` "LATE (caught up)" behavior
and `unsupported-trigger.timer`'s `OnBootSec=` handling are covered by
unit tests in `src/systemd/timer_unit.rs` directly (no external tool to
cross-check against for those: they are this project's own stated policy,
not systemd-analyze output). That classification is wired into the
matcher/ledger via `ledger::matcher::apply_persistent_catchup` (unit tests
in `src/ledger/matcher.rs`, an integration test against `build_ledger` in
`tests/ledger_golden.rs`, and a CLI-level golden scenario in
`tests/fixtures/report/persistent/` driven from `tests/cli_report.rs`),
so the CLI itself can report `LATE (caught up)`, not just the classifier
function in isolation.
