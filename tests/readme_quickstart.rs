//! Runs the commands of the section "Aan de slag in 5 minuten" in README.md
//! and README.nl.md literally. Each command line runs through `sh -c` from the
//! repository root, with the built binary first on PATH. All commands use the
//! fixtures in `examples/`.

use std::path::{Path, PathBuf};
use std::process::Command;

const HEADING: &str = "## Aan de slag in 5 minuten";
const FORBIDDEN: [&str; 5] = ["/var/log", "journalctl", "http://", "https://", "--tui"];

fn commands(readme: &str) -> Vec<String> {
    let mut in_section = false;
    let mut in_fence = false;
    let mut out = Vec::new();
    for line in readme.lines() {
        if line.starts_with("## ") {
            if in_section {
                break;
            }
            in_section = line.trim_end() == HEADING;
            continue;
        }
        if !in_section {
            continue;
        }
        if line.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence && !line.trim().is_empty() && !line.trim_start().starts_with('#') {
            out.push(line.to_string());
        }
    }
    out
}

#[test]
fn quickstart_commands_in_both_readmes_run_literally() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let bin_dir = Path::new(env!("CARGO_BIN_EXE_cron-ledger"))
        .parent()
        .expect("bin dir")
        .to_path_buf();
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut per_readme = Vec::new();
    for name in ["README.md", "README.nl.md"] {
        let text = std::fs::read_to_string(root.join(name)).expect(name);
        let cmds = commands(&text);
        assert!(!cmds.is_empty(), "{name}: no commands under {HEADING:?}");
        for cmd in &cmds {
            for bad in FORBIDDEN {
                assert!(!cmd.contains(bad), "{name}: {cmd:?} contains {bad:?}");
            }
            let output = Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .current_dir(&root)
                .env("PATH", &path)
                .output()
                .expect("start sh");
            assert_eq!(
                output.status.code(),
                Some(0),
                "{name}: {cmd:?} stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!output.stdout.is_empty(), "{name}: {cmd:?} printed nothing");
        }
        per_readme.push(cmds);
    }
    assert_eq!(
        per_readme[0], per_readme[1],
        "both READMEs list the same commands"
    );
}
