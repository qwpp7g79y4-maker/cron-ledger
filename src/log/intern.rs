//! A per-parse string interner for `user`/`command` text.
//!
//! A real cron log's `user` is almost always one of a handful of
//! accounts, and its `command` is the same handful of job lines firing
//! over and over, repeated once per run across a log that can hold
//! millions of lines. Sharing one `Rc<str>` allocation per distinct
//! string (scoped to a single parse call, like the file-label sharing
//! in [`super::syslog`] and [`super::journal`]) keeps that repetition
//! from reallocating identical text on every line.

use std::collections::HashSet;
use std::rc::Rc;

#[derive(Default)]
pub(crate) struct Interner {
    seen: HashSet<Rc<str>>,
}

impl Interner {
    pub(crate) fn new() -> Self {
        Interner::default()
    }

    /// Returns the shared `Rc<str>` for `s`, allocating one only the
    /// first time this exact text is seen.
    pub(crate) fn intern(&mut self, s: &str) -> Rc<str> {
        if let Some(existing) = self.seen.get(s) {
            return Rc::clone(existing);
        }
        let rc: Rc<str> = Rc::from(s);
        self.seen.insert(Rc::clone(&rc));
        rc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_text_shares_one_allocation() {
        let mut interner = Interner::new();
        let a = interner.intern("root");
        let b = interner.intern("root");
        assert!(Rc::ptr_eq(&a, &b));
        assert_eq!(Rc::strong_count(&a), 3); // a, b, and the set's own copy
    }

    #[test]
    fn distinct_text_gets_distinct_allocations() {
        let mut interner = Interner::new();
        let a = interner.intern("root");
        let b = interner.intern("alice");
        assert!(!Rc::ptr_eq(&a, &b));
    }
}
