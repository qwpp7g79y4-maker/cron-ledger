use std::io::IsTerminal;

use cron_ledger::{app, cli, report};

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
        print!("{}", cli::usage());
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

    if parsed.json {
        println!("{}", report::render_json(&audit.ledger, &ctx));
    } else {
        let no_color_set = std::env::var_os("NO_COLOR").is_some();
        let is_tty = std::io::stdout().is_terminal();
        let color = report::use_color(no_color_set, is_tty);
        print!("{}", report::render_text(&audit.ledger, &ctx, color));
    }

    std::process::exit(code);
}
