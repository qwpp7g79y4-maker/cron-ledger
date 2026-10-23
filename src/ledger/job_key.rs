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

/// Normalize a command for matching: trim the ends and unescape `\%` to
/// `%`. Whitespace inside the command is kept, including inside quoted
/// arguments. Cron keeps that whitespace, so collapsing it would merge
/// commands that differ. This does not touch quoting, environment
/// expansion, or argument order.
///
/// Unescaping `\%` covers a log line that still has the crontab spelling.
/// The parser has already turned `\%` into `%` while splitting stdin off
/// the command. Cron's own `CMD` line is logged after that same split, so
/// it already contains `%`; a line copied from the crontab text may not.
pub fn normalize_command(command: &str) -> String {
    command.replace(r"\%", "%").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_trims_boundaries_and_keeps_internal_whitespace() {
        // Only the boundaries are trimmed. Whitespace inside the command,
        // including inside a quoted argument, is preserved.
        assert_eq!(normalize_command("  echo hi  "), "echo hi");
        assert_eq!(
            normalize_command("echo \"hello  world\""),
            "echo \"hello  world\""
        );
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
