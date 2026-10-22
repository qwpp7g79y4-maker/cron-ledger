//! The five statuses a scheduled run (or a `@reboot`/unsupported-trigger
//! job) can end up with.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Status {
    /// A matching event was found within tolerance.
    Ok,
    /// A matching event was found, but later than the tolerance.
    Late,
    /// Like `Late`, but explained: this run was missed while the machine
    /// was down and a systemd timer's `Persistent=true` caught it up at
    /// the next boot, per `systemd.timer(5)`'s "triggered immediately if
    /// it would have been triggered at least once" guarantee.
    LateCaughtUp,
    /// No matching event, but the logs cover this instant: the run
    /// genuinely appears to be missing.
    Missed,
    /// No matching event, and the logs don't cover this instant: absence
    /// of evidence, not evidence of absence.
    NoTrace,
    /// `@reboot`: no expected run times, only found boots, for info.
    RebootOnly,
    /// A systemd timer with only monotonic triggers (`OnBootSec=`, ...)
    /// this project cannot compute expected times for.
    UnsupportedTrigger,
}
