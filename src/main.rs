use std::io::{self, IsTerminal, Write};

use cron_ledger::{app, cli, report};

/// Write the whole report. `print!` / `println!` panic when this fails
/// (a full disk, for example), which exits 101. A failed report is an
/// ordinary error instead.
fn write_stdout(text: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(text.as_bytes())?;
    out.flush()
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
        Err(e) => fail_with(&e.to_string(), &format!("\n{}\n", cli::usage())),
    };

    if parsed.help {
        if let Err(e) = write_stdout(cli::usage()) {
            report_write_error(e);
        }
        return;
    }

    let now = jiff::Timestamp::now();
    let audit = match app::build(&parsed, now) {
        Ok(a) => a,
        Err(e) => {
            let details: String = e
                .details
                .iter()
                .map(|d| format!("  - {}\n", report::neutralize_controls(d)))
                .collect();
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

    let rendered = if parsed.json {
        let mut text = report::render_json(&audit.ledger, &ctx);
        text.push('\n');
        text
    } else {
        let no_color_set = std::env::var_os("NO_COLOR").is_some();
        let is_tty = io::stdout().is_terminal();
        let color = report::use_color(no_color_set, is_tty);
        report::render_text(&audit.ledger, &ctx, color)
    };
    if let Err(e) = write_stdout(&rendered) {
        report_write_error(e);
    }

    std::process::exit(code);
}
