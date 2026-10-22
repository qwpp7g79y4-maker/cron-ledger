//! Command-line argument parsing for the documented CLI.
//! Pure parsing only: no file is read and no clock is consulted here, so
//! this is fully unit-testable without touching the filesystem.

use std::path::PathBuf;
use std::time::Duration;

use crate::systemd::parse_duration;

/// One `PATH|-` input argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputPath {
    Stdin,
    File(PathBuf),
}

impl InputPath {
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

#[derive(Debug, Clone)]
pub struct Cli {
    pub crontabs: Vec<InputPath>,
    pub crontab_users: Vec<(String, InputPath)>,
    pub system_root: PathBuf,
    pub timer_units: Vec<InputPath>,
    pub timers_list: Vec<InputPath>,
    pub syslogs: Vec<InputPath>,
    pub journal_json: Vec<InputPath>,
    pub since: String,
    pub until: Option<String>,
    pub tz: Option<String>,
    pub tolerance: Duration,
    pub json: bool,
    pub help: bool,
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
            help: false,
        }
    }
}

fn next_value<'a>(
    args: &mut std::iter::Peekable<std::slice::Iter<'a, String>>,
    flag: &str,
) -> Result<&'a str, String> {
    args.next()
        .map(|s| s.as_str())
        .ok_or_else(|| format!("{flag} requires a value"))
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
            "--json" => cli.json = true,
            "--crontab" => {
                let p = parse_input_path(next_value(&mut args, "--crontab")?);
                count_stdin(&p);
                cli.crontabs.push(p);
            }
            "--crontab-user" => {
                let v = next_value(&mut args, "--crontab-user")?;
                let (user, path) = v
                    .split_once('=')
                    .ok_or_else(|| format!("--crontab-user expects USER=PATH, got '{v}'"))?;
                if user.is_empty() {
                    return Err(format!("--crontab-user expects USER=PATH, got '{v}'"));
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
                cli.tolerance = parse_duration(v).map_err(|e| format!("--tolerance: {e}"))?;
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }

    if cli.help {
        return Ok(cli);
    }
    if !since_set {
        return Err("--since is required".to_string());
    }
    if stdin_uses > 1 {
        return Err(format!(
            "at most one source may read from stdin ('-'), got {stdin_uses}"
        ));
    }

    Ok(cli)
}

pub fn usage() -> &'static str {
    "cron-ledger [--crontab PATH|-]... [--crontab-user USER=PATH]... [--system-root DIR]\n\
     \x20           [--timer-unit PATH]... [--timers-list PATH|-]...\n\
     \x20           [--syslog PATH|-]... [--journal-json PATH|-]...\n\
     \x20           --since <RFC3339|30d> [--until <RFC3339>] [--tz Europe/Amsterdam]\n\
     \x20           [--tolerance 5m] [--json]\n"
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
    fn help_short_circuits_since_requirement() {
        let cli = parse(&args(&["--help"])).unwrap();
        assert!(cli.help);
    }
}
