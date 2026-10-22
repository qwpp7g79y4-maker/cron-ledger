//! Parsing of vixie-cron-style crontab files: per-user crontabs and
//! system-style crontabs (`/etc/crontab`, `/etc/cron.d/*`).

mod command;
mod field;
mod model;
mod parser;

pub use command::split_command_stdin;
pub use model::{
    describe_schedule, matches_day, CronJob, Field, ParseResult, ParseWarning, Schedule,
    ScheduleKind,
};
pub use parser::{parse_system_crontab, parse_user_crontab};
