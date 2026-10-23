//! Parsing of syslog-style cron logs: Debian's `/var/log/syslog`
//! (`CRON[pid]: (user) CMD (cmd)`) and RHEL/cronie's `/var/log/cron`
//! (`CROND[pid]: (user) CMD (cmd)`, plus `CMDOUT`/`CMDEND`). RFC3164
//! (`Mon DD HH:MM:SS`, no year), RFC5424 (a leading `<PRI>` and a full
//! timestamp), and rsyslog's high-precision file format (a leading
//! RFC3339 timestamp, no priority) are all supported.

use super::event::{EventSource, LogEvent, LogParseResult, LogWarning};
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use jiff::Timestamp;

const MONTH_NAMES: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

fn month_number(name: &str) -> Option<i8> {
    let lower = name.to_ascii_lowercase();
    MONTH_NAMES
        .iter()
        .position(|m| *m == lower)
        .map(|i| (i + 1) as i8)
}

/// A year rollover is only ever this project's own invention to fill in a
/// gap RFC3164 leaves open; a backward jump this large can only be a wrap
/// from December back to January, not ordinary log reordering.
const ROLLOVER_THRESHOLD_SECS: i64 = 300 * 24 * 3600;

/// Fills in the year RFC3164 timestamps omit, from the window's `since`
/// and the fact that a real log's lines are in non-decreasing time order.
struct YearInference<'a> {
    tz: &'a TimeZone,
    since: Timestamp,
    state: Option<(i16, Timestamp)>,
}

impl<'a> YearInference<'a> {
    fn new(tz: &'a TimeZone, since: Timestamp) -> Self {
        YearInference {
            tz,
            since,
            state: None,
        }
    }

    fn build(&self, year: i16, month: i8, day: i8, h: i8, m: i8, s: i8) -> Option<Timestamp> {
        let civil = DateTime::new(year, month, day, h, m, s, 0).ok()?;
        civil.to_zoned(self.tz.clone()).ok().map(|z| z.timestamp())
    }

    fn resolve(&mut self, month: i8, day: i8, h: i8, m: i8, s: i8) -> Option<Timestamp> {
        let resolved = match self.state {
            None => {
                let since_year = self.since.to_zoned(self.tz.clone()).date().year();
                [since_year - 1, since_year, since_year + 1]
                    .into_iter()
                    .filter_map(|y| self.build(y, month, day, h, m, s).map(|ts| (y, ts)))
                    .min_by_key(|(_, ts)| (ts.as_second() - self.since.as_second()).abs())
            }
            Some((year, prev_ts)) => match self.build(year, month, day, h, m, s) {
                Some(ts) if prev_ts.as_second() - ts.as_second() > ROLLOVER_THRESHOLD_SECS => self
                    .build(year + 1, month, day, h, m, s)
                    .map(|ts2| (year + 1, ts2)),
                Some(ts) => Some((year, ts)),
                None => self
                    .build(year + 1, month, day, h, m, s)
                    .map(|ts2| (year + 1, ts2)),
            },
        };
        if let Some((year, ts)) = resolved {
            self.state = Some((year, ts));
        }
        resolved.map(|(_, ts)| ts)
    }
}

fn warn(warnings: &mut Vec<LogWarning>, file: &str, line: usize, message: impl Into<String>) {
    warnings.push(LogWarning {
        file: file.to_string(),
        line,
        message: message.into(),
    });
}

/// Split the part of a line after the hostname into `(tag[pid], message)`.
///
/// The tag is one token. A `]: ` later in the message (a bare `CRON:` tag
/// whose command contains `x[1]: y`) is not the tag's own separator.
fn split_tag(rest: &str) -> Option<(&str, &str)> {
    if let Some(idx) = rest.find("]: ") {
        let tag = &rest[..=idx];
        if !tag.chars().any(char::is_whitespace) {
            return Some((tag, rest[idx + 3..].trim_start()));
        }
    }
    rest.split_once(": ")
        .map(|(tag, msg)| (tag, msg.trim_start()))
}

/// Strip a leading `<PRI>` when `PRI` is one to three digits. `None` when
/// the line has no such prefix.
fn strip_pri(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('<')?;
    let gt = rest.find('>')?;
    let pri = &rest[..gt];
    if pri.is_empty() || pri.len() > 3 || !pri.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(&rest[gt + 1..])
}

/// RFC5424 after the priority is `VERSION SP TIMESTAMP ...`, and VERSION
/// is a short integer. `<PRI>Oct ...` and `<PRI>2026-10-20T...` are not.
fn looks_like_rfc5424_version(after_pri: &str) -> bool {
    let Some((ver, _)) = after_pri.split_once(' ') else {
        return false;
    };
    !ver.is_empty() && ver.len() <= 3 && ver.bytes().all(|b| b.is_ascii_digit())
}

/// Split `n` leading whitespace-delimited fields off `line`, collapsing
/// runs of whitespace between them (RFC3164 pads a single-digit day with
/// an extra space: `"Oct  2 ..."`), and return the untouched remainder.
fn split_ws_fields(line: &str, n: usize) -> Option<(Vec<&str>, &str)> {
    let mut rest = line;
    let mut fields = Vec::with_capacity(n);
    for _ in 0..n {
        let trimmed = rest.trim_start();
        if trimmed.is_empty() {
            return None;
        }
        let tok_len = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
        let (tok, after) = trimmed.split_at(tok_len);
        fields.push(tok);
        rest = after;
    }
    Some((fields, rest.trim_start()))
}

/// Split `TAG[pid]` (or bare `TAG`) into its name and optional pid.
fn parse_tag(tag: &str) -> (&str, Option<u32>) {
    if let Some(open) = tag.find('[') {
        if let Some(close) = tag[open..].find(']') {
            let close = open + close;
            let name = &tag[..open];
            let pid = tag[open + 1..close].parse::<u32>().ok();
            return (name, pid);
        }
    }
    (tag, None)
}

fn is_cron_tag(name: &str) -> bool {
    name.eq_ignore_ascii_case("cron") || name.eq_ignore_ascii_case("crond")
}

/// Parse `(user) ACTION (arg)` into `(user, ACTION, arg)`.
fn parse_cron_message(msg: &str) -> Option<(String, String, String)> {
    let msg = msg.trim();
    let rest = msg.strip_prefix('(')?;
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

enum Outcome {
    Event(EventSource),
    Skip,
    Garbage(String),
}

/// Debian/Ubuntu's cron writes a PAM session line for every run, tagged
/// as cron but with no `(user) ACTION (arg)` body: `pam_unix(cron:session):
/// session opened/closed for user ...`. Real traffic, not a malformed line.
fn is_pam_session_message(message: &str) -> bool {
    message.starts_with("pam_unix(cron:session):")
}

fn classify(name: &str, pid: Option<u32>, message: &str) -> Outcome {
    if !is_cron_tag(name) {
        return Outcome::Skip;
    }
    if is_pam_session_message(message) {
        return Outcome::Skip;
    }
    let Some((user, action, arg)) = parse_cron_message(message) else {
        return Outcome::Garbage(format!("'{name}' line has no '(user) ACTION (arg)' body"));
    };
    match action.to_ascii_uppercase().as_str() {
        "CMD" => Outcome::Event(EventSource::CronCmd {
            user,
            pid,
            command: arg,
        }),
        "CMDOUT" => Outcome::Event(EventSource::CronCmdOut { user, pid }),
        "CMDEND" => Outcome::Event(EventSource::CronCmdEnd {
            user,
            pid,
            command: arg,
        }),
        _ => Outcome::Skip,
    }
}

/// Try RFC5424 framing: `<PRI>VERSION TIMESTAMP HOST APP PROCID MSGID SD MSG`.
/// Only the `-` (nil) structured-data value is supported; anything else is
/// reported as a warning rather than guessed at.
fn try_rfc5424(
    line: &str,
    file: &str,
    line_no: usize,
    warnings: &mut Vec<LogWarning>,
) -> Option<(Timestamp, EventSource)> {
    let rest = line.strip_prefix('<')?;
    let gt = rest.find('>')?;
    let rest = &rest[gt + 1..];
    let mut fields = rest.splitn(7, ' ');
    let _version = fields.next()?;
    let timestamp_str = fields.next()?;
    let _host = fields.next()?;
    let app = fields.next()?;
    if !is_cron_tag(app) {
        return None;
    }
    let procid = fields.next()?;
    let _msgid = fields.next()?;
    let sd_and_msg = fields.next()?;
    let timestamp: Timestamp = match timestamp_str.parse() {
        Ok(ts) => ts,
        Err(e) => {
            warn(
                warnings,
                file,
                line_no,
                format!("bad RFC5424 timestamp: {e}"),
            );
            return None;
        }
    };
    let message = match sd_and_msg.strip_prefix("- ") {
        Some(msg) => msg,
        None if sd_and_msg == "-" => "",
        None => {
            warn(
                warnings,
                file,
                line_no,
                "RFC5424 structured data other than '-' is not supported",
            );
            return None;
        }
    };
    let pid = procid.parse::<u32>().ok();
    match classify(app, pid, message) {
        Outcome::Event(source) => Some((timestamp, source)),
        Outcome::Skip => None,
        Outcome::Garbage(reason) => {
            warn(warnings, file, line_no, reason);
            None
        }
    }
}

/// `YYYY-MM-DDTHH:MM:SS` at the start of the first token. The rest of an
/// RFC3339 timestamp (fraction, `Z`, or a numeric offset) may be missing
/// or malformed; a recognised cron line with a bad timestamp still warns.
fn looks_like_leading_rfc3339(line: &str) -> bool {
    let token = line.split_whitespace().next().unwrap_or("");
    let b = token.as_bytes();
    if b.len() < 19 {
        return false;
    }
    let digit = |i: usize| b[i].is_ascii_digit();
    digit(0)
        && digit(1)
        && digit(2)
        && digit(3)
        && b[4] == b'-'
        && digit(5)
        && digit(6)
        && b[7] == b'-'
        && digit(8)
        && digit(9)
        && (b[10] == b'T' || b[10] == b't')
        && digit(11)
        && digit(12)
        && b[13] == b':'
        && digit(14)
        && digit(15)
        && b[16] == b':'
        && digit(17)
        && digit(18)
}

/// rsyslog's `RSYSLOG_FileFormat`: one RFC3339 timestamp, the hostname,
/// then `TAG[pid]: message`, with no `<PRI>`. Debian made this the
/// default in rsyslog 8.2210.0-3 (November 2022). Ubuntu 22.04 still
/// forces the traditional format in `rsyslog.conf`.
/// The timestamp is one token, so the RFC3164 four-field split does not
/// apply. A non-cron line is skipped. A cron line whose timestamp does
/// not parse is a warning, same as a bad RFC3164 timestamp.
fn parse_leading_rfc3339(
    line: &str,
    file: &str,
    line_no: usize,
    events: &mut Vec<LogEvent>,
    warnings: &mut Vec<LogWarning>,
) {
    let Some((fields, rest)) = split_ws_fields(line, 2) else {
        return;
    };
    let Some((tag, message)) = split_tag(rest) else {
        return;
    };
    let (name, pid) = parse_tag(tag);
    let outcome = classify(name, pid, message);
    if matches!(outcome, Outcome::Skip) {
        return;
    }
    let timestamp = match fields[0].parse::<Timestamp>() {
        Ok(ts) => ts,
        Err(e) => {
            warn(
                warnings,
                file,
                line_no,
                format!("bad RFC3339 timestamp: {e}"),
            );
            return;
        }
    };
    match outcome {
        Outcome::Event(source) => events.push(LogEvent {
            timestamp,
            source,
            file: file.to_string(),
            line: line_no,
        }),
        Outcome::Garbage(reason) => warn(warnings, file, line_no, reason),
        Outcome::Skip => {}
    }
}

/// Parse a syslog-format cron log: every `CRON`/`CROND` `CMD`/`CMDOUT`/
/// `CMDEND` line becomes a [`LogEvent`]; unrelated syslog lines are
/// skipped silently; lines that look like a cron entry but are malformed
/// are counted as warnings, never fatal.
///
/// `since` and `tz` seed the RFC3164 year inference (RFC3164 timestamps
/// carry no year). RFC5424 lines and rsyslog's high-precision file format
/// carry their own timestamp and do not need either. A traditional or
/// file-format line may start with `<PRI>`; only `<PRI>VERSION ...` is
/// RFC5424.
pub fn parse_syslog_cron(
    content: &str,
    file: &str,
    since: Timestamp,
    tz: &TimeZone,
) -> LogParseResult {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut events = Vec::new();
    let mut warnings = Vec::new();
    let mut years = YearInference::new(tz, since);

    for (idx, raw_line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let mut line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }

        // `<PRI>` is shared by RFC5424 and by traditional lines. Only a
        // numeric VERSION after the priority is RFC5424; anything else
        // is the same line with the priority removed.
        if let Some(after_pri) = strip_pri(line) {
            if looks_like_rfc5424_version(after_pri) {
                if let Some((timestamp, source)) = try_rfc5424(line, file, line_no, &mut warnings) {
                    events.push(LogEvent {
                        timestamp,
                        source,
                        file: file.to_string(),
                        line: line_no,
                    });
                }
                continue;
            }
            line = after_pri;
        } else if line.starts_with('<') {
            if let Some((timestamp, source)) = try_rfc5424(line, file, line_no, &mut warnings) {
                events.push(LogEvent {
                    timestamp,
                    source,
                    file: file.to_string(),
                    line: line_no,
                });
            }
            continue;
        }

        // rsyslog high-precision lines lead with one RFC3339 token.
        // Detect that before the RFC3164 split, which would otherwise
        // consume the timestamp, the host, the tag, and the user, and
        // then drop the line because the remainder has no ": ".
        if looks_like_leading_rfc3339(line) {
            parse_leading_rfc3339(line, file, line_no, &mut events, &mut warnings);
            continue;
        }

        // Decide whether this is a cron line before complaining about
        // the timestamp. Unrelated non-PRI traffic (and lines that are
        // not RFC3164 at all) is skipped. A recognized CRON/CROND entry
        // still warns when its timestamp does not parse.
        let Some((fields, rest)) = split_ws_fields(line, 4) else {
            continue;
        };
        let Some((tag, message)) = split_tag(rest) else {
            continue;
        };
        let (name, pid) = parse_tag(tag);
        let outcome = classify(name, pid, message);
        if matches!(outcome, Outcome::Skip) {
            continue;
        }

        let (mon, day, time) = (fields[0], fields[1], fields[2]);
        let Some(month) = month_number(mon) else {
            warn(
                &mut warnings,
                file,
                line_no,
                format!("'{mon}' is not a month name"),
            );
            continue;
        };
        let Ok(day) = day.parse::<i8>() else {
            warn(
                &mut warnings,
                file,
                line_no,
                format!("'{day}' is not a day of month"),
            );
            continue;
        };
        let mut time_parts = time.splitn(3, ':');
        let (Some(h), Some(m), Some(s)) = (time_parts.next(), time_parts.next(), time_parts.next())
        else {
            warn(
                &mut warnings,
                file,
                line_no,
                format!("'{time}' is not HH:MM:SS"),
            );
            continue;
        };
        let (Ok(h), Ok(m), Ok(s)) = (h.parse::<i8>(), m.parse::<i8>(), s.parse::<i8>()) else {
            warn(
                &mut warnings,
                file,
                line_no,
                format!("'{time}' is not HH:MM:SS"),
            );
            continue;
        };

        match outcome {
            Outcome::Event(source) => {
                let Some(timestamp) = years.resolve(month, day, h, m, s) else {
                    warn(
                        &mut warnings,
                        file,
                        line_no,
                        "timestamp does not exist in any nearby year",
                    );
                    continue;
                };
                events.push(LogEvent {
                    timestamp,
                    source,
                    file: file.to_string(),
                    line: line_no,
                });
            }
            Outcome::Skip => {}
            Outcome::Garbage(reason) => warn(&mut warnings, file, line_no, reason),
        }
    }

    LogParseResult {
        events,
        warnings,
        coverage_timestamps: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::EventSource;

    fn utc() -> TimeZone {
        TimeZone::UTC
    }

    #[test]
    fn debian_cmd_line() {
        let content = "Oct 20 03:00:01 host CRON[1234]: (alice) CMD (backup.sh)\n";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.warnings, vec![]);
        assert_eq!(result.events.len(), 1);
        assert_eq!(
            result.events[0].source,
            EventSource::CronCmd {
                user: "alice".to_string(),
                pid: Some(1234),
                command: "backup.sh".to_string(),
            }
        );
        assert_eq!(
            result.events[0].timestamp.to_string(),
            "2026-10-20T03:00:01Z"
        );
    }

    #[test]
    fn rhel_cmdout_cmdend_sequence() {
        let content = "\
Oct 20 03:00:01 host CROND[5555]: (alice) CMD (backup.sh)
Oct 20 03:00:02 host CROND[5555]: (alice) CMDOUT (done)
Oct 20 03:00:03 host CROND[5555]: (alice) CMDEND (backup.sh)
";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "cron", since, &utc());
        assert_eq!(result.warnings, vec![]);
        assert_eq!(result.events.len(), 3);
        assert!(matches!(
            result.events[0].source,
            EventSource::CronCmd { .. }
        ));
        assert!(matches!(
            result.events[1].source,
            EventSource::CronCmdOut { .. }
        ));
        assert!(matches!(
            result.events[2].source,
            EventSource::CronCmdEnd { .. }
        ));
    }

    #[test]
    fn unrelated_syslog_lines_are_skipped_not_garbage() {
        let content = "Oct 20 03:00:01 host sshd[1]: Accepted publickey for root\n";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.events, vec![]);
        assert_eq!(result.warnings, vec![]);
    }

    #[test]
    fn malformed_cron_line_is_a_warning_not_fatal() {
        let content = "\
Oct 20 03:00:01 host CRON[1]: (alice) CMD (ok.sh)
Oct 20 03:00:02 host CRON[2]: garbage, no parens at all
";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.warnings[0].line, 2);
    }

    #[test]
    fn year_is_inferred_from_the_window() {
        let content = "Mar 15 12:00:00 host CRON[1]: (alice) CMD (x)\n";
        let since: Timestamp = "2026-03-01T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(
            result.events[0].timestamp.to_string(),
            "2026-03-15T12:00:00Z"
        );
    }

    #[test]
    fn year_boundary_rolls_over_forward() {
        let content = "\
Dec 31 23:59:00 host CRON[1]: (alice) CMD (x)
Jan  1 00:01:00 host CRON[2]: (alice) CMD (x)
";
        let since: Timestamp = "2026-12-31T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(
            result.events[0].timestamp.to_string(),
            "2026-12-31T23:59:00Z"
        );
        assert_eq!(
            result.events[1].timestamp.to_string(),
            "2027-01-01T00:01:00Z"
        );
    }

    #[test]
    fn pri_prefixed_rfc3164_cron_line_is_read() {
        let content = "<78>Oct 20 03:00:01 host CRON[1]: (root) CMD (/bin/backup.sh)\n";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.warnings, vec![]);
        assert_eq!(result.events.len(), 1);
        assert_eq!(
            result.events[0].source,
            EventSource::CronCmd {
                user: "root".to_string(),
                pid: Some(1),
                command: "/bin/backup.sh".to_string(),
            }
        );
    }

    #[test]
    fn pri_prefixed_rfc3339_cron_line_is_read() {
        let content = "<78>2026-10-20T03:00:01+00:00 host CRON[1]: (root) CMD (/bin/backup.sh)\n";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.warnings, vec![]);
        assert_eq!(result.events.len(), 1);
        assert_eq!(
            result.events[0].timestamp.to_string(),
            "2026-10-20T03:00:01Z"
        );
    }

    #[test]
    fn pri_prefixed_non_cron_line_stays_silent() {
        let content = "<78>Oct 20 03:00:01 host sshd[1]: Accepted publickey for root\n";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.events, vec![]);
        assert_eq!(result.warnings, vec![]);
    }

    #[test]
    fn bare_cron_tag_keeps_a_message_that_contains_bracket_colon() {
        let content = "Oct 20 03:00:01 host CRON: (root) CMD (/bin/backup.sh x[1]: y)\n";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.warnings, vec![]);
        assert_eq!(
            result.events[0].source,
            EventSource::CronCmd {
                user: "root".to_string(),
                pid: None,
                command: "/bin/backup.sh x[1]: y".to_string(),
            }
        );
    }

    #[test]
    fn leading_bom_does_not_drop_the_first_syslog_line() {
        let content = "\u{feff}2026-10-20T03:00:01Z host CRON[1]: (root) CMD (/bin/backup.sh)\n";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.warnings, vec![]);
        assert_eq!(result.events.len(), 1);
    }

    #[test]
    fn rfc5424_timestamp_is_supported() {
        let content =
            "<13>1 2026-10-20T03:00:01.500000+02:00 host crond 1234 - - (alice) CMD (backup.sh)\n";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.warnings, vec![]);
        assert_eq!(result.events.len(), 1);
        assert_eq!(
            result.events[0].timestamp.to_string(),
            "2026-10-20T01:00:01.5Z"
        );
        assert_eq!(
            result.events[0].source,
            EventSource::CronCmd {
                user: "alice".to_string(),
                pid: Some(1234),
                command: "backup.sh".to_string(),
            }
        );
    }

    #[test]
    fn non_cron_rfc5424_line_with_structured_data_is_skipped_silently() {
        let content = "<13>1 2026-10-20T03:00:01+02:00 host rsyslogd 1234 - [origin swVersion=\"8.0\"] some message\n";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.events, vec![]);
        assert_eq!(result.warnings, vec![]);
    }

    #[test]
    fn non_cron_rfc5424_line_with_a_bad_timestamp_is_skipped_silently() {
        let content = "<13>1 not-a-timestamp host rsyslogd 1234 - - some message\n";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.events, vec![]);
        assert_eq!(result.warnings, vec![]);
    }

    #[test]
    fn percent_in_command_is_kept_literal_in_the_log_line() {
        let content = "Oct 20 06:00:00 host CRON[1]: (alice) CMD (date +\\%F)\n";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(
            result.events[0].source,
            EventSource::CronCmd {
                user: "alice".to_string(),
                pid: Some(1),
                command: "date +\\%F".to_string(),
            }
        );
    }

    #[test]
    fn out_of_range_hour_is_a_warning_not_a_panic() {
        let content = "Oct 20 25:00:00 host CRON[1]: (alice) CMD (x)\n";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.events, vec![]);
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.warnings[0].line, 1);
    }

    #[test]
    fn leap_second_is_a_warning_not_a_panic() {
        let content = "Oct 20 23:59:60 host CRON[1]: (alice) CMD (x)\n";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.events, vec![]);
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.warnings[0].line, 1);
    }

    #[test]
    fn unrelated_line_does_not_warn_about_the_month_a_cron_line_still_does() {
        // "this line..." and the sshd line are not cron. The CRON line
        // is, so its bad month is still a warning, and the good line
        // is still an event.
        let content = "\
this line is not syslog at all
NotAMonth 99 99:99:99 web1 sshd[9]: Accepted
NotAMonth 99 99:99:99 web1 CRON[4]: (alice) CMD (echo hi)
Oct 20 03:00:01 web1 CRON[5]: (alice) CMD (echo ok)
";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.warnings[0].line, 3);
        assert!(result.warnings[0].message.contains("NotAMonth"));
        assert_eq!(result.events.len(), 1);
        assert_eq!(
            result.events[0].source,
            EventSource::CronCmd {
                user: "alice".to_string(),
                pid: Some(5),
                command: "echo ok".to_string(),
            }
        );
    }

    #[test]
    fn rsyslog_fileformat_rfc3339_line_is_a_cron_event() {
        let content = "2026-10-20T03:00:01.123456+02:00 host CRON[1]: (root) CMD (x)\n";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.warnings, vec![]);
        assert_eq!(result.events.len(), 1, "high-precision line was dropped");
        assert_eq!(
            result.events[0].timestamp.to_string(),
            "2026-10-20T01:00:01.123456Z"
        );
        assert_eq!(
            result.events[0].source,
            EventSource::CronCmd {
                user: "root".to_string(),
                pid: Some(1),
                command: "x".to_string(),
            }
        );
    }

    #[test]
    fn rsyslog_fileformat_zulu_timestamp_is_a_cron_event() {
        let content = "2026-10-20T01:00:01.123456Z host CROND[7]: (root) CMD (x)\n";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.warnings, vec![]);
        assert_eq!(
            result.events.len(),
            1,
            "Zulu high-precision line was dropped"
        );
        assert_eq!(
            result.events[0].timestamp.to_string(),
            "2026-10-20T01:00:01.123456Z"
        );
    }

    #[test]
    fn malformed_rfc3339_cron_line_warns_instead_of_vanishing() {
        let content = "2026-10-20T25:00:01.123456+02:00 host CRON[1]: (root) CMD (x)\n";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.events, vec![]);
        assert_eq!(
            result.warnings.len(),
            1,
            "malformed CRON timestamp was silent"
        );
        assert_eq!(result.warnings[0].line, 1);
        assert!(result.warnings[0].message.contains("bad RFC3339 timestamp"));
    }

    #[test]
    fn rsyslog_fileformat_without_a_fraction_is_a_cron_event() {
        // Forwarded records often omit the sub-second part. The date is
        // still one RFC3339 token.
        let content = "2026-10-20T03:00:01+02:00 host CRON[1]: (root) CMD (x)\n";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.warnings, vec![]);
        assert_eq!(result.events.len(), 1);
        assert_eq!(
            result.events[0].timestamp.to_string(),
            "2026-10-20T01:00:01Z"
        );
    }

    #[test]
    fn non_cron_rfc3339_lines_are_skipped_silently() {
        let content = "\
2026-10-20T03:00:01.123456+02:00 host sshd[1]: Accepted publickey for root
2026-10-20T25:00:01Z host sshd[2]: Accepted publickey for root
";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.events, vec![]);
        assert_eq!(result.warnings, vec![]);
    }

    #[test]
    fn pam_session_lines_are_skipped_not_garbage() {
        let content = "\
Oct 20 03:00:00 host CRON[1]: pam_unix(cron:session): session opened for user root(uid=0) by (uid=0)
Oct 20 03:00:01 host CRON[1]: (root) CMD (backup.sh)
Oct 20 03:00:02 host CRON[1]: pam_unix(cron:session): session closed for user root
";
        let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let result = parse_syslog_cron(content, "syslog", since, &utc());
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.warnings, vec![]);
    }
}
