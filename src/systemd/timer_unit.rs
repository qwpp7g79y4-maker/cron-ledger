//! Parsing of the `[Timer]` section of a systemd `.timer` unit file.

use std::time::Duration;

/// A parsed `.timer` unit's `[Timer]` section.
#[derive(Debug, Clone, PartialEq)]
pub struct TimerUnit {
    /// Raw `OnCalendar=` expressions, in file order (a unit may have
    /// several; the timer fires at the union of all of them).
    pub on_calendar: Vec<String>,
    pub persistent: bool,
    pub randomized_delay: Duration,
    /// Defaults to 1 minute, matching `systemd.timer(5)`'s documented
    /// default for `AccuracySec=`.
    pub accuracy: Duration,
    /// `Unit=`, if set; otherwise the service this timer activates is the
    /// same-named `.service` unit, which is the caller's concern, not the
    /// parser's.
    pub unit: Option<String>,
    /// `OnBootSec=`, `OnStartupSec=`, `OnActiveSec=`, `OnUnitActiveSec=`,
    /// `OnUnitInactiveSec=` directives seen but not supported in v1: these
    /// triggers depend on boot/activation history this tool has no way to
    /// read from files alone. Callers should report `UNSUPPORTED_TRIGGER`
    /// for a unit that has any of these and no `OnCalendar=`.
    pub unsupported_triggers: Vec<String>,
    pub warnings: Vec<String>,
}

impl Default for TimerUnit {
    fn default() -> Self {
        TimerUnit {
            on_calendar: Vec::new(),
            persistent: false,
            randomized_delay: Duration::ZERO,
            accuracy: Duration::from_secs(60),
            unit: None,
            unsupported_triggers: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

const MONOTONIC_TRIGGERS: &[&str] = &[
    "OnBootSec",
    "OnStartupSec",
    "OnActiveSec",
    "OnUnitActiveSec",
    "OnUnitInactiveSec",
];

/// Parse a systemd time-span value (`systemd.time(7)`), e.g. `5min`,
/// `1h30min`, `500ms`, or a bare number of seconds.
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("empty time span".to_string());
    }
    if let Ok(secs) = text.parse::<f64>() {
        if !secs.is_finite() {
            return Err(format!("'{text}' is not a finite time span"));
        }
        return Duration::try_from_secs_f64(secs.max(0.0))
            .map_err(|_| format!("'{text}' is out of range for a time span"));
    }

    let mut total_secs = 0.0_f64;
    let mut rest = text;
    let mut matched_any = false;
    while !rest.is_empty() {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let num_len = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        if num_len == 0 {
            return Err(format!("expected a number in time span '{text}'"));
        }
        let (num_str, after_num) = rest.split_at(num_len);
        let num: f64 = num_str
            .parse()
            .map_err(|_| format!("'{num_str}' is not a number"))?;
        let after_num = after_num.trim_start();
        let unit_len = after_num
            .find(|c: char| c.is_ascii_digit() || c.is_whitespace())
            .unwrap_or(after_num.len());
        if unit_len == 0 {
            return Err(format!("expected a unit in time span '{text}'"));
        }
        let (unit, after_unit) = after_num.split_at(unit_len);
        let secs_per_unit =
            unit_seconds(unit).ok_or_else(|| format!("'{unit}' is not a recognized time unit"))?;
        total_secs += num * secs_per_unit;
        matched_any = true;
        rest = after_unit;
    }
    if !matched_any {
        return Err(format!("'{text}' is not a valid time span"));
    }
    if !total_secs.is_finite() {
        return Err(format!("'{text}' is not a finite time span"));
    }
    Duration::try_from_secs_f64(total_secs.max(0.0))
        .map_err(|_| format!("'{text}' is out of range for a time span"))
}

fn unit_seconds(unit: &str) -> Option<f64> {
    const SEC: f64 = 1.0;
    const MIN: f64 = 60.0;
    const HOUR: f64 = 60.0 * MIN;
    const DAY: f64 = 24.0 * HOUR;
    const WEEK: f64 = 7.0 * DAY;
    // systemd.time(7): 1 month = 30 days, 1 year = 365.25 days.
    const MONTH: f64 = 30.0 * DAY;
    const YEAR: f64 = 365.25 * DAY;
    Some(match unit {
        "usec" | "us" | "µs" => 0.000_001,
        "msec" | "ms" => 0.001,
        "seconds" | "second" | "sec" | "s" => SEC,
        "minutes" | "minute" | "min" | "m" => MIN,
        "hours" | "hour" | "hr" | "h" => HOUR,
        "days" | "day" | "d" => DAY,
        "weeks" | "week" | "w" => WEEK,
        "months" | "month" | "M" => MONTH,
        "years" | "year" | "y" => YEAR,
        _ => return None,
    })
}

fn parse_bool(text: &str) -> Option<bool> {
    match text.to_ascii_lowercase().as_str() {
        "1" | "yes" | "true" | "on" => Some(true),
        "0" | "no" | "false" | "off" => Some(false),
        _ => None,
    }
}

/// Parse a `.timer` unit file's `[Timer]` section. Lines outside
/// `[Timer]`, comments (`#`/`;`), and blank lines are skipped. An
/// unrecognized directive inside `[Timer]` is ignored (systemd has many we
/// don't need); an unparseable value for a directive we do care about
/// becomes a warning, and that directive keeps its default.
pub fn parse_timer_unit(content: &str, file: &str) -> TimerUnit {
    let mut unit = TimerUnit::default();
    let mut in_timer_section = false;

    for (idx, raw_line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_timer_section = line.eq_ignore_ascii_case("[Timer]");
            continue;
        }
        if !in_timer_section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            unit.warnings.push(format!(
                "{file}:{line_no}: '{line}' is not a KEY=value directive"
            ));
            continue;
        };
        let key = key.trim();
        let value = value.trim();

        if MONOTONIC_TRIGGERS.contains(&key) {
            unit.unsupported_triggers.push(key.to_string());
            continue;
        }

        match key {
            "OnCalendar" => unit.on_calendar.push(value.to_string()),
            "Persistent" => match parse_bool(value) {
                Some(b) => unit.persistent = b,
                None => unit.warnings.push(format!(
                    "{file}:{line_no}: '{value}' is not a valid Persistent= value"
                )),
            },
            "RandomizedDelaySec" => match parse_duration(value) {
                Ok(d) => unit.randomized_delay = d,
                Err(e) => unit
                    .warnings
                    .push(format!("{file}:{line_no}: RandomizedDelaySec: {e}")),
            },
            "AccuracySec" => match parse_duration(value) {
                Ok(d) => unit.accuracy = d,
                Err(e) => unit
                    .warnings
                    .push(format!("{file}:{line_no}: AccuracySec: {e}")),
            },
            "Unit" => unit.unit = Some(value.to_string()),
            _ => {}
        }
    }

    unit
}

/// The matching tolerance for a timer: the caller's base tolerance,
/// widened by this timer's `RandomizedDelaySec=` and `AccuracySec=`
/// (tolerance + RandomizedDelaySec + AccuracySec).
pub fn effective_tolerance(base: Duration, timer: &TimerUnit) -> Duration {
    base.saturating_add(timer.randomized_delay)
        .saturating_add(timer.accuracy)
}

/// Outcome of one expected run that fell before the machine booted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissedRunStatus {
    /// `Persistent=true` caught the run up at boot; late by `by` relative
    /// to its original expected time.
    LateCaughtUp { by: Duration },
    /// No catch-up: the run was simply missed.
    Missed,
}

/// Classify a single expected run (`expected`) that fell before
/// `boot_time`, per `Persistent=`. With `Persistent=true`, systemd runs
/// the timer once at the next start if it missed its last elapse while
/// stopped; without it, a missed run is simply skipped.
pub fn evaluate_missed_run(
    expected: jiff::Timestamp,
    boot_time: jiff::Timestamp,
    persistent: bool,
) -> MissedRunStatus {
    if persistent && boot_time > expected {
        let by = Duration::try_from(boot_time.duration_since(expected)).unwrap_or(Duration::ZERO);
        MissedRunStatus::LateCaughtUp { by }
    } else {
        MissedRunStatus::Missed
    }
}

/// Classify every expected run in `expected` that fell before
/// `boot_time`, per `Persistent=`. `systemd.timer(5)` is explicit that
/// `Persistent=true` triggers the unit "immediately if it would have been
/// triggered at least once" while stopped — i.e. at most **one** catch-up
/// run happens at boot, not one per missed tick. So only the most recent
/// (chronologically last) missed run is `LateCaughtUp`; every earlier one
/// in the same downtime window stays `Missed`, same as without
/// `Persistent=` at all.
///
/// `expected` need not be sorted; the result is in the same order as the
/// input.
pub fn evaluate_missed_runs(
    expected: &[jiff::Timestamp],
    boot_time: jiff::Timestamp,
    persistent: bool,
) -> Vec<MissedRunStatus> {
    let most_recent_missed = expected.iter().filter(|e| boot_time > **e).max().copied();
    expected
        .iter()
        .map(|&e| {
            if persistent && boot_time > e && Some(e) == most_recent_missed {
                let by = Duration::try_from(boot_time.duration_since(e)).unwrap_or(Duration::ZERO);
                MissedRunStatus::LateCaughtUp { by }
            } else {
                MissedRunStatus::Missed
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_timer() {
        let content = "\
[Unit]
Description=Nightly backup

[Timer]
OnCalendar=*-*-* 03:00:00
Persistent=true
RandomizedDelaySec=1h
AccuracySec=1min
";
        let unit = parse_timer_unit(content, "backup.timer");
        assert_eq!(unit.warnings, Vec::<String>::new());
        assert_eq!(unit.on_calendar, vec!["*-*-* 03:00:00".to_string()]);
        assert!(unit.persistent);
        assert_eq!(unit.randomized_delay, Duration::from_secs(3600));
        assert_eq!(unit.accuracy, Duration::from_secs(60));
    }

    #[test]
    fn multiple_oncalendar_lines_all_kept() {
        let content = "\
[Timer]
OnCalendar=Mon *-*-* 09:00:00
OnCalendar=Fri *-*-* 17:00:00
";
        let unit = parse_timer_unit(content, "f");
        assert_eq!(
            unit.on_calendar,
            vec![
                "Mon *-*-* 09:00:00".to_string(),
                "Fri *-*-* 17:00:00".to_string()
            ]
        );
    }

    #[test]
    fn defaults_without_timer_section() {
        let unit = parse_timer_unit("[Unit]\nDescription=x\n", "f");
        assert_eq!(unit, TimerUnit::default());
    }

    #[test]
    fn onbootsec_is_unsupported_not_a_crash() {
        let content = "\
[Timer]
OnBootSec=5min
OnUnitActiveSec=1h
";
        let unit = parse_timer_unit(content, "f");
        assert_eq!(unit.warnings, Vec::<String>::new());
        assert_eq!(
            unit.unsupported_triggers,
            vec!["OnBootSec".to_string(), "OnUnitActiveSec".to_string()]
        );
        assert!(unit.on_calendar.is_empty());
    }

    #[test]
    fn bad_directive_line_is_a_warning() {
        let unit = parse_timer_unit("[Timer]\nnonsense\n", "f");
        assert_eq!(unit.warnings.len(), 1);
        assert!(unit.warnings[0].contains("f:2"));
    }

    #[test]
    fn tolerance_past_i64_seconds_parses_without_panicking() {
        // `--tolerance` values at 2^63 seconds used to reach the matcher
        // and overflow the search window. Parsing must hand back a duration
        // at least that large (f64 rounding may nudge the exact integer).
        for text in ["9223372036854775807s", "9223372036854775808s"] {
            let got = parse_duration(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert!(
                got.as_secs() >= i64::MAX as u64,
                "{text} -> {}s",
                got.as_secs()
            );
        }
        // u64::MAX seconds is not representable as f64 (it rounds to 2^64)
        // and is rejected rather than wrapped.
        assert!(parse_duration("18446744073709551615s").is_err());
    }

    #[test]
    fn duration_parsing_units() {
        assert_eq!(parse_duration("5").unwrap(), Duration::from_secs(5));
        assert_eq!(parse_duration("5min").unwrap(), Duration::from_secs(300));
        assert_eq!(
            parse_duration("1h30min").unwrap(),
            Duration::from_secs(5400)
        );
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(
            parse_duration("1h 30min").unwrap(),
            Duration::from_secs(5400)
        );
    }

    #[test]
    fn duration_parsing_rejects_garbage() {
        assert!(parse_duration("banana").is_err());
        assert!(parse_duration("").is_err());
        assert!(parse_duration("5bananas").is_err());
    }

    #[test]
    fn duration_parsing_rejects_non_finite_values_instead_of_panicking() {
        // Regression: `f64::from_str` accepts "inf"/"nan", and
        // `Duration::from_secs_f64` panics on a non-finite input. A
        // `RandomizedDelaySec=inf` (or a typo'd `=nan`) used to crash the
        // whole parse instead of becoming a warning like any other
        // unparseable duration.
        assert!(parse_duration("inf").is_err());
        assert!(parse_duration("-inf").is_err());
        assert!(parse_duration("infinity").is_err());
        assert!(parse_duration("nan").is_err());
    }

    #[test]
    fn randomized_delay_sec_inf_is_a_warning_not_a_panic() {
        let content = "[Timer]\nRandomizedDelaySec=inf\n";
        let unit = parse_timer_unit(content, "f");
        assert_eq!(unit.randomized_delay, Duration::ZERO);
        assert_eq!(unit.warnings.len(), 1);
        assert!(unit.warnings[0].contains("RandomizedDelaySec"));
    }

    #[test]
    fn duration_parsing_rejects_a_finite_value_too_big_for_duration() {
        // `1e300` is finite, but far beyond `Duration`'s range; the old
        // `Duration::from_secs_f64` call panicked on it instead of
        // returning an error like any other bad duration.
        assert!(parse_duration("1e300").is_err());
        assert!(parse_duration("99999999999999999999years").is_err());
    }

    #[test]
    fn randomized_delay_sec_huge_value_is_a_warning_not_a_panic() {
        let content = "[Timer]\nRandomizedDelaySec=1e300\n";
        let unit = parse_timer_unit(content, "f");
        assert_eq!(unit.randomized_delay, Duration::ZERO);
        assert_eq!(unit.warnings.len(), 1);
        assert!(unit.warnings[0].contains("RandomizedDelaySec"));
    }

    #[test]
    fn effective_tolerance_saturates_instead_of_overflowing() {
        let unit = TimerUnit {
            randomized_delay: Duration::MAX,
            accuracy: Duration::MAX,
            ..TimerUnit::default()
        };
        let tol = effective_tolerance(Duration::from_secs(300), &unit);
        assert_eq!(tol, Duration::MAX);
    }

    #[test]
    fn effective_tolerance_adds_randomized_delay_and_accuracy() {
        let unit = TimerUnit {
            randomized_delay: Duration::from_secs(60 * 60),
            accuracy: Duration::from_secs(60),
            ..TimerUnit::default()
        };
        let tol = effective_tolerance(Duration::from_secs(300), &unit);
        assert_eq!(tol, Duration::from_secs(300 + 3600 + 60));
    }

    #[test]
    fn persistent_missed_run_is_caught_up_late() {
        let expected: jiff::Timestamp = "2026-10-20T03:00:00Z".parse().unwrap();
        let boot: jiff::Timestamp = "2026-10-20T08:12:00Z".parse().unwrap();
        let status = evaluate_missed_run(expected, boot, true);
        assert_eq!(
            status,
            MissedRunStatus::LateCaughtUp {
                by: Duration::from_secs(5 * 3600 + 12 * 60)
            }
        );
    }

    #[test]
    fn non_persistent_missed_run_is_simply_missed() {
        let expected: jiff::Timestamp = "2026-10-20T03:00:00Z".parse().unwrap();
        let boot: jiff::Timestamp = "2026-10-20T08:12:00Z".parse().unwrap();
        assert_eq!(
            evaluate_missed_run(expected, boot, false),
            MissedRunStatus::Missed
        );
    }

    #[test]
    fn persistent_catches_up_only_the_most_recent_of_several_missed_runs() {
        // Golden scenario: a daily timer, machine off for three days
        // before booting back up. Regression: calling
        // `evaluate_missed_run` once per missed tick marks *every* one
        // of them `LateCaughtUp`, but systemd.timer(5) only triggers the
        // unit once at boot ("at least once"), so only the most recent
        // miss is actually caught up; the two earlier ones were simply
        // never going to run and stay `Missed`.
        let expected: Vec<jiff::Timestamp> = [
            "2026-10-18T03:00:00Z",
            "2026-10-19T03:00:00Z",
            "2026-10-20T03:00:00Z",
        ]
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();
        let boot: jiff::Timestamp = "2026-10-20T08:12:00Z".parse().unwrap();

        let got = evaluate_missed_runs(&expected, boot, true);
        assert_eq!(
            got,
            vec![
                MissedRunStatus::Missed,
                MissedRunStatus::Missed,
                MissedRunStatus::LateCaughtUp {
                    by: Duration::from_secs(5 * 3600 + 12 * 60)
                },
            ]
        );
    }

    #[test]
    fn non_persistent_several_missed_runs_all_stay_missed() {
        let expected: Vec<jiff::Timestamp> = [
            "2026-10-18T03:00:00Z",
            "2026-10-19T03:00:00Z",
            "2026-10-20T03:00:00Z",
        ]
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();
        let boot: jiff::Timestamp = "2026-10-20T08:12:00Z".parse().unwrap();

        let got = evaluate_missed_runs(&expected, boot, false);
        assert_eq!(got, vec![MissedRunStatus::Missed; 3]);
    }

    #[test]
    fn persistent_but_boot_before_expected_is_not_a_catchup() {
        // The run hasn't happened yet; this isn't a missed-run situation.
        let expected: jiff::Timestamp = "2026-10-20T03:00:00Z".parse().unwrap();
        let boot: jiff::Timestamp = "2026-10-20T01:00:00Z".parse().unwrap();
        assert_eq!(
            evaluate_missed_run(expected, boot, true),
            MissedRunStatus::Missed
        );
    }
}
