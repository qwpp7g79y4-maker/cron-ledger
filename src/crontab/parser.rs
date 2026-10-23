//! Line-level parsing of crontab files: `/etc/crontab` and `cron.d/*` (which
//! have a user column) and per-user crontabs (which do not).

use super::command::split_command_stdin;
use super::field::{parse_field, DOM, DOW, HOUR, MINUTE, MONTH};
use super::model::{CronJob, ParseResult, ParseWarning, Schedule, ScheduleKind};

struct SpecialSchedule {
    minute: &'static str,
    hour: &'static str,
    dom: &'static str,
    month: &'static str,
    dow: &'static str,
}

fn special_schedule(name: &str) -> Option<SpecialSchedule> {
    Some(match name {
        "@yearly" | "@annually" => SpecialSchedule {
            minute: "0",
            hour: "0",
            dom: "1",
            month: "1",
            dow: "*",
        },
        "@monthly" => SpecialSchedule {
            minute: "0",
            hour: "0",
            dom: "1",
            month: "*",
            dow: "*",
        },
        "@weekly" => SpecialSchedule {
            minute: "0",
            hour: "0",
            dom: "*",
            month: "*",
            dow: "0",
        },
        "@daily" | "@midnight" => SpecialSchedule {
            minute: "0",
            hour: "0",
            dom: "*",
            month: "*",
            dow: "*",
        },
        "@hourly" => SpecialSchedule {
            minute: "0",
            hour: "*",
            dom: "*",
            month: "*",
            dow: "*",
        },
        _ => return None,
    })
}

/// Split a line into `n` whitespace-delimited leading fields plus the exact
/// remainder of the line (the command field keeps its internal spacing).
fn split_n_fields(line: &str, n: usize) -> Option<(Vec<&str>, &str)> {
    let mut rest = line;
    let mut fields = Vec::with_capacity(n);
    for _ in 0..n {
        let trimmed = rest.trim_start();
        if trimmed.is_empty() {
            return None;
        }
        let tok_len = trimmed
            .find(|c: char| c.is_whitespace())
            .unwrap_or(trimmed.len());
        let (tok, after) = trimmed.split_at(tok_len);
        fields.push(tok);
        rest = after;
    }
    let command = rest.trim_start().trim_end_matches('\r');
    if command.is_empty() {
        return None;
    }
    Some((fields, command))
}

/// Recognized as a `NAME=value` environment assignment line. `CRON_TZ` and
/// `TZ` affect schedule interpretation; everything else (`MAILTO`, `SHELL`,
/// `PATH`, ...) is accepted but otherwise ignored.
fn parse_env_line(line: &str) -> Option<(&str, &str)> {
    let eq = line.find('=')?;
    let name = line[..eq].trim_end();
    if name.is_empty()
        || !name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    let mut value = line[eq + 1..].trim_start();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            value = &value[1..value.len() - 1];
        }
    }
    Some((name, value))
}

struct EnvState {
    tz: Option<String>,
    cron_tz: Option<String>,
}

fn build_schedule(fields: &[&str], base: usize) -> Result<Schedule, String> {
    Ok(Schedule {
        minute: parse_field(&MINUTE, fields[base]).map_err(|e| format!("minute: {e}"))?,
        hour: parse_field(&HOUR, fields[base + 1]).map_err(|e| format!("hour: {e}"))?,
        dom: parse_field(&DOM, fields[base + 2]).map_err(|e| format!("day-of-month: {e}"))?,
        month: parse_field(&MONTH, fields[base + 3]).map_err(|e| format!("month: {e}"))?,
        dow: parse_field(&DOW, fields[base + 4]).map_err(|e| format!("day-of-week: {e}"))?,
        dom_star: fields[base + 2].starts_with('*'),
        dow_star: fields[base + 4].starts_with('*'),
    })
}

fn special_as_schedule(spec: &SpecialSchedule) -> Schedule {
    // These literal strings are always valid, so unwrap() here can't panic.
    Schedule {
        minute: parse_field(&MINUTE, spec.minute).unwrap(),
        hour: parse_field(&HOUR, spec.hour).unwrap(),
        dom: parse_field(&DOM, spec.dom).unwrap(),
        month: parse_field(&MONTH, spec.month).unwrap(),
        dow: parse_field(&DOW, spec.dow).unwrap(),
        dom_star: spec.dom.starts_with('*'),
        dow_star: spec.dow.starts_with('*'),
    }
}

#[allow(clippy::too_many_arguments)]
fn parse_lines(
    content: &str,
    file: &str,
    has_user_column: bool,
    implied_user: Option<&str>,
) -> ParseResult {
    let mut result = ParseResult::default();
    let mut env = EnvState {
        tz: None,
        cron_tz: None,
    };

    for (idx, raw_line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim_start();

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if let Some((name, value)) = parse_env_line(trimmed) {
            match name {
                "TZ" => env.tz = Some(value.to_string()),
                "CRON_TZ" => env.cron_tz = Some(value.to_string()),
                _ => {}
            }
            continue;
        }

        let first_token_len = trimmed
            .find(|c: char| c.is_whitespace())
            .unwrap_or(trimmed.len());
        let first_token = &trimmed[..first_token_len];

        let (schedule, user, command_field) = if first_token.starts_with('@') {
            let kind = if first_token == "@reboot" {
                Some(ScheduleKind::Reboot)
            } else {
                special_schedule(first_token)
                    .map(|spec| ScheduleKind::Standard(special_as_schedule(&spec)))
            };
            let Some(kind) = kind else {
                result.warnings.push(ParseWarning {
                    file: file.to_string(),
                    line: line_no,
                    message: format!("unknown special string '{first_token}'"),
                });
                continue;
            };
            let n = if has_user_column { 2 } else { 1 };
            let Some((fields, cmd)) = split_n_fields(trimmed, n) else {
                result.warnings.push(ParseWarning {
                    file: file.to_string(),
                    line: line_no,
                    message: "missing user or command".to_string(),
                });
                continue;
            };
            let user = if has_user_column {
                Some(fields[1].to_string())
            } else {
                implied_user.map(|u| u.to_string())
            };
            (kind, user, cmd)
        } else {
            let n = if has_user_column { 6 } else { 5 };
            let Some((fields, cmd)) = split_n_fields(trimmed, n) else {
                result.warnings.push(ParseWarning {
                    file: file.to_string(),
                    line: line_no,
                    message: "not enough schedule fields, or missing command".to_string(),
                });
                continue;
            };
            let schedule = match build_schedule(&fields, 0) {
                Ok(s) => s,
                Err(message) => {
                    result.warnings.push(ParseWarning {
                        file: file.to_string(),
                        line: line_no,
                        message,
                    });
                    continue;
                }
            };
            let user = if has_user_column {
                Some(fields[5].to_string())
            } else {
                implied_user.map(|u| u.to_string())
            };
            (ScheduleKind::Standard(schedule), user, cmd)
        };

        let (command, stdin) = split_command_stdin(command_field);
        result.jobs.push(CronJob {
            schedule,
            tz: env.tz.clone(),
            cron_tz: env.cron_tz.clone(),
            user,
            command,
            stdin,
            file: file.to_string(),
            line: line_no,
        });
    }

    result
}

/// Parse a per-user crontab (`crontab -l` style, or
/// `/var/spool/cron/crontabs/<user>`): 5 schedule fields plus command, no
/// inline user column. `implied_user` sets [`CronJob::user`] from the file's
/// location.
pub fn parse_user_crontab(content: &str, file: &str, implied_user: Option<&str>) -> ParseResult {
    parse_lines(content, file, false, implied_user)
}

/// Parse a system-style crontab (`/etc/crontab`, `/etc/cron.d/*`): 5
/// schedule fields, a user column, then the command.
pub fn parse_system_crontab(content: &str, file: &str) -> ParseResult {
    parse_lines(content, file, true, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crontab::model::Field;

    fn star_schedule() -> Schedule {
        Schedule {
            minute: Field::Star,
            hour: Field::Star,
            dom: Field::Star,
            month: Field::Star,
            dow: Field::Star,
            dom_star: true,
            dow_star: true,
        }
    }

    #[test]
    fn basic_user_crontab_line() {
        let result = parse_user_crontab("* * * * * echo hi\n", "crontab", Some("alice"));
        assert_eq!(result.warnings, vec![]);
        assert_eq!(result.jobs.len(), 1);
        let job = &result.jobs[0];
        assert_eq!(job.schedule, ScheduleKind::Standard(star_schedule()));
        assert_eq!(job.user.as_deref(), Some("alice"));
        assert_eq!(job.command, "echo hi");
        assert_eq!(job.stdin, None);
    }

    #[test]
    fn system_crontab_has_user_column() {
        let result = parse_system_crontab("0 3 * * * root backup.sh\n", "/etc/crontab");
        assert_eq!(result.jobs.len(), 1);
        assert_eq!(result.jobs[0].user.as_deref(), Some("root"));
        assert_eq!(result.jobs[0].command, "backup.sh");
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let result = parse_user_crontab("# hello\n\n   \n* * * * * echo hi\n", "f", None);
        assert_eq!(result.warnings, vec![]);
        assert_eq!(result.jobs.len(), 1);
    }

    #[test]
    fn mailto_is_recognized_and_ignored() {
        let result =
            parse_user_crontab("MAILTO=someone@example.com\n* * * * * echo hi\n", "f", None);
        assert_eq!(result.warnings, vec![]);
        assert_eq!(result.jobs.len(), 1);
        assert_eq!(result.jobs[0].tz, None);
        assert_eq!(result.jobs[0].cron_tz, None);
    }

    #[test]
    fn tz_and_cron_tz_apply_to_later_lines_cron_tz_wins() {
        let content = "\
TZ=UTC
* * * * * echo a
CRON_TZ=Europe/Amsterdam
* * * * * echo b
";
        let result = parse_user_crontab(content, "f", None);
        assert_eq!(result.jobs.len(), 2);
        assert_eq!(result.jobs[0].effective_tz(), Some("UTC"));
        assert_eq!(result.jobs[1].tz.as_deref(), Some("UTC"));
        assert_eq!(result.jobs[1].cron_tz.as_deref(), Some("Europe/Amsterdam"));
        assert_eq!(result.jobs[1].effective_tz(), Some("Europe/Amsterdam"));
    }

    #[test]
    fn whitespace_around_env_equals_sign_is_accepted() {
        let content = "CRON_TZ = Europe/Amsterdam\n* * * * * echo hi\n";
        let result = parse_user_crontab(content, "f", None);
        assert_eq!(result.warnings, vec![]);
        assert_eq!(result.jobs.len(), 1);
        assert_eq!(result.jobs[0].cron_tz.as_deref(), Some("Europe/Amsterdam"));
    }

    #[test]
    fn invalid_line_is_a_warning_not_a_crash() {
        let content = "\
* * * * * echo ok
nonsense line with no valid fields
60 * * * * echo bad-minute
";
        let result = parse_user_crontab(content, "f", None);
        assert_eq!(result.jobs.len(), 1);
        assert_eq!(result.warnings.len(), 2);
        assert_eq!(result.warnings[0].line, 2);
        assert_eq!(result.warnings[1].line, 3);
    }

    #[test]
    fn unknown_special_is_a_warning() {
        let result = parse_user_crontab("@fortnightly echo hi\n", "f", None);
        assert_eq!(result.jobs.len(), 0);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].message.contains("@fortnightly"));
    }

    #[test]
    fn reboot_has_no_schedule_fields() {
        let result = parse_user_crontab("@reboot echo booted\n", "f", None);
        assert_eq!(result.jobs.len(), 1);
        assert_eq!(result.jobs[0].schedule, ScheduleKind::Reboot);
    }

    #[test]
    fn all_specials_recognized() {
        let content = "\
@yearly echo a
@annually echo b
@monthly echo c
@weekly echo d
@daily echo e
@midnight echo f
@hourly echo g
@reboot echo h
";
        let result = parse_user_crontab(content, "f", None);
        assert_eq!(result.warnings, vec![]);
        assert_eq!(result.jobs.len(), 8);
    }

    #[test]
    fn percent_in_command_is_split_from_stdin() {
        let result = parse_user_crontab(r"* * * * * cat%line1%line2", "f", None);
        assert_eq!(result.jobs[0].command, "cat");
        assert_eq!(result.jobs[0].stdin.as_deref(), Some("line1\nline2"));
    }

    #[test]
    fn escaped_percent_in_command_is_literal() {
        let result = parse_user_crontab(r"* * * * * date +\%F", "f", None);
        assert_eq!(result.jobs[0].command, "date +%F");
        assert_eq!(result.jobs[0].stdin, None);
    }

    #[test]
    fn vixie_dom_dow_or_rule_field_shapes_preserved() {
        // "0 0 13 * 5" (the 13th OR a Friday) must parse with neither
        // day field marked `*`-unrestricted.
        let result = parse_user_crontab("0 0 13 * 5 echo hi\n", "f", None);
        let ScheduleKind::Standard(schedule) = &result.jobs[0].schedule else {
            panic!("expected standard schedule");
        };
        assert!(!schedule.dom_star);
        assert!(!schedule.dow_star);
        assert_eq!(schedule.month, Field::Star);
    }

    #[test]
    fn dom_step_field_is_still_star_for_the_or_and_rule() {
        // "0 0 */2 * 1": the day-of-month field's raw text starts with
        // `*`, so it stays unrestricted for the OR/AND rule even though
        // it only matches odd days.
        let result = parse_user_crontab("0 0 */2 * 1 echo hi\n", "f", None);
        let ScheduleKind::Standard(schedule) = &result.jobs[0].schedule else {
            panic!("expected standard schedule");
        };
        assert!(schedule.dom_star);
        assert!(!schedule.dow_star);
    }
}
