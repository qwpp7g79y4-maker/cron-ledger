//! Parsing of `journalctl -o json` output (newline-delimited JSON, one
//! object per entry): `SYSLOG_IDENTIFIER` cron/CRON/crond with the same
//! `(user) CMD (cmd)` style `MESSAGE` cronie/vixie write to syslog,
//! a manager `Started ...` message (`SYSLOG_IDENTIFIER=systemd`) for
//! timer-activated services, preferring `UNIT` over `_SYSTEMD_UNIT`, and
//! `_BOOT_ID` for `@reboot`'s informational boot list.
//! `MESSAGE` may be a JSON string, or (for non-UTF-8 output) a JSON array
//! of byte values.

use super::event::{EventSource, LogEvent, LogParseResult, LogWarning};
use jiff::Timestamp;
use serde_json::Value;
use std::collections::HashSet;

fn warn(warnings: &mut Vec<LogWarning>, file: &str, line: usize, message: impl Into<String>) {
    warnings.push(LogWarning {
        file: file.to_string(),
        line,
        message: message.into(),
    });
}

fn extract_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => {
            let bytes: Option<Vec<u8>> =
                items.iter().map(|v| v.as_u64().map(|n| n as u8)).collect();
            bytes.map(|b| String::from_utf8_lossy(&b).into_owned())
        }
        _ => None,
    }
}

fn extract_realtime_timestamp(value: Option<&Value>) -> Option<Timestamp> {
    let micros = match value? {
        Value::String(s) => s.parse::<i64>().ok()?,
        Value::Number(n) => n.as_i64()?,
        _ => return None,
    };
    Timestamp::from_microsecond(micros).ok()
}

fn extract_pid(obj: &serde_json::Map<String, Value>) -> Option<u32> {
    match obj.get("_PID")? {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_u64().map(|n| n as u32),
        _ => None,
    }
}

/// `(user) ACTION (arg)`, same body cronie/vixie write to syslog, minus
/// the `TAG[pid]:` prefix the journal already splits into fields.
fn parse_cron_message(msg: &str) -> Option<(String, String, String)> {
    let rest = msg.trim().strip_prefix('(')?;
    let close = rest.find(')')?;
    let user = rest[..close].to_string();
    let rest = rest[close + 1..].trim_start();
    let (action, arg_part) = rest.split_once(' ')?;
    let mut arg = arg_part.trim();
    if let Some(stripped) = arg.strip_prefix('(') {
        arg = stripped;
    }
    if let Some(stripped) = arg.strip_suffix(')') {
        arg = stripped;
    }
    Some((user, action.to_string(), arg.to_string()))
}

/// Debian/Ubuntu's cron writes a PAM session line for every run, tagged
/// as cron but with no `(user) ACTION (arg)` body: `pam_unix(cron:session):
/// session opened/closed for user ...`. Real traffic, not a malformed line.
fn is_pam_session_message(message: &str) -> bool {
    message.starts_with("pam_unix(cron:session):")
}

/// Parse `journalctl -o json` NDJSON output into [`LogEvent`]s.
pub fn parse_journal_json(content: &str, file: &str) -> LogParseResult {
    let mut events = Vec::new();
    let mut warnings = Vec::new();
    let mut seen_boots: HashSet<String> = HashSet::new();

    for (idx, raw_line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                warn(&mut warnings, file, line_no, format!("invalid JSON: {e}"));
                continue;
            }
        };
        let Some(obj) = value.as_object() else {
            warn(
                &mut warnings,
                file,
                line_no,
                "journal entry is not a JSON object",
            );
            continue;
        };

        let Some(timestamp) = extract_realtime_timestamp(obj.get("__REALTIME_TIMESTAMP")) else {
            warn(
                &mut warnings,
                file,
                line_no,
                "missing or unparseable __REALTIME_TIMESTAMP",
            );
            continue;
        };

        if let Some(boot_id) = obj.get("_BOOT_ID").and_then(|v| v.as_str()) {
            if seen_boots.insert(boot_id.to_string()) {
                events.push(LogEvent {
                    timestamp,
                    source: EventSource::Boot {
                        boot_id: Some(boot_id.to_string()),
                    },
                    file: file.to_string(),
                    line: line_no,
                });
            }
        }

        let identifier = obj
            .get("SYSLOG_IDENTIFIER")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // A real PID 1 message about a unit carries both fields:
        // `_SYSTEMD_UNIT` is the journal's own unit (`init.scope` for PID
        // 1), `UNIT` is the unit the message is about. Prefer `UNIT`.
        let unit = obj
            .get("UNIT")
            .and_then(|v| v.as_str())
            .or_else(|| obj.get("_SYSTEMD_UNIT").and_then(|v| v.as_str()));

        if identifier.eq_ignore_ascii_case("cron") || identifier.eq_ignore_ascii_case("crond") {
            let Some(message) = extract_string(obj.get("MESSAGE")) else {
                warn(&mut warnings, file, line_no, "cron entry has no MESSAGE");
                continue;
            };
            if is_pam_session_message(&message) {
                continue;
            }
            let Some((user, action, arg)) = parse_cron_message(&message) else {
                warn(
                    &mut warnings,
                    file,
                    line_no,
                    "cron MESSAGE has no '(user) ACTION (arg)' body",
                );
                continue;
            };
            let pid = extract_pid(obj);
            let source = match action.to_ascii_uppercase().as_str() {
                "CMD" => Some(EventSource::CronCmd {
                    user,
                    pid,
                    command: arg,
                }),
                "CMDOUT" => Some(EventSource::CronCmdOut { user, pid }),
                "CMDEND" => Some(EventSource::CronCmdEnd {
                    user,
                    pid,
                    command: arg,
                }),
                _ => None,
            };
            if let Some(source) = source {
                events.push(LogEvent {
                    timestamp,
                    source,
                    file: file.to_string(),
                    line: line_no,
                });
            }
        } else if identifier.eq_ignore_ascii_case("systemd") {
            // Only the manager itself (not a service logging its own
            // "Started ..." line, under its own unit, with its own
            // identifier) gets to report that *it* started a unit.
            if let (Some(unit), Some(message)) = (unit, extract_string(obj.get("MESSAGE"))) {
                if message.starts_with("Started ") {
                    events.push(LogEvent {
                        timestamp,
                        source: EventSource::SystemdUnitStart {
                            unit: unit.to_string(),
                        },
                        file: file.to_string(),
                        line: line_no,
                    });
                }
            }
        }
    }

    LogParseResult { events, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cron_cmd_as_plain_string_message() {
        let content = r#"{"__REALTIME_TIMESTAMP":"1792897201000000","SYSLOG_IDENTIFIER":"CROND","MESSAGE":"(alice) CMD (backup.sh)"}"#;
        let result = parse_journal_json(content, "journal.json");
        assert_eq!(result.warnings, vec![]);
        assert_eq!(result.events.len(), 1);
        assert_eq!(
            result.events[0].source,
            EventSource::CronCmd {
                user: "alice".to_string(),
                pid: None,
                command: "backup.sh".to_string(),
            }
        );
    }

    #[test]
    fn message_as_byte_array_is_decoded() {
        let bytes: Vec<u8> = "(alice) CMD (backup.sh)".bytes().collect();
        let content = format!(
            r#"{{"__REALTIME_TIMESTAMP":"1792897201000000","SYSLOG_IDENTIFIER":"cron","MESSAGE":{bytes:?}}}"#
        );
        let result = parse_journal_json(&content, "journal.json");
        assert_eq!(result.warnings, vec![]);
        assert_eq!(result.events.len(), 1);
        assert_eq!(
            result.events[0].source,
            EventSource::CronCmd {
                user: "alice".to_string(),
                pid: None,
                command: "backup.sh".to_string(),
            }
        );
    }

    #[test]
    fn pid_is_read_from_underscore_pid_field() {
        let content = r#"{"__REALTIME_TIMESTAMP":"1792897201000000","SYSLOG_IDENTIFIER":"CROND","_PID":"4242","MESSAGE":"(alice) CMD (backup.sh)"}"#;
        let result = parse_journal_json(content, "journal.json");
        assert_eq!(
            result.events[0].source,
            EventSource::CronCmd {
                user: "alice".to_string(),
                pid: Some(4242),
                command: "backup.sh".to_string(),
            }
        );
    }

    #[test]
    fn systemd_unit_started_message_is_an_event() {
        let content = r#"{"__REALTIME_TIMESTAMP":"1792897201000000","SYSLOG_IDENTIFIER":"systemd","_SYSTEMD_UNIT":"backup.service","MESSAGE":"Started Nightly backup."}"#;
        let result = parse_journal_json(content, "journal.json");
        assert_eq!(result.warnings, vec![]);
        assert_eq!(
            result.events[0].source,
            EventSource::SystemdUnitStart {
                unit: "backup.service".to_string()
            }
        );
    }

    #[test]
    fn real_pid1_message_prefers_unit_over_underscore_systemd_unit() {
        // Real `journalctl -o json` output from PID 1 about a unit carries
        // both fields: `_SYSTEMD_UNIT=init.scope` (the journal's own unit,
        // since PID 1 logs under init.scope) and `UNIT=<name>.service`
        // (the unit the message is about).
        let content = r#"{"__REALTIME_TIMESTAMP":"1792897201000000","SYSLOG_IDENTIFIER":"systemd","_SYSTEMD_UNIT":"init.scope","UNIT":"backup.service","MESSAGE":"Started Nightly backup."}"#;
        let result = parse_journal_json(content, "journal.json");
        assert_eq!(result.warnings, vec![]);
        assert_eq!(
            result.events[0].source,
            EventSource::SystemdUnitStart {
                unit: "backup.service".to_string()
            }
        );
    }

    #[test]
    fn systemd_starting_message_is_not_a_run_event() {
        let content = r#"{"__REALTIME_TIMESTAMP":"1792897201000000","SYSLOG_IDENTIFIER":"systemd","_SYSTEMD_UNIT":"backup.service","MESSAGE":"Starting Nightly backup..."}"#;
        let result = parse_journal_json(content, "journal.json");
        assert_eq!(result.events, vec![]);
    }

    #[test]
    fn a_services_own_message_starting_with_started_is_not_a_unit_start() {
        // The service's own output, logged under its own unit and its
        // own SYSLOG_IDENTIFIER (not "systemd"), happens to start with
        // "Started ". Only the manager itself gets to report that it
        // started a unit; the ledger must not read this as a trigger.
        let content = r#"{"__REALTIME_TIMESTAMP":"1792897201000000","SYSLOG_IDENTIFIER":"backup-tool","_SYSTEMD_UNIT":"backup.service","MESSAGE":"Started processing 4 files"}"#;
        let result = parse_journal_json(content, "journal.json");
        assert_eq!(result.events, vec![]);
    }

    #[test]
    fn new_boot_id_emits_one_boot_event_only_once() {
        let content = "\
{\"__REALTIME_TIMESTAMP\":\"1792897201000000\",\"_BOOT_ID\":\"abc\",\"SYSLOG_IDENTIFIER\":\"kernel\",\"MESSAGE\":\"booting\"}
{\"__REALTIME_TIMESTAMP\":\"1792897202000000\",\"_BOOT_ID\":\"abc\",\"SYSLOG_IDENTIFIER\":\"kernel\",\"MESSAGE\":\"booted\"}
";
        let result = parse_journal_json(content, "journal.json");
        let boots: Vec<_> = result
            .events
            .iter()
            .filter(|e| matches!(e.source, EventSource::Boot { .. }))
            .collect();
        assert_eq!(boots.len(), 1);
    }

    #[test]
    fn pam_session_lines_are_skipped_not_a_warning() {
        let content = "\
{\"__REALTIME_TIMESTAMP\":\"1792897200000000\",\"SYSLOG_IDENTIFIER\":\"CRON\",\"MESSAGE\":\"pam_unix(cron:session): session opened for user root(uid=0) by (uid=0)\"}
{\"__REALTIME_TIMESTAMP\":\"1792897201000000\",\"SYSLOG_IDENTIFIER\":\"CROND\",\"MESSAGE\":\"(root) CMD (backup.sh)\"}
{\"__REALTIME_TIMESTAMP\":\"1792897202000000\",\"SYSLOG_IDENTIFIER\":\"CRON\",\"MESSAGE\":\"pam_unix(cron:session): session closed for user root\"}
";
        let result = parse_journal_json(content, "journal.json");
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.warnings, vec![]);
    }

    #[test]
    fn invalid_json_line_is_a_warning_not_fatal() {
        let content = "not json at all\n{\"__REALTIME_TIMESTAMP\":\"1792897201000000\",\"SYSLOG_IDENTIFIER\":\"CROND\",\"MESSAGE\":\"(alice) CMD (ok.sh)\"}\n";
        let result = parse_journal_json(content, "journal.json");
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.warnings[0].line, 1);
    }
}
