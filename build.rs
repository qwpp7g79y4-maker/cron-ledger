//! Records the git commit for debug builds of `cron-ledger`.
//!
//! The value is whatever `git rev-parse --verify HEAD` prints, and only
//! when that is a hex object name. A missing git, a failed command, or any
//! other answer leaves the file empty. Release builds leave it empty too.

use std::path::Path;
use std::process::Command;

fn main() {
    watch_git_head();
    let sha = if release_profile() {
        String::new()
    } else {
        git_head_sha().unwrap_or_default()
    };
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let path = Path::new(&out_dir).join("git_sha.txt");
    std::fs::write(path, sha).expect("write git_sha.txt");
}

fn release_profile() -> bool {
    std::env::var("PROFILE").ok().as_deref() == Some("release")
}

fn git_head_sha() -> Option<String> {
    let output = git(&["rev-parse", "--verify", "HEAD"])?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let sha = text.trim();
    if git_object_name(sha) {
        Some(sha.to_string())
    } else {
        None
    }
}

fn git_object_name(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn watch_git_head() {
    let Some(head) = git_path("HEAD") else {
        return;
    };
    println!("cargo:rerun-if-changed={head}");
    let Ok(contents) = std::fs::read_to_string(&head) else {
        return;
    };
    let Some(rest) = contents.strip_prefix("ref: ") else {
        return;
    };
    let rest = rest.trim();
    if rest.is_empty() {
        return;
    }
    if let Some(ref_path) = git_path(rest) {
        println!("cargo:rerun-if-changed={ref_path}");
    }
}

fn git_path(spec: &str) -> Option<String> {
    let output = git(&["rev-parse", "--git-path", spec])?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let path = path.trim();
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

fn git(args: &[&str]) -> Option<std::process::Output> {
    let mut command = Command::new("git");
    if let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") {
        command.current_dir(dir);
    }
    command.args(args).output().ok()
}
