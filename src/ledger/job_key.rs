//! Identifying which job a log event belongs to.

use serde::Serialize;

/// A job's matching identity: a cron job by `(user, normalized command)`,
/// a timer by the unit name it activates. Two cron jobs with the same
/// command but different users are different jobs; the same is true in
/// reverse.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub enum JobKey {
    Cron { user: String, command: String },
    Timer { unit: String },
}

/// Normalize a command for matching: collapse runs of whitespace to a
/// single space, trim the ends, and unescape `\%` to `%`. This is
/// deliberately conservative — it does not touch quoting, environment
/// expansion, or argument order — since cron logs the command verbatim
/// and this project has no shell to run it through.
///
/// The `\%` unescaping matters because the two sides of a match disagree
/// on it otherwise: the crontab parser already unescapes `\%` to `%` when
/// it splits the command from its stdin data, but a cron log line echoes
/// the command as written in the crontab, backslash and all.
pub fn normalize_command(command: &str) -> String {
    command
        .replace(r"\%", "%")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_whitespace() {
        assert_eq!(normalize_command("  echo   hi  "), "echo hi");
    }

    #[test]
    fn normalize_unescapes_percent() {
        assert_eq!(normalize_command(r"date +\%F"), "date +%F");
    }

    #[test]
    fn same_command_different_user_is_a_different_key() {
        let a = JobKey::Cron {
            user: "alice".to_string(),
            command: "echo hi".to_string(),
        };
        let b = JobKey::Cron {
            user: "bob".to_string(),
            command: "echo hi".to_string(),
        };
        assert_ne!(a, b);
    }
}
