//! Parsing of `journalctl -o json` output (newline-delimited JSON, one
//! object per entry): `SYSLOG_IDENTIFIER` cron/CRON/crond with the same
//! `(user) CMD (cmd)` style `MESSAGE` cronie/vixie write to syslog,
//! a manager `Started ...` or `Finished ...` message
//! (`SYSLOG_IDENTIFIER=systemd`) for timer-activated services, preferring
//! `UNIT` over `_SYSTEMD_UNIT`, and `_BOOT_ID` names a boot for
//! `@reboot`'s informational list. `Starting ...` is not a run.
//! `Finished ...` after `Started ...` for the same unit is the same run,
//! not a second one. The boot's time comes only from an entry that shows
//! it: a kernel banner, or any entry received within two minutes of
//! `CLOCK_MONOTONIC` zero.
//! `MESSAGE` may be a JSON string, or (for non-UTF-8 output) a JSON array
//! of byte values. A repeated key in one object is ambiguous, so that
//! line is skipped with a warning and is not evidence: it is not a run
//! and it does not extend log coverage.

use super::event::{EventSource, LogEvent, LogParseResult, LogWarning};
use jiff::Timestamp;
use serde::de::{self, Deserialize, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;
use std::rc::Rc;

fn warn(warnings: &mut Vec<LogWarning>, file: &str, line: usize, message: impl Into<String>) {
    warnings.push(LogWarning {
        file: file.to_string(),
        line,
        message: message.into(),
    });
}

/// One journal field as text. A string is itself. journalctl encodes a
/// repeated field as an array of that value; every element has to be
/// the same string, otherwise the field is ambiguous and rejected.
fn single_string_field(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => {
            let first = items.first()?.as_str()?;
            if items.iter().all(|item| item.as_str() == Some(first)) {
                Some(first.to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

fn extract_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => Some(s.clone()),
        // journalctl encodes a non-UTF-8 field as an array of byte
        // values, each 0..=255. A larger number is not that encoding;
        // wrapping it with `as u8` would invent a different message.
        Value::Array(items) => {
            let mut bytes = Vec::with_capacity(items.len());
            for item in items {
                let n = item.as_u64()?;
                bytes.push(u8::try_from(n).ok()?);
            }
            Some(String::from_utf8_lossy(&bytes).into_owned())
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
    // `/dev/kmsg` entries arrive with `_TRANSPORT=kernel`. A line a user
    // wrote with `logger -t kernel` has `_TRANSPORT=syslog` and the same
    // identifier; that is not a boot banner. An export that simply omits
    // `_TRANSPORT` still counts on the identifier, which is how a
    // hand-trimmed journal line names the kernel.
    let identifier_is_kernel = obj
        .get("SYSLOG_IDENTIFIER")
        .and_then(|v| v.as_str())
        .is_some_and(|id| id.eq_ignore_ascii_case("kernel"));
    match obj.get("_TRANSPORT").and_then(|v| v.as_str()) {
        Some("kernel") => true,
        Some(_) => false,
        None => identifier_is_kernel,
    }
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
        Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
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

/// One journal line, after duplicate keys have been rejected.
enum JournalLine {
    Object(serde_json::Map<String, Value>),
    /// The first repeated key. Later copies are not read.
    DuplicateKey(String),
    NotObject,
}

impl<'de> Deserialize<'de> for JournalLine {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JournalLineVisitor)
    }
}

struct JournalLineVisitor;

impl<'de> Visitor<'de> for JournalLineVisitor {
    type Value = JournalLine;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a journal JSON value")
    }

    fn visit_bool<E: de::Error>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(JournalLine::NotObject)
    }

    fn visit_i64<E: de::Error>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(JournalLine::NotObject)
    }

    fn visit_u64<E: de::Error>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(JournalLine::NotObject)
    }

    fn visit_f64<E: de::Error>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(JournalLine::NotObject)
    }

    fn visit_i128<E: de::Error>(self, _value: i128) -> Result<Self::Value, E> {
        Ok(JournalLine::NotObject)
    }

    fn visit_u128<E: de::Error>(self, _value: u128) -> Result<Self::Value, E> {
        Ok(JournalLine::NotObject)
    }

    fn visit_str<E: de::Error>(self, _value: &str) -> Result<Self::Value, E> {
        Ok(JournalLine::NotObject)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(JournalLine::NotObject)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(JournalLine::NotObject)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut out = serde_json::Map::new();
        let mut duplicate = None;
        while let Some((key, value)) = map.next_entry::<String, Value>()? {
            if duplicate.is_some() {
                continue;
            }
            if out.contains_key(&key) {
                duplicate = Some(key);
                continue;
            }
            out.insert(key, value);
        }
        Ok(match duplicate {
            Some(key) => JournalLine::DuplicateKey(key),
            None => JournalLine::Object(out),
        })
    }
}

/// Read one JSON value. A repeated key is [`JournalLine::DuplicateKey`]
/// rather than a silently overwritten field.
fn read_journal_line(line: &str) -> Result<JournalLine, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_str(line);
    let parsed = JournalLine::deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(parsed)
}

/// Parse `journalctl -o json` NDJSON output into [`LogEvent`]s.
pub fn parse_journal_json(content: &str, file: &str) -> LogParseResult {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    // Shared by every event this call produces; see the same note on
    // `parse_syslog_cron` in `syslog.rs`.
    let file_rc: Rc<str> = Rc::from(file);
    let mut events = Vec::new();
    let mut warnings = Vec::new();
    let mut coverage_timestamps = Vec::new();
    let mut seen_boots: HashSet<String> = HashSet::new();
    // Units whose current activation already logged "Started ". A later
    // "Finished " for that unit closes it and is not a second run.
    // Type=oneshot never logs "Started ", so its "Finished " still counts.
    let mut open_started: HashSet<String> = HashSet::new();

    for (idx, raw_line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        let obj = match read_journal_line(line) {
            Ok(JournalLine::Object(obj)) => obj,
            Ok(JournalLine::NotObject) => {
                warn(
                    &mut warnings,
                    file,
                    line_no,
                    "journal entry is not a JSON object",
                );
                continue;
            }
            Ok(JournalLine::DuplicateKey(key)) => {
                warn(&mut warnings, file, line_no, format!("duplicate key {key}"));
                continue;
            }
            Err(e) => {
                warn(&mut warnings, file, line_no, format!("invalid JSON: {e}"));
                continue;
            }
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
        // failed before this point (invalid JSON, not an object, a
        // repeated key, no usable timestamp) are not.
        coverage_timestamps.push(timestamp);

        if let Some(boot_id) = obj.get("_BOOT_ID").and_then(|v| v.as_str()) {
            // Do not mark the id seen until this entry actually shows
            // the boot time. A later line of the same boot may be the
            // one that does.
            if !seen_boots.contains(boot_id)
                && (is_kernel_boot_banner(&obj) || received_at_boot_start(&obj))
            {
                seen_boots.insert(boot_id.to_string());
                events.push(LogEvent {
                    timestamp,
                    source: EventSource::Boot {
                        boot_id: Some(boot_id.to_string()),
                    },
                    file: Rc::clone(&file_rc),
                    line: line_no,
                });
            }
        }

        // Missing is normal (not every entry is cron). An array that is
        // not one repeated string, or some other type, is a field we
        // cannot attribute; ignoring it would drop a real cron line.
        let identifier = match single_string_field(obj.get("SYSLOG_IDENTIFIER")) {
            Some(s) => s,
            None if obj.get("SYSLOG_IDENTIFIER").is_none() => String::new(),
            None => {
                warn(
                    &mut warnings,
                    file,
                    line_no,
                    "SYSLOG_IDENTIFIER is not a single string",
                );
                String::new()
            }
        };
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
            let pid = extract_pid(&obj);
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
                    file: Rc::clone(&file_rc),
                    line: line_no,
                });
            }
        } else if identifier.eq_ignore_ascii_case("systemd") {
            // Only the manager itself (not a service logging its own
            // "Started ..." line, under its own unit, with its own
            // identifier) gets to report that *it* started a unit.
            // Type=simple and Type=forking log "Started ". Type=oneshot
            // logs "Finished " and not "Started ". "Starting " is the
            // announcement, not a run. Both lines for one activation
            // are one run.
            if let (Some(unit), Some(message)) = (unit, extract_string(obj.get("MESSAGE"))) {
                let counts_as_run = if message.starts_with("Started ") {
                    let _ = open_started.insert(unit.to_string());
                    true
                } else if message.starts_with("Finished ") {
                    !open_started.remove(unit)
                } else {
                    false
                };
                if counts_as_run {
                    events.push(LogEvent {
                        timestamp,
                        source: EventSource::SystemdUnitStart {
                            unit: unit.to_string(),
                        },
                        file: Rc::clone(&file_rc),
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
    fn syslog_transport_kernel_banner_is_not_a_boot() {
        // `logger -t kernel` stores this shape: the identifier says
        // kernel, the transport says the message came from syslog.
        let content = r#"{"__REALTIME_TIMESTAMP":"1792483920000000","__MONOTONIC_TIMESTAMP":"28800000000","_BOOT_ID":"b1","_TRANSPORT":"syslog","SYSLOG_IDENTIFIER":"kernel","_UID":"1000","MESSAGE":"Linux version fake"}"#;
        assert!(boot_events(content).is_empty());
    }

    #[test]
    fn byte_above_255_is_not_decoded_as_a_different_message() {
        let content = r#"{"__REALTIME_TIMESTAMP":"1792897201000000","SYSLOG_IDENTIFIER":"CROND","MESSAGE":[40,114,111,111,116,41,32,67,77,68,32,40,354,97,99,107,117,112,46,115,104,41]}"#;
        let result = parse_journal_json(content, "journal.json");
        assert!(result.events.is_empty());
        assert_eq!(result.warnings.len(), 1);
    }

    #[test]
    fn pid_that_does_not_fit_in_u32_is_absent() {
        let content = r#"{"__REALTIME_TIMESTAMP":"1792897201000000","SYSLOG_IDENTIFIER":"CROND","_PID":4294967297,"MESSAGE":"(root) CMD (backup.sh)"}"#;
        let result = parse_journal_json(content, "journal.json");
        assert_eq!(result.warnings, vec![]);
        assert_eq!(
            result.events[0].source,
            EventSource::CronCmd {
                user: "root".to_string(),
                pid: None,
                command: "backup.sh".to_string(),
            }
        );
    }

    #[test]
    fn syslog_identifier_array_of_one_value_is_read() {
        // journalctl stores a repeated field as an array. One value, or
        // the same value repeated, is that identifier.
        let content = r#"{"__REALTIME_TIMESTAMP":"1792897201000000","SYSLOG_IDENTIFIER":["CROND","CROND"],"MESSAGE":"(root) CMD (backup.sh)"}"#;
        let result = parse_journal_json(content, "journal.json");
        assert_eq!(result.warnings, vec![]);
        assert_eq!(
            result.events[0].source,
            EventSource::CronCmd {
                user: "root".to_string(),
                pid: None,
                command: "backup.sh".to_string(),
            }
        );
    }

    #[test]
    fn syslog_identifier_array_of_two_values_is_not_a_cron_line() {
        let content = r#"{"__REALTIME_TIMESTAMP":"1792897201000000","SYSLOG_IDENTIFIER":["sshd","CROND"],"MESSAGE":"(root) CMD (backup.sh)"}"#;
        let result = parse_journal_json(content, "journal.json");
        assert!(result.events.is_empty());
        assert_eq!(result.warnings.len(), 1);
    }

    #[test]
    fn leading_bom_does_not_drop_the_first_journal_line() {
        let content = "\u{feff}{\"__REALTIME_TIMESTAMP\":\"1792897201000000\",\"SYSLOG_IDENTIFIER\":\"CROND\",\"MESSAGE\":\"(root) CMD (backup.sh)\"}\n";
        let result = parse_journal_json(content, "journal.json");
        assert_eq!(result.warnings, vec![]);
        assert_eq!(result.events.len(), 1);
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

    #[test]
    fn duplicate_message_key_is_not_evidence() {
        // The second MESSAGE would be a real cron run if the last value
        // won. A repeated key is ambiguous, so the line is not evidence.
        // The following line is still read.
        let content = "\
{\"__REALTIME_TIMESTAMP\":\"1792465201000000\",\"SYSLOG_IDENTIFIER\":\"CROND\",\"MESSAGE\":\"(root) CMD (/bin/other.sh)\",\"MESSAGE\":\"(root) CMD (/bin/backup.sh)\"}
{\"__REALTIME_TIMESTAMP\":\"1792465202000000\",\"SYSLOG_IDENTIFIER\":\"CROND\",\"MESSAGE\":\"(root) CMD (/bin/ok.sh)\"}
";
        let result = parse_journal_json(content, "journal.json");
        assert!(
            !result.events.iter().any(|event| {
                matches!(
                    &event.source,
                    EventSource::CronCmd { command, .. }
                        if command == "/bin/backup.sh" || command == "/bin/other.sh"
                )
            }),
            "duplicate MESSAGE was used as evidence: {:?}",
            result.events
        );
        assert_eq!(result.events.len(), 1);
        assert!(matches!(
            &result.events[0].source,
            EventSource::CronCmd { command, .. } if command == "/bin/ok.sh"
        ));
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.warnings[0].line, 1);
        assert!(
            result.warnings[0].message.contains("duplicate key"),
            "{}",
            result.warnings[0].message
        );
        assert!(result.warnings[0].message.contains("MESSAGE"));
        assert_eq!(result.coverage_timestamps.len(), 1);
    }

    #[test]
    fn repeated_key_in_a_journal_object_is_not_evidence() {
        // Each line would be evidence if the last copy of the key won:
        // a cron run, a boot, or a timer start.
        let lines = [
            r#"{"__REALTIME_TIMESTAMP":"1792465201000000","SYSLOG_IDENTIFIER":"sshd","SYSLOG_IDENTIFIER":"CROND","MESSAGE":"(root) CMD (/bin/backup.sh)"}"#,
            r#"{"__REALTIME_TIMESTAMP":"0","__REALTIME_TIMESTAMP":"1792465201000000","SYSLOG_IDENTIFIER":"CROND","MESSAGE":"(root) CMD (/bin/backup.sh)"}"#,
            r#"{"__REALTIME_TIMESTAMP":"1792483800000000","__MONOTONIC_TIMESTAMP":"600000000","_BOOT_ID":"abc","_TRANSPORT":"syslog","_TRANSPORT":"kernel","SYSLOG_IDENTIFIER":"kernel","MESSAGE":"Linux version 6.1.0 (builder@host) (gcc) #1 SMP"}"#,
            r#"{"__REALTIME_TIMESTAMP":"1792483805000000","__MONOTONIC_TIMESTAMP":"600000000","__MONOTONIC_TIMESTAMP":"5000000","_BOOT_ID":"abc","SYSLOG_IDENTIFIER":"systemd","MESSAGE":"Started Journal Service."}"#,
            r#"{"__REALTIME_TIMESTAMP":"1792483800000000","__MONOTONIC_TIMESTAMP":"5000000","_BOOT_ID":"old","_BOOT_ID":"new","SYSLOG_IDENTIFIER":"systemd","MESSAGE":"Started Journal Service."}"#,
            r#"{"__REALTIME_TIMESTAMP":"1792897201000000","SYSLOG_IDENTIFIER":"CROND","_PID":"1","_PID":"4242","MESSAGE":"(root) CMD (backup.sh)"}"#,
            r#"{"__REALTIME_TIMESTAMP":"1792897201000000","SYSLOG_IDENTIFIER":"systemd","UNIT":"other.service","UNIT":"backup.service","MESSAGE":"Started Nightly backup."}"#,
            r#"{"__REALTIME_TIMESTAMP":"1792897201000000","SYSLOG_IDENTIFIER":"systemd","_SYSTEMD_UNIT":"init.scope","_SYSTEMD_UNIT":"backup.service","MESSAGE":"Started Nightly backup."}"#,
            r#"{"__REALTIME_TIMESTAMP":"1792897201000000","SYSLOG_IDENTIFIER":"CROND","_UID":"0","_UID":"1","MESSAGE":"(root) CMD (backup.sh)"}"#,
        ];
        for line in lines {
            let result = parse_journal_json(line, "journal.json");
            assert!(
                result.events.is_empty(),
                "{line} was used as evidence: {:?}",
                result.events
            );
            assert_eq!(result.warnings.len(), 1, "{line}");
            assert!(
                result.warnings[0].message.contains("duplicate key"),
                "{line}: {}",
                result.warnings[0].message
            );
            assert!(result.coverage_timestamps.is_empty(), "{line}");
        }
    }
}
