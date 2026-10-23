//! Matching expected run times against log evidence, and the ledger of
//! outcomes that falls out of it.

mod build;
mod job_key;
mod matcher;
mod status;

pub(crate) use build::build_ledger_from_coverage;
pub use build::{
    build_ledger, build_ledger_with_coverage, JobLedgerEntry, JobOutcome, JobSchedule, Ledger,
    ScheduledJob,
};
pub use job_key::{normalize_command, JobKey};
pub use matcher::{match_runs, LogCoverage, RunOutcome};
pub use status::Status;
