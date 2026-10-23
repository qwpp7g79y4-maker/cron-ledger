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
    eprintln!("cron-ledger: cannot write report: {err}");
    std::process::exit(2);
}

fn main() {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = match cli::parse(&raw_args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cron-ledger: {e}\n\n{}", cli::usage());
            std::process::exit(2);
        }
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
            eprintln!("cron-ledger: {e}");
            std::process::exit(2);
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
