//! Parsing of systemd `.timer` units: `OnCalendar=` (systemd's own calendar
//! syntax), `Persistent=`, `RandomizedDelaySec=`, `AccuracySec=`, and
//! `Unit=`. Also the text of a saved `systemctl list-timers --all`
//! listing (LAST, NEXT and UNIT only). No `systemctl` or D-Bus call is
//! ever made; only files and stdin are read.

pub mod calendar;
pub mod list_timers;
pub mod timer_unit;

pub use calendar::{
    expand as expand_calendar, expand_bounded as expand_calendar_bounded,
    first_on_or_after as first_calendar_on_or_after, parse as parse_calendar, SystemdCalendar,
};
pub use list_timers::{
    evaluate_listed_timer, parse_list_timers, ListTimersParse, ListedTimer, ListedTimerVerdict,
};
pub use timer_unit::{
    effective_tolerance, evaluate_missed_run, evaluate_missed_runs, parse_duration,
    parse_timer_unit, MissedRunStatus, TimerUnit,
};
