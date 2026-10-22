//! cron-ledger: read-only audit of crontab and systemd-timer schedules
//! against syslog/journal evidence.

#![forbid(unsafe_code)]

pub mod app;
pub mod cli;
pub mod crontab;
pub mod ledger;
pub mod log;
pub mod report;
pub mod schedule;
pub mod systemd;

#[cfg(test)]
mod tests {
    #[test]
    fn empty_lib_test() {
        assert_eq!(2 + 2, 4);
    }
}
