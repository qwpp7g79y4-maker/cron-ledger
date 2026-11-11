//! `--diff OLD NEW`: compare two saved `--json` reports.
//!
//! Jobs are matched by their `key`; a key that occurs more than once is
//! paired in file order. For two matched `Standard` jobs every expected
//! time is compared on its `status`. Anything else is compared whole.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use crate::input_limit::{explain_read, read_capped_file};
use crate::report::neutralize_controls;

/// Text of one `--diff` and whether the two reports differ.
pub struct DiffResult {
    /// The comparison, one line per difference, or `no differences`.
    pub text: String,
    /// `true` when a job was added, removed, or a matched outcome differs.
    pub changed: bool,
}

/// Load two saved `--json` reports and compare them.
///
/// A file past the input limit is not loaded.
pub fn diff_files(old: &Path, new: &Path) -> Result<DiffResult, String> {
    let a = load(old)?;
    let b = load(new)?;
    diff_reports(&a, &b)
}

fn load(path: &Path) -> Result<Value, String> {
    let bytes =
        read_capped_file(path).map_err(|e| explain_read(&path.display().to_string(), &e, false))?;
    serde_json::from_slice(&bytes).map_err(|e| {
        format!(
            "{} is not a JSON report: {e}; pass a file written by cron-ledger --json",
            path.display()
        )
    })
}

fn label(key: &Value) -> String {
    if let Some(c) = key.get("Cron") {
        let user = c.get("user").and_then(Value::as_str).unwrap_or("?");
        let cmd = c.get("command").and_then(Value::as_str).unwrap_or("?");
        format!("{user}: {cmd}")
    } else if let Some(t) = key.get("Timer") {
        t.get("unit")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string()
    } else {
        key.to_string()
    }
}

fn check(report: &Value, which: &str) -> Result<(), String> {
    match report.get("schema_version").and_then(Value::as_u64) {
        Some(1) => {}
        other => {
            return Err(format!(
                "{which} report has schema_version {other:?}, expected 1; regenerate it with this cron-ledger version using --json"
            ))
        }
    }
    if report.get("jobs").and_then(Value::as_array).is_none() {
        return Err(format!(
            "{which} report has no \"jobs\" list; pass a file written by cron-ledger --json"
        ));
    }
    Ok(())
}

type Grouped<'a> = BTreeMap<String, Vec<&'a Value>>;

fn group(report: &Value) -> Grouped<'_> {
    let mut map: Grouped = BTreeMap::new();
    for job in report["jobs"].as_array().into_iter().flatten() {
        let key = job.get("key").cloned().unwrap_or(Value::Null).to_string();
        map.entry(key).or_default().push(job);
    }
    map
}

fn statuses(job: &Value) -> Option<BTreeMap<String, String>> {
    let runs = job.get("outcome")?.get("Standard")?.as_array()?;
    Some(
        runs.iter()
            .map(|r| {
                (
                    r["expected"].as_str().unwrap_or("?").to_string(),
                    r["status"].as_str().unwrap_or("?").to_string(),
                )
            })
            .collect(),
    )
}

/// Compare two parsed `--json` reports.
///
/// Both must have `schema_version` 1 and a `jobs` list. Jobs are matched
/// by `key`, in file order when a key occurs more than once.
pub fn diff_reports(old: &Value, new: &Value) -> Result<DiffResult, String> {
    check(old, "OLD")?;
    check(new, "NEW")?;
    let (a, b) = (group(old), group(new));
    let mut lines: Vec<String> = Vec::new();

    let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    for key in keys {
        let empty = Vec::new();
        let (ja, jb) = (a.get(key).unwrap_or(&empty), b.get(key).unwrap_or(&empty));
        for i in 0..ja.len().max(jb.len()) {
            match (ja.get(i), jb.get(i)) {
                (Some(x), None) => lines.push(format!("- job removed: {}", label(&x["key"]))),
                (None, Some(y)) => lines.push(format!("+ job added: {}", label(&y["key"]))),
                (Some(x), Some(y)) => job_changes(x, y, &mut lines),
                (None, None) => {}
            }
        }
    }

    let changed = !lines.is_empty();
    let mut text = String::new();
    for l in &lines {
        text.push_str(&neutralize_controls(l));
        text.push('\n');
    }
    if !changed {
        text.push_str("no differences\n");
    }
    Ok(DiffResult { text, changed })
}

fn job_changes(x: &Value, y: &Value, lines: &mut Vec<String>) {
    let name = label(&x["key"]);
    match (statuses(x), statuses(y)) {
        (Some(sa), Some(sb)) => {
            let times: BTreeSet<&String> = sa.keys().chain(sb.keys()).collect();
            for t in times {
                match (sa.get(t), sb.get(t)) {
                    (Some(p), Some(q)) if p != q => {
                        lines.push(format!("~ {name} {t}: {p} -> {q}"));
                    }
                    (Some(p), None) => lines.push(format!("- {name} {t}: {p} (run gone)")),
                    (None, Some(q)) => lines.push(format!("+ {name} {t}: {q} (new run)")),
                    _ => {}
                }
            }
        }
        _ => {
            if x["outcome"] != y["outcome"] {
                lines.push(format!("~ {name}: outcome changed"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn job(cmd: &str, runs: &[(&str, &str)]) -> Value {
        json!({
            "key": {"Cron": {"user": "root", "command": cmd}},
            "outcome": {"Standard": runs.iter()
                .map(|(e, s)| json!({"expected": e, "status": s}))
                .collect::<Vec<_>>()}
        })
    }

    fn report(jobs: Vec<Value>) -> Value {
        json!({"schema_version": 1, "jobs": jobs})
    }

    #[test]
    fn identical_reports_have_no_differences() {
        let r = report(vec![job("a", &[("2026-10-20T03:00:00Z", "Ok")])]);
        let d = diff_reports(&r, &r).unwrap();
        assert!(!d.changed);
        assert_eq!(d.text, "no differences\n");
    }

    #[test]
    fn status_change_is_listed() {
        let a = report(vec![job("a", &[("2026-10-20T03:00:00Z", "Ok")])]);
        let b = report(vec![job("a", &[("2026-10-20T03:00:00Z", "Missed")])]);
        let d = diff_reports(&a, &b).unwrap();
        assert!(d.changed);
        assert_eq!(d.text, "~ root: a 2026-10-20T03:00:00Z: Ok -> Missed\n");
    }

    #[test]
    fn added_and_removed_jobs() {
        let a = report(vec![job("old", &[])]);
        let b = report(vec![job("new", &[])]);
        let d = diff_reports(&a, &b).unwrap();
        assert_eq!(d.text, "+ job added: root: new\n- job removed: root: old\n");
    }

    #[test]
    fn runs_added_and_gone() {
        let a = report(vec![job("a", &[("2026-10-20T00:00:00Z", "Ok")])]);
        let b = report(vec![job("a", &[("2026-10-21T00:00:00Z", "Ok")])]);
        let d = diff_reports(&a, &b).unwrap();
        assert!(d
            .text
            .contains("- root: a 2026-10-20T00:00:00Z: Ok (run gone)"));
        assert!(d
            .text
            .contains("+ root: a 2026-10-21T00:00:00Z: Ok (new run)"));
    }

    #[test]
    fn duplicate_keys_are_paired_in_order() {
        let a = report(vec![job("a", &[]), job("a", &[])]);
        let b = report(vec![job("a", &[])]);
        let d = diff_reports(&a, &b).unwrap();
        assert_eq!(d.text, "- job removed: root: a\n");
    }

    #[test]
    fn wrong_schema_or_shape_is_an_error() {
        let ok = report(vec![]);
        assert!(diff_reports(&json!({"schema_version": 2, "jobs": []}), &ok).is_err());
        assert!(diff_reports(&ok, &json!({"schema_version": 1})).is_err());
    }

    #[test]
    fn control_characters_in_a_command_are_neutralized() {
        let a = report(vec![]);
        let b = report(vec![job("x\n- job removed: fake", &[])]);
        let d = diff_reports(&a, &b).unwrap();
        assert_eq!(d.text.lines().count(), 1);
    }
}
