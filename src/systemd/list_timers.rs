//! A saved `systemctl list-timers --all` listing.
//!
//! The text has LAST, NEXT and UNIT (plus LEFT, PASSED and ACTIVATES,
//! which are only column boundaries). There is no calendar, so this is
//! not a schedule expansion and it is not matched against logs. The only
//! check is whether the recorded last run is older than a next elapse
//! that was already due.
//!
//! No `systemctl` binary is executed. The caller passes the text.

use std::time::Duration;

use jiff::civil::{DateTime, Weekday};
use jiff::tz::{Offset, TimeZone};
use jiff::Timestamp;

/// One data row. `line` is 1-based in `file`. `last` and `next` are
/// `None` for `-` or `n/a`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedTimer {
    /// 1-based line in the listing file.
    pub line: usize,
    /// UNIT column.
    pub unit: String,
    /// LAST column. `None` for `-` or `n/a`.
    pub last: Option<Timestamp>,
    /// NEXT column. `None` for `-` or `n/a`.
    pub next: Option<Timestamp>,
}

/// What [`parse_list_timers`] read. Malformed rows are warnings, not a
/// hard error; the caller decides whether zero usable rows is fatal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListTimersParse {
    /// Data rows that parsed.
    pub timers: Vec<ListedTimer>,
    /// Malformed rows and other notes. Not a hard error.
    pub warnings: Vec<String>,
}

/// The list-timers check for one row, against the audit window's end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListedTimerVerdict {
    /// LAST is present and no NEXT was already due past `tolerance`.
    Current {
        /// LAST column of a row that is not overdue.
        last: Timestamp,
    },
    /// NEXT was due before `until` by more than `tolerance`, and LAST is
    /// missing or earlier than that NEXT by more than `tolerance`.
    OlderThanExpected {
        /// LAST column, when the row has one.
        last: Option<Timestamp>,
        /// NEXT time that was already due past the tolerance.
        expected: Timestamp,
    },
    /// Nothing to compare: no LAST, and NEXT is absent or not yet due.
    NothingToCheck,
}

/// Parse the human table from `systemctl list-timers --all`.
///
/// The header row supplies the column starts, because LEFT and PASSED
/// contain spaces (`1 day 4h ago`). A missing header yields no rows and
/// one warning. The `N timers listed.` footer ends the table. An unknown
/// zone abbreviation is read in `tz` and warned about; `UTC` and the
/// usual fixed abbreviations are applied as fixed offsets.
pub fn parse_list_timers(text: &str, file: &str, tz: &TimeZone) -> ListTimersParse {
    let mut warnings = Vec::new();
    let mut header: Option<Header> = None;
    let mut timers = Vec::new();

    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = strip_ansi(raw_line);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if header.is_none() {
            if let Some(found) = parse_header(&line) {
                header = Some(found);
            }
            continue;
        }
        if is_footer(trimmed) {
            break;
        }
        if trimmed.starts_with("Pass --all") {
            continue;
        }
        let Some(columns) = &header else {
            continue;
        };
        if let Some(row) = parse_row(&line, columns, tz, file, line_no, &mut warnings) {
            timers.push(row);
        }
    }

    if header.is_none() {
        warnings.push(format!(
            "{file}: no systemctl list-timers header (expected NEXT, LAST and UNIT)"
        ));
    }

    ListTimersParse { timers, warnings }
}

/// `next` is due when it falls far enough before `until` that the grace
/// (`tolerance`) has also expired. LAST is older than that due time when
/// it is missing or earlier than NEXT by more than `tolerance`. The
/// window start does not pardon a NEXT that was already overdue.
pub fn evaluate_listed_timer(
    last: Option<Timestamp>,
    next: Option<Timestamp>,
    until: Timestamp,
    tolerance: Duration,
) -> ListedTimerVerdict {
    if let Some(expected) = next.filter(|&next| past_tolerance(next, until, tolerance)) {
        let older = match last {
            Some(last) => past_tolerance(last, expected, tolerance),
            None => true,
        };
        if older {
            return ListedTimerVerdict::OlderThanExpected { last, expected };
        }
    }
    match last {
        Some(last) => ListedTimerVerdict::Current { last },
        None => ListedTimerVerdict::NothingToCheck,
    }
}

/// `earlier + tolerance < later`. A tolerance that does not fit in a
/// span, or a sum that overflows, is not "past": the grace cannot be
/// shown to have expired.
fn past_tolerance(earlier: Timestamp, later: Timestamp, tolerance: Duration) -> bool {
    let Ok(secs) = i64::try_from(tolerance.as_secs()) else {
        return false;
    };
    match earlier.checked_add(jiff::Span::new().seconds(secs)) {
        Ok(limit) => limit < later,
        Err(_) => false,
    }
}

struct Header {
    next: usize,
    last: usize,
    unit: usize,
    /// Every column start, so a value ends where the next column begins.
    starts: Vec<usize>,
}

fn parse_header(line: &str) -> Option<Header> {
    let mut next = None;
    let mut last = None;
    let mut unit = None;
    let mut starts = Vec::new();
    for (idx, word) in token_starts(line) {
        starts.push(idx);
        match word.to_ascii_uppercase().as_str() {
            "NEXT" if next.is_none() => next = Some(idx),
            "LAST" if last.is_none() => last = Some(idx),
            "UNIT" if unit.is_none() => unit = Some(idx),
            _ => {}
        }
    }
    Some(Header {
        next: next?,
        last: last?,
        unit: unit?,
        starts,
    })
}

fn column_end(header: &Header, start: usize) -> usize {
    header
        .starts
        .iter()
        .copied()
        .filter(|&idx| idx > start)
        .min()
        .unwrap_or(usize::MAX)
}

fn parse_row(
    line: &str,
    header: &Header,
    tz: &TimeZone,
    file: &str,
    line_no: usize,
    warnings: &mut Vec<String>,
) -> Option<ListedTimer> {
    let unit = char_slice(line, header.unit, column_end(header, header.unit))
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if unit.is_empty() || unit == "-" || unit.eq_ignore_ascii_case("n/a") {
        warnings.push(format!("{file}:{line_no}: timer row has no UNIT"));
        return None;
    }
    let next = match parse_time_cell(
        char_slice(line, header.next, column_end(header, header.next)),
        tz,
        file,
        line_no,
        "NEXT",
        warnings,
    ) {
        Ok(ts) => ts,
        Err(message) => {
            warnings.push(message);
            return None;
        }
    };
    let last = match parse_time_cell(
        char_slice(line, header.last, column_end(header, header.last)),
        tz,
        file,
        line_no,
        "LAST",
        warnings,
    ) {
        Ok(ts) => ts,
        Err(message) => {
            warnings.push(message);
            return None;
        }
    };
    Some(ListedTimer {
        line: line_no,
        unit,
        last,
        next,
    })
}

/// `Ok(None)` is `-` or `n/a`. `Err` skips the row. A zone this table
/// does not know is still `Ok(Some)` and adds a warning: the civil time
/// is read in `tz`.
fn parse_time_cell(
    cell: &str,
    tz: &TimeZone,
    file: &str,
    line_no: usize,
    column: &str,
    warnings: &mut Vec<String>,
) -> Result<Option<Timestamp>, String> {
    let cell = cell.trim();
    if cell.is_empty() || cell == "-" || cell.eq_ignore_ascii_case("n/a") {
        return Ok(None);
    }
    let mut parts = cell.split_whitespace();
    let dow = parts.next().unwrap_or("");
    let date = parts.next().unwrap_or("");
    let time = parts.next().unwrap_or("");
    let zone = parts.next().unwrap_or("");
    if dow.is_empty()
        || date.is_empty()
        || time.is_empty()
        || zone.is_empty()
        || parts.next().is_some()
    {
        return Err(format!(
            "{file}:{line_no}: {column} '{cell}' is not a systemctl timestamp"
        ));
    }
    if weekday_name(dow).is_none() {
        return Err(format!(
            "{file}:{line_no}: {column} '{cell}' is not a systemctl timestamp"
        ));
    }
    let civil: DateTime = format!("{date}T{time}")
        .parse()
        .map_err(|_| format!("{file}:{line_no}: {column} '{cell}' is not a systemctl timestamp"))?;
    if let Some(want) = weekday_name(dow) {
        if civil.weekday() != want {
            warnings.push(format!(
                "{file}:{line_no}: {column} weekday {dow} does not match {date}; the date is used"
            ));
        }
    }
    let instant = match zone_offset(zone) {
        Some(offset) => civil
            .to_zoned(TimeZone::fixed(offset))
            .map_err(|e| format!("{file}:{line_no}: {column} '{cell}': {e}"))?
            .timestamp(),
        None => {
            warnings.push(format!(
                "{file}:{line_no}: {column} zone '{zone}' is not a known abbreviation; read as the audit timezone"
            ));
            civil
                .to_zoned(tz.clone())
                .map_err(|e| format!("{file}:{line_no}: {column} '{cell}': {e}"))?
                .timestamp()
        }
    };
    Ok(Some(instant))
}

fn weekday_name(name: &str) -> Option<Weekday> {
    Some(match name {
        "Mon" => Weekday::Monday,
        "Tue" => Weekday::Tuesday,
        "Wed" => Weekday::Wednesday,
        "Thu" => Weekday::Thursday,
        "Fri" => Weekday::Friday,
        "Sat" => Weekday::Saturday,
        "Sun" => Weekday::Sunday,
        _ => return None,
    })
}

/// Fixed offset for the abbreviation systemd prints. These are not a
/// full timezone: `EST` is always UTC−5, including on a date when that
/// region would have been on daylight time. `UTC` needs no guess.
fn zone_offset(token: &str) -> Option<Offset> {
    if let Some(seconds) = named_zone_seconds(token).or_else(|| numeric_offset_seconds(token)) {
        return Offset::from_seconds(seconds).ok();
    }
    None
}

fn named_zone_seconds(token: &str) -> Option<i32> {
    let hours_mins: (i32, i32) = match token {
        "UTC" | "GMT" | "Z" | "UT" | "U" | "WET" => (0, 0),
        "WEST" | "CET" | "BST" => (1, 0),
        "CEST" | "EET" => (2, 0),
        "EEST" => (3, 0),
        "EST" => (-5, 0),
        "EDT" => (-4, 0),
        "CST" => (-6, 0),
        "CDT" => (-5, 0),
        "MST" => (-7, 0),
        "MDT" => (-6, 0),
        "PST" => (-8, 0),
        "PDT" => (-7, 0),
        "IST" => (5, 30),
        "JST" | "KST" => (9, 0),
        "ACST" => (9, 30),
        "AEST" => (10, 0),
        "AEDT" => (11, 0),
        "NZST" => (12, 0),
        "NZDT" => (13, 0),
        _ => return None,
    };
    Some(hours_mins.0 * 3600 + hours_mins.1 * 60)
}

fn numeric_offset_seconds(token: &str) -> Option<i32> {
    let (sign, rest) = if let Some(rest) = token.strip_prefix('+') {
        (1, rest)
    } else {
        let rest = token.strip_prefix('-')?;
        (-1, rest)
    };
    if rest.is_empty() {
        return None;
    }
    let (hours, mins) = if let Some((hours, mins)) = rest.split_once(':') {
        if hours.len() != 2 || mins.len() != 2 {
            return None;
        }
        (hours.parse::<i32>().ok()?, mins.parse::<i32>().ok()?)
    } else if rest.len() == 2 || rest.len() == 4 {
        if !rest.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let (hours, mins) = if rest.len() == 2 {
            (rest, "0")
        } else {
            rest.split_at(2)
        };
        (hours.parse::<i32>().ok()?, mins.parse::<i32>().ok()?)
    } else {
        return None;
    };
    if hours > 23 || mins > 59 {
        return None;
    }
    Some(sign * (hours * 3600 + mins * 60))
}

fn token_starts(line: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut chars = line.char_indices().peekable();
    let mut char_idx = 0usize;
    while let Some(&(byte, ch)) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            char_idx += 1;
            continue;
        }
        let start_char = char_idx;
        let start_byte = byte;
        while let Some(&(_, ch)) = chars.peek() {
            if ch.is_whitespace() {
                break;
            }
            chars.next();
            char_idx += 1;
        }
        let end_byte = chars.peek().map(|&(byte, _)| byte).unwrap_or(line.len());
        out.push((start_char, &line[start_byte..end_byte]));
    }
    out
}

fn char_slice(line: &str, start: usize, end: usize) -> &str {
    if start >= end {
        return "";
    }
    let mut start_byte = None;
    let mut end_byte = line.len();
    for (idx, (byte, _)) in line.char_indices().enumerate() {
        if idx == start {
            start_byte = Some(byte);
        }
        if idx == end {
            end_byte = byte;
            break;
        }
    }
    match start_byte {
        Some(start_byte) if start_byte < end_byte => &line[start_byte..end_byte],
        _ => "",
    }
}

fn is_footer(line: &str) -> bool {
    let mut parts = line.split_whitespace();
    let Some(count) = parts.next() else {
        return false;
    };
    if count.is_empty() || !count.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let Some(word) = parts.next() else {
        return false;
    };
    if word != "timer" && word != "timers" {
        return false;
    }
    matches!(parts.next(), Some("listed." | "listed")) && parts.next().is_none()
}

/// Drop CSI color sequences (`ESC [ ... letter`) and carriage returns.
/// `systemctl` writes those when the capture was a terminal. Column
/// positions are then the visible ones, matching the header.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            continue;
        }
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn utc() -> TimeZone {
        TimeZone::UTC
    }

    /// Pad columns the way systemd's table does: each cell starts at the
    /// same index as its header word.
    fn table(header: &[&str], rows: &[&[&str]]) -> String {
        let width = header.len();
        let mut widths = header.iter().map(|cell| cell.len()).collect::<Vec<_>>();
        for row in rows {
            for (idx, cell) in row.iter().enumerate() {
                widths[idx] = widths[idx].max(cell.len());
            }
        }
        let fmt = |cells: &[&str]| -> String {
            let mut parts = Vec::new();
            for (idx, cell) in cells.iter().enumerate() {
                if idx + 1 == width {
                    parts.push((*cell).to_string());
                } else {
                    parts.push(format!("{cell:<width$}", width = widths[idx]));
                }
            }
            parts.join(" ")
        };
        let mut lines = vec![fmt(header)];
        for row in rows {
            lines.push(fmt(row));
        }
        lines.join("\n")
    }

    #[test]
    fn fixture_last_runs_are_parsed() {
        let text = include_str!("../../tests/fixtures/systemd/list-timers.txt");
        let parsed = parse_list_timers(text, "list-timers.txt", &utc());
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.timers.len(), 4);

        let logrotate = &parsed.timers[0];
        assert_eq!(logrotate.line, 2);
        assert_eq!(logrotate.unit, "logrotate.timer");
        assert_eq!(logrotate.last, Some(ts("2026-10-24T00:00:00Z")));
        assert_eq!(logrotate.next, Some(ts("2026-10-25T00:00:00Z")));

        let backup = &parsed.timers[1];
        assert_eq!(backup.unit, "backup.timer");
        assert_eq!(backup.last, Some(ts("2026-10-23T06:00:00Z")));
        assert_eq!(backup.next, Some(ts("2026-10-24T12:00:00Z")));

        assert_eq!(parsed.timers[2].unit, "pending.timer");
        assert_eq!(parsed.timers[2].last, None);
        assert_eq!(parsed.timers[2].next, Some(ts("2026-10-26T00:00:00Z")));
        assert_eq!(parsed.timers[3].unit, "idle.timer");
        assert_eq!(parsed.timers[3].last, None);
        assert_eq!(parsed.timers[3].next, None);

        let until = ts("2026-10-25T00:00:00Z");
        let tolerance = Duration::from_secs(300);
        assert_eq!(
            evaluate_listed_timer(logrotate.last, logrotate.next, until, tolerance),
            ListedTimerVerdict::Current {
                last: ts("2026-10-24T00:00:00Z")
            }
        );
        assert_eq!(
            evaluate_listed_timer(backup.last, backup.next, until, tolerance),
            ListedTimerVerdict::OlderThanExpected {
                last: Some(ts("2026-10-23T06:00:00Z")),
                expected: ts("2026-10-24T12:00:00Z"),
            }
        );
        assert_eq!(
            evaluate_listed_timer(
                parsed.timers[2].last,
                parsed.timers[2].next,
                until,
                tolerance
            ),
            ListedTimerVerdict::NothingToCheck
        );
        assert_eq!(
            evaluate_listed_timer(
                parsed.timers[3].last,
                parsed.timers[3].next,
                until,
                tolerance
            ),
            ListedTimerVerdict::NothingToCheck
        );
    }

    #[test]
    fn man_page_row_keeps_last_apart_from_spaced_passed_column() {
        // The shape from systemctl(1): LEFT and PASSED contain spaces,
        // and the zone abbreviation is a fixed offset (EST = UTC−5).
        let text = table(
            &["NEXT", "LEFT", "LAST", "PASSED", "UNIT", "ACTIVATES"],
            &[&[
                "Sun 2017-02-26 18:55:42 EST",
                "1min 14s left",
                "Thu 2017-02-23 13:54:44 EST",
                "3 days ago",
                "systemd-tmpfiles-clean.timer",
                "systemd-tmpfiles-clean.service",
            ]],
        );
        let parsed = parse_list_timers(&text, "man.txt", &utc());
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.timers.len(), 1);
        assert_eq!(parsed.timers[0].unit, "systemd-tmpfiles-clean.timer");
        assert_eq!(parsed.timers[0].last, Some(ts("2017-02-23T18:54:44Z")));
        assert_eq!(parsed.timers[0].next, Some(ts("2017-02-26T23:55:42Z")));
    }

    #[test]
    fn missing_header_is_a_warning_and_no_rows() {
        let parsed = parse_list_timers("not a table\n", "x.txt", &utc());
        assert!(parsed.timers.is_empty());
        assert!(parsed.warnings[0].contains("no systemctl list-timers header"));
    }

    #[test]
    fn unknown_zone_is_read_in_the_audit_timezone() {
        let text = table(
            &["NEXT", "LEFT", "LAST", "PASSED", "UNIT", "ACTIVATES"],
            &[&[
                "-",
                "-",
                "Mon 2026-10-26 03:00:00 XYZ",
                "1h ago",
                "backup.timer",
                "backup.service",
            ]],
        );
        let parsed = parse_list_timers(&text, "z.txt", &utc());
        assert_eq!(parsed.timers[0].last, Some(ts("2026-10-26T03:00:00Z")));
        assert!(
            parsed.warnings.iter().any(|w| w.contains("XYZ")),
            "{:?}",
            parsed.warnings
        );
    }

    #[test]
    fn ansi_header_still_lines_up_with_the_row() {
        let plain = table(
            &["NEXT", "LEFT", "LAST", "PASSED", "UNIT", "ACTIVATES"],
            &[&[
                "-",
                "-",
                "Sat 2026-10-24 00:00:00 UTC",
                "2h ago",
                "logrotate.timer",
                "logrotate.service",
            ]],
        );
        let mut colored = String::new();
        for line in plain.lines() {
            colored.push_str(&format!("\u{1b}[4m{line}\u{1b}[0m\n"));
        }
        let parsed = parse_list_timers(&colored, "color.txt", &utc());
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.timers[0].last, Some(ts("2026-10-24T00:00:00Z")));
    }

    #[test]
    fn a_due_next_within_tolerance_is_not_older() {
        let until = ts("2026-10-24T12:04:00Z");
        let next = Some(ts("2026-10-24T12:00:00Z"));
        let last = Some(ts("2026-10-23T06:00:00Z"));
        assert_eq!(
            evaluate_listed_timer(last, next, until, Duration::from_secs(300)),
            ListedTimerVerdict::Current {
                last: ts("2026-10-23T06:00:00Z")
            }
        );
    }

    #[test]
    fn last_within_tolerance_of_the_due_next_is_not_older() {
        let until = ts("2026-10-24T13:00:00Z");
        let next = Some(ts("2026-10-24T12:00:00Z"));
        let last = Some(ts("2026-10-24T11:56:00Z"));
        assert_eq!(
            evaluate_listed_timer(last, next, until, Duration::from_secs(300)),
            ListedTimerVerdict::Current {
                last: ts("2026-10-24T11:56:00Z")
            }
        );
    }

    #[test]
    fn a_due_next_with_no_last_run_is_older_than_expected() {
        let until = ts("2026-10-25T00:00:00Z");
        let next = Some(ts("2026-10-24T12:00:00Z"));
        assert_eq!(
            evaluate_listed_timer(None, next, until, Duration::from_secs(300)),
            ListedTimerVerdict::OlderThanExpected {
                last: None,
                expected: ts("2026-10-24T12:00:00Z"),
            }
        );
    }
}
