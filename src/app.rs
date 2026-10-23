//! Orchestration: turning a parsed [`Cli`] into a built [`Ledger`] by
//! reading every named (or defaulted) input source and feeding the
//! crontab/systemd/log/ledger modules that already do the real parsing
//! and matching work.
//!
//! A source named explicitly on the command line (`--crontab`,
//! `--timer-unit`, `--syslog`, ...) that cannot be read is a hard usage
//! error (exit 2): the user asked for it by name. The `--system-root`
//! scan runs only when no `--crontab`, `--crontab-user`, or `--timer-unit`
//! was given. It is best-effort: a missing path is silently skipped, and
//! an unreadable one becomes a warning, because the default root is `/`
//! and much of it may not exist or may not be readable.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::tz::TimeZone;
use jiff::Timestamp;

use crate::cli::{Cli, InputPath};
use crate::crontab::{self, describe_schedule, CronJob, ScheduleKind};
use crate::ledger::{
    build_ledger_from_coverage, normalize_command, JobKey, JobSchedule, Ledger, LogCoverage,
    ScheduledJob,
};
use crate::log::{self, LogEvent};
use crate::schedule::{self, default_timezone, resolve_timezone};
use crate::systemd;

#[derive(Debug)]
pub struct Audit {
    pub ledger: Ledger,
    pub descriptions: HashMap<JobKey, String>,
    pub extra_warnings: Vec<String>,
    pub since: Timestamp,
    pub until: Timestamp,
    pub tz: TimeZone,
    pub tolerance: Duration,
}

/// Resolve `--since`/`--until`'s `<RFC3339|30d>` shape: an absolute
/// RFC3339 timestamp, or a duration (`parse_duration`'s `systemd.time(7)`
/// syntax) counted back from `now`.
fn resolve_timestamp(value: &str, now: Timestamp) -> Result<Timestamp, String> {
    if let Ok(ts) = value.parse::<Timestamp>() {
        return Ok(ts);
    }
    let ago = systemd::parse_duration(value)
        .map_err(|_| format!("'{value}' is not an RFC3339 timestamp or a duration like '30d'"))?;
    now.checked_sub(ago)
        .map_err(|e| format!("'{value}' is out of range: {e}"))
}

fn read_text(path: &InputPath) -> Result<String, String> {
    match path {
        InputPath::Stdin => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
                .map_err(|e| format!("cannot read <stdin>: {e}"))?;
            Ok(buf)
        }
        InputPath::File(p) => {
            std::fs::read_to_string(p).map_err(|e| format!("cannot read {}: {e}", p.display()))
        }
    }
}

fn read_log_text(path: &InputPath) -> Result<log::LogText, String> {
    match path {
        InputPath::Stdin => {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut std::io::stdin(), &mut buf)
                .map_err(|e| format!("cannot read <stdin>: {e}"))?;
            Ok(log::decode_log_bytes(buf))
        }
        InputPath::File(p) => {
            log::read_log_source(p).map_err(|e| format!("cannot read {}: {e}", p.display()))
        }
    }
}

fn note_invalid_utf8(warnings: &mut Vec<String>, label: &str, text: &log::LogText) {
    if let Some(at) = text.invalid_utf8_at {
        warnings.push(format!(
            "{label}: invalid UTF-8 at byte {at}; replaced so the rest of the log can be read"
        ));
    }
}

struct Collector {
    jobs: Vec<ScheduledJob>,
    descriptions: HashMap<JobKey, String>,
    warnings: Vec<String>,
    events: Vec<LogEvent>,
}

impl Collector {
    fn add_cron_job(
        &mut self,
        job: &CronJob,
        default_tz: &TimeZone,
        since: Timestamp,
        until: Timestamp,
        tolerance: Duration,
    ) {
        let user = job.user.clone().unwrap_or_default();
        let key = JobKey::Cron {
            user,
            command: normalize_command(&job.command),
        };
        let (job_schedule, description, next_after_window) = match &job.schedule {
            ScheduleKind::Reboot => (JobSchedule::Reboot, "@reboot".to_string(), None),
            ScheduleKind::Standard(s) => {
                let tz = match job.effective_tz() {
                    Some(name) => match resolve_timezone(name) {
                        Ok(tz) => tz,
                        Err(e) => {
                            self.warnings
                                .push(format!("{}:{}: {e}", job.file, job.line));
                            default_tz.clone()
                        }
                    },
                    None => default_tz.clone(),
                };
                let expected = schedule::expand(s, &tz, since, until);
                // The first firing at or after `until` is outside this
                // window. The late search stops there, so that later
                // slot's run is not reported as LATE for the last
                // in-window time.
                let next_after_window = schedule::first_on_or_after(s, &tz, until);
                (
                    JobSchedule::Standard(expected),
                    describe_schedule(s),
                    next_after_window,
                )
            }
        };
        self.merge_description(&key, description);
        self.jobs.push(ScheduledJob {
            key,
            file: job.file.clone(),
            line: job.line,
            schedule: job_schedule,
            tolerance,
            persistent: false,
            next_after_window,
        });
    }

    fn add_parse_result(
        &mut self,
        result: crontab::ParseResult,
        default_tz: &TimeZone,
        since: Timestamp,
        until: Timestamp,
        tolerance: Duration,
    ) {
        for w in &result.warnings {
            self.warnings
                .push(format!("{}:{}: {}", w.file, w.line, w.message));
        }
        for job in &result.jobs {
            self.add_cron_job(job, default_tz, since, until, tolerance);
        }
    }

    /// Keep every description for `key`. Several crontab lines, or several
    /// timer files with the same `Unit=`, share one key, and the report
    /// looks that key up for each of them. A repeated description is stored
    /// once.
    fn merge_description(&mut self, key: &JobKey, description: String) {
        self.descriptions
            .entry(key.clone())
            .and_modify(|existing| {
                if *existing != description {
                    existing.push_str(", ");
                    existing.push_str(&description);
                }
            })
            .or_insert(description);
    }
}

fn derive_service_unit(path: &InputPath) -> String {
    match path {
        InputPath::File(p) => {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("timer");
            format!("{stem}.service")
        }
        InputPath::Stdin => "stdin.service".to_string(),
    }
}

/// How `/etc/cron.d` scanning treats one file name.
enum CronDFileName {
    /// Both Debian cron and cronie would run it.
    Accepted,
    /// Skipped by cronie as well as Debian cron: a dotfile, a name
    /// starting with `#`, a trailing `~`, or an rpm backup suffix
    /// (`.rpmsave`, `.rpmorig`, `.rpmnew`). cronie's `not_a_crontab()`
    /// rejects exactly these. Skip silently.
    Ignored,
    /// Outside Debian cron's `[A-Za-z0-9_-]` rule, but not one of the
    /// names cronie skips. Debian cron ignores it; cronie still runs it
    /// (including `*.dpkg-old` and other dotted names). Skip it here,
    /// and say so.
    AmbiguousAcrossImplementations,
}

/// Classify one `/etc/cron.d` file name. See [`CronDFileName`].
fn classify_cron_d_file_name(name: &str) -> CronDFileName {
    // Suffixes cronie skips. Debian's dpkg leftovers are not in this
    // list: cronie loads `*.dpkg-old` and any other dotted name.
    const CRONIE_SKIPPED_SUFFIXES: &[&str] = &["~", ".rpmsave", ".rpmorig", ".rpmnew"];
    if name.starts_with('.')
        || name.starts_with('#')
        || CRONIE_SKIPPED_SUFFIXES
            .iter()
            .any(|suf| name.ends_with(suf))
    {
        return CronDFileName::Ignored;
    }
    if name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        CronDFileName::Accepted
    } else {
        CronDFileName::AmbiguousAcrossImplementations
    }
}

/// Crontab files under `dir`, in sorted path order.
///
/// A symlink to a regular file is included: Debian cron runs those in
/// `cron.d` and in the spool. A symlink to a directory is not walked.
/// `is_dir` would follow it, and a pair of links back to `dir` then
/// walks without bound. An extracted tree passed as `--system-root` is
/// not trusted to be free of those. `recurse` is for `var/spool/cron/**`.
/// `/etc/cron.d` is one directory: cron does not run a file in a
/// subdirectory.
fn collect_files(dir: &Path, recurse: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();
    for child in children {
        let Ok(meta) = std::fs::symlink_metadata(&child) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            // `metadata` follows the link and errors on a loop, so a
            // cycle contributes nothing. A directory target is not a
            // crontab and is not entered.
            if std::fs::metadata(&child).is_ok_and(|target| target.is_file()) {
                out.push(child);
            }
        } else if recurse && meta.is_dir() {
            collect_files(&child, true, out);
        } else if meta.is_file() {
            out.push(child);
        }
    }
}

/// Build the ledger for `cli`, using `now` as "the current time" for a
/// relative `--since`/`--until` and a defaulted `--until`.
pub fn build(cli: &Cli, now: Timestamp) -> Result<Audit, String> {
    let tz = match &cli.tz {
        Some(name) => resolve_timezone(name)?,
        None => default_timezone(),
    };
    let since = resolve_timestamp(&cli.since, now)?;
    let until = match &cli.until {
        Some(v) => resolve_timestamp(v, now)?,
        None => now,
    };
    // An equal bound is an empty window and still a normal audit. An
    // inverted one expands to nothing and would otherwise exit 0.
    if until < since {
        return Err(format!(
            "--until ({until}) is earlier than --since ({since})"
        ));
    }
    let tolerance = cli.tolerance;

    let mut c = Collector {
        jobs: Vec::new(),
        descriptions: HashMap::new(),
        warnings: Vec::new(),
        events: Vec::new(),
    };

    // One closed span per input file. Syslog contributes its recognised
    // lines. The journal contributes every entry that had a real
    // timestamp, recognised or not. Spans are not joined: a hole between
    // two files (two non-adjacent rotations, for example) is not coverage.
    let mut coverage = LogCoverage::default();

    for path in &cli.crontabs {
        let content = read_text(path)?;
        let label = path.label();
        let parsed = crontab::parse_system_crontab(&content, &label);
        c.add_parse_result(parsed, &tz, since, until, tolerance);
    }

    for (user, path) in &cli.crontab_users {
        let content = read_text(path)?;
        let label = path.label();
        let parsed = crontab::parse_user_crontab(&content, &label, Some(user));
        c.add_parse_result(parsed, &tz, since, until, tolerance);
    }

    // --system-root is the fallback source. A named --crontab,
    // --crontab-user, or --timer-unit is the whole set of jobs: also
    // walking the root would schedule a file the user already named a
    // second time. Naming --system-root by itself still scans that
    // directory. A missing path stays silent; an unreadable file is a
    // warning, never a hard error.
    let named_job_source =
        !cli.crontabs.is_empty() || !cli.crontab_users.is_empty() || !cli.timer_units.is_empty();
    if !named_job_source {
        let root = &cli.system_root;
        let etc_crontab = root.join("etc/crontab");
        if etc_crontab.is_file() {
            match std::fs::read_to_string(&etc_crontab) {
                Ok(content) => {
                    let label = etc_crontab.display().to_string();
                    let parsed = crontab::parse_system_crontab(&content, &label);
                    c.add_parse_result(parsed, &tz, since, until, tolerance);
                }
                Err(e) => c.warnings.push(format!("{}: {e}", etc_crontab.display())),
            }
        }

        let cron_d = root.join("etc/cron.d");
        if cron_d.is_dir() {
            let mut files = Vec::new();
            collect_files(&cron_d, false, &mut files);
            for file in files {
                let name = match file.file_name().and_then(|n| n.to_str()) {
                    Some(name) => name,
                    None => continue,
                };
                match classify_cron_d_file_name(name) {
                    CronDFileName::Ignored => continue,
                    CronDFileName::AmbiguousAcrossImplementations => {
                        c.warnings.push(format!(
                            "{}: Debian cron ignores this name; cronie may still run it, so it is \
                         skipped here but could be an unaudited job",
                            file.display()
                        ));
                        continue;
                    }
                    CronDFileName::Accepted => {}
                }
                match std::fs::read_to_string(&file) {
                    Ok(content) => {
                        let label = file.display().to_string();
                        let parsed = crontab::parse_system_crontab(&content, &label);
                        c.add_parse_result(parsed, &tz, since, until, tolerance);
                    }
                    Err(e) => c.warnings.push(format!("{}: {e}", file.display())),
                }
            }
        }

        let spool = root.join("var/spool/cron");
        if spool.is_dir() {
            let mut files = Vec::new();
            collect_files(&spool, true, &mut files);
            for file in files {
                let user = file
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                match std::fs::read_to_string(&file) {
                    Ok(content) => {
                        let label = file.display().to_string();
                        let parsed = crontab::parse_user_crontab(&content, &label, Some(&user));
                        c.add_parse_result(parsed, &tz, since, until, tolerance);
                    }
                    Err(e) => c.warnings.push(format!("{}: {e}", file.display())),
                }
            }
        }
    }

    for path in &cli.timer_units {
        let content = read_text(path)?;
        let label = path.label();
        let timer = systemd::parse_timer_unit(&content, &label);
        c.warnings.extend(timer.warnings.clone());

        let unit_name = timer
            .unit
            .clone()
            .unwrap_or_else(|| derive_service_unit(path));
        let key = JobKey::Timer { unit: unit_name };
        let timer_tolerance = systemd::effective_tolerance(tolerance, &timer);

        if timer.on_calendar.is_empty() && !timer.unsupported_triggers.is_empty() {
            c.merge_description(
                &key,
                format!("unsupported: {}", timer.unsupported_triggers.join(", ")),
            );
            c.jobs.push(ScheduledJob {
                key,
                file: label,
                line: 0,
                schedule: JobSchedule::UnsupportedTrigger,
                tolerance: timer_tolerance,
                persistent: timer.persistent,
                next_after_window: None,
            });
        } else {
            let mut all_times: Vec<Timestamp> = Vec::new();
            // The first elapse at or after `until` is the next slot, same
            // bound a crontab uses. Without it a timer whose window holds
            // one elapse treats every later start, including the next
            // day's, as LATE. Several `OnCalendar=` lines share one unit,
            // so the bound is the earliest of their next elapses.
            // `Persistent=true` stays unbounded: the catch-up runs at the
            // next boot, which can be after that next elapse, and the
            // observed start has to stay attached. The search is not
            // capped at a few tolerances.
            let mut next_after_window: Option<Timestamp> = None;
            let next_elapse = |cal: &systemd::SystemdCalendar| -> Option<Timestamp> {
                if timer.persistent {
                    None
                } else {
                    systemd::first_calendar_on_or_after(cal, &tz, until)
                        .ok()
                        .flatten()
                }
            };
            for cal_str in &timer.on_calendar {
                match systemd::parse_calendar(cal_str) {
                    Ok(cal) => match systemd::expand_calendar(&cal, &tz, since, until) {
                        Ok(times) => {
                            all_times.extend(times);
                            if let Some(t) = next_elapse(&cal) {
                                next_after_window =
                                    Some(next_after_window.map_or(t, |prev| prev.min(t)));
                            }
                        }
                        Err(e) => c
                            .warnings
                            .push(format!("{label}: OnCalendar '{cal_str}': {e}")),
                    },
                    Err(e) => c
                        .warnings
                        .push(format!("{label}: OnCalendar '{cal_str}': {e}")),
                }
            }
            all_times.sort();
            all_times.dedup();
            c.merge_description(&key, timer.on_calendar.join("; "));
            c.jobs.push(ScheduledJob {
                key,
                file: label,
                line: 0,
                schedule: JobSchedule::Standard(all_times),
                tolerance: timer_tolerance,
                persistent: timer.persistent,
                next_after_window,
            });
        }
    }

    for path in &cli.timers_list {
        c.warnings.push(format!(
            "{}: --timers-list input is not implemented yet; ignored",
            path.label()
        ));
    }

    for path in &cli.syslogs {
        let label = path.label();
        let read = read_log_text(path)?;
        note_invalid_utf8(&mut c.warnings, &label, &read);
        let parsed = log::parse_syslog_cron(&read.text, &label, since, &tz);
        for w in parsed.warnings {
            c.warnings
                .push(format!("{}:{}: {}", w.file, w.line, w.message));
        }
        coverage = coverage.merge(LogCoverage::from_timestamps(
            parsed.events.iter().map(|e| e.timestamp),
        ));
        c.events.extend(parsed.events);
    }

    for path in &cli.journal_json {
        let label = path.label();
        let read = read_log_text(path)?;
        note_invalid_utf8(&mut c.warnings, &label, &read);
        let parsed = log::parse_journal_json(&read.text, &label);
        for w in parsed.warnings {
            c.warnings
                .push(format!("{}:{}: {}", w.file, w.line, w.message));
        }
        coverage = coverage.merge(LogCoverage::from_timestamps(parsed.coverage_timestamps));
        c.events.extend(parsed.events);
    }

    let ledger = build_ledger_from_coverage(&c.jobs, &c.events, coverage);

    Ok(Audit {
        ledger,
        descriptions: c.descriptions,
        extra_warnings: c.warnings,
        since,
        until,
        tz,
        tolerance,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn resolve_timestamp_accepts_rfc3339() {
        let now = ts("2026-10-22T00:00:00Z");
        assert_eq!(
            resolve_timestamp("2026-10-20T00:00:00Z", now).unwrap(),
            ts("2026-10-20T00:00:00Z")
        );
    }

    #[test]
    fn resolve_timestamp_accepts_a_duration_ago() {
        let now = ts("2026-10-22T00:00:00Z");
        assert_eq!(
            resolve_timestamp("2d", now).unwrap(),
            ts("2026-10-20T00:00:00Z")
        );
    }

    #[test]
    fn resolve_timestamp_rejects_garbage() {
        let now = ts("2026-10-22T00:00:00Z");
        assert!(resolve_timestamp("banana", now).is_err());
    }

    #[test]
    fn explicit_crontab_source_reports_a_usage_error_when_missing() {
        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-21T00:00:00Z".to_string()),
            crontabs: vec![InputPath::File(PathBuf::from("/nonexistent/crontab"))],
            ..Cli::default()
        };
        let err = build(&cli, ts("2026-10-22T00:00:00Z")).unwrap_err();
        assert!(err.contains("/nonexistent/crontab"));
    }

    #[test]
    fn default_system_root_is_a_soft_no_op_when_isolated() {
        let dir = std::env::temp_dir().join(format!("cron-ledger-app-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-21T00:00:00Z".to_string()),
            system_root: dir.clone(),
            ..Cli::default()
        };
        let audit = build(&cli, ts("2026-10-22T00:00:00Z")).unwrap();
        assert!(audit.ledger.entries.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn journal_entries_after_the_last_event_count_as_coverage() {
        let dir = std::env::temp_dir().join(format!(
            "cron-ledger-app-journal-span-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let crontab_path = dir.join("crontab");
        std::fs::write(&crontab_path, "0 6 * * * root /bin/job.sh\n").unwrap();
        let journal_path = dir.join("journal.jsonl");
        let us = |s: &str| -> i64 { ts(s).as_microsecond() };
        std::fs::write(
            &journal_path,
            format!(
                "{{\"__REALTIME_TIMESTAMP\":\"{}\",\"SYSLOG_IDENTIFIER\":\"CROND\",\"MESSAGE\":\"(root) CMD (other.sh)\"}}\n\
                 {{\"__REALTIME_TIMESTAMP\":\"{}\",\"SYSLOG_IDENTIFIER\":\"sshd\",\"MESSAGE\":\"Disconnected\"}}\n",
                us("2026-10-20T03:00:00Z"),
                us("2026-10-20T09:00:00Z"),
            ),
        )
        .unwrap();

        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-21T00:00:00Z".to_string()),
            tz: Some("UTC".to_string()),
            system_root: dir.clone(),
            crontabs: vec![InputPath::File(crontab_path)],
            journal_json: vec![InputPath::File(journal_path)],
            ..Cli::default()
        };
        let audit = build(&cli, ts("2026-10-22T00:00:00Z")).unwrap();
        let crate::ledger::JobOutcome::Standard(outcomes) = &audit.ledger.entries[0].outcome else {
            panic!("expected a standard outcome");
        };
        assert_eq!(outcomes[0].status, crate::ledger::Status::Missed);
        assert_eq!(
            audit.ledger.log_coverage.last,
            Some(ts("2026-10-20T09:00:00Z"))
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn end_to_end_crontab_and_syslog_via_cli_args() {
        let dir = std::env::temp_dir().join(format!("cron-ledger-app-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let crontab_path = dir.join("crontab");
        std::fs::write(&crontab_path, "0 3 * * * root /usr/local/bin/backup.sh\n").unwrap();
        let syslog_path = dir.join("syslog");
        std::fs::write(
            &syslog_path,
            "Oct 20 03:00:02 host CRON[1]: (root) CMD (/usr/local/bin/backup.sh)\n",
        )
        .unwrap();

        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-21T00:00:00Z".to_string()),
            system_root: dir.clone(), // isolate from the real filesystem
            crontabs: vec![InputPath::File(crontab_path)],
            syslogs: vec![InputPath::File(syslog_path)],
            ..Cli::default()
        };

        let audit = build(&cli, ts("2026-10-22T00:00:00Z")).unwrap();
        assert_eq!(audit.ledger.entries.len(), 1);
        assert_eq!(crate::report::exit_code(&audit.ledger), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn crontab_user_flag_reads_a_per_user_crontab_file() {
        let dir = std::env::temp_dir().join(format!(
            "cron-ledger-app-crontab-user-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let alice_path = dir.join("alice.cron");
        std::fs::write(&alice_path, "0 3 * * * /usr/local/bin/alice-job.sh\n").unwrap();

        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-21T00:00:00Z".to_string()),
            system_root: dir.clone(), // isolate from the real filesystem
            crontab_users: vec![("alice".to_string(), InputPath::File(alice_path))],
            ..Cli::default()
        };

        let audit = build(&cli, ts("2026-10-22T00:00:00Z")).unwrap();
        assert_eq!(audit.ledger.entries.len(), 1);
        assert_eq!(
            audit.ledger.entries[0].key,
            JobKey::Cron {
                user: "alice".to_string(),
                command: "/usr/local/bin/alice-job.sh".to_string(),
            }
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn system_root_default_scan_reads_etc_crontab_cron_d_and_spool() {
        let dir = std::env::temp_dir().join(format!("cron-ledger-app-scan-{}", std::process::id()));
        let etc_cron_d = dir.join("etc/cron.d");
        let spool = dir.join("var/spool/cron");
        std::fs::create_dir_all(&etc_cron_d).unwrap();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(
            dir.join("etc/crontab"),
            "0 3 * * * root /usr/local/bin/backup.sh\n",
        )
        .unwrap();
        std::fs::write(
            etc_cron_d.join("myjob"),
            "0 4 * * * root /usr/local/bin/other.sh\n",
        )
        .unwrap();
        std::fs::write(spool.join("alice"), "0 5 * * * /usr/local/bin/alice.sh\n").unwrap();

        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-21T00:00:00Z".to_string()),
            system_root: dir.clone(),
            ..Cli::default()
        };

        let audit = build(&cli, ts("2026-10-22T00:00:00Z")).unwrap();
        let mut keys: Vec<JobKey> = audit.ledger.entries.iter().map(|e| e.key.clone()).collect();
        keys.sort_by_key(|k| format!("{k:?}"));
        assert_eq!(
            keys,
            vec![
                JobKey::Cron {
                    user: "alice".to_string(),
                    command: "/usr/local/bin/alice.sh".to_string(),
                },
                JobKey::Cron {
                    user: "root".to_string(),
                    command: "/usr/local/bin/backup.sh".to_string(),
                },
                JobKey::Cron {
                    user: "root".to_string(),
                    command: "/usr/local/bin/other.sh".to_string(),
                },
            ]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn explicit_crontab_is_not_counted_again_by_the_system_root_scan() {
        // The file is both `<root>/etc/crontab` and the path passed as
        // `--crontab`. It is one job. A second copy would take the only
        // matching log line and leave the other copy MISSED.
        let dir = std::env::temp_dir().join(format!(
            "cron-ledger-app-no-double-scan-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join("etc")).unwrap();
        let crontab_path = dir.join("etc/crontab");
        std::fs::write(&crontab_path, "0 3 * * * root /bin/backup.sh\n").unwrap();
        let syslog_path = dir.join("cov.log");
        std::fs::write(
            &syslog_path,
            "Oct 20 02:00:00 host CRON[1]: (root) CMD (/bin/other.sh)\n\
             Oct 20 03:00:01 host CRON[2]: (root) CMD (/bin/backup.sh)\n\
             Oct 20 04:00:00 host CRON[3]: (root) CMD (/bin/other.sh)\n",
        )
        .unwrap();

        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-21T00:00:00Z".to_string()),
            tz: Some("UTC".to_string()),
            system_root: dir.clone(),
            crontabs: vec![InputPath::File(crontab_path)],
            syslogs: vec![InputPath::File(syslog_path)],
            ..Cli::default()
        };
        let audit = build(&cli, ts("2026-10-22T00:00:00Z")).unwrap();
        assert_eq!(audit.ledger.entries.len(), 1);
        let crate::ledger::JobOutcome::Standard(outcomes) = &audit.ledger.entries[0].outcome else {
            panic!("expected a standard outcome");
        };
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, crate::ledger::Status::Ok);
        assert_eq!(crate::report::exit_code(&audit.ledger), 0);
        let warnings = audit.ledger.warnings.join("\n");
        assert!(!warnings.contains("share this command"), "{warnings}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn etc_cron_d_scan_skips_files_cron_ignores() {
        let dir = std::env::temp_dir().join(format!(
            "cron-ledger-app-cron-d-filter-{}",
            std::process::id()
        ));
        let etc_cron_d = dir.join("etc/cron.d");
        std::fs::create_dir_all(&etc_cron_d).unwrap();
        std::fs::write(
            etc_cron_d.join("myjob"),
            "0 4 * * * root /usr/local/bin/kept.sh\n",
        )
        .unwrap();
        std::fs::write(
            etc_cron_d.join(".hidden"),
            "0 4 * * * root /usr/local/bin/dotfile.sh\n",
        )
        .unwrap();
        std::fs::write(
            etc_cron_d.join("myjob~"),
            "0 4 * * * root /usr/local/bin/backup-tilde.sh\n",
        )
        .unwrap();
        std::fs::write(
            etc_cron_d.join("myjob.dpkg-old"),
            "0 4 * * * root /usr/local/bin/dpkg-old.sh\n",
        )
        .unwrap();
        std::fs::write(
            etc_cron_d.join("myjob.rpmsave"),
            "0 4 * * * root /usr/local/bin/rpmsave.sh\n",
        )
        .unwrap();

        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-21T00:00:00Z".to_string()),
            system_root: dir.clone(),
            ..Cli::default()
        };

        let audit = build(&cli, ts("2026-10-22T00:00:00Z")).unwrap();
        let keys: Vec<JobKey> = audit.ledger.entries.iter().map(|e| e.key.clone()).collect();
        assert_eq!(
            keys,
            vec![JobKey::Cron {
                user: "root".to_string(),
                command: "/usr/local/bin/kept.sh".to_string(),
            }]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn etc_cron_d_scan_warns_on_a_name_only_cronie_would_run() {
        // Debian cron only runs names of letters, digits, `_` and `-`.
        // cronie's not_a_crontab() does not: it skips dotfiles, a leading
        // `#`, a trailing `~`, and the rpm backup suffixes, and it runs
        // everything else, including `my.job` and `*.dpkg-old`. Those
        // are still skipped here, but the skip has to be visible.
        let dir = std::env::temp_dir().join(format!(
            "cron-ledger-app-cron-d-ambiguous-{}",
            std::process::id()
        ));
        let etc_cron_d = dir.join("etc/cron.d");
        std::fs::create_dir_all(&etc_cron_d).unwrap();
        for name in [
            "my.job",
            "myjob.dpkg-old",
            "#skip",
            "myjob~",
            "myjob.rpmsave",
            ".hidden",
        ] {
            std::fs::write(
                etc_cron_d.join(name),
                "0 4 * * * root /usr/local/bin/skipped.sh\n",
            )
            .unwrap();
        }

        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-21T00:00:00Z".to_string()),
            system_root: dir.clone(),
            ..Cli::default()
        };

        let audit = build(&cli, ts("2026-10-22T00:00:00Z")).unwrap();
        assert!(audit.ledger.entries.is_empty());
        let warnings = audit.extra_warnings.join("\n");
        assert!(warnings.contains("my.job") && warnings.contains("cronie"));
        assert!(warnings.contains("myjob.dpkg-old"));
        assert!(!warnings.contains("#skip"));
        assert!(!warnings.contains("myjob~"));
        assert!(!warnings.contains("myjob.rpmsave"));
        assert!(!warnings.contains(".hidden"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn two_schedules_for_the_same_job_key_both_keep_their_description() {
        let dir =
            std::env::temp_dir().join(format!("cron-ledger-app-shared-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let crontab_path = dir.join("crontab");
        std::fs::write(
            &crontab_path,
            "0 3 * * * root backup.sh\n0 15 * * * root backup.sh\n",
        )
        .unwrap();

        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-21T00:00:00Z".to_string()),
            system_root: dir.clone(),
            crontabs: vec![InputPath::File(crontab_path)],
            ..Cli::default()
        };

        let audit = build(&cli, ts("2026-10-22T00:00:00Z")).unwrap();
        assert_eq!(audit.ledger.entries.len(), 2);
        let key = JobKey::Cron {
            user: "root".to_string(),
            command: "backup.sh".to_string(),
        };
        let description = audit.descriptions.get(&key).unwrap();
        assert!(
            description.contains("0 3"),
            "description {description:?} lost the first schedule"
        );
        assert!(
            description.contains("0 15"),
            "description {description:?} lost the second schedule"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn two_timers_sharing_a_unit_combine_their_descriptions() {
        // `Unit=` makes several timer files one JobKey::Timer. The report
        // looks that key up once per ledger entry, so a later insert used
        // to replace the description on every entry.
        let dir = std::env::temp_dir().join(format!(
            "cron-ledger-app-shared-timer-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let early = dir.join("early.timer");
        let late = dir.join("late.timer");
        std::fs::write(
            &early,
            "[Timer]\nOnCalendar=*-*-* 03:00:00\nUnit=backup.service\n",
        )
        .unwrap();
        std::fs::write(
            &late,
            "[Timer]\nOnCalendar=*-*-* 15:00:00\nUnit=backup.service\n",
        )
        .unwrap();

        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-21T00:00:00Z".to_string()),
            tz: Some("UTC".to_string()),
            system_root: dir.clone(),
            timer_units: vec![InputPath::File(early), InputPath::File(late.clone())],
            ..Cli::default()
        };
        let now = ts("2026-10-22T00:00:00Z");
        let key = JobKey::Timer {
            unit: "backup.service".to_string(),
        };

        let audit = build(&cli, now).unwrap();
        assert_eq!(audit.ledger.entries.len(), 2);
        assert!(audit.ledger.entries.iter().all(|e| e.key == key));
        assert_eq!(
            audit.descriptions.get(&key).map(String::as_str),
            Some("*-*-* 03:00:00, *-*-* 15:00:00")
        );
        let text = render(&audit);
        assert_eq!(
            text.matches("[*-*-* 03:00:00, *-*-* 15:00:00]").count(),
            2,
            "{text}"
        );

        // The same description from a second file is kept once.
        std::fs::write(
            &late,
            "[Timer]\nOnCalendar=*-*-* 03:00:00\nUnit=backup.service\n",
        )
        .unwrap();
        let audit = build(&cli, now).unwrap();
        assert_eq!(
            audit.descriptions.get(&key).map(String::as_str),
            Some("*-*-* 03:00:00")
        );

        // The monotonic-only branch joins the same map.
        std::fs::write(&late, "[Timer]\nOnBootSec=5min\nUnit=backup.service\n").unwrap();
        let audit = build(&cli, now).unwrap();
        assert_eq!(
            audit.descriptions.get(&key).map(String::as_str),
            Some("*-*-* 03:00:00, unsupported: OnBootSec")
        );
        let text = render(&audit);
        assert_eq!(
            text.matches("[*-*-* 03:00:00, unsupported: OnBootSec]")
                .count(),
            2,
            "{text}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn daily_job_does_not_claim_the_next_days_run_outside_the_window() {
        // One in-window firing (20 Oct 03:00). The only later run is the
        // next day's firing, so 20 Oct is missed. Coverage includes that
        // instant because the log also has the 19 Oct run.
        let dir =
            std::env::temp_dir().join(format!("cron-ledger-app-next-day-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let crontab_path = dir.join("crontab");
        std::fs::write(&crontab_path, "0 3 * * * root /bin/job.sh\n").unwrap();
        let syslog_path = dir.join("syslog");
        std::fs::write(
            &syslog_path,
            "Oct 19 03:00:01 h CRON[1]: (root) CMD (/bin/job.sh)\n\
             Oct 21 03:00:01 h CRON[1]: (root) CMD (/bin/job.sh)\n",
        )
        .unwrap();
        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-21T00:00:00Z".to_string()),
            tz: Some("UTC".to_string()),
            system_root: dir.clone(),
            crontabs: vec![InputPath::File(crontab_path)],
            syslogs: vec![InputPath::File(syslog_path)],
            ..Cli::default()
        };
        let audit = build(&cli, ts("2026-10-22T00:00:00Z")).unwrap();
        let crate::ledger::JobOutcome::Standard(outcomes) = &audit.ledger.entries[0].outcome else {
            panic!("expected a standard outcome");
        };
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, crate::ledger::Status::Missed);
        assert!(outcomes[0].actual.is_none());
        assert_eq!(crate::report::exit_code(&audit.ledger), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn same_day_late_run_stays_late_when_the_next_firing_is_tomorrow() {
        let dir = std::env::temp_dir().join(format!(
            "cron-ledger-app-same-day-late-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let crontab_path = dir.join("crontab");
        std::fs::write(&crontab_path, "0 3 * * * root /bin/job.sh\n").unwrap();
        let syslog_path = dir.join("syslog");
        std::fs::write(
            &syslog_path,
            "Oct 20 05:00:00 h CRON[1]: (root) CMD (/bin/job.sh)\n",
        )
        .unwrap();
        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-21T00:00:00Z".to_string()),
            tz: Some("UTC".to_string()),
            system_root: dir.clone(),
            crontabs: vec![InputPath::File(crontab_path)],
            syslogs: vec![InputPath::File(syslog_path)],
            ..Cli::default()
        };
        let audit = build(&cli, ts("2026-10-22T00:00:00Z")).unwrap();
        let crate::ledger::JobOutcome::Standard(outcomes) = &audit.ledger.entries[0].outcome else {
            panic!("expected a standard outcome");
        };
        assert_eq!(outcomes[0].status, crate::ledger::Status::Late);
        assert_eq!(outcomes[0].actual, Some(ts("2026-10-20T05:00:00Z")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn irregular_cadence_keeps_a_late_run_before_the_real_next_firing() {
        // 03:00 then 03:05. Copying the five-minute gap would stop the
        // late search at 03:10 and call the 03:12 run missed. The next
        // firing is the next day, so 03:12 is late for 03:05.
        let dir =
            std::env::temp_dir().join(format!("cron-ledger-app-irregular-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let crontab_path = dir.join("crontab");
        std::fs::write(&crontab_path, "0,5 3 * * * root /bin/job.sh\n").unwrap();
        let syslog_path = dir.join("syslog");
        std::fs::write(
            &syslog_path,
            "Oct 20 02:00:00 h CRON[1]: (root) CMD (/bin/other.sh)\n\
             Oct 20 03:12:00 h CRON[1]: (root) CMD (/bin/job.sh)\n",
        )
        .unwrap();
        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-20T04:00:00Z".to_string()),
            tz: Some("UTC".to_string()),
            system_root: dir.clone(),
            crontabs: vec![InputPath::File(crontab_path)],
            syslogs: vec![InputPath::File(syslog_path)],
            ..Cli::default()
        };
        let audit = build(&cli, ts("2026-10-22T00:00:00Z")).unwrap();
        let crate::ledger::JobOutcome::Standard(outcomes) = &audit.ledger.entries[0].outcome else {
            panic!("expected a standard outcome");
        };
        assert_eq!(outcomes[0].status, crate::ledger::Status::Missed);
        assert_eq!(outcomes[1].status, crate::ledger::Status::Late);
        assert_eq!(outcomes[1].actual, Some(ts("2026-10-20T03:12:00Z")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_utf8_in_a_syslog_is_a_warning_and_the_audit_continues() {
        let dir =
            std::env::temp_dir().join(format!("cron-ledger-app-bad-utf8-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let crontab_path = dir.join("crontab");
        std::fs::write(&crontab_path, "0 3 * * * root /bin/job.sh\n").unwrap();
        let syslog_path = dir.join("syslog");
        let mut bytes = b"Oct 20 02:00:00 h CRON[9]: (root) CMD (/bin/noise".to_vec();
        let bad_at = bytes.len();
        bytes.push(0xff);
        bytes.extend(b")\nOct 20 03:00:01 h CRON[1]: (root) CMD (/bin/job.sh)\n");
        std::fs::write(&syslog_path, &bytes).unwrap();
        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-21T00:00:00Z".to_string()),
            tz: Some("UTC".to_string()),
            system_root: dir.clone(),
            crontabs: vec![InputPath::File(crontab_path)],
            syslogs: vec![InputPath::File(syslog_path.clone())],
            ..Cli::default()
        };
        let audit = build(&cli, ts("2026-10-22T00:00:00Z")).unwrap();
        assert!(
            audit
                .extra_warnings
                .iter()
                .any(|w| w.contains(&format!("invalid UTF-8 at byte {bad_at}"))),
            "{:?}",
            audit.extra_warnings
        );
        let crate::ledger::JobOutcome::Standard(outcomes) = &audit.ledger.entries[0].outcome else {
            panic!("expected a standard outcome");
        };
        assert_eq!(outcomes[0].status, crate::ledger::Status::Ok);
        assert_eq!(outcomes[0].actual, Some(ts("2026-10-20T03:00:01Z")));
        std::fs::remove_dir_all(&dir).ok();
    }

    fn render(audit: &Audit) -> String {
        let ctx = crate::report::ReportContext {
            since: audit.since,
            until: audit.until,
            tz: &audit.tz,
            tolerance: audit.tolerance,
            descriptions: &audit.descriptions,
            extra_warnings: &audit.extra_warnings,
        };
        crate::report::render_text(&audit.ledger, &ctx, false)
    }

    fn timer_window(dir: &Path, unit_text: &str, journal: &str) -> Audit {
        let timer_path = dir.join("d.timer");
        std::fs::write(&timer_path, unit_text).unwrap();
        let journal_path = dir.join("journal.jsonl");
        std::fs::write(&journal_path, journal).unwrap();
        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-21T00:00:00Z".to_string()),
            tz: Some("UTC".to_string()),
            system_root: dir.to_path_buf(),
            timer_units: vec![InputPath::File(timer_path)],
            journal_json: vec![InputPath::File(journal_path)],
            ..Cli::default()
        };
        build(&cli, ts("2026-10-22T00:00:00Z")).unwrap()
    }

    fn started_line(when: &str) -> String {
        let us = ts(when).as_microsecond();
        format!(
            "{{\"__REALTIME_TIMESTAMP\":\"{us}\",\"SYSLOG_IDENTIFIER\":\"systemd\",\"UNIT\":\"d.service\",\"MESSAGE\":\"Started d.\"}}\n"
        )
    }

    #[test]
    fn daily_timer_does_not_claim_the_next_days_start() {
        // One elapse in the window (daily, midnight on the 20th). A start
        // at the next midnight is that schedule's next firing. With a log
        // line that covers the elapse, the elapse is MISSED. With only the
        // next day's line, the elapse sits outside the log span and is
        // NO_TRACE. Neither outcome is LATE.
        let dir = std::env::temp_dir().join(format!(
            "cron-ledger-app-timer-nextday-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cover_us = ts("2026-10-20T00:00:00Z").as_microsecond();
        let journal = format!(
            "{{\"__REALTIME_TIMESTAMP\":\"{cover_us}\",\"SYSLOG_IDENTIFIER\":\"sshd\",\"MESSAGE\":\"cover\"}}\n{}",
            started_line("2026-10-21T00:00:01Z")
        );
        let audit = timer_window(&dir, "[Timer]\nOnCalendar=daily\n", &journal);
        let crate::ledger::JobOutcome::Standard(outcomes) = &audit.ledger.entries[0].outcome else {
            panic!("expected a standard outcome");
        };
        assert_eq!(outcomes[0].status, crate::ledger::Status::Missed);
        assert!(outcomes[0].actual.is_none());

        let only_next = timer_window(
            &dir,
            "[Timer]\nOnCalendar=daily\n",
            &started_line("2026-10-21T00:00:01Z"),
        );
        let crate::ledger::JobOutcome::Standard(outcomes) = &only_next.ledger.entries[0].outcome
        else {
            panic!("expected a standard outcome");
        };
        assert_eq!(outcomes[0].status, crate::ledger::Status::NoTrace);
        assert!(outcomes[0].actual.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn daily_timer_keeps_a_same_day_late_start_and_a_persistent_catchup() {
        let dir =
            std::env::temp_dir().join(format!("cron-ledger-app-timer-late-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let late = timer_window(
            &dir,
            "[Timer]\nOnCalendar=daily\n",
            &started_line("2026-10-20T02:00:00Z"),
        );
        let crate::ledger::JobOutcome::Standard(outcomes) = &late.ledger.entries[0].outcome else {
            panic!("expected a standard outcome");
        };
        assert_eq!(outcomes[0].status, crate::ledger::Status::Late);
        assert_eq!(outcomes[0].actual, Some(ts("2026-10-20T02:00:00Z")));

        let boot_us = ts("2026-10-20T08:12:00Z").as_microsecond();
        let start_us = ts("2026-10-20T08:14:00Z").as_microsecond();
        let journal = format!(
            "{{\"__REALTIME_TIMESTAMP\":\"{boot_us}\",\"__MONOTONIC_TIMESTAMP\":\"28800000000\",\"_BOOT_ID\":\"b1\",\"_TRANSPORT\":\"kernel\",\"SYSLOG_IDENTIFIER\":\"kernel\",\"MESSAGE\":\"Linux version 6.1.0\"}}\n\
             {{\"__REALTIME_TIMESTAMP\":\"{start_us}\",\"SYSLOG_IDENTIFIER\":\"systemd\",\"UNIT\":\"d.service\",\"MESSAGE\":\"Started d.\"}}\n"
        );
        let caught = timer_window(
            &dir,
            "[Timer]\nOnCalendar=daily\nPersistent=true\n",
            &journal,
        );
        let crate::ledger::JobOutcome::Standard(outcomes) = &caught.ledger.entries[0].outcome
        else {
            panic!("expected a standard outcome");
        };
        assert_eq!(outcomes[0].status, crate::ledger::Status::LateCaughtUp);
        assert_eq!(outcomes[0].actual, Some(ts("2026-10-20T08:14:00Z")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn persistent_catchup_after_the_next_elapse_keeps_the_start_time() {
        // The window holds one daily elapse (the 20th). The next elapse
        // is the 21st at midnight. The machine comes back on the 22nd
        // and `Persistent=true` starts the unit then. That start is the
        // catch-up, so its own timestamp stays the actual time. A
        // non-persistent timer must not claim a start past that next
        // elapse; this one must.
        let dir = std::env::temp_dir().join(format!(
            "cron-ledger-app-timer-catchup-after-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cover_us = ts("2026-10-20T00:00:00Z").as_microsecond();
        let boot_us = ts("2026-10-22T08:12:00Z").as_microsecond();
        let start_us = ts("2026-10-22T08:14:00Z").as_microsecond();
        let journal = format!(
            "{{\"__REALTIME_TIMESTAMP\":\"{cover_us}\",\"SYSLOG_IDENTIFIER\":\"sshd\",\"MESSAGE\":\"cover\"}}\n\
             {{\"__REALTIME_TIMESTAMP\":\"{boot_us}\",\"__MONOTONIC_TIMESTAMP\":\"28800000000\",\"_BOOT_ID\":\"b1\",\"_TRANSPORT\":\"kernel\",\"SYSLOG_IDENTIFIER\":\"kernel\",\"MESSAGE\":\"Linux version 6.1.0\"}}\n\
             {{\"__REALTIME_TIMESTAMP\":\"{start_us}\",\"SYSLOG_IDENTIFIER\":\"systemd\",\"UNIT\":\"d.service\",\"MESSAGE\":\"Started d.\"}}\n"
        );
        let caught = timer_window(
            &dir,
            "[Timer]\nOnCalendar=daily\nPersistent=true\n",
            &journal,
        );
        let crate::ledger::JobOutcome::Standard(outcomes) = &caught.ledger.entries[0].outcome
        else {
            panic!("expected a standard outcome");
        };
        assert_eq!(outcomes[0].status, crate::ledger::Status::LateCaughtUp);
        assert_eq!(outcomes[0].actual, Some(ts("2026-10-22T08:14:00Z")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cron_d_ignores_a_subdirectory_and_spool_reads_one() {
        let dir = std::env::temp_dir().join(format!(
            "cron-ledger-app-cron-d-depth-{}",
            std::process::id()
        ));
        let cron_d = dir.join("etc/cron.d");
        let nested = cron_d.join("sub");
        let spool_user = dir.join("var/spool/cron/crontabs");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&spool_user).unwrap();
        std::fs::write(
            nested.join("x"),
            "0 4 * * * root /usr/local/bin/nested.sh\n",
        )
        .unwrap();
        std::fs::write(
            cron_d.join("kept"),
            "0 4 * * * root /usr/local/bin/kept.sh\n",
        )
        .unwrap();
        std::fs::write(
            spool_user.join("alice"),
            "0 5 * * * /usr/local/bin/alice.sh\n",
        )
        .unwrap();

        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-21T00:00:00Z".to_string()),
            tz: Some("UTC".to_string()),
            system_root: dir.clone(),
            ..Cli::default()
        };
        let audit = build(&cli, ts("2026-10-22T00:00:00Z")).unwrap();
        let mut commands: Vec<String> = audit
            .ledger
            .entries
            .iter()
            .map(|e| match &e.key {
                JobKey::Cron { command, .. } => command.clone(),
                JobKey::Timer { unit } => unit.clone(),
            })
            .collect();
        commands.sort();
        assert_eq!(
            commands,
            vec![
                "/usr/local/bin/alice.sh".to_string(),
                "/usr/local/bin/kept.sh".to_string(),
            ]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cycle_in_cron_d_and_spool_is_not_walked() {
        use std::os::unix::fs::symlink;

        let dir = std::env::temp_dir().join(format!(
            "cron-ledger-app-symlink-cycle-{}",
            std::process::id()
        ));
        let cron_d = dir.join("etc/cron.d");
        let spool = dir.join("var/spool/cron");
        std::fs::create_dir_all(&cron_d).unwrap();
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(
            cron_d.join("kept"),
            "0 4 * * * root /usr/local/bin/kept.sh\n",
        )
        .unwrap();
        // A symlink to a file is a crontab. The two links back to this
        // directory are not, and walking them would not finish.
        symlink("kept", cron_d.join("linked")).unwrap();
        symlink(".", cron_d.join("a")).unwrap();
        symlink(".", cron_d.join("b")).unwrap();
        symlink(".", spool.join("a")).unwrap();
        symlink(".", spool.join("b")).unwrap();

        let cli = Cli {
            since: "2026-10-20T00:00:00Z".to_string(),
            until: Some("2026-10-21T00:00:00Z".to_string()),
            tz: Some("UTC".to_string()),
            system_root: dir.clone(),
            ..Cli::default()
        };
        let audit = build(&cli, ts("2026-10-22T00:00:00Z")).unwrap();
        let commands: Vec<String> = audit
            .ledger
            .entries
            .iter()
            .map(|e| match &e.key {
                JobKey::Cron { command, .. } => command.clone(),
                JobKey::Timer { unit } => unit.clone(),
            })
            .collect();
        assert_eq!(
            commands,
            vec![
                "/usr/local/bin/kept.sh".to_string(),
                "/usr/local/bin/kept.sh".to_string(),
            ]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn non_adjacent_rotation_files_are_separate_coverage_spans() {
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!(
            "cron-ledger-app-rotation-gap-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let crontab_path = dir.join("crontab");
        std::fs::write(&crontab_path, "0 3 * * * root /bin/backup.sh\n").unwrap();

        // s.2.gz holds only 18 October. s.1 holds only 22 October.
        // Nothing covers the days between those two rotations.
        let older = dir.join("s.2.gz");
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder
            .write_all(b"Oct 18 03:00:01 host CRON[1]: (root) CMD (/bin/backup.sh)\n")
            .unwrap();
        std::fs::write(&older, encoder.finish().unwrap()).unwrap();
        let newer = dir.join("s.1");
        std::fs::write(
            &newer,
            "Oct 22 03:00:01 host CRON[1]: (root) CMD (/bin/backup.sh)\n",
        )
        .unwrap();

        let cli = Cli {
            since: "2026-10-17T00:00:00Z".to_string(),
            until: Some("2026-10-24T00:00:00Z".to_string()),
            tz: Some("UTC".to_string()),
            system_root: PathBuf::from("/nonexistent"),
            crontabs: vec![InputPath::File(crontab_path)],
            syslogs: vec![InputPath::File(older), InputPath::File(newer)],
            ..Cli::default()
        };
        let audit = build(&cli, ts("2026-10-25T00:00:00Z")).unwrap();
        let crate::ledger::JobOutcome::Standard(outcomes) = &audit.ledger.entries[0].outcome else {
            panic!("expected a standard outcome");
        };
        let statuses: Vec<_> = outcomes.iter().map(|o| o.status).collect();
        assert_eq!(
            statuses,
            vec![
                crate::ledger::Status::NoTrace, // 17th
                crate::ledger::Status::Ok,      // 18th, the line in s.2.gz
                crate::ledger::Status::NoTrace, // 19th, between the files
                crate::ledger::Status::NoTrace, // 20th
                crate::ledger::Status::NoTrace, // 21st
                crate::ledger::Status::Ok,      // 22nd, the line in s.1
                crate::ledger::Status::NoTrace, // 23rd
            ]
        );
        assert_eq!(crate::report::exit_code(&audit.ledger), 3);
        assert_eq!(audit.ledger.log_coverage.spans().len(), 2);
        assert_eq!(
            audit.ledger.log_coverage.first,
            Some(ts("2026-10-18T03:00:01Z"))
        );
        assert_eq!(
            audit.ledger.log_coverage.last,
            Some(ts("2026-10-22T03:00:01Z"))
        );

        let tz = TimeZone::UTC;
        let ctx = crate::report::ReportContext {
            since: ts("2026-10-17T00:00:00Z"),
            until: ts("2026-10-24T00:00:00Z"),
            tz: &tz,
            tolerance: audit.tolerance,
            descriptions: &audit.descriptions,
            extra_warnings: &audit.extra_warnings,
        };
        let text = crate::report::render_text(&audit.ledger, &ctx, false);
        assert!(
            text.contains("Logdekking: 2026-10-18T03:00:01+00:00[UTC] .. 2026-10-18T03:00:01+00:00[UTC]; 2026-10-22T03:00:01+00:00[UTC] .. 2026-10-22T03:00:01+00:00[UTC]"),
            "two files must not be printed as one span: {text}"
        );
        assert!(text.contains("0 MISSED"), "{text}");
        assert!(text.contains("5 NO_TRACE"), "{text}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
