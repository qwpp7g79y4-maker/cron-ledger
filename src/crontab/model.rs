//! Parsed representation of crontab schedules and jobs.

use serde::Serialize;
use std::collections::BTreeSet;

/// One schedule field (minute, hour, day-of-month, month, or day-of-week).
///
/// `Star` means "every legal value, unrestricted". `Values` containing
/// every legal value (e.g. from `1-31`) is kept distinct from it purely
/// for display (`describe_field`); neither the wildcard DST rule in
/// [`crate::schedule::expand`] nor the day-of-month/day-of-week OR/AND
/// rule uses this distinction (see [`Schedule::minute_star`]/
/// [`Schedule::hour_star`] and [`Schedule::dom_star`]/
/// [`Schedule::dow_star`] instead).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Field {
    Star,
    Values(BTreeSet<u8>),
}

impl Field {
    pub fn contains(&self, v: u32) -> bool {
        match self {
            Field::Star => true,
            Field::Values(set) => v <= u8::MAX as u32 && set.contains(&(v as u8)),
        }
    }
}

/// The five vixie-cron schedule fields of a non-`@`-special line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Schedule {
    pub minute: Field,
    pub hour: Field,
    pub dom: Field,
    pub month: Field,
    pub dow: Field,
    /// Whether the raw day-of-month field's first character was `*`
    /// (`crontab(5)`: "don't start with *"), e.g. true for both `*` and
    /// `*/2`. Drives the OR/AND choice in [`matches_day`] independently of
    /// whether `dom` parsed to [`Field::Star`] or [`Field::Values`].
    pub dom_star: bool,
    /// Same as `dom_star`, for the day-of-week field.
    pub dow_star: bool,
    /// Whether the raw minute field's first character was `*` (cronie's
    /// `MIN_STAR`), e.g. true for `*` and `*/15`. Together with
    /// `hour_star` this makes the job a wildcard job for DST in
    /// [`crate::schedule::expand`]. Not part of the JSON output.
    #[serde(skip)]
    pub minute_star: bool,
    /// Same as `minute_star`, for the hour field (cronie's `HR_STAR`).
    #[serde(skip)]
    pub hour_star: bool,
}

/// `@reboot` has no computable schedule of expected run times; every other
/// `@`-special expands to an equivalent [`Schedule`] before parsing stops.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ScheduleKind {
    Standard(Schedule),
    Reboot,
}

/// One parsed, schedulable crontab line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CronJob {
    pub schedule: ScheduleKind,
    /// From a `TZ=` env line active at this point in the file. This only
    /// reaches the job's own environment (as cronie documents it); it has
    /// no effect on schedule interpretation, so it is not read by
    /// [`CronJob::effective_tz`].
    pub tz: Option<String>,
    /// From a `CRON_TZ=` env line active at this point in the file: the
    /// only env line that governs schedule interpretation (see
    /// [`CronJob::effective_tz`]).
    pub cron_tz: Option<String>,
    /// `None` for a per-user crontab where the user comes from the file's
    /// location, not an inline column.
    pub user: Option<String>,
    pub command: String,
    pub stdin: Option<String>,
    pub file: String,
    pub line: usize,
}

impl CronJob {
    /// The timezone that governs this job's schedule: `CRON_TZ` if set,
    /// otherwise `None` (caller falls back to the system timezone). A
    /// plain `TZ=` line does not affect scheduling in cronie, only the
    /// job's own environment, so it is deliberately not consulted here.
    pub fn effective_tz(&self) -> Option<&str> {
        self.cron_tz.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParseWarning {
    pub file: String,
    pub line: usize,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ParseResult {
    pub jobs: Vec<CronJob>,
    pub warnings: Vec<ParseWarning>,
}

/// Render a field back to a cron-like field expression, for display only
/// (a report's "schema" column). Step/range syntax (`*/15`, `1-5`) is not
/// reconstructed since [`Field`] no longer carries it; a restricted field
/// always renders as its sorted, comma-separated values.
pub fn describe_field(field: &Field) -> String {
    match field {
        Field::Star => "*".to_string(),
        Field::Values(set) => set
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(","),
    }
}

/// Render a [`Schedule`] back to a `minute hour dom month dow` cron-like
/// expression, for display only.
pub fn describe_schedule(schedule: &Schedule) -> String {
    format!(
        "{} {} {} {} {}",
        describe_field(&schedule.minute),
        describe_field(&schedule.hour),
        describe_field(&schedule.dom),
        describe_field(&schedule.month),
        describe_field(&schedule.dow),
    )
}

/// vixie-cron's day-of-month/day-of-week rule: if *both* fields are
/// restricted (`dom_star` and `dow_star` both false), a day matches if it
/// satisfies *either* field (OR), not both. If either field is
/// unrestricted (its raw text started with `*`, e.g. `*` or `*/2`), the
/// day must satisfy *both* fields (AND) — which for a literal `*` field
/// (whose `contains` is always true) reduces to "that field alone
/// decides".
pub fn matches_day(
    dom: &Field,
    dow: &Field,
    dom_star: bool,
    dow_star: bool,
    day_of_month: u32,
    day_of_week: u32,
) -> bool {
    if dom_star || dow_star {
        dom.contains(day_of_month) && dow.contains(day_of_week)
    } else {
        dom.contains(day_of_month) || dow.contains(day_of_week)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(v: &[u8]) -> Field {
        Field::Values(v.iter().copied().collect())
    }

    #[test]
    fn describe_schedule_renders_star_and_values() {
        let schedule = Schedule {
            minute: values(&[0, 15, 30, 45]),
            hour: Field::Star,
            dom: Field::Star,
            month: Field::Star,
            dow: values(&[1, 5]),
            dom_star: true,
            dow_star: false,
            minute_star: false,
            hour_star: true,
        };
        assert_eq!(describe_schedule(&schedule), "0,15,30,45 * * * 1,5");
    }

    #[test]
    fn both_unrestricted_matches_every_day() {
        assert!(matches_day(&Field::Star, &Field::Star, true, true, 1, 0));
        assert!(matches_day(&Field::Star, &Field::Star, true, true, 31, 6));
    }

    #[test]
    fn only_dom_restricted() {
        let dom = values(&[13]);
        assert!(matches_day(&dom, &Field::Star, false, true, 13, 2));
        assert!(!matches_day(&dom, &Field::Star, false, true, 14, 2));
    }

    #[test]
    fn only_dow_restricted() {
        let dow = values(&[5]);
        assert!(matches_day(&Field::Star, &dow, true, false, 1, 5));
        assert!(!matches_day(&Field::Star, &dow, true, false, 1, 4));
    }

    #[test]
    fn both_restricted_is_or_not_and() {
        // "0 0 13 * 5" = the 13th OR a Friday.
        let dom = values(&[13]);
        let dow = values(&[5]);
        assert!(matches_day(&dom, &dow, false, false, 13, 1)); // 13th, not Friday: still matches.
        assert!(matches_day(&dom, &dow, false, false, 1, 5)); // Friday, not 13th: still matches.
        assert!(matches_day(&dom, &dow, false, false, 13, 5)); // both: matches.
        assert!(!matches_day(&dom, &dow, false, false, 14, 1)); // neither: no match.
    }

    #[test]
    fn dom_star_with_step_forces_and_not_or() {
        // "0 0 */2 * 1": both fields look "restricted" by their expanded
        // values, but dom's raw text started with `*`, so AND applies:
        // only odd days that are also Mondays.
        let dom = values(&[1, 3, 5, 7, 9, 11, 13, 15, 17, 19, 21, 23, 25, 27, 29, 31]);
        let dow = values(&[1]);
        assert!(matches_day(&dom, &dow, true, false, 5, 1)); // odd day 5, Monday: matches.
        assert!(!matches_day(&dom, &dow, true, false, 5, 2)); // odd day 5, Tuesday: no match.
        assert!(!matches_day(&dom, &dow, true, false, 6, 1)); // even day 6, Monday: no match.
    }
}
