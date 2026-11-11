//! SIGINT flag for the library. The handler itself lives in the binary:
//! this crate forbids unsafe code.
//!
//! `Read::read_to_end` retries [`std::io::ErrorKind::Interrupted`], so
//! [`StopOnSigint`] turns a caught SIGINT into a different error.

use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};

/// Exit status for SIGINT. Ordinary success and error statuses stay as they are.
pub const EXIT_INTERRUPTED: i32 = 130;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// Called from the signal handler. A store is async-signal-safe.
pub fn mark_interrupted() {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Reads from `inner`, and returns a non-interrupted error once SIGINT was caught.
pub struct StopOnSigint<R> {
    inner: R,
}

impl<R> StopOnSigint<R> {
    pub fn new(inner: R) -> Self {
        Self { inner }
    }
}

impl<R: Read> Read for StopOnSigint<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if interrupted() {
            return Err(stopped());
        }
        match self.inner.read(buf) {
            Err(err) if err.kind() == io::ErrorKind::Interrupted && interrupted() => Err(stopped()),
            other => other,
        }
    }
}

fn stopped() -> io::Error {
    io::Error::other("interrupted by SIGINT")
}
