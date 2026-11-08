//! `--version` and `-V` print crate version 1.0.0 and exit 0.
//!
//! A debug build also prints the commit when `git rev-parse --verify HEAD`
//! provides a hex object name. A release build prints the version only.

use std::process::Command;

fn git_sha() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let sha = text.trim();
    if is_git_sha(sha) {
        Some(sha.to_string())
    } else {
        None
    }
}

fn is_git_sha(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64) && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn debug_bin() -> Command {
    let path = option_env!("CARGO_BIN_EXE_cron_ledger")
        .or(option_env!("CARGO_BIN_EXE_cron-ledger"))
        .expect("CARGO_BIN_EXE_cron-ledger");
    Command::new(path)
}

#[test]
fn debug_version_flags_print_crate_version_and_git_sha_when_git_gives_one() {
    let sha = git_sha();
    for flag in ["--version", "-V"] {
        let output = debug_bin().arg(flag).output().expect("run debug binary");
        assert_eq!(output.status.code(), Some(0), "{flag}");
        assert!(output.stderr.is_empty(), "{flag} stderr={output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        match &sha {
            Some(sha) => assert_eq!(stdout, format!("cron-ledger 1.0.0 {sha}\n")),
            None => assert_eq!(stdout, "cron-ledger 1.0.0\n"),
        }
    }
}

#[test]
fn release_version_flags_print_crate_version_without_a_sha() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let target = std::env::temp_dir().join("cron-ledger-w66-release");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let built = Command::new(&cargo)
        .args([
            "build",
            "--release",
            "--offline",
            "--quiet",
            "--bin",
            "cron-ledger",
            "--manifest-path",
        ])
        .arg(format!("{manifest}/Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .current_dir(manifest)
        .env("CARGO_TARGET_DIR", &target)
        .env_remove("CRON_LEDGER_GIT_SHA")
        .output()
        .expect("start cargo build --release");
    assert!(
        built.status.success(),
        "cargo build --release failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&built.stdout),
        String::from_utf8_lossy(&built.stderr)
    );
    let bin = target.join("release").join("cron-ledger");
    let sha = git_sha();
    for flag in ["--version", "-V"] {
        let output = Command::new(&bin)
            .arg(flag)
            .output()
            .expect("run release binary");
        assert_eq!(output.status.code(), Some(0), "{flag}");
        assert!(output.stderr.is_empty(), "{flag} stderr={output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(stdout, "cron-ledger 1.0.0\n", "{flag}");
        if let Some(sha) = &sha {
            assert!(!stdout.contains(sha), "{flag}: {stdout}");
        }
    }
}
