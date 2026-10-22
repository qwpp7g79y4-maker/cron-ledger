//! The parsed representation that every log source (syslog, journal)
//! reduces to: one [`LogEvent`] per recognized line, plus a count of lines
//! that looked like evidence but could not be parsed.

use jiff::Timestamp;
use serde::Serialize;

/// What a single log line is evidence of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum EventSource {
    /// A cron/cronie job actually starting: `CRON[pid]: (user) CMD (cmd)`
    /// (Debian) or `CROND[pid]: (user) CMD (cmd)` (RHEL/cronie). This is
    /// the instant a matcher compares against a schedule's expected run
    /// times.
    CronCmd {
        user: String,
        pid: Option<u32>,
        command: String,
    },
    /// cronie-only: a line of the command's captured output
    /// (`CMDOUT (...)`). Not a run instant by itself; kept only so a
    /// matcher can tell a job produced output.
    CronCmdOut { user: String, pid: Option<u32> },
    /// cronie-only: the command finished (`CMDEND (...)`). Real-world
    /// cronie `CMDEND` lines do not carry a numeric exit code (confirmed
    /// against published `/var/log/cron` examples); this only marks that
    /// the run completed.
    CronCmdEnd {
        user: String,
        pid: Option<u32>,
        command: String,
    },
    /// A systemd timer-activated service starting, from the journal's own
    /// `Started ...` message for the unit named in `_SYSTEMD_UNIT`/`UNIT`.
    SystemdUnitStart { unit: String },
    /// A boot, inferred from a new `_BOOT_ID` appearing in the journal.
    /// Used only for `@reboot` jobs' informational `REBOOT_ONLY` status.
    Boot { boot_id: Option<String> },
}

/// Whether a job's own exit status is known, and to what precision.
///
/// v1 can only ever report [`ExitStatus::Completed`], never a numeric
/// code: neither cronie's plain-text `CMDEND` line nor the journal fields
/// this project reads (`SYSLOG_IDENTIFIER`, `MESSAGE`, `_SYSTEMD_UNIT`)
/// carry one. This is the literal, honest reading of "exit code known"
/// for v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ExitStatus {
    /// No `CMDEND` (or equivalent) was observed for this run.
    Unknown,
    /// A `CMDEND` (or equivalent) was observed: the job ran to completion.
    Completed,
}

/// One recognized log line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogEvent {
    pub timestamp: Timestamp,
    pub source: EventSource,
    pub file: String,
    pub line: usize,
}

/// A log line that looked like it belonged to a recognized source (a cron
/// tag, valid-looking JSON, ...) but could not be parsed further. Counted,
/// never fatal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogWarning {
    pub file: String,
    pub line: usize,
    pub message: String,
}

/// The result of parsing one log source: every recognized event, plus a
/// count of lines that were evidently broken rather than simply
/// unrelated (e.g. an `sshd` line in the same syslog is skipped silently,
/// not counted as a warning).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LogParseResult {
    pub events: Vec<LogEvent>,
    pub warnings: Vec<LogWarning>,
}
