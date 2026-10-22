//! Golden tests: each fixture in `tests/fixtures/crontab/` parses to the
//! exact JSON recorded in `tests/golden/parse_<name>.json`.

use cron_ledger::crontab::{parse_system_crontab, parse_user_crontab};

fn golden_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("parse_{name}.json"))
}

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/crontab")
        .join(format!("{name}.crontab"))
}

fn assert_matches_golden<T: serde::Serialize>(name: &str, actual: &T) {
    let actual_json = serde_json::to_string_pretty(actual).unwrap() + "\n";
    if std::env::var_os("CRON_LEDGER_WRITE_GOLDEN").is_some() {
        std::fs::write(golden_path(name), &actual_json).unwrap();
        return;
    }
    let golden = std::fs::read_to_string(golden_path(name))
        .unwrap_or_else(|e| panic!("missing golden file for '{name}': {e}"));
    assert_eq!(
        actual_json, golden,
        "parse output for fixture '{name}' no longer matches tests/golden/parse_{name}.json"
    );
}

#[test]
fn basic_user_crontab_matches_golden() {
    let content = std::fs::read_to_string(fixture_path("basic")).unwrap();
    let result = parse_user_crontab(&content, "basic.crontab", Some("alice"));
    assert_matches_golden("basic", &result);
}

#[test]
fn system_crontab_matches_golden() {
    let content = std::fs::read_to_string(fixture_path("system")).unwrap();
    let result = parse_system_crontab(&content, "system.crontab");
    assert_matches_golden("system", &result);
}

#[test]
fn specials_crontab_matches_golden() {
    let content = std::fs::read_to_string(fixture_path("specials")).unwrap();
    let result = parse_user_crontab(&content, "specials.crontab", Some("alice"));
    assert_matches_golden("specials", &result);
}

#[test]
fn percent_crontab_matches_golden() {
    let content = std::fs::read_to_string(fixture_path("percent")).unwrap();
    let result = parse_user_crontab(&content, "percent.crontab", Some("alice"));
    assert_matches_golden("percent", &result);
}

#[test]
fn invalid_crontab_matches_golden() {
    let content = std::fs::read_to_string(fixture_path("invalid")).unwrap();
    let result = parse_user_crontab(&content, "invalid.crontab", Some("alice"));
    assert_matches_golden("invalid", &result);
}
