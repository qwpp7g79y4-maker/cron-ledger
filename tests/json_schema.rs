//! Schema test for `--json` (`schema_version` 1): the exact set of keys
//! and the JSON type of each value. Adding, removing or renaming a key,
//! or changing a type, fails here. The golden files pin the values; this
//! pins the shape, on a report that has every outcome status.

use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;

fn keys(v: &Value) -> Vec<&str> {
    let mut k: Vec<&str> = v
        .as_object()
        .expect("object")
        .keys()
        .map(|s| s.as_str())
        .collect();
    k.sort_unstable();
    k
}

fn report(scenario: &str, until: &str) -> Value {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/report")
        .join(scenario);
    let root = std::env::temp_dir().join(format!("cron-ledger-schema-root-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let out = Command::cargo_bin("cron-ledger")
        .unwrap()
        .arg("--system-root")
        .arg(&root)
        .arg("--crontab")
        .arg(dir.join("crontab"))
        .arg("--syslog")
        .arg(dir.join("syslog"))
        .args([
            "--since",
            "2026-10-20T00:00:00Z",
            "--until",
            until,
            "--json",
        ])
        .output()
        .unwrap();
    std::fs::remove_dir_all(&root).ok();
    assert!(
        matches!(out.status.code(), Some(0 | 1 | 3)),
        "{:?}",
        out.status
    );
    serde_json::from_slice(&out.stdout).expect("one json object")
}

#[test]
fn json_report_has_the_documented_keys_and_types() {
    let v = report("missed", "2026-11-01T00:00:00Z");
    assert_eq!(
        keys(&v),
        [
            "exit_code",
            "jobs",
            "log_coverage",
            "schema_version",
            "since",
            "tolerance_secs",
            "tz",
            "until",
            "warnings"
        ]
    );
    assert_eq!(v["schema_version"], 1);
    assert!(v["exit_code"].is_i64());
    assert!(v["since"].is_string() && v["until"].is_string() && v["tz"].is_string());
    assert!(v["tolerance_secs"].is_u64());
    assert!(v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .all(Value::is_string));
    assert_eq!(keys(&v["log_coverage"]), ["first", "last"]);

    let jobs = v["jobs"].as_array().expect("jobs");
    assert!(!jobs.is_empty());
    for job in jobs {
        assert_eq!(
            keys(job),
            ["file", "key", "line", "outcome", "schedule", "summary"]
        );
        assert!(job["file"].is_string() && job["schedule"].is_string());
        assert!(job["line"].is_u64());
        assert_eq!(
            keys(&job["summary"]),
            ["expected", "found", "late", "missed"]
        );
        for k in ["expected", "found", "late", "missed"] {
            assert!(job["summary"][k].is_u64(), "{k}");
        }
        // `key` and `outcome` are externally tagged enums: one variant key.
        assert_eq!(job["key"].as_object().unwrap().len(), 1);
        assert_eq!(job["outcome"].as_object().unwrap().len(), 1);
        if let Some(occ) = job["outcome"].get("Standard") {
            for o in occ.as_array().unwrap() {
                assert_eq!(
                    keys(o),
                    ["actual", "exit_status", "expected", "late_by", "status"]
                );
                assert!(o["expected"].is_string() && o["status"].is_string());
                assert!(o["actual"].is_null() || o["actual"].is_string());
                assert!(o["late_by"].is_null() || keys(&o["late_by"]) == ["nanos", "secs"]);
            }
        }
    }
}
