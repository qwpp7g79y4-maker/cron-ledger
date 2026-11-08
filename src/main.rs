use std::io::{self, IsTerminal, Write};

use cron_ledger::{app, cli, diff, report};

/// Write the whole report. `print!` / `println!` panic when this fails
/// (a full disk, for example), which exits 101. A failed report is an
/// ordinary error instead.
fn write_stdout(text: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(text.as_bytes())?;
    out.flush()
}

/// Write the report to `--output`, or to stdout when that flag is absent.
/// A missing parent directory is not created.
fn write_report(path: Option<&std::path::Path>, text: &str) -> io::Result<()> {
    let Some(path) = path else {
        return write_stdout(text);
    };
    let mut file = std::fs::File::create(path).map_err(|err| annotate_path(path, err))?;
    file.write_all(text.as_bytes())
        .map_err(|err| annotate_path(path, err))?;
    file.flush().map_err(|err| annotate_path(path, err))
}

fn annotate_path(path: &std::path::Path, err: io::Error) -> io::Error {
    io::Error::new(
        err.kind(),
        report::neutralize_controls(&format!("{}: {err}", path.display())),
    )
}

fn report_write_error(err: io::Error) -> ! {
    let _ = writeln!(io::stderr(), "cron-ledger: cannot write report: {err}");
    std::process::exit(2);
}

/// Write an error to stderr as `cron-ledger: <message>`, followed by the
/// layout lines `extra` (already built by us, e.g. the usage text). The
/// message is untrusted (it can hold a path), so every control character,
/// newlines included, is made visible: it cannot start a line of its own.
fn fail_with(message: &str, extra: &str) -> ! {
    let mut err = io::stderr();
    let _ = writeln!(err, "cron-ledger: {}", report::neutralize_controls(message));
    let _ = write!(err, "{extra}");
    std::process::exit(2);
}

fn fail(message: &str) -> ! {
    fail_with(message, "")
}

/// The usage text after an error is extra context.
///
/// A terminal shows it. A pipe or a redirect does not. `--verbose` shows it
/// anyway. `--quiet` hides it even on a terminal. The report on stdout is
/// unchanged either way. Both flags together are a usage error (exit 2)
/// and never reach a report.
fn wants_extra(quiet: bool, verbose: bool, terminal: bool) -> bool {
    if quiet {
        false
    } else if verbose {
        true
    } else {
        terminal
    }
}

fn args_want_extra(args: &[String]) -> bool {
    let quiet = args.iter().any(|arg| arg == "--quiet");
    let verbose = args.iter().any(|arg| arg == "--verbose");
    wants_extra(quiet, verbose, io::stderr().is_terminal())
}

fn main() {
    let mut raw_args: Vec<String> = Vec::new();
    for (i, arg) in std::env::args_os().skip(1).enumerate() {
        match arg.into_string() {
            Ok(a) => raw_args.push(a),
            Err(bad) => fail(&format!(
                "argument {} is not valid UTF-8 ({}); cron-ledger needs UTF-8 paths and arguments",
                i + 1,
                bad.to_string_lossy()
            )),
        }
    }
    let parsed = match cli::parse(&raw_args) {
        Ok(c) => c,
        Err(e) => {
            let trailer = if args_want_extra(&raw_args) {
                format!("\n{}\n", cli::usage())
            } else {
                String::new()
            };
            fail_with(&e.to_string(), &trailer);
        }
    };

    if parsed.version && !parsed.help {
        if let Err(e) = write_stdout(concat!("cron-ledger ", env!("CARGO_PKG_VERSION"), "\n")) {
            report_write_error(e);
        }
        return;
    }

    if parsed.help {
        if let Err(e) = write_stdout(cli::usage()) {
            report_write_error(e);
        }
        return;
    }

    if let Some((old, new)) = &parsed.diff {
        match diff::diff_files(old, new) {
            Ok(result) => {
                if let Err(e) = write_stdout(&result.text) {
                    report_write_error(e);
                }
                if parsed.verbose {
                    let lines = result.text.lines().filter(|line| !line.is_empty()).count();
                    let _ = writeln!(io::stderr(), "context: lines {lines}");
                }
                std::process::exit(if result.changed { 1 } else { 0 });
            }
            Err(message) => fail(&message),
        }
    }

    let now = jiff::Timestamp::now();
    let audit = match app::build(&parsed, now) {
        Ok(a) => a,
        Err(e) => {
            let details = e.details.iter().fold(String::new(), |mut acc, d| {
                acc.push_str("  - ");
                acc.push_str(&report::neutralize_controls(d));
                acc.push('\n');
                acc
            });
            fail_with(&e.message, &details)
        }
    };

    let ctx = report::ReportContext {
        since: audit.since,
        until: audit.until,
        tz: &audit.tz,
        tolerance: audit.tolerance,
        descriptions: &audit.descriptions,
        extra_warnings: &audit.extra_warnings,
    };
    let code = report::exit_code(&audit.ledger);

    let rendered = if parsed.csv {
        report::render_csv(&audit.ledger, &ctx)
    } else if parsed.json {
        let mut text = report::render_json(&audit.ledger, &ctx);
        text.push('\n');
        text
    } else {
        // A file is not a terminal, so the saved report has no color.
        let color = if parsed.output.is_some() {
            false
        } else {
            let no_color_set = std::env::var_os("NO_COLOR").is_some();
            let is_tty = io::stdout().is_terminal();
            report::use_color(no_color_set, is_tty)
        };
        report::render_text(&audit.ledger, &ctx, color)
    };
    if let Err(e) = write_report(parsed.output.as_deref(), &rendered) {
        report_write_error(e);
    }
    if parsed.verbose {
        let warnings = audit.ledger.warnings.len() + audit.extra_warnings.len();
        let _ = writeln!(
            io::stderr(),
            "context: jobs {}, warnings {warnings}",
            audit.ledger.entries.len()
        );
    }

    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::wants_extra;

    #[test]
    fn extra_follows_quiet_verbose_and_the_terminal() {
        assert!(!wants_extra(true, false, true));
        assert!(wants_extra(false, true, false));
        assert!(!wants_extra(false, false, false));
        assert!(wants_extra(false, false, true));
        assert!(!wants_extra(true, true, true));
    }
}
