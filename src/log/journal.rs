//! Parsing of `journalctl -o json` output (newline-delimited JSON, one
//! object per entry): `SYSLOG_IDENTIFIER` cron/CRON/crond with the same
//! `(user) CMD (cmd)` style `MESSAGE` cronie/vixie write to syslog,
//! a manager `Started ...` message (`SYSLOG_IDENTIFIER=systemd`) for
//! timer-activated services, preferring `UNIT` over `_SYSTEMD_UNIT`, and
//! `_BOOT_ID` names a boot for `@reboot`'s informational list. The boot's
//! time comes only from an entry that shows it: a kernel banner, or any
//! entry received within two minutes of `CLOCK_MONOTONIC` zero.
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

fn extract_i64(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

fn extract_realtime_timestamp(value: Option<&Value>) -> Option<Timestamp> {
    Timestamp::from_microsecond(extract_i64(value)?).ok()
}

/// `__MONOTONIC_TIMESTAMP` is `CLOCK_MONOTONIC` at the moment journald
/// received the entry (`systemd.journal-fields(7)`), in microseconds.
/// That clock starts at kernel boot and does not count time spent
/// suspended. An entry received within this long of zero was logged at
/// the start of the boot; a later entry was not.
///
/// The entry's own `__REALTIME_TIMESTAMP` is the boot observation.
/// Subtracting the two fields is not: for kernel messages the realtime
/// field is reception time, and `_SOURCE_MONOTONIC_TIMESTAMP` is
/// `CLOCK_BOOTTIME`, which is not interchangeable with it across suspend.
const BOOT_START_MAX_MONOTONIC_US: i64 = 120_000_000;

fn is_kernel_record(obj: &serde_json::Map<String, Value>) -> bool {
    let identifier = obj
        .get("SYSLOG_IDENTIFIER")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let transport = obj.get("_TRANSPORT").and_then(|v| v.as_str()).unwrap_or("");
    identifier.eq_ignore_ascii_case("kernel") || transport == "kernel"
}

/// The first lines a kernel prints, imported by journald from `/dev/kmsg`.
/// `dmesg` shows them at monotonic 0 (`Linux version ...`, and on some
/// machines `Booting Linux on physical CPU ...` or
/// `Booting paravirtualized kernel on bare hardware`). Journald may
/// receive them only once it is running, so the reception monotonic
/// timestamp can already be minutes later; the text is still the boot.
fn is_kernel_boot_banner(obj: &serde_json::Map<String, Value>) -> bool {
    if !is_kernel_record(obj) {
        return false;
    }
    let Some(message) = extract_string(obj.get("MESSAGE")) else {
        return false;
    };
    let message = message.trim();
    message.starts_with("Linux version ")
        || message.starts_with("Booting Linux")
        || message.starts_with("Booting paravirtualized kernel")
}

fn received_at_boot_start(obj: &serde_json::Map<String, Value>) -> bool {
    extract_i64(obj.get("__MONOTONIC_TIMESTAMP"))
        .is_some_and(|us| (0..BOOT_START_MAX_MONOTONIC_US).contains(&us))
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
    let mut coverage_timestamps = Vec::new();
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

        // A parsed object with a real timestamp is part of the log span
        // even when it is not a cron, timer, or boot event. Lines that
        // failed before this point (invalid JSON, not an object, no
        // usable timestamp) are not.
        coverage_timestamps.push(timestamp);

        if let Some(boot_id) = obj.get("_BOOT_ID").and_then(|v| v.as_str()) {
            // Do not mark the id seen until this entry actually shows
            // the boot time. A later line of the same boot may be the
            // one that does.
            if !seen_boots.contains(boot_id)
                && (is_kernel_boot_banner(obj) || received_at_boot_start(obj))
            {
                seen_boots.insert(boot_id.to_string());
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

    LogParseResult {
        events,
        warnings,
        coverage_timestamps,
    }
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
{\"__REALTIME_TIMESTAMP\":\"1792897201000000\",\"_BOOT_ID\":\"abc\",\"SYSLOG_IDENTIFIER\":\"kernel\",\"MESSAGE\":\"Linux version 6.1.0 (builder@host) (gcc) #1 SMP\"}
{\"__REALTIME_TIMESTAMP\":\"1792897202000000\",\"_BOOT_ID\":\"abc\",\"SYSLOG_IDENTIFIER\":\"kernel\",\"MESSAGE\":\"Linux version 6.1.0 (builder@host) (gcc) #1 SMP\"}
";
        let result = parse_journal_json(content, "journal.json");
        let boots: Vec<_> = result
            .events
            .iter()
            .filter(|e| matches!(e.source, EventSource::Boot { .. }))
            .collect();
        assert_eq!(boots.len(), 1);
        assert_eq!(
            boots[0].timestamp,
            Timestamp::from_microsecond(1_792_897_201_000_000).unwrap()
        );
    }

    #[test]
    fn kernel_boot_banners_record_the_entry_even_when_reception_is_later() {
        // journald imports `/dev/kmsg` when it starts. Reception can be
        // minutes after monotonic zero (the 10-minute value below) while
        // the banner text is still the boot.
        for message in [
            "Linux version 6.1.0 (builder@host) (gcc) #1 SMP",
            "Booting Linux on physical CPU 0x0",
            "Booting paravirtualized kernel on bare hardware",
        ] {
            let content = format!(
                r#"{{"__REALTIME_TIMESTAMP":"1792483800000000","__MONOTONIC_TIMESTAMP":"600000000","_BOOT_ID":"abc","SYSLOG_IDENTIFIER":"kernel","MESSAGE":"{message}"}}"#
            );
            let boots = boot_events(&content);
            assert_eq!(boots.len(), 1, "{message}");
            assert_eq!(
                boots[0].timestamp,
                Timestamp::from_microsecond(1_792_483_800_000_000).unwrap(),
                "{message}"
            );
        }
    }

    #[test]
    fn kernel_banner_without_syslog_identifier_still_counts_via_transport() {
        let content = r#"{"__REALTIME_TIMESTAMP":"1792483800000000","__MONOTONIC_TIMESTAMP":"600000000","_BOOT_ID":"abc","_TRANSPORT":"kernel","MESSAGE":"Linux version 6.1.0 (builder@host) (gcc) #1 SMP"}"#;
        let boots = boot_events(content);
        assert_eq!(boots.len(), 1);
    }

    #[test]
    fn userspace_line_quoting_linux_version_is_not_a_boot() {
        let content = r#"{"__REALTIME_TIMESTAMP":"1792483800000000","__MONOTONIC_TIMESTAMP":"28800000000","_BOOT_ID":"abc","SYSLOG_IDENTIFIER":"backup-tool","MESSAGE":"Linux version 1.0 started"}"#;
        assert!(boot_events(content).is_empty());
    }

    #[test]
    fn entry_received_within_two_minutes_of_monotonic_zero_is_a_boot() {
        // 5s, and one microsecond short of two minutes. The boot time is
        // this entry's realtime, not realtime minus monotonic.
        for monotonic in ["5000000", "119999999"] {
            let content = format!(
                r#"{{"__REALTIME_TIMESTAMP":"1792483805000000","__MONOTONIC_TIMESTAMP":"{monotonic}","_BOOT_ID":"abc","SYSLOG_IDENTIFIER":"systemd","MESSAGE":"Started Journal Service."}}"#
            );
            let boots = boot_events(&content);
            assert_eq!(boots.len(), 1, "monotonic {monotonic}");
            assert_eq!(
                boots[0].timestamp,
                Timestamp::from_microsecond(1_792_483_805_000_000).unwrap(),
                "monotonic {monotonic}"
            );
        }
    }

    #[test]
    fn entry_received_at_two_minutes_is_not_the_start_of_the_boot() {
        let content = r#"{"__REALTIME_TIMESTAMP":"1792483800000000","__MONOTONIC_TIMESTAMP":"120000000","_BOOT_ID":"abc","SYSLOG_IDENTIFIER":"systemd","MESSAGE":"Started Journal Service."}"#;
        assert!(boot_events(content).is_empty());
    }

    fn boot_events(content: &str) -> Vec<LogEvent> {
        parse_journal_json(content, "journal.json")
            .events
            .into_iter()
            .filter(|e| matches!(e.source, EventSource::Boot { .. }))
            .collect()
    }

    #[test]
    fn boot_id_on_a_later_line_is_not_a_boot_time() {
        // Eight hours after CLOCK_MONOTONIC zero. `_BOOT_ID` only names
        // the boot; this line's own timestamp is not when the boot happened.
        let content = "\
{\"__REALTIME_TIMESTAMP\":\"1792491120000000\",\"_BOOT_ID\":\"abc\",\"__MONOTONIC_TIMESTAMP\":\"28800000000\",\"SYSLOG_IDENTIFIER\":\"sshd\",\"MESSAGE\":\"Disconnected from user\"}
{\"__REALTIME_TIMESTAMP\":\"1792494720000000\",\"_BOOT_ID\":\"abc\",\"__MONOTONIC_TIMESTAMP\":\"32400000000\",\"SYSLOG_IDENTIFIER\":\"CROND\",\"MESSAGE\":\"(root) CMD (backup.sh)\"}
";
        let result = parse_journal_json(content, "journal.json");
        assert!(
            boot_events(content).is_empty(),
            "recorded a boot without boot-time evidence"
        );
        assert_eq!(result.events.len(), 1);
    }

    #[test]
    fn boot_time_is_the_banner_line_not_an_earlier_entry_of_the_same_boot() {
        let content = "\
{\"__REALTIME_TIMESTAMP\":\"1792483200000000\",\"_BOOT_ID\":\"abc\",\"__MONOTONIC_TIMESTAMP\":\"28800000000\",\"SYSLOG_IDENTIFIER\":\"sshd\",\"MESSAGE\":\"Disconnected from user\"}
{\"__REALTIME_TIMESTAMP\":\"1792483800000000\",\"_BOOT_ID\":\"abc\",\"__MONOTONIC_TIMESTAMP\":\"29400000000\",\"SYSLOG_IDENTIFIER\":\"kernel\",\"_TRANSPORT\":\"kernel\",\"MESSAGE\":\"Linux version 6.1.0 (builder@host) (gcc) #1 SMP\"}
";
        let boots = boot_events(content);
        assert_eq!(boots.len(), 1);
        assert_eq!(
            boots[0].timestamp,
            Timestamp::from_microsecond(1_792_483_800_000_000).unwrap(),
            "boot timestamp followed the earlier line instead of the kernel banner"
        );
        assert_eq!(boots[0].line, 2);
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
