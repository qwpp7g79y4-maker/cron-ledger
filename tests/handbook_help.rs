//! Handbook checks against the built `cron-ledger` binary.
//!
//! `--help` is the usage line. Every long option in that text (and in
//! `--help` of any subcommand that usage line itself names) must appear
//! in `man/cron-ledger.1`, and the manual page must not add one.
//! `examples/README.md` gives one command and the exit code it produces.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn bin() -> Command {
    // Binary name is `cron-ledger`. Cargo 1.99 exports
    // `CARGO_BIN_EXE_cron-ledger`. Older Cargo exported
    // `CARGO_BIN_EXE_cron_ledger` (hyphen replaced by underscore).
    let path = option_env!("CARGO_BIN_EXE_cron_ledger")
        .or(option_env!("CARGO_BIN_EXE_cron-ledger"))
        .expect("CARGO_BIN_EXE_cron_ledger");
    Command::new(path)
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-'
}

/// Long options: a `--` at a token boundary plus the following name.
fn long_options(text: &str) -> BTreeSet<String> {
    let bytes = text.as_bytes();
    let mut opts = BTreeSet::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'-' && bytes[i + 1] == b'-' {
            let boundary = i == 0 || !is_name_byte(bytes[i - 1]);
            if boundary {
                let start = i;
                i += 2;
                let name_at = i;
                while i < bytes.len() && is_name_byte(bytes[i]) {
                    i += 1;
                }
                if i > name_at {
                    opts.insert(text[start..i].to_string());
                }
                continue;
            }
        }
        i += 1;
    }
    opts
}

/// Subcommands the main help text itself names, under a Commands heading.
fn subcommands_named_by_help(help: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_commands = false;
    for line in help.lines() {
        let trimmed = line.trim();
        let heading = trimmed.eq_ignore_ascii_case("commands:")
            || trimmed.eq_ignore_ascii_case("subcommands:");
        if heading {
            in_commands = true;
            continue;
        }
        if !in_commands {
            continue;
        }
        if trimmed.is_empty() || trimmed.ends_with(':') {
            break;
        }
        if let Some(word) = trimmed.split_whitespace().next() {
            if !word.starts_with('-') {
                names.push(word.to_string());
            }
        }
    }
    names
}

fn read_repo(rel: &str) -> String {
    let path = manifest().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn help_long_options_match_the_man_page_and_unknown_option_is_not_a_panic() {
    let output = bin().arg("--help").output().expect("spawn --help");
    assert_eq!(output.status.code(), Some(0), "stderr: {:?}", output.stderr);
    let help = String::from_utf8(output.stdout).expect("--help stdout is utf-8");
    assert!(!help.is_empty(), "--help stdout is empty");

    let man = read_repo("man/cron-ledger.1");
    for section in [
        "NAME",
        "SYNOPSIS",
        "DESCRIPTION",
        "OPTIONS",
        "EXIT STATUS",
        "EXAMPLES",
    ] {
        assert!(
            man.contains(&format!(".SH {section}")),
            "man page is missing .SH {section}"
        );
    }
    assert!(
        man.contains(&help),
        "man page SYNOPSIS must contain the --help stdout verbatim"
    );

    let mut option_source = help.clone();
    for name in subcommands_named_by_help(&help) {
        let sub = bin()
            .arg(&name)
            .arg("--help")
            .output()
            .unwrap_or_else(|e| panic!("spawn {name} --help: {e}"));
        assert_eq!(
            sub.status.code(),
            Some(0),
            "{name} --help failed: {}",
            String::from_utf8_lossy(&sub.stderr)
        );
        let sub_help = String::from_utf8(sub.stdout).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            man.contains(&sub_help),
            "man page must contain {name} --help"
        );
        option_source.push_str(&sub_help);
    }

    let from_help = long_options(&option_source);
    let from_man = long_options(&man);
    assert!(!from_help.is_empty(), "help text has no long options");
    assert_eq!(
        from_man, from_help,
        "long options in man/cron-ledger.1 must be exactly those in --help"
    );

    let unknown = bin()
        .arg("--handbook-unknown")
        .output()
        .expect("spawn unknown option");
    assert_ne!(unknown.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&unknown.stderr);
    assert!(
        !stderr.contains("panicked"),
        "unknown option panicked: {stderr}"
    );
}

#[test]
fn documented_example_exits_with_the_code_in_examples_readme() {
    let readme = read_repo("examples/README.md");
    let (command, code) = example_command_and_exit(&readme);
    let mut parts = command.split_whitespace();
    let program = parts.next().expect("command has a program name");
    assert_eq!(program, "cron-ledger", "example command: {command}");
    let args: Vec<&str> = parts.collect();

    let output = bin()
        .args(&args)
        .current_dir(manifest())
        .output()
        .expect("spawn documented example");
    assert_eq!(
        output.status.code(),
        Some(code),
        "command: {command}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn example_command_and_exit(readme: &str) -> (String, i32) {
    let start = readme
        .find("```")
        .expect("examples/README.md has a code fence");
    let rest = &readme[start + 3..];
    let newline = rest.find('\n').expect("code fence has a body");
    let tag = rest[..newline].trim();
    assert!(
        tag.is_empty() || tag.chars().all(|c| c.is_ascii_alphanumeric()),
        "unexpected fence tag {tag:?}"
    );
    let body = &rest[newline + 1..];
    let end = body.find("```").expect("code fence is closed");
    let command = body[..end].trim();
    assert!(
        !command.contains('\n'),
        "the documented command is one line: {command}"
    );

    let marker = "Expected exit code:";
    let at = readme
        .find(marker)
        .expect("examples/README.md states an exit code");
    let after = readme[at + marker.len()..].trim_start();
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    assert!(
        !digits.is_empty(),
        "exit code digits missing after {marker}"
    );
    let code: i32 = digits.parse().expect("exit code");
    (command.to_string(), code)
}
