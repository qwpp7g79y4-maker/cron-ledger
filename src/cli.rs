//! Command-line argument parsing for the documented CLI.
//! Pure parsing only: no file is read and no clock is consulted here, so
//! this is fully unit-testable without touching the filesystem.

use std::path::PathBuf;
use std::time::Duration;

use crate::systemd::parse_duration;

/// One `PATH|-` input argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputPath {
    /// `-`: read this source from standard input.
    Stdin,
    /// A filesystem path.
    File(PathBuf),
}

impl InputPath {
    /// `<stdin>`, or the path as displayed.
    pub fn label(&self) -> String {
        match self {
            InputPath::Stdin => "<stdin>".to_string(),
            InputPath::File(p) => p.display().to_string(),
        }
    }
}

fn parse_input_path(raw: &str) -> InputPath {
    if raw == "-" {
        InputPath::Stdin
    } else {
        InputPath::File(PathBuf::from(raw))
    }
}

/// Parsed command line. Absent flags keep the [`Default`] values.
#[derive(Debug, Clone)]
pub struct Cli {
    /// `--crontab PATH|-`, in the order given.
    pub crontabs: Vec<InputPath>,
    /// `--crontab-user USER=PATH`.
    pub crontab_users: Vec<(String, InputPath)>,
    /// `--system-root`. The default is `/`.
    pub system_root: PathBuf,
    /// `--timer-unit PATH`.
    pub timer_units: Vec<InputPath>,
    /// `--timers-list PATH|-`.
    pub timers_list: Vec<InputPath>,
    /// `--syslog PATH|-`.
    pub syslogs: Vec<InputPath>,
    /// `--journal-json PATH|-`.
    pub journal_json: Vec<InputPath>,
    /// `--since`, still the raw RFC3339 timestamp or duration text.
    pub since: String,
    /// `--until`, when passed. Same shape as `since`.
    pub until: Option<String>,
    /// `--tz`, when passed. Absent means the system timezone.
    pub tz: Option<String>,
    /// `--tolerance`. The default is 5 minutes.
    pub tolerance: Duration,
    /// `--json`.
    pub json: bool,
    /// `--csv`.
    pub csv: bool,
    /// `--output PATH`: write the report to this file instead of stdout.
    pub output: Option<PathBuf>,
    /// `--diff OLD NEW`: compare two saved `--json` reports.
    pub diff: Option<(PathBuf, PathBuf)>,
    /// `--quiet`: hide the usage text after an error. No context line.
    pub quiet: bool,
    /// `--verbose`: print the usage text after an error, and on success one context line.
    pub verbose: bool,
    /// `-h` or `--help`.
    pub help: bool,
    /// `-V` or `--version`.
    pub version: bool,
}

impl Default for Cli {
    fn default() -> Self {
        Cli {
            crontabs: Vec::new(),
            crontab_users: Vec::new(),
            system_root: PathBuf::from("/"),
            timer_units: Vec::new(),
            timers_list: Vec::new(),
            syslogs: Vec::new(),
            journal_json: Vec::new(),
            since: String::new(),
            until: None,
            tz: None,
            tolerance: Duration::from_secs(300),
            json: false,
            csv: false,
            output: None,
            diff: None,
            quiet: false,
            verbose: false,
            help: false,
            version: false,
        }
    }
}

fn next_value<'a>(
    args: &mut std::iter::Peekable<std::slice::Iter<'a, String>>,
    flag: &str,
) -> Result<&'a str, String> {
    args.next()
        .map(|s| s.as_str())
        .ok_or_else(|| format!("{flag} requires a value; run cron-ledger --help for the syntax"))
}

fn crontab_user_error(v: &str) -> String {
    format!(
        "--crontab-user expects USER=PATH, got '{v}'; name the user, then the file, for example --crontab-user alice=/path/to/crontab"
    )
}

/// Parse CLI arguments (everything after the binary name).
pub fn parse(raw_args: &[String]) -> Result<Cli, String> {
    let mut cli = Cli::default();
    let mut since_set = false;
    let mut stdin_uses = 0usize;

    let mut count_stdin = |p: &InputPath| {
        if *p == InputPath::Stdin {
            stdin_uses += 1;
        }
    };

    let mut args = raw_args.iter().peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => cli.help = true,
            "-V" | "--version" => cli.version = true,
            "--json" => cli.json = true,
            "--csv" => cli.csv = true,
            "--output" => {
                cli.output = Some(PathBuf::from(next_value(&mut args, "--output")?));
            }
            "--quiet" => cli.quiet = true,
            "--verbose" => cli.verbose = true,
            "--diff" => {
                let old = next_value(&mut args, "--diff")?;
                let new = next_value(&mut args, "--diff")?;
                cli.diff = Some((PathBuf::from(old), PathBuf::from(new)));
            }
            "--crontab" => {
                let p = parse_input_path(next_value(&mut args, "--crontab")?);
                count_stdin(&p);
                cli.crontabs.push(p);
            }
            "--crontab-user" => {
                let v = next_value(&mut args, "--crontab-user")?;
                let (user, path) = v.split_once('=').ok_or_else(|| crontab_user_error(v))?;
                if user.is_empty() {
                    return Err(crontab_user_error(v));
                }
                let p = parse_input_path(path);
                count_stdin(&p);
                cli.crontab_users.push((user.to_string(), p));
            }
            "--system-root" => {
                cli.system_root = PathBuf::from(next_value(&mut args, "--system-root")?);
            }
            "--timer-unit" => {
                let p = parse_input_path(next_value(&mut args, "--timer-unit")?);
                count_stdin(&p);
                cli.timer_units.push(p);
            }
            "--timers-list" => {
                let p = parse_input_path(next_value(&mut args, "--timers-list")?);
                count_stdin(&p);
                cli.timers_list.push(p);
            }
            "--syslog" => {
                let p = parse_input_path(next_value(&mut args, "--syslog")?);
                count_stdin(&p);
                cli.syslogs.push(p);
            }
            "--journal-json" => {
                let p = parse_input_path(next_value(&mut args, "--journal-json")?);
                count_stdin(&p);
                cli.journal_json.push(p);
            }
            "--since" => {
                cli.since = next_value(&mut args, "--since")?.to_string();
                since_set = true;
            }
            "--until" => {
                cli.until = Some(next_value(&mut args, "--until")?.to_string());
            }
            "--tz" => {
                cli.tz = Some(next_value(&mut args, "--tz")?.to_string());
            }
            "--tolerance" => {
                let v = next_value(&mut args, "--tolerance")?;
                cli.tolerance = parse_duration(v).map_err(|e| {
                    format!("--tolerance: {e}; use a time span such as 90s or 5min")
                })?;
            }
            other => {
                return Err(format!(
                    "unknown argument '{other}'; run cron-ledger --help for the valid options"
                ))
            }
        }
    }

    if cli.help || cli.version {
        return Ok(cli);
    }
    if cli.quiet && cli.verbose {
        return Err("--quiet and --verbose cannot be combined; pick one".to_string());
    }
    if cli.diff.is_some() {
        // `--quiet` and `--verbose` do not change the comparison.
        let rest = raw_args
            .iter()
            .filter(|arg| arg.as_str() != "--quiet" && arg.as_str() != "--verbose")
            .count();
        if rest != 3 {
            return Err(
                "--diff takes exactly two report files and no other option: cron-ledger --diff OLD.json NEW.json"
                    .to_string(),
            );
        }
        return Ok(cli);
    }
    if cli.json && cli.csv {
        return Err("--json and --csv cannot be combined; pick one output format".to_string());
    }
    if !since_set {
        return Err(
            "--since is required: say how far back to audit, for example --since 30d".to_string(),
        );
    }
    if stdin_uses > 1 {
        return Err(format!(
            "at most one source may read from stdin ('-'), got {stdin_uses}; save the others to files and pass their paths"
        ));
    }

    Ok(cli)
}

/// Usage text printed for `--help` and after a usage error.
pub fn usage() -> &'static str {
    "cron-ledger [--crontab PATH|-]... [--crontab-user USER=PATH]... [--system-root DIR]\n\
     \x20           [--timer-unit PATH]... [--timers-list PATH|-]...\n\
     \x20           [--syslog PATH|-]... [--journal-json PATH|-]...\n\
     \x20           --since <RFC3339|30d> [--until <RFC3339|30d>] [--tz Europe/Amsterdam]\n\
     \x20           [--tolerance 5m] [--json | --csv] [--output PATH] [--quiet | --verbose] [-h | --help]\n\
     cron-ledger --diff OLD.json NEW.json\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn minimal_valid_args() {
        let cli = parse(&args(&["--since", "30d"])).unwrap();
        assert_eq!(cli.since, "30d");
        assert_eq!(cli.tolerance, Duration::from_secs(300));
        assert_eq!(cli.system_root, PathBuf::from("/"));
    }

    #[test]
    fn missing_since_is_an_error() {
        assert!(parse(&args(&["--json"])).is_err());
    }

    #[test]
    fn repeated_flags_accumulate() {
        let cli = parse(&args(&[
            "--since",
            "30d",
            "--crontab",
            "a",
            "--crontab",
            "b",
        ]))
        .unwrap();
        assert_eq!(
            cli.crontabs,
            vec![
                InputPath::File(PathBuf::from("a")),
                InputPath::File(PathBuf::from("b")),
            ]
        );
    }

    #[test]
    fn stray_positional_argument_is_an_error() {
        assert!(parse(&args(&["--since", "30d", "--crontab", "a", "-"])).is_err());
    }

    #[test]
    fn crontab_user_splits_on_equals() {
        let cli = parse(&args(&[
            "--since",
            "30d",
            "--crontab-user",
            "alice=/tmp/alice.cron",
        ]))
        .unwrap();
        assert_eq!(cli.crontab_users.len(), 1);
        assert_eq!(cli.crontab_users[0].0, "alice");
        assert_eq!(
            cli.crontab_users[0].1,
            InputPath::File(PathBuf::from("/tmp/alice.cron"))
        );
    }

    #[test]
    fn crontab_user_without_equals_is_an_error() {
        assert!(parse(&args(&["--since", "30d", "--crontab-user", "alice"])).is_err());
    }

    #[test]
    fn at_most_one_stdin_source() {
        let cli = parse(&args(&[
            "--since",
            "30d",
            "--crontab",
            "-",
            "--syslog",
            "-",
        ]));
        assert!(cli.is_err());
        assert!(cli.unwrap_err().contains("stdin"));
    }

    #[test]
    fn single_stdin_source_is_fine() {
        let cli = parse(&args(&["--since", "30d", "--crontab", "-"])).unwrap();
        assert_eq!(cli.crontabs, vec![InputPath::Stdin]);
    }

    #[test]
    fn unknown_flag_is_an_error() {
        assert!(parse(&args(&["--since", "30d", "--nonsense"])).is_err());
    }

    #[test]
    fn bad_tolerance_is_an_error() {
        assert!(parse(&args(&["--since", "30d", "--tolerance", "banana"])).is_err());
    }

    #[test]
    fn version_short_circuits_since_requirement() {
        for flag in ["--version", "-V"] {
            let cli = parse(&args(&[flag])).unwrap();
            assert!(cli.version);
        }
    }

    #[test]
    fn help_short_circuits_since_requirement() {
        let cli = parse(&args(&["--help"])).unwrap();
        assert!(cli.help);
    }

    #[test]
    fn csv_flag_is_parsed_and_excludes_json() {
        assert!(parse(&args(&["--since", "30d", "--csv"])).unwrap().csv);
        let err = parse(&args(&["--since", "30d", "--csv", "--json"])).unwrap_err();
        assert!(err.contains("--csv"));
    }

    #[test]
    fn output_path_is_parsed() {
        let cli = parse(&args(&["--since", "30d", "--output", "out/report.txt"])).unwrap();
        assert_eq!(cli.output, Some(PathBuf::from("out/report.txt")));
        let err = parse(&args(&["--since", "30d", "--output"])).unwrap_err();
        assert!(err.contains("--output"), "{err}");
    }

    #[test]
    fn diff_takes_two_paths_and_needs_no_since() {
        let cli = parse(&args(&["--diff", "a.json", "b.json"])).unwrap();
        assert_eq!(
            cli.diff,
            Some((PathBuf::from("a.json"), PathBuf::from("b.json")))
        );
    }

    #[test]
    fn diff_with_one_path_or_extra_options_is_an_error() {
        assert!(parse(&args(&["--diff", "a.json"])).is_err());
        assert!(parse(&args(&["--diff", "a.json", "b.json", "--json"])).is_err());
        assert!(parse(&args(&["--since", "30d", "--diff", "a.json", "b.json"])).is_err());
    }

    #[test]
    fn usage_shows_until_accepts_rfc3339_or_a_duration() {
        assert!(usage().contains("--until <RFC3339|30d>"));
    }

    #[test]
    fn quiet_and_verbose_parse_and_conflict() {
        let quiet = parse(&args(&["--quiet", "--since", "30d"])).unwrap();
        assert!(quiet.quiet && !quiet.verbose);
        let verbose = parse(&args(&["--since", "30d", "--verbose"])).unwrap();
        assert!(verbose.verbose && !verbose.quiet);
        let err = parse(&args(&["--quiet", "--verbose"])).unwrap_err();
        assert!(err.contains("cannot be combined"), "{err}");
    }

    #[test]
    fn diff_allows_quiet_or_verbose_and_still_rejects_other_options() {
        let cli = parse(&args(&["--quiet", "--diff", "a.json", "b.json"])).unwrap();
        assert!(cli.quiet);
        assert!(cli.diff.is_some());
        let cli = parse(&args(&["--diff", "a.json", "b.json", "--verbose"])).unwrap();
        assert!(cli.verbose);
        assert!(parse(&args(&["--diff", "a.json", "b.json", "--json"])).is_err());
    }
}
