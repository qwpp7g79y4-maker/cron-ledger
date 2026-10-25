//! Parsing and expansion of systemd calendar event syntax
//! (`systemd.time(7)`'s `OnCalendar=` grammar), a subset covering:
//! named aliases (`daily`, `weekly`, ...), an optional weekday list
//! (`Mon..Fri`), a `year-month-day` date (each component `*`, a list,
//! a `..` range, and/or a `/step`), an `hour:minute[:second]` time with the
//! same list/range/step syntax, and an optional trailing IANA timezone.
//!
//! Not supported (produces a parse error, not a panic): the `~` "nearest
//! day of month" syntax, and comma-separated full `OnCalendar=` repetition
//! inside a single expression (use multiple `OnCalendar=` lines instead,
//! which is what real unit files do for that case).

use std::collections::BTreeSet;

/// One numeric calendar component (year, month, day, hour, minute, or
/// second): either unrestricted, or a comma-separated list of single
/// values, `start..end` ranges, and/or `/step` repeats (open-ended when
/// no range end is given, as in systemd's `year` component).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalComponent {
    Any,
    List(Vec<CalItem>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalItem {
    pub start: u32,
    pub end: Option<u32>,
    pub step: Option<u32>,
}

impl CalComponent {
    fn contains(&self, v: u32) -> bool {
        match self {
            CalComponent::Any => true,
            CalComponent::List(items) => items.iter().any(|item| item_contains(item, v)),
        }
    }

    /// Every value in `min..=max` that this component matches. Used for
    /// the small, fixed-size components (month/day/hour/minute/second);
    /// `year` is checked with [`CalComponent::contains`] directly instead,
    /// since it is conceptually unbounded.
    fn bounded_values(&self, min: u32, max: u32) -> Vec<u32> {
        (min..=max).filter(|v| self.contains(*v)).collect()
    }
}

fn item_contains(item: &CalItem, v: u32) -> bool {
    if v < item.start {
        return false;
    }
    match (item.end, item.step) {
        (Some(end), Some(step)) if step > 0 => v <= end && (v - item.start) % step == 0,
        (Some(end), _) => v <= end,
        (None, Some(step)) if step > 0 => (v - item.start) % step == 0,
        (None, _) => v == item.start,
    }
}

/// Weekday predicate: Monday = 0 .. Sunday = 6 (ISO ordering, as used by
/// `jiff::civil::Weekday::to_monday_zero_offset`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeekdaySpec {
    Any,
    Some(BTreeSet<u8>),
}

impl WeekdaySpec {
    fn contains(&self, v: u8) -> bool {
        match self {
            WeekdaySpec::Any => true,
            WeekdaySpec::Some(set) => set.contains(&v),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemdCalendar {
    pub weekdays: WeekdaySpec,
    pub year: CalComponent,
    pub month: CalComponent,
    pub day: CalComponent,
    pub hour: CalComponent,
    pub minute: CalComponent,
    pub second: CalComponent,
    pub timezone: Option<String>,
}

const WEEKDAY_NAMES: [(&str, u8); 7] = [
    ("mon", 0),
    ("tue", 1),
    ("wed", 2),
    ("thu", 3),
    ("fri", 4),
    ("sat", 5),
    ("sun", 6),
];

fn single(v: u32) -> CalComponent {
    CalComponent::List(vec![CalItem {
        start: v,
        end: Some(v),
        step: None,
    }])
}

fn multi(vs: &[u32]) -> CalComponent {
    CalComponent::List(
        vs.iter()
            .map(|v| CalItem {
                start: *v,
                end: Some(*v),
                step: None,
            })
            .collect(),
    )
}

/// `systemd.time(7)`'s named calendar aliases.
fn parse_special(name: &str) -> Option<SystemdCalendar> {
    let base = |year, month, day, hour, minute, second| SystemdCalendar {
        weekdays: WeekdaySpec::Any,
        year,
        month,
        day,
        hour,
        minute,
        second,
        timezone: None,
    };
    let any = CalComponent::Any;
    let zero = single(0);
    let one = single(1);
    Some(match name.to_ascii_lowercase().as_str() {
        "minutely" => base(
            any.clone(),
            any.clone(),
            any.clone(),
            any.clone(),
            any,
            zero,
        ),
        "hourly" => base(
            any.clone(),
            any.clone(),
            any.clone(),
            any,
            zero.clone(),
            zero,
        ),
        "daily" | "midnight" => base(
            any.clone(),
            any.clone(),
            any,
            zero.clone(),
            zero.clone(),
            zero,
        ),
        "monthly" => base(any.clone(), any, one, zero.clone(), zero.clone(), zero),
        "weekly" => {
            let mut c = base(
                any.clone(),
                any.clone(),
                any,
                zero.clone(),
                zero.clone(),
                zero,
            );
            c.weekdays = WeekdaySpec::Some([0].into_iter().collect());
            c
        }
        "yearly" | "annually" => base(any, one.clone(), one, zero.clone(), zero.clone(), zero),
        "quarterly" => base(
            any,
            multi(&[1, 4, 7, 10]),
            one,
            zero.clone(),
            zero.clone(),
            zero,
        ),
        "semiannually" | "semi-annually" => {
            base(any, multi(&[1, 7]), one, zero.clone(), zero.clone(), zero)
        }
        _ => return None,
    })
}

fn looks_like_date_or_time(tok: &str) -> bool {
    if tok == "*" || tok.starts_with(|c: char| c.is_ascii_digit()) || tok.contains(':') {
        return true;
    }
    if tok.contains('-') {
        // A date like "2026-03-29" or "*-*-*" only ever has '-'-separated
        // parts that are "*" or start with a digit. An IANA timezone name
        // can also contain a hyphen (e.g. "America/Port-au-Prince"), but its
        // parts don't look like that.
        return tok
            .split('-')
            .all(|part| part == "*" || part.starts_with(|c: char| c.is_ascii_digit()));
    }
    false
}

fn looks_like_weekday_spec(tok: &str) -> bool {
    tok.split(',').all(|part| {
        let mut sides = part.split("..");
        let first = sides.next().unwrap_or("");
        let rest_ok = sides.next().is_none_or(|b| weekday_value(b).is_some());
        !first.is_empty() && weekday_value(first).is_some() && rest_ok && sides.next().is_none()
    })
}

fn weekday_value(name: &str) -> Option<u8> {
    let lower = name.to_ascii_lowercase();
    WEEKDAY_NAMES
        .iter()
        .find(|(n, _)| *n == lower)
        .map(|(_, v)| *v)
}

fn parse_weekday_component(tok: &str) -> Result<WeekdaySpec, String> {
    let mut set = BTreeSet::new();
    for part in tok.split(',') {
        if let Some((a, b)) = part.split_once("..") {
            let a = weekday_value(a).ok_or_else(|| format!("'{a}' is not a weekday"))?;
            let b = weekday_value(b).ok_or_else(|| format!("'{b}' is not a weekday"))?;
            if a > b {
                return Err(format!("'{part}' is a backwards weekday range"));
            }
            for v in a..=b {
                set.insert(v);
            }
        } else {
            let v = weekday_value(part).ok_or_else(|| format!("'{part}' is not a weekday"))?;
            set.insert(v);
        }
    }
    Ok(WeekdaySpec::Some(set))
}

/// Parse one calendar component. `bounds`, when given as `(min, max)`, is
/// the field's own legal span (e.g. `(0, 59)` for minutes); a `/step` wider
/// than `max - min` can only ever match its own start value, which is
/// exactly what real `systemd-analyze` rejects as "Invalid argument"
/// instead of silently accepting (see breaktest-cronledger-0004). `year`
/// has no such span and passes `None`.
fn parse_cal_component(text: &str, bounds: Option<(u32, u32)>) -> Result<CalComponent, String> {
    if text == "*" {
        return Ok(CalComponent::Any);
    }
    if text.contains('~') {
        return Err("the '~' (nearest day of month) syntax is not supported".to_string());
    }
    let mut items = Vec::new();
    for part in text.split(',') {
        if part.is_empty() {
            return Err("empty item in list".to_string());
        }
        let (range_part, step_part) = match part.split_once('/') {
            Some((r, s)) => (r, Some(s)),
            None => (part, None),
        };
        let step = match step_part {
            Some(s) => Some(
                s.parse::<u32>()
                    .map_err(|_| format!("'{s}' is not a valid step"))?,
            ),
            None => None,
        };
        if step == Some(0) {
            return Err("step cannot be zero".to_string());
        }
        if range_part == "*" && step.is_some() {
            return Err(
                "'*/step' is not allowed; give the step an explicit start value".to_string(),
            );
        }
        if let (Some(step), Some((min, max))) = (step, bounds) {
            if step > max - min {
                return Err(format!(
                    "step '{step}' is wider than this field's range of {min}..={max}"
                ));
            }
        }
        let check_bound = |v: u32| -> Result<(), String> {
            if let Some((min, max)) = bounds {
                if v < min || v > max {
                    return Err(format!(
                        "'{v}' is outside this field's range of {min}..={max}"
                    ));
                }
            }
            Ok(())
        };
        let item = if range_part == "*" {
            CalItem {
                start: 0,
                end: Some(u32::MAX),
                step,
            }
        } else if let Some((a, b)) = range_part.split_once("..") {
            let start = a
                .parse::<u32>()
                .map_err(|_| format!("'{a}' is not a number"))?;
            let end = b
                .parse::<u32>()
                .map_err(|_| format!("'{b}' is not a number"))?;
            if start > end {
                return Err(format!("'{range_part}' is a backwards range"));
            }
            check_bound(start)?;
            check_bound(end)?;
            CalItem {
                start,
                end: Some(end),
                step,
            }
        } else {
            let v = range_part
                .parse::<u32>()
                .map_err(|_| format!("'{range_part}' is not a number"))?;
            check_bound(v)?;
            CalItem {
                start: v,
                end: None,
                step,
            }
        };
        items.push(item);
    }
    Ok(CalComponent::List(items))
}

fn parse_date(text: &str) -> Result<(CalComponent, CalComponent, CalComponent), String> {
    let parts: Vec<&str> = text.split('-').collect();
    if parts.len() != 3 {
        return Err(format!("'{text}' is not a year-month-day date"));
    }
    Ok((
        parse_cal_component(parts[0], None).map_err(|e| format!("year: {e}"))?,
        parse_cal_component(parts[1], Some((1, 12))).map_err(|e| format!("month: {e}"))?,
        parse_cal_component(parts[2], Some((1, 31))).map_err(|e| format!("day: {e}"))?,
    ))
}

fn parse_time(text: &str) -> Result<(CalComponent, CalComponent, CalComponent), String> {
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() != 2 && parts.len() != 3 {
        return Err(format!("'{text}' is not an hour:minute[:second] time"));
    }
    let hour = parse_cal_component(parts[0], Some((0, 23))).map_err(|e| format!("hour: {e}"))?;
    let minute =
        parse_cal_component(parts[1], Some((0, 59))).map_err(|e| format!("minute: {e}"))?;
    let second = if parts.len() == 3 {
        parse_cal_component(parts[2], Some((0, 59))).map_err(|e| format!("second: {e}"))?
    } else {
        single(0)
    };
    Ok((hour, minute, second))
}

/// Resolve `name` as an IANA timezone, rejecting it at parse time rather
/// than deferring to `expand`/`first_on_or_after`: a real `[Timer]` unit
/// with an unresolvable name otherwise got this same message a second
/// time, plus an unrelated, misleading "schedule can never run" warning
/// from the end-of-loop check in `app.rs` (see breaktest-cronledger-0008).
fn validate_timezone(name: &str) -> Result<(), String> {
    jiff::tz::TimeZone::get(name)
        .map(|_| ())
        .map_err(|e| format!("unknown timezone '{name}': {e}"))
}

/// Parse one `OnCalendar=`-style systemd calendar expression.
pub fn parse(input: &str) -> Result<SystemdCalendar, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("empty calendar expression".to_string());
    }

    let mut tokens: Vec<&str> = trimmed.split_whitespace().collect();

    let timezone = if tokens.len() > 1 && !looks_like_date_or_time(tokens[tokens.len() - 1]) {
        let tz = tokens.pop().unwrap().to_string();
        validate_timezone(&tz)?;
        Some(tz)
    } else {
        None
    };

    // A named special (`daily`, `weekly`, ...) is exactly one token once
    // a trailing timezone has been split off; `systemd-analyze` accepts
    // such a timezone suffix on a named special (e.g. "daily UTC"), but
    // `parse_special` only ever matches the bare name, not the whole
    // "name tz" string (see breaktest-cronledger-0008).
    if tokens.len() == 1 {
        if let Some(mut special) = parse_special(tokens[0]) {
            special.timezone = timezone;
            return Ok(special);
        }
    }

    let weekdays = if !tokens.is_empty() && looks_like_weekday_spec(tokens[0]) {
        parse_weekday_component(tokens.remove(0))?
    } else {
        WeekdaySpec::Any
    };

    let (date_tok, time_tok): (Option<&str>, Option<&str>) = match tokens.len() {
        0 => (None, None),
        1 => {
            if tokens[0].contains('-') {
                (Some(tokens[0]), None)
            } else {
                (None, Some(tokens[0]))
            }
        }
        2 => (Some(tokens[0]), Some(tokens[1])),
        _ => return Err(format!("too many fields in '{trimmed}'")),
    };

    let (year, month, day) = match date_tok {
        Some(d) => parse_date(d)?,
        None => (CalComponent::Any, CalComponent::Any, CalComponent::Any),
    };
    let (hour, minute, second) = match time_tok {
        Some(t) => parse_time(t)?,
        None => (single(0), single(0), single(0)),
    };

    Ok(SystemdCalendar {
        weekdays,
        year,
        month,
        day,
        hour,
        minute,
        second,
        timezone,
    })
}

/// Expand `cal` into every expected instant in `[since, until)`, in
/// `default_tz` unless `cal` carries its own trailing timezone. Unlike
/// [`crate::schedule::expand`], systemd's own civil-time semantics (not
/// cronie's) apply: a nonexistent (gap) civil time contributes nothing,
/// and a repeated (fold) civil time contributes its one wall-clock
/// occurrence (`Disambiguation::Compatible`) — systemd does not have
/// cron's day-of-month/day-of-week OR rule or minute-daemon polling
/// behavior to compensate for either case.
pub fn expand(
    cal: &SystemdCalendar,
    default_tz: &jiff::tz::TimeZone,
    since: jiff::Timestamp,
    until: jiff::Timestamp,
) -> Result<Vec<jiff::Timestamp>, String> {
    expand_bounded(cal, default_tz, since, until, usize::MAX)
}

/// Like [`expand`], but stop the day-walk once the raw candidate count
/// is already past `budget` by more than two days of this calendar.
/// Only the first and last processed day can lose entries to the final
/// window filter, each by at most one day's worth, so a result longer
/// than `budget` is still longer than `budget` after that filter.
pub fn expand_bounded(
    cal: &SystemdCalendar,
    default_tz: &jiff::tz::TimeZone,
    since: jiff::Timestamp,
    until: jiff::Timestamp,
    budget: usize,
) -> Result<Vec<jiff::Timestamp>, String> {
    use jiff::tz::{Disambiguation, TimeZone};

    if since >= until {
        return Ok(Vec::new());
    }
    let tz = match &cal.timezone {
        Some(name) => TimeZone::get(name).map_err(|e| format!("unknown timezone '{name}': {e}"))?,
        None => default_tz.clone(),
    };

    let months = cal.month.bounded_values(1, 12);
    let hours = cal.hour.bounded_values(0, 23);
    let minutes = cal.minute.bounded_values(0, 59);
    let seconds = cal.second.bounded_values(0, 59);
    let per_day_max = hours
        .len()
        .saturating_mul(minutes.len())
        .saturating_mul(seconds.len());
    let stop_at = budget.saturating_add(per_day_max.saturating_mul(2));

    let mut date = since.to_zoned(tz.clone()).date();
    let until_date = until.to_zoned(tz.clone()).date();
    let mut out = Vec::new();

    loop {
        if date > until_date || out.len() > stop_at {
            break;
        }
        if cal.year.contains(date.year().max(0) as u32)
            && months.contains(&u32::from(date.month() as u8))
            && cal.day.contains(u32::from(date.day() as u8))
            && cal
                .weekdays
                .contains(date.weekday().to_monday_zero_offset() as u8)
        {
            for &h in &hours {
                for &m in &minutes {
                    for &s in &seconds {
                        let civil = date.at(h as i8, m as i8, s as i8, 0);
                        let ambiguous = tz.to_ambiguous_zoned(civil);
                        if matches!(ambiguous.offset(), jiff::tz::AmbiguousOffset::Gap { .. }) {
                            // Nonexistent civil time: systemd-analyze
                            // simply has no elapse that day, it does not
                            // invent one at a gap-shifted wall-clock
                            // reading.
                            continue;
                        }
                        if let Ok(z) = ambiguous.disambiguate(Disambiguation::Compatible) {
                            out.push(z.timestamp());
                        }
                    }
                }
            }
        }
        date = match date.tomorrow() {
            Ok(d) => d,
            Err(_) => break,
        };
    }

    out.sort();
    out.dedup();
    out.retain(|t| *t >= since && *t < until);
    Ok(out)
}

/// First elapse at or after `at`. The audit window is `[since, until)`,
/// so `at = until` is the first firing the window itself does not
/// contain. `Ok(None)` when nothing fires within about eleven years
/// (enough for a 29 February calendar). The window widens so a
/// minutely calendar does not materialise years of instants.
pub fn first_on_or_after(
    cal: &SystemdCalendar,
    default_tz: &jiff::tz::TimeZone,
    at: jiff::Timestamp,
) -> Result<Option<jiff::Timestamp>, String> {
    let mut span_days: i64 = 2;
    for _ in 0..12 {
        let secs = span_days.saturating_mul(24 * 60 * 60);
        // Seconds, not calendar days: a `Timestamp` rejects a span whose
        // units are not fixed-length. Overflow means there is no later
        // elapse this function can name.
        let Ok(horizon) = at.checked_add(jiff::Span::new().seconds(secs)) else {
            return Ok(None);
        };
        if let Some(t) = expand(cal, default_tz, at, horizon)?.into_iter().next() {
            return Ok(Some(t));
        }
        span_days = span_days.saturating_mul(2);
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::tz::TimeZone;
    use jiff::Timestamp;

    fn expand_str(expr: &str, since: &str, until: &str) -> Vec<String> {
        let cal = parse(expr).unwrap();
        let tz = TimeZone::UTC;
        let since: Timestamp = since.parse().unwrap();
        let until: Timestamp = until.parse().unwrap();
        expand(&cal, &tz, since, until)
            .unwrap()
            .into_iter()
            .map(|t| {
                t.to_zoned(TimeZone::get(cal.timezone.as_deref().unwrap_or("UTC")).unwrap())
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn expand_bounded_stops_materialising_once_over_budget() {
        let cal = parse("*-*-* *:*:*").unwrap();
        let tz = TimeZone::UTC;
        let since: Timestamp = "2020-01-01T00:00:00Z".parse().unwrap();
        let until: Timestamp = "2030-01-01T00:00:00Z".parse().unwrap();
        let budget = 10;
        let got = expand_bounded(&cal, &tz, since, until, budget).unwrap();
        assert!(
            budget < got.len() && got.len() < 1_000_000,
            "len={}",
            got.len()
        );
    }

    #[test]
    fn daily_first_on_or_after_is_the_elapse_the_window_excludes() {
        let cal = parse("daily").unwrap();
        let tz = TimeZone::UTC;
        let at: Timestamp = "2026-10-21T00:00:00Z".parse().unwrap();
        let got = first_on_or_after(&cal, &tz, at).unwrap();
        assert_eq!(got, Some(at));
    }

    #[test]
    fn daily_alias() {
        let got = expand_str("daily", "2026-01-01T00:00:00Z", "2026-01-04T00:00:00Z");
        assert_eq!(
            got,
            vec![
                "2026-01-01T00:00:00+00:00[UTC]",
                "2026-01-02T00:00:00+00:00[UTC]",
                "2026-01-03T00:00:00+00:00[UTC]",
            ]
        );
    }

    #[test]
    fn weekday_range_with_explicit_date_and_time() {
        // 2026-01-05 is a Monday.
        let got = expand_str(
            "Mon..Fri *-*-* 09:00:00",
            "2026-01-05T00:00:00Z",
            "2026-01-12T00:00:00Z",
        );
        assert_eq!(
            got,
            vec![
                "2026-01-05T09:00:00+00:00[UTC]",
                "2026-01-06T09:00:00+00:00[UTC]",
                "2026-01-07T09:00:00+00:00[UTC]",
                "2026-01-08T09:00:00+00:00[UTC]",
                "2026-01-09T09:00:00+00:00[UTC]",
            ]
        );
    }

    #[test]
    fn shorthand_time_only_every_quarter_hour() {
        let got = expand_str("*:0/15", "2026-01-01T08:00:00Z", "2026-01-01T09:00:00Z");
        assert_eq!(
            got,
            vec![
                "2026-01-01T08:00:00+00:00[UTC]",
                "2026-01-01T08:15:00+00:00[UTC]",
                "2026-01-01T08:30:00+00:00[UTC]",
                "2026-01-01T08:45:00+00:00[UTC]",
            ]
        );
    }

    #[test]
    fn spring_forward_gap_is_skipped_not_invented() {
        // Regression: `*-*-* 02:30:00 Europe/Amsterdam` used to invent a
        // run at the gap-shifted wall-clock reading (03:30) on
        // 2026-03-29, where real `systemd-analyze calendar` has no
        // elapse that day at all (confirmed on this machine with
        // `--base-time`; see tests/fixtures/systemd/README.md).
        let cal = parse("*-*-* 02:30:00 Europe/Amsterdam").unwrap();
        let since: Timestamp = "2026-03-28T00:00:00Z".parse().unwrap();
        let until: Timestamp = "2026-03-30T00:00:00Z".parse().unwrap();
        let got = expand(&cal, &TimeZone::UTC, since, until).unwrap();
        let zoned: Vec<String> = got
            .into_iter()
            .map(|t| {
                t.to_zoned(TimeZone::get("Europe/Amsterdam").unwrap())
                    .to_string()
            })
            .collect();
        assert_eq!(
            zoned,
            vec!["2026-03-28T02:30:00+01:00[Europe/Amsterdam]"],
            "29 March must have no elapse at all, not an invented 03:30"
        );
    }

    #[test]
    fn trailing_timezone_suffix_with_a_hyphen_in_its_name() {
        // Regression: `looks_like_date_or_time` treated any token
        // containing '-' as a date (to tell "*-*-* 03:00:00" apart from
        // a trailing tz name), which misclassified IANA zone names that
        // themselves contain a hyphen, like "America/Port-au-Prince".
        let cal = parse("*-*-* 03:00:00 America/Port-au-Prince").unwrap();
        assert_eq!(cal.timezone.as_deref(), Some("America/Port-au-Prince"));
    }

    #[test]
    fn trailing_timezone_suffix() {
        let cal = parse("*-*-* 03:00:00 Europe/Amsterdam").unwrap();
        assert_eq!(cal.timezone.as_deref(), Some("Europe/Amsterdam"));
        let since: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
        let until: Timestamp = "2026-01-02T00:00:00Z".parse().unwrap();
        let got = expand(&cal, &TimeZone::UTC, since, until).unwrap();
        let z = got[0].to_zoned(TimeZone::get("Europe/Amsterdam").unwrap());
        assert_eq!(z.to_string(), "2026-01-01T03:00:00+01:00[Europe/Amsterdam]");
    }

    #[test]
    fn named_special_with_trailing_timezone_matches_systemd_analyze() {
        // Regression: `systemd-analyze calendar` (249) accepts a trailing
        // timezone on a named special ("daily UTC" normalizes to
        // "*-*-* 00:00:00 UTC"), but `parse_special` only ever saw the
        // whole trimmed string, so any trailing word made it miss the
        // name and fall through to a failing time parse. See
        // breaktest-cronledger-0008.
        for (expr, tz) in [
            ("daily UTC", "UTC"),
            ("weekly Europe/Amsterdam", "Europe/Amsterdam"),
            ("hourly Europe/Amsterdam", "Europe/Amsterdam"),
        ] {
            let cal = parse(expr).unwrap_or_else(|e| panic!("'{expr}' should parse: {e}"));
            assert_eq!(cal.timezone.as_deref(), Some(tz));
        }
    }

    #[test]
    fn unknown_timezone_name_is_rejected_at_parse_not_only_at_expand() {
        // Regression: the timezone name was stored unchecked and only
        // resolved later in expand()/first_on_or_after(), so a real timer
        // unit got both the correct "unknown timezone" warning AND a
        // misleading extra "can never run" warning from app.rs's
        // end-of-loop check. See breaktest-cronledger-0008.
        let err = parse("*-*-* 12:00:00 Mars/Nonexistent").unwrap_err();
        assert!(
            err.contains("unknown timezone"),
            "expected an 'unknown timezone' parse error, got: {err}"
        );
    }

    #[test]
    fn named_aliases_match_systemd_documented_definitions() {
        assert_eq!(parse("minutely").unwrap().second, single(0));
        assert_eq!(parse("hourly").unwrap().minute, single(0));
        assert_eq!(
            parse("weekly").unwrap().weekdays,
            WeekdaySpec::Some([0].into_iter().collect())
        );
        assert_eq!(parse("quarterly").unwrap().month, multi(&[1, 4, 7, 10]));
        assert_eq!(parse("yearly").unwrap(), parse("annually").unwrap());
    }

    #[test]
    fn tilde_is_a_clear_error_not_a_panic() {
        assert!(parse("*-*~1 00:00:00").is_err());
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(parse("not a calendar expression").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn step_wider_than_the_field_range_is_rejected() {
        // Regression: `systemd-analyze calendar '*:0/100:00'` fails with
        // "Invalid argument" since a minute step past 59 can only ever
        // match the start value; this parser used to accept it and
        // expand() to minute 0 every hour, see breaktest-cronledger-0004.
        assert!(parse("*:0/60:00").is_err());
        assert!(parse("*:0/100:00").is_err());
        // A step up to the field's own span (0..=59) stays valid.
        assert!(parse("*:0/59:00").is_ok());
    }

    #[test]
    fn bare_value_outside_the_field_range_is_rejected() {
        // Regression: a bare value (no range, no step) was stored without
        // ever being checked against the field's own min..=max, so
        // `*:*:60` parsed as "second 60", a value that can never occur,
        // unlike `systemd-analyze calendar` (249), which rejects it with
        // "Invalid argument". See breaktest-cronledger-0007.
        assert!(parse("*:*:60").is_err());
        assert!(parse("*:60:00").is_err());
        assert!(parse("25:00:00").is_err());
        assert!(parse("*-13-*").is_err());
        assert!(parse("*-*-32").is_err());
        // The field's own min is also enforced (month/day are 1-based).
        assert!(parse("*-0-*").is_err());
        assert!(parse("*-*-0").is_err());
        // In-range values stay valid.
        assert!(parse("*:*:59").is_ok());
        assert!(parse("*:*:0/15").is_ok());
        assert!(parse("*:0/5:00").is_ok());
    }

    #[test]
    fn range_bound_outside_the_field_range_is_rejected() {
        // Same check, but for a `start..end` range whose bound (not just a
        // bare value) falls outside min..=max; real `systemd-analyze
        // calendar` rejects these too, even when the other bound is fine.
        assert!(parse("*:50..65:00").is_err());
        assert!(parse("*:*:50..65").is_err());
        assert!(parse("*-1..13-*").is_err());
        assert!(parse("*-*-1..32").is_err());
        assert!(parse("*:0..59:00").is_ok());
    }

    #[test]
    fn star_with_a_step_is_rejected() {
        // Regression: `*/step` (no explicit start) used to be accepted
        // and treated as if it meant "every step", but real
        // `systemd-analyze calendar` (249) rejects any `/step` applied to
        // a bare `*` with "Invalid argument" in every field.
        assert!(parse("*:*/5:00").is_err());
        assert!(parse("*-*-*/1").is_err());
        assert!(parse("*-*/3-*").is_err());
        // A step on an explicit start is still fine.
        assert!(parse("*:0/5:00").is_ok());
    }
}
