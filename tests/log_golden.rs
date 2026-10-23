//! Golden tests: each fixture in `tests/fixtures/logs/` parses to the exact
//! JSON recorded in `tests/golden/log_<name>.json`.

use cron_ledger::log::{parse_journal_json, parse_syslog_cron, read_log_source};
use jiff::tz::TimeZone;
use jiff::Timestamp;

fn golden_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("log_{name}.json"))
}

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/logs")
        .join(name)
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
        "parse output for fixture '{name}' no longer matches tests/golden/log_{name}.json"
    );
}

#[test]
fn debian_syslog_matches_golden() {
    let content = std::fs::read_to_string(fixture_path("debian-syslog.log")).unwrap();
    let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
    let result = parse_syslog_cron(&content, "debian-syslog.log", since, &TimeZone::UTC);
    assert_matches_golden("debian_syslog", &result);
}

#[test]
fn rhel_cron_matches_golden() {
    let content = std::fs::read_to_string(fixture_path("rhel-cron.log")).unwrap();
    let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
    let result = parse_syslog_cron(&content, "rhel-cron.log", since, &TimeZone::UTC);
    assert_matches_golden("rhel_cron", &result);
}

#[test]
fn year_boundary_matches_golden() {
    let content = std::fs::read_to_string(fixture_path("year-boundary.log")).unwrap();
    let since: Timestamp = "2026-12-31T00:00:00Z".parse().unwrap();
    let result = parse_syslog_cron(&content, "year-boundary.log", since, &TimeZone::UTC);
    assert_matches_golden("year_boundary", &result);
}

#[test]
fn garbage_lines_are_counted_not_fatal_and_match_golden() {
    let content = std::fs::read_to_string(fixture_path("garbage.log")).unwrap();
    let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
    let result = parse_syslog_cron(&content, "garbage.log", since, &TimeZone::UTC);
    // Two events. Two warnings: the malformed CRON body, and the CRON
    // line whose month does not parse. The unrelated non-cron line is
    // skipped with no warning.
    assert_eq!(result.events.len(), 2);
    assert_eq!(result.warnings.len(), 2);
    assert_matches_golden("garbage", &result);
}

#[test]
fn rfc5424_matches_golden() {
    let content = std::fs::read_to_string(fixture_path("rfc5424.log")).unwrap();
    let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
    let result = parse_syslog_cron(&content, "rfc5424.log", since, &TimeZone::UTC);
    assert_matches_golden("rfc5424", &result);
}

#[test]
fn journal_json_matches_golden() {
    let content = std::fs::read_to_string(fixture_path("journal.jsonl")).unwrap();
    let result = parse_journal_json(&content, "journal.jsonl");
    assert_matches_golden("journal", &result);
}

#[test]
fn explicit_gz_rotation_is_decompressed_and_parsed() {
    let content = read_log_source(&fixture_path("syslog.2.gz")).unwrap();
    let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
    let result = parse_syslog_cron(&content, "syslog.2.gz", since, &TimeZone::UTC);
    assert_eq!(result.warnings, vec![]);
    assert_eq!(result.events.len(), 1);
}

/// `tests/fixtures/logs/README.md` explains why this is only strictly
/// timed in release builds.
#[test]
fn one_million_lines_parses_fast_in_release() {
    let mut content = String::with_capacity(1_000_000 * 70);
    for pid in 1..=1_000_000u32 {
        content.push_str(&format!(
            "Oct 20 03:00:{:02} web1 CRON[{pid}]: (root) CMD (/usr/local/bin/job.sh)\n",
            pid % 60
        ));
    }
    let since: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();

    let start = std::time::Instant::now();
    let result = parse_syslog_cron(&content, "perf.log", since, &TimeZone::UTC);
    let elapsed = start.elapsed();

    assert_eq!(result.warnings, vec![]);
    assert_eq!(result.events.len(), 1_000_000);
    if !cfg!(debug_assertions) {
        assert!(
            elapsed.as_secs_f64() < 5.0,
            "parsing 1,000,000 lines took {elapsed:?}, want < 5s in release"
        );
    }
}
