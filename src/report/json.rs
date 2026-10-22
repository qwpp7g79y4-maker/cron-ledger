//! The `--json` report: a stable schema (`schema_version: 1`) built on
//! top of the `ledger` module's own `Serialize` types, plus the display
//! fields (`schema`, per-job summary counts) the text report also needs.

use serde::Serialize;

use crate::ledger::{JobKey, JobOutcome, Ledger, LogCoverage};

use super::{job_views, ReportContext, SCHEMA_VERSION};

#[derive(Serialize)]
struct JobSummary {
    expected: usize,
    found: usize,
    missed: usize,
    late: usize,
}

#[derive(Serialize)]
struct JobReport<'a> {
    key: &'a JobKey,
    file: &'a str,
    line: usize,
    schedule: String,
    summary: JobSummary,
    outcome: &'a JobOutcome,
}

#[derive(Serialize)]
struct Report<'a> {
    schema_version: u32,
    since: String,
    until: String,
    tz: &'a str,
    tolerance_secs: u64,
    log_coverage: &'a LogCoverage,
    warnings: Vec<&'a str>,
    jobs: Vec<JobReport<'a>>,
    exit_code: i32,
}

/// Render `ledger` as the stable `--json` report.
pub fn render_json(ledger: &Ledger, ctx: &ReportContext) -> String {
    let views = job_views(ledger, ctx.descriptions);
    let jobs = views
        .into_iter()
        .map(|v| JobReport {
            key: v.key,
            file: v.file,
            line: v.line,
            schedule: v.schedule,
            summary: JobSummary {
                expected: v.expected,
                found: v.found,
                missed: v.missed,
                late: v.late,
            },
            outcome: v.outcome,
        })
        .collect();

    let mut warnings: Vec<&str> = ledger.warnings.iter().map(|w| w.as_str()).collect();
    warnings.extend(ctx.extra_warnings.iter().map(|w| w.as_str()));

    let report = Report {
        schema_version: SCHEMA_VERSION,
        since: ctx.since.to_string(),
        until: ctx.until.to_string(),
        tz: ctx.tz.iana_name().unwrap_or("?"),
        tolerance_secs: ctx.tolerance.as_secs(),
        log_coverage: &ledger.log_coverage,
        warnings,
        jobs,
        exit_code: super::exit_code(ledger),
    };

    serde_json::to_string_pretty(&report).expect("report serialization cannot fail")
}
