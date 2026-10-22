//! Parsing of systemd `.timer` units: `OnCalendar=` (systemd's own calendar
//! syntax), `Persistent=`, `RandomizedDelaySec=`, `AccuracySec=`, and
//! `Unit=`. No `systemctl` or D-Bus call is ever made; only files and
//! stdin are read.

pub mod calendar;
pub mod timer_unit;

pub use calendar::{expand as expand_calendar, parse as parse_calendar, SystemdCalendar};
pub use timer_unit::{
    effective_tolerance, evaluate_missed_run, evaluate_missed_runs, parse_duration,
    parse_timer_unit, MissedRunStatus, TimerUnit,
};
