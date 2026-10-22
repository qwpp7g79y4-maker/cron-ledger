//! Running the binary against a read-only copy of every fixture
//! must change nothing in it — not a byte, not an mtime. The copy is
//! `chmod a-w`'d (write bits cleared, nothing else touched) before the
//! binary ever sees it.
//!
//! Unix only: the check relies on POSIX permission bits.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

fn copy_dir_recursive(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to);
        } else {
            fs::copy(&from, &to).unwrap();
        }
    }
}

/// Clear the write bits (owner, group, other) on every file and
/// directory under (and including) `path`, like `chmod -R a-w`.
fn remove_write_recursive(path: &Path) {
    if fs::symlink_metadata(path).unwrap().is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            remove_write_recursive(&entry.unwrap().path());
        }
    }
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(perms.mode() & !0o222);
    fs::set_permissions(path, perms).unwrap();
}

/// Restore the owner write bit everywhere under `path`, so the temp
/// directory can be removed again during cleanup.
fn restore_write_recursive(path: &Path) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(perms.mode() | 0o200);
    fs::set_permissions(path, perms).unwrap();
    if fs::symlink_metadata(path).unwrap().is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            restore_write_recursive(&entry.unwrap().path());
        }
    }
}

fn file_hash(path: &Path) -> u64 {
    let bytes = fs::read(path).unwrap();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Every file's path (relative to `dir`), mtime, and content hash.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (SystemTime, u64)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let meta = fs::metadata(&path).unwrap();
                let rel = path.strip_prefix(dir).unwrap().to_path_buf();
                out.insert(rel, (meta.modified().unwrap(), file_hash(&path)));
            }
        }
    }
    out
}

#[test]
fn running_against_a_read_only_fixture_copy_changes_nothing() {
    let tmp =
        std::env::temp_dir().join(format!("cron-ledger-readonly-test-{}", std::process::id()));
    let copy = tmp.join("fixtures");
    let fixtures_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    copy_dir_recursive(&fixtures_src, &copy);
    remove_write_recursive(&copy);

    let before = snapshot(&copy);

    // A representative slice across every input kind this project reads,
    // plus the whole copy as --system-root to exercise the default scan
    // too (its etc/ and var/ subpaths simply don't exist here, which is
    // the normal, silent "nothing to scan" case).
    let output = assert_cmd::Command::cargo_bin("cron-ledger")
        .unwrap()
        .arg("--system-root")
        .arg(&copy)
        .arg("--crontab")
        .arg(copy.join("crontab/system.crontab"))
        .arg("--timer-unit")
        .arg(copy.join("systemd/daily.timer"))
        .arg("--syslog")
        .arg(copy.join("logs/debian-syslog.log"))
        .arg("--journal-json")
        .arg(copy.join("logs/journal.jsonl"))
        .arg("--since")
        .arg("2020-01-01T00:00:00Z")
        .arg("--until")
        .arg("2030-01-01T00:00:00Z")
        .output()
        .unwrap();
    // The report's own verdict on these mismatched-window fixtures is not
    // under test, so any of the three "the audit ran" exit codes is
    // accepted; what must be ruled out is exit 2 (usage error) or a crash,
    // either of which would mean the CLI never got near a fixture.
    assert!(
        matches!(output.status.code(), Some(0) | Some(1) | Some(3)),
        "expected the audit to complete (exit 0, 1 or 3), got {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.stdout.is_empty(),
        "a completed audit must print a report"
    );

    let after = snapshot(&copy);
    assert_eq!(
        before, after,
        "the read-only fixture copy must be byte- and mtime-identical after a run"
    );

    restore_write_recursive(&copy);
    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn a_usage_error_before_any_fixture_read_is_not_mistaken_for_a_completed_audit() {
    // Regression: a run that hits a hard usage error (here, a missing
    // explicit --crontab path) never gets near a fixture, so the
    // read-only copy stays byte-identical too. Snapshot equality alone
    // cannot tell that apart from a completed, read-only audit; the exit
    // code is what distinguishes them.
    let tmp = std::env::temp_dir().join(format!(
        "cron-ledger-readonly-usage-error-test-{}",
        std::process::id()
    ));
    let copy = tmp.join("fixtures");
    let fixtures_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    copy_dir_recursive(&fixtures_src, &copy);
    remove_write_recursive(&copy);

    let before = snapshot(&copy);
    let output = assert_cmd::Command::cargo_bin("cron-ledger")
        .unwrap()
        .arg("--crontab")
        .arg(copy.join("crontab/does-not-exist.crontab"))
        .output()
        .unwrap();
    let after = snapshot(&copy);

    assert_eq!(
        before, after,
        "a usage error must not touch the copy either"
    );
    assert_eq!(
        output.status.code(),
        Some(2),
        "a missing explicit --crontab is documented as a hard usage error"
    );

    restore_write_recursive(&copy);
    fs::remove_dir_all(&tmp).unwrap();
}
