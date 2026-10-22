//! Expansion of a parsed crontab [`Schedule`] into concrete expected run
//! instants within a `[since, until)` window, timezone- and DST-aware.
//!
//! DST handling follows the behavior described in the project's task
//! brief as "like cronie": a civil time that falls in a spring-forward gap
//! is reported as a single run right at the moment the clock resumes; a
//! civil time repeated by a fall-back fold runs once, at its earlier
//! occurrence, for a fixed hour, but once per physical hour for a wildcard
//! hour field. This has **not** been independently verified against
//! cronie's source or a VM.

use crate::crontab::{matches_day, Field, Schedule};
use jiff::tz::{AmbiguousOffset, Disambiguation, TimeZone};
use jiff::{Timestamp, Zoned};

/// The system timezone (read from `/etc/localtime` or `TZ`), falling back
/// to UTC if it cannot be determined.
pub fn default_timezone() -> TimeZone {
    TimeZone::try_system().unwrap_or(TimeZone::UTC)
}

/// Resolve a timezone name (from `--tz`, `TZ=`, or `CRON_TZ=`) to a
/// [`TimeZone`].
pub fn resolve_timezone(name: &str) -> Result<TimeZone, String> {
    TimeZone::get(name).map_err(|e| format!("unknown timezone '{name}': {e}"))
}

fn field_values(field: &Field, min: u32, max: u32) -> Vec<u32> {
    match field {
        Field::Star => (min..=max).collect(),
        Field::Values(set) => set.iter().map(|v| u32::from(*v)).collect(),
    }
}

/// Expand `schedule` in `tz` into every expected run instant in
/// `[since, until)`. The result is sorted, strictly increasing, and
/// deduplicated.
pub fn expand(
    schedule: &Schedule,
    tz: &TimeZone,
    since: Timestamp,
    until: Timestamp,
) -> Vec<Timestamp> {
    if since >= until {
        return Vec::new();
    }

    let months = field_values(&schedule.month, 1, 12);
    let hours = field_values(&schedule.hour, 0, 23);
    let minutes = field_values(&schedule.minute, 0, 59);
    let hour_is_wildcard = schedule.hour == Field::Star;

    let mut date = since.to_zoned(tz.clone()).date();
    let until_date = until.to_zoned(tz.clone()).date();
    let mut candidates: Vec<Timestamp> = Vec::new();

    loop {
        if date > until_date {
            break;
        }
        if months.contains(&u32::from(date.month() as u8)) {
            let dom = u32::from(date.day() as u8);
            let dow = u32::from(date.weekday().to_sunday_zero_offset() as u8);
            if matches_day(&schedule.dom, &schedule.dow, dom, dow) {
                // Every fixed-hour tick that falls in the same day's
                // spring-forward gap is collected here first (in
                // ascending civil-time order, since `hours`/`minutes` are
                // sorted), so they can be anchored together below instead
                // of each independently snapping to the same instant.
                let mut gap_hits: Vec<Timestamp> = Vec::new();
                for &h in &hours {
                    for &m in &minutes {
                        let civil = date.at(h as i8, m as i8, 0, 0);
                        push_resolved(tz, civil, hour_is_wildcard, &mut candidates, &mut gap_hits);
                    }
                }
                push_gap_hits(tz, gap_hits, &mut candidates);
            }
        }
        date = match date.tomorrow() {
            Ok(d) => d,
            Err(_) => break,
        };
    }

    candidates.sort();
    candidates.dedup();
    candidates.retain(|t| *t >= since && *t < until);
    candidates
}

fn push_resolved(
    tz: &TimeZone,
    civil: jiff::civil::DateTime,
    hour_is_wildcard: bool,
    out: &mut Vec<Timestamp>,
    gap_hits: &mut Vec<Timestamp>,
) {
    let ambiguous = tz.to_ambiguous_zoned(civil);
    match ambiguous.offset() {
        AmbiguousOffset::Unambiguous { .. } => {
            if let Ok(z) = ambiguous.disambiguate(Disambiguation::Compatible) {
                out.push(z.timestamp());
            }
        }
        AmbiguousOffset::Gap { .. } => {
            if hour_is_wildcard {
                // This civil hour never physically occurs; a wildcard-hour
                // job simply has one fewer tick that day.
                return;
            }
            // A fixed-hour job that falls in the gap is reported as
            // running right after the jump; the exact instant is worked
            // out once all of the day's gap hits are known, by
            // `push_gap_hits` below.
            if let Ok(after) = ambiguous.disambiguate(Disambiguation::Compatible) {
                gap_hits.push(after.timestamp());
            }
        }
        AmbiguousOffset::Fold { .. } => {
            if let Ok(earlier) = ambiguous.clone().disambiguate(Disambiguation::Earlier) {
                out.push(earlier.timestamp());
            }
            if hour_is_wildcard {
                if let Ok(later) = ambiguous.disambiguate(Disambiguation::Later) {
                    out.push(later.timestamp());
                }
            }
        }
    }
}

/// Resolve one day's collected spring-forward-gap hits (each already the
/// gap-shifted wall-clock reading, via `Disambiguation::Compatible`) into
/// the instants actually reported: the earliest hit is anchored to the
/// transition instant itself (so a single gap-hour job runs right at the
/// moment the clock resumes), and every later hit on
/// the same day keeps its original spacing from that earliest hit. This
/// keeps several distinct missed minutes from collapsing onto one
/// instant while still placing the first of them exactly at the jump.
fn push_gap_hits(tz: &TimeZone, gap_hits: Vec<Timestamp>, out: &mut Vec<Timestamp>) {
    let Some(&anchor) = gap_hits.first() else {
        return;
    };
    // `preceding` is exclusive of its argument, and `anchor` can land
    // exactly on the transition instant itself (e.g. civil 02:00 in a
    // 02:00->03:00 gap), so search just past it to find that transition
    // rather than skipping over it to an earlier one.
    let just_after_anchor = anchor
        .checked_add(jiff::Span::new().nanoseconds(1))
        .unwrap_or(anchor);
    let trans = tz
        .preceding(just_after_anchor)
        .next()
        .map(|t| t.timestamp())
        .unwrap_or(anchor);
    for hit in gap_hits {
        out.push(trans + (hit - anchor));
    }
}

/// Convenience: `expand`, but returning [`Zoned`] values in `tz` for
/// display.
pub fn expand_zoned(
    schedule: &Schedule,
    tz: &TimeZone,
    since: Timestamp,
    until: Timestamp,
) -> Vec<Zoned> {
    expand(schedule, tz, since, until)
        .into_iter()
        .map(|t| t.to_zoned(tz.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crontab::parse_user_crontab;
    use crate::crontab::ScheduleKind;
    use jiff::civil::date;
    use proptest::strategy::Strategy;

    fn schedule_of(line: &str) -> Schedule {
        let result = parse_user_crontab(line, "f", None);
        match &result.jobs[0].schedule {
            ScheduleKind::Standard(s) => s.clone(),
            ScheduleKind::Reboot => panic!("expected a standard schedule"),
        }
    }

    fn window(tz: &TimeZone, from: jiff::civil::Date, days: i64) -> (Timestamp, Timestamp) {
        let since = from
            .at(0, 0, 0, 0)
            .to_zoned(tz.clone())
            .unwrap()
            .timestamp();
        let until_date = from.checked_add(jiff::Span::new().days(days)).unwrap();
        let until = until_date
            .at(0, 0, 0, 0)
            .to_zoned(tz.clone())
            .unwrap()
            .timestamp();
        (since, until)
    }

    #[test]
    fn amsterdam_spring_gap_fixed_time_runs_once_right_after_jump() {
        let tz = TimeZone::get("Europe/Amsterdam").unwrap();
        let schedule = schedule_of("30 2 * * * echo hi\n");
        let (since, until) = window(&tz, date(2026, 3, 29), 1);
        let got = expand(&schedule, &tz, since, until);
        let zoned: Vec<String> = got
            .iter()
            .map(|t| t.to_zoned(tz.clone()).to_string())
            .collect();
        assert_eq!(zoned, vec!["2026-03-29T03:00:00+02:00[Europe/Amsterdam]"]);
    }

    #[test]
    fn amsterdam_spring_gap_at_gap_start_runs_once_right_after_jump() {
        // Regression: the gap's first minute (civil 02:00, the exact
        // instant the clock resumes 02:00->03:00) used to vanish entirely,
        // because `tz.preceding(after.timestamp())` is exclusive and
        // `after.timestamp()` for this civil time lands exactly on the
        // transition itself, so the search skipped past it to the
        // *previous* (autumn 2025) transition and the result was filtered
        // out of the window.
        let tz = TimeZone::get("Europe/Amsterdam").unwrap();
        let schedule = schedule_of("0 2 * * * echo hi\n");
        let (since, until) = window(&tz, date(2026, 3, 29), 1);
        let got = expand(&schedule, &tz, since, until);
        let zoned: Vec<String> = got
            .iter()
            .map(|t| t.to_zoned(tz.clone()).to_string())
            .collect();
        assert_eq!(zoned, vec!["2026-03-29T03:00:00+02:00[Europe/Amsterdam]"]);
    }

    #[test]
    fn amsterdam_spring_gap_several_minutes_shift_to_distinct_times() {
        // Regression: every one of these four minutes falls inside the
        // same spring-forward gap. All four used to collapse onto the
        // single transition instant and then get deduplicated away to
        // just one run; each missed minute is a separate expected cron
        // tick and must survive as its own (shifted) run, in order,
        // anchored so the earliest one lands right at the transition.
        let tz = TimeZone::get("Europe/Amsterdam").unwrap();
        let schedule = schedule_of("0,15,30,45 2 * * * echo hi\n");
        let (since, until) = window(&tz, date(2026, 3, 29), 1);
        let got = expand(&schedule, &tz, since, until);
        let zoned: Vec<String> = got
            .iter()
            .map(|t| t.to_zoned(tz.clone()).to_string())
            .collect();
        assert_eq!(
            zoned,
            vec![
                "2026-03-29T03:00:00+02:00[Europe/Amsterdam]",
                "2026-03-29T03:15:00+02:00[Europe/Amsterdam]",
                "2026-03-29T03:30:00+02:00[Europe/Amsterdam]",
                "2026-03-29T03:45:00+02:00[Europe/Amsterdam]",
            ]
        );
    }

    #[test]
    fn amsterdam_fall_fold_fixed_time_runs_once() {
        let tz = TimeZone::get("Europe/Amsterdam").unwrap();
        let schedule = schedule_of("30 2 * * * echo hi\n");
        let (since, until) = window(&tz, date(2026, 10, 25), 1);
        let got = expand(&schedule, &tz, since, until);
        assert_eq!(
            got.len(),
            1,
            "fixed time must run exactly once on a fold day"
        );
        let zoned = got[0].to_zoned(tz.clone());
        assert_eq!(
            zoned.to_string(),
            "2026-10-25T02:30:00+02:00[Europe/Amsterdam]"
        );
    }

    #[test]
    fn amsterdam_fall_fold_wildcard_hour_runs_twenty_five_times() {
        let tz = TimeZone::get("Europe/Amsterdam").unwrap();
        let schedule = schedule_of("0 * * * * echo hi\n");
        let (since, until) = window(&tz, date(2026, 10, 25), 1);
        let got = expand(&schedule, &tz, since, until);
        assert_eq!(
            got.len(),
            25,
            "wildcard hour must tick once per physical hour"
        );
    }

    #[test]
    fn amsterdam_normal_day_wildcard_hour_runs_twenty_four_times() {
        let tz = TimeZone::get("Europe/Amsterdam").unwrap();
        let schedule = schedule_of("0 * * * * echo hi\n");
        let (since, until) = window(&tz, date(2026, 6, 15), 1);
        let got = expand(&schedule, &tz, since, until);
        assert_eq!(got.len(), 24);
    }

    #[test]
    fn amsterdam_spring_gap_wildcard_hour_runs_twenty_three_times() {
        let tz = TimeZone::get("Europe/Amsterdam").unwrap();
        let schedule = schedule_of("0 * * * * echo hi\n");
        let (since, until) = window(&tz, date(2026, 3, 29), 1);
        let got = expand(&schedule, &tz, since, until);
        assert_eq!(got.len(), 23, "the skipped hour never physically ticks");
    }

    #[test]
    fn new_york_spring_gap_fixed_time_runs_once_right_after_jump() {
        let tz = TimeZone::get("America/New_York").unwrap();
        let schedule = schedule_of("30 2 * * * echo hi\n");
        let (since, until) = window(&tz, date(2026, 3, 8), 1);
        let got = expand(&schedule, &tz, since, until);
        let zoned: Vec<String> = got
            .iter()
            .map(|t| t.to_zoned(tz.clone()).to_string())
            .collect();
        assert_eq!(zoned, vec!["2026-03-08T03:00:00-04:00[America/New_York]"]);
    }

    #[test]
    fn new_york_fall_fold_fixed_time_runs_once() {
        let tz = TimeZone::get("America/New_York").unwrap();
        let schedule = schedule_of("30 1 * * * echo hi\n");
        let (since, until) = window(&tz, date(2026, 11, 1), 1);
        let got = expand(&schedule, &tz, since, until);
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn times_are_strictly_increasing_for_a_busy_schedule() {
        let tz = TimeZone::get("Europe/Amsterdam").unwrap();
        // Covers both 2026 DST transitions in one window.
        let schedule = schedule_of("*/7 * * * * echo hi\n");
        let (since, until) = window(&tz, date(2026, 1, 1), 365);
        let got = expand(&schedule, &tz, since, until);
        assert!(got.len() > 1000);
        for pair in got.windows(2) {
            assert!(pair[0] < pair[1], "not strictly increasing: {pair:?}");
        }
    }

    fn arb_field(min: u8, max: u8) -> impl proptest::strategy::Strategy<Value = Field> {
        use proptest::prelude::*;
        prop_oneof![
            3 => Just(Field::Star),
            7 => prop::collection::btree_set(min..=max, 1..6).prop_map(Field::Values),
        ]
    }

    fn arb_schedule() -> impl proptest::strategy::Strategy<Value = Schedule> {
        (
            arb_field(0, 59),
            arb_field(0, 23),
            arb_field(1, 31),
            arb_field(1, 12),
            arb_field(0, 6),
        )
            .prop_map(|(minute, hour, dom, month, dow)| Schedule {
                minute,
                hour,
                dom,
                month,
                dow,
            })
    }

    proptest::proptest! {
        // For any schedule, timezone, and window, expanded run times are
        // sorted, strictly increasing (no duplicate instants), and land
        // inside the requested [since, until) window. This is the
        // property the DST gap/fold handling above must never break,
        // across field shapes the fixed examples don't cover.
        #[test]
        fn expand_is_always_sorted_and_in_range(
            schedule in arb_schedule(),
            tz_index in 0usize..3,
            start_day in 0i64..330,
            span_days in 1i64..35,
        ) {
            let tz_name = ["UTC", "Europe/Amsterdam", "America/New_York"][tz_index];
            let tz = TimeZone::get(tz_name).unwrap();
            let from = date(2026, 1, 1)
                .checked_add(jiff::Span::new().days(start_day))
                .unwrap();
            let (since, until) = window(&tz, from, span_days);
            let got = expand(&schedule, &tz, since, until);
            for pair in got.windows(2) {
                proptest::prop_assert!(pair[0] < pair[1], "not strictly increasing: {:?}", pair);
            }
            for t in &got {
                proptest::prop_assert!(*t >= since && *t < until);
            }
        }
    }
}
