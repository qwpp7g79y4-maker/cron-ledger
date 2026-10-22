//! The human-readable text report: one block per job, then a summary
//! with the log coverage. Color is applied to status words only, and
//! only when the caller says to (see [`super::use_color`]).

use std::fmt::Write as _;

use jiff::Timestamp;

use crate::ledger::{JobKey, JobOutcome, Ledger, Status};

use super::{job_views, JobView, ReportContext};

fn job_label(key: &JobKey) -> String {
    match key {
        JobKey::Cron { user, command } => format!("{user}: {command}"),
        JobKey::Timer { unit } => unit.clone(),
    }
}

fn status_word(status: Status) -> &'static str {
    match status {
        Status::Ok => "OK",
        Status::Late => "LATE",
        Status::LateCaughtUp => "LATE (caught up)",
        Status::Missed => "MISSED",
        Status::NoTrace => "NO_TRACE",
        Status::RebootOnly => "REBOOT_ONLY",
        Status::UnsupportedTrigger => "UNSUPPORTED_TRIGGER",
    }
}

fn status_color_code(status: Status) -> &'static str {
    match status {
        Status::Ok => "32",                 // green
        Status::Late => "33",               // yellow
        Status::LateCaughtUp => "33",       // yellow
        Status::Missed => "31",             // red
        Status::NoTrace => "2",             // dim
        Status::RebootOnly => "36",         // cyan
        Status::UnsupportedTrigger => "35", // magenta
    }
}

fn colored_status(status: Status, color: bool) -> String {
    let word = status_word(status);
    if color {
        format!("\x1b[{}m{word}\x1b[0m", status_color_code(status))
    } else {
        word.to_string()
    }
}

fn fmt_ts(ts: Timestamp, tz: &jiff::tz::TimeZone) -> String {
    ts.to_zoned(tz.clone()).to_string()
}

fn render_job_block(out: &mut String, view: &JobView, ctx: &ReportContext, color: bool) {
    let _ = writeln!(
        out,
        "{} ({}:{}) [{}]",
        job_label(view.key),
        view.file,
        view.line,
        view.schedule
    );
    match view.outcome {
        JobOutcome::Standard(_) => {
            let _ = writeln!(
                out,
                "  verwacht: {}, gevonden: {}, missed: {}, late: {}",
                view.expected, view.found, view.missed, view.late
            );
            match view.last_run {
                Some((ts, status)) => {
                    let _ = writeln!(
                        out,
                        "  laatste run: {} ({})",
                        fmt_ts(ts, ctx.tz),
                        colored_status(status, color)
                    );
                }
                None => {
                    let _ = writeln!(out, "  laatste run: -");
                }
            }
            if view.first_missed.is_empty() {
                let _ = writeln!(out, "  gemiste tijden: -");
            } else {
                let times: Vec<String> = view
                    .first_missed
                    .iter()
                    .map(|t| fmt_ts(*t, ctx.tz))
                    .collect();
                let _ = writeln!(out, "  eerste gemiste tijden: {}", times.join(", "));
            }
        }
        JobOutcome::Reboot { boots } => {
            let _ = writeln!(
                out,
                "  status: {}",
                colored_status(Status::RebootOnly, color)
            );
            if boots.is_empty() {
                let _ = writeln!(out, "  boots gevonden: 0");
            } else {
                let times: Vec<String> = boots.iter().map(|t| fmt_ts(*t, ctx.tz)).collect();
                let _ = writeln!(
                    out,
                    "  boots gevonden: {} ({})",
                    boots.len(),
                    times.join(", ")
                );
            }
        }
        JobOutcome::UnsupportedTrigger => {
            let _ = writeln!(
                out,
                "  status: {}",
                colored_status(Status::UnsupportedTrigger, color)
            );
        }
    }
}

fn status_counts(ledger: &Ledger) -> [(Status, usize); 7] {
    let statuses = [
        Status::Ok,
        Status::Late,
        Status::LateCaughtUp,
        Status::Missed,
        Status::NoTrace,
        Status::RebootOnly,
        Status::UnsupportedTrigger,
    ];
    let mut counts = [0usize; 7];
    for entry in &ledger.entries {
        match &entry.outcome {
            JobOutcome::Standard(outcomes) => {
                for o in outcomes {
                    let idx = statuses.iter().position(|s| *s == o.status).unwrap();
                    counts[idx] += 1;
                }
            }
            JobOutcome::Reboot { .. } => counts[5] += 1,
            JobOutcome::UnsupportedTrigger => counts[6] += 1,
        }
    }
    std::array::from_fn(|i| (statuses[i], counts[i]))
}

/// Render the full text report for `ledger` within `ctx`.
pub fn render_text(ledger: &Ledger, ctx: &ReportContext, color: bool) -> String {
    let mut out = String::new();
    let views = job_views(ledger, ctx.descriptions);

    if views.is_empty() {
        out.push_str("(geen jobs gevonden)\n\n");
    } else {
        for view in &views {
            render_job_block(&mut out, view, ctx, color);
            out.push('\n');
        }
    }

    out.push_str("----\n");
    out.push_str("Samenvatting: ");
    let counts = status_counts(ledger);
    let parts: Vec<String> = counts
        .iter()
        .map(|(s, n)| format!("{n} {}", colored_status(*s, color)))
        .collect();
    out.push_str(&parts.join(", "));
    out.push('\n');

    match (ledger.log_coverage.first, ledger.log_coverage.last) {
        (Some(first), Some(last)) => {
            let _ = writeln!(
                out,
                "Logdekking: {} .. {}",
                fmt_ts(first, ctx.tz),
                fmt_ts(last, ctx.tz)
            );
        }
        _ => out.push_str("Logdekking: geen logs\n"),
    }

    let _ = writeln!(
        out,
        "Venster: {} .. {} (tz {})",
        fmt_ts(ctx.since, ctx.tz),
        fmt_ts(ctx.until, ctx.tz),
        ctx.tz.iana_name().unwrap_or("?"),
    );
    let _ = writeln!(out, "Tolerantie: {}s", ctx.tolerance.as_secs());

    let mut all_warnings: Vec<&str> = ledger.warnings.iter().map(|w| w.as_str()).collect();
    all_warnings.extend(ctx.extra_warnings.iter().map(|w| w.as_str()));
    if !all_warnings.is_empty() {
        out.push_str("Waarschuwingen:\n");
        for w in all_warnings {
            let _ = writeln!(out, "  - {w}");
        }
    }

    out
}
