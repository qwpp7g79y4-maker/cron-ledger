//! Parsing of log evidence: syslog-style cron logs (Debian, RHEL/cronie)
//! and `journalctl -o json` output. No log source is ever read unless the
//! caller names it explicitly; `.gz` rotations are only decompressed when
//! the caller passes a `.gz` path directly.

mod event;
mod journal;
mod source;
mod syslog;

pub use event::{EventSource, ExitStatus, LogEvent, LogParseResult, LogWarning};
pub use journal::parse_journal_json;
pub use source::read_log_source;
pub use syslog::parse_syslog_cron;
