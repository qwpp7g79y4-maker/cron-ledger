//! Parsed representation of crontab schedules and jobs.

use serde::Serialize;
use std::collections::BTreeSet;

/// One schedule field (minute, hour, day-of-month, month, or day-of-week).
///
/// `Star` means "every legal value, unrestricted" — this is kept distinct
/// from `Values` containing every legal value, because the vixie-cron
/// day-of-month/day-of-week OR rule (see [`matches_day`]) only applies when
/// a field is *restricted*.
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

    pub fn is_restricted(&self) -> bool {
        !matches!(self, Field::Star)
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
    /// From a `TZ=` env line active at this point in the file.
    pub tz: Option<String>,
    /// From a `CRON_TZ=` env line active at this point in the file; takes
    /// precedence over `tz` for schedule interpretation (see
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
    /// otherwise `TZ`, otherwise `None` (caller falls back to the system
    /// timezone).
    pub fn effective_tz(&self) -> Option<&str> {
        self.cron_tz.as_deref().or(self.tz.as_deref())
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
/// restricted (not `*`), a day matches if it satisfies *either* one (OR),
/// not both (AND). If only one field is restricted, that field alone
/// decides. If neither is restricted, every day matches.
pub fn matches_day(dom: &Field, dow: &Field, day_of_month: u32, day_of_week: u32) -> bool {
    match (dom.is_restricted(), dow.is_restricted()) {
        (false, false) => true,
        (true, false) => dom.contains(day_of_month),
        (false, true) => dow.contains(day_of_week),
        (true, true) => dom.contains(day_of_month) || dow.contains(day_of_week),
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
        };
        assert_eq!(describe_schedule(&schedule), "0,15,30,45 * * * 1,5");
    }

    #[test]
    fn both_unrestricted_matches_every_day() {
        assert!(matches_day(&Field::Star, &Field::Star, 1, 0));
        assert!(matches_day(&Field::Star, &Field::Star, 31, 6));
    }

    #[test]
    fn only_dom_restricted() {
        let dom = values(&[13]);
        assert!(matches_day(&dom, &Field::Star, 13, 2));
        assert!(!matches_day(&dom, &Field::Star, 14, 2));
    }

    #[test]
    fn only_dow_restricted() {
        let dow = values(&[5]);
        assert!(matches_day(&Field::Star, &dow, 1, 5));
        assert!(!matches_day(&Field::Star, &dow, 1, 4));
    }

    #[test]
    fn both_restricted_is_or_not_and() {
        // "0 0 13 * 5" = the 13th OR a Friday.
        let dom = values(&[13]);
        let dow = values(&[5]);
        assert!(matches_day(&dom, &dow, 13, 1)); // 13th, not Friday: still matches.
        assert!(matches_day(&dom, &dow, 1, 5)); // Friday, not 13th: still matches.
        assert!(matches_day(&dom, &dow, 13, 5)); // both: matches.
        assert!(!matches_day(&dom, &dow, 14, 1)); // neither: no match.
    }
}
