mod signals;

use std::io::{self, IsTerminal, Write};

use cron_ledger::interrupt;
use cron_ledger::{app, cli, diff, report};

enum ReportWrite {
    Done,
    Failed(io::Error),
    Interrupted,
}

/// Path of a temporary file next to `path`, on the same filesystem, so the
/// finished report can replace `path` in one rename. SIGINT before that
/// rename deletes the temporary file and leaves `path` as it was.
fn temp_beside(path: &std::path::Path) -> std::path::PathBuf {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut name = std::ffi::OsString::from(".");
    name.push(
        path.file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("report")),
    );
    name.push(format!(".cron-ledger-{}.tmp", std::process::id()));
    parent.join(name)
}

/// Write the whole report. `print!` / `println!` panic when this fails
/// (a full disk, for example), which exits 101. A failed report is an
/// ordinary error instead. SIGINT during the write is held back so the
/// bytes are not cut in half; the process then exits 130.
fn write_stdout(text: &str) -> ReportWrite {
    if interrupt::interrupted() {
        return ReportWrite::Interrupted;
    }
    let result = signals::with_sigint_blocked(|| {
        if interrupt::interrupted() {
            return None;
        }
        let mut out = io::stdout().lock();
        Some(out.write_all(text.as_bytes()).and_then(|()| out.flush()))
    });
    match result {
        None => ReportWrite::Interrupted,
        Some(Ok(())) if interrupt::interrupted() => ReportWrite::Interrupted,
        Some(Ok(())) => ReportWrite::Done,
        Some(Err(_)) if interrupt::interrupted() => ReportWrite::Interrupted,
        Some(Err(err)) => ReportWrite::Failed(err),
    }
}

/// Write the report to `--output`, or to stdout when that flag is absent.
/// A missing parent directory is not created. SIGINT publishes nothing
/// partial: the destination is replaced only after the new bytes are complete.
fn write_report(path: Option<&std::path::Path>, text: &str) -> ReportWrite {
    if interrupt::interrupted() {
        return ReportWrite::Interrupted;
    }
    let Some(path) = path else {
        return write_stdout(text);
    };
    let tmp = temp_beside(path);
    let created = std::fs::File::create(&tmp).and_then(|mut file| {
        file.write_all(text.as_bytes())?;
        file.flush()
    });
    if let Err(err) = created {
        let _ = std::fs::remove_file(&tmp);
        if interrupt::interrupted() {
            return ReportWrite::Interrupted;
        }
        return ReportWrite::Failed(annotate_path(path, err));
    }
    let renamed = signals::with_sigint_blocked(|| {
        if interrupt::interrupted() {
            return None;
        }
        Some(std::fs::rename(&tmp, path))
    });
    match renamed {
        None => {
            let _ = std::fs::remove_file(&tmp);
            ReportWrite::Interrupted
        }
        Some(Ok(())) if interrupt::interrupted() => ReportWrite::Interrupted,
        Some(Ok(())) => ReportWrite::Done,
        Some(Err(_)) if interrupt::interrupted() => {
            let _ = std::fs::remove_file(&tmp);
            ReportWrite::Interrupted
        }
        Some(Err(err)) => {
            let _ = std::fs::remove_file(&tmp);
            ReportWrite::Failed(annotate_path(path, err))
        }
    }
}

/// Publish `text`, or stop with exit 130 when SIGINT was caught.
fn publish(path: Option<&std::path::Path>, text: &str) {
    match write_report(path, text) {
        ReportWrite::Done => {}
        ReportWrite::Interrupted => finish(interrupt::EXIT_INTERRUPTED),
        ReportWrite::Failed(err) => report_write_error(err),
    }
}

fn annotate_path(path: &std::path::Path, err: io::Error) -> io::Error {
    io::Error::new(
        err.kind(),
        report::neutralize_controls(&format!("{}: {err}", path.display())),
    )
}

fn report_write_error(err: io::Error) -> ! {
    if interrupt::interrupted() {
        finish(interrupt::EXIT_INTERRUPTED);
    }
    let _ = writeln!(io::stderr(), "cron-ledger: cannot write report: {err}");
    std::process::exit(2);
}

/// Exit `code`, unless SIGINT was caught: then stderr is one line and the
/// status is 130. Ordinary codes are not renumbered.
fn finish(code: i32) -> ! {
    if interrupt::interrupted() {
        let _ = writeln!(io::stderr(), "cron-ledger: interrupted by SIGINT");
        std::process::exit(interrupt::EXIT_INTERRUPTED);
    }
    std::process::exit(code);
}

/// Write an error to stderr as `cron-ledger: <message>`, followed by the
/// layout lines `extra` (already built by us, e.g. the usage text). The
/// message is untrusted (it can hold a path), so every control character,
/// newlines included, is made visible: it cannot start a line of its own.
fn fail_with(message: &str, extra: &str) -> ! {
    if interrupt::interrupted() {
        finish(interrupt::EXIT_INTERRUPTED);
    }
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

/// `cron-ledger <crate version>` for `--version` and `-V`.
///
/// A debug build appends the commit when `git rev-parse --verify HEAD`
/// printed a hex object name at build time. Any other answer is left off.
/// A release build does not carry a commit.
fn version_text() -> String {
    let version = env!("CARGO_PKG_VERSION");
    #[cfg(debug_assertions)]
    if let Some(sha) = recorded_git_sha() {
        if git_object_name(sha) {
            return format!("cron-ledger {version} {sha}\n");
        }
    }
    format!("cron-ledger {version}\n")
}

/// Commit recorded by `build.rs`, or nothing when git did not provide one.
#[cfg(debug_assertions)]
fn recorded_git_sha() -> Option<&'static str> {
    let sha = include_str!(concat!(env!("OUT_DIR"), "/git_sha.txt")).trim();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
}

/// A hex object name as `git rev-parse` prints it (SHA-1 or SHA-256).
#[cfg(debug_assertions)]
fn git_object_name(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn main() {
    signals::install();
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
        let text = version_text();
        publish(None, &text);
        finish(0);
    }

    if parsed.help {
        publish(None, cli::usage());
        finish(0);
    }

    if let Some((old, new)) = &parsed.diff {
        match diff::diff_files(old, new) {
            Ok(result) => {
                publish(None, &result.text);
                if parsed.verbose {
                    let lines = result.text.lines().filter(|line| !line.is_empty()).count();
                    let _ = writeln!(io::stderr(), "context: lines {lines}");
                }
                finish(if result.changed { 1 } else { 0 });
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
    publish(parsed.output.as_deref(), &rendered);
    if parsed.verbose {
        let warnings = audit.ledger.warnings.len() + audit.extra_warnings.len();
        let _ = writeln!(
            io::stderr(),
            "context: jobs {}, warnings {warnings}",
            audit.ledger.entries.len()
        );
    }

    finish(code);
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

    #[cfg(debug_assertions)]
    #[test]
    fn git_object_name_accepts_only_a_rev_parse_object_name() {
        use super::git_object_name;

        assert!(git_object_name("0123456789abcdef0123456789abcdef01234567"));
        assert!(git_object_name(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!git_object_name(""));
        assert!(!git_object_name("unknown"));
        assert!(!git_object_name("20d46b5"));
        assert!(!git_object_name("gggggggggggggggggggggggggggggggggggggggg"));
    }
}
