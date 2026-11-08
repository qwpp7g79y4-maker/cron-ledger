//! The `--csv` report: RFC 4180, one row per audited run, so a
//! spreadsheet or `csvkit` can filter on status.

use crate::ledger::JobOutcome;

use super::{job_views, neutralize_controls, ReportContext};

const HEADER: &str = "job,file,line,expected,status,actual,late_by_secs,exit_status";

/// Quote one field. Control characters are made visible first, and a
/// leading `=`, `+`, `-` or `@` gets a `'` in front so a spreadsheet does
/// not read untrusted text (a command, a path) as a formula.
fn field(raw: &str) -> String {
    let mut s = neutralize_controls(raw);
    if s.starts_with(['=', '+', '-', '@']) {
        s.insert(0, '\'');
    }
    if s.contains([',', '"']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

fn label(key: &crate::ledger::JobKey) -> String {
    match key {
        crate::ledger::JobKey::Cron { user, command } => format!("{user}: {command}"),
        crate::ledger::JobKey::Timer { unit } => unit.clone(),
    }
}

/// Render `ledger` as CSV. Every line ends in `\n`.
pub fn render_csv(ledger: &crate::ledger::Ledger, ctx: &ReportContext) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    for v in job_views(ledger, ctx.descriptions) {
        let head = format!("{},{},{}", field(&label(v.key)), field(v.file), v.line);
        match v.outcome {
            JobOutcome::Standard(runs) => {
                for r in runs {
                    let actual = r.actual.map(|a| a.to_string()).unwrap_or_default();
                    let late = r
                        .late_by
                        .map(|d| d.as_secs().to_string())
                        .unwrap_or_default();
                    out.push_str(&format!(
                        "{head},{},{:?},{actual},{late},{:?}\n",
                        r.expected, r.status, r.exit_status
                    ));
                }
            }
            JobOutcome::Reboot { boots } => {
                if boots.is_empty() {
                    out.push_str(&format!("{head},,RebootOnly,,,\n"));
                }
                for b in boots {
                    out.push_str(&format!("{head},,RebootOnly,{b},,\n"));
                }
            }
            JobOutcome::UnsupportedTrigger => {
                out.push_str(&format!("{head},,UnsupportedTrigger,,,\n"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_field_is_unchanged() {
        assert_eq!(field("root: backup"), "root: backup");
    }

    #[test]
    fn comma_and_quote_are_quoted_and_doubled() {
        assert_eq!(field("a,\"b\""), "\"a,\"\"b\"\"\"");
    }

    #[test]
    fn formula_prefix_is_defused() {
        assert_eq!(field("=1+1"), "'=1+1");
        assert_eq!(field("@x"), "'@x");
    }

    #[test]
    fn newline_cannot_start_a_new_record() {
        assert!(!field("a\nb").contains('\n'));
    }
}
