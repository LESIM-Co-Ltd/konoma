//! Shared names for the scene model: the typefaces and the language tags of the runs of a deck are
//! a handful of distinct strings repeated on every run, so a run holds an `Arc<str>` from here
//! instead of a `String` of its own.
//!
//! The table is per thread (a deck is read on one thread) and bounded: a deck full of distinct
//! names fills it, and it starts again empty (the names already handed out stay valid, they are
//! just no longer shared with the ones that come after). A name over [`MAX_NAME_BYTES`] is cut to
//! that length, so every name is shared and none can be a big allocation of its own.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Arc;

/// Most distinct names the table keeps before it starts again.
pub const MAX_NAMES: usize = 1024;

/// Longest name the table keeps (a typeface or language tag is a few bytes to a few dozen). A
/// longer one is cut to it at a character boundary: no real typeface or BCP-47 tag is that long,
/// and a document's attribute can be thousands of bytes that every run would otherwise hold a
/// private copy of (a theme font inherited by 100k runs).
pub const MAX_NAME_BYTES: usize = 64;

thread_local! {
    static NAMES: RefCell<HashSet<Arc<str>>> = RefCell::new(HashSet::new());
}

/// The shared `Arc<str>` for `s` (cut to [`MAX_NAME_BYTES`]): the same allocation for equal
/// names, while the table holds them.
pub fn intern(s: &str) -> Arc<str> {
    let s = cut_to_name(s);
    NAMES.with(|names| {
        let mut names = names.borrow_mut();
        if let Some(a) = names.get(s) {
            return Arc::clone(a);
        }
        if names.len() >= MAX_NAMES {
            names.clear();
        }
        let a: Arc<str> = Arc::from(s);
        names.insert(Arc::clone(&a));
        a
    })
}

/// `s` cut to at most [`MAX_NAME_BYTES`] bytes, at a character boundary.
fn cut_to_name(s: &str) -> &str {
    if s.len() <= MAX_NAME_BYTES {
        return s;
    }
    let mut end = MAX_NAME_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_names_share_one_allocation() {
        let a = intern("Calibri");
        let b = intern("Calibri");
        let c = intern("Arial");
        assert!(Arc::ptr_eq(&a, &b));
        assert!(!Arc::ptr_eq(&a, &c));
        assert_eq!(&*a, "Calibri");
        assert_eq!(&*c, "Arial");
    }

    #[test]
    fn a_long_name_is_cut_and_shared() {
        let long = "x".repeat(MAX_NAME_BYTES + 1);
        let a = intern(&long);
        let b = intern(&long);
        assert!(Arc::ptr_eq(&a, &b), "a long name is shared like any other");
        assert_eq!(a.len(), MAX_NAME_BYTES);
        // A 4 KB attribute (the most a reader accepts) is the same 64 bytes.
        let huge = "z".repeat(4096);
        assert_eq!(intern(&huge).len(), MAX_NAME_BYTES);
        assert!(Arc::ptr_eq(&intern(&huge), &intern(&huge)));
        // Cut at a character boundary: byte 64 is inside the 22nd three-byte character.
        let cjk = "\u{3042}".repeat(40);
        let c = intern(&cjk);
        assert_eq!(c.len(), 63);
        assert_eq!(&*c, "\u{3042}".repeat(21));
        assert!(Arc::ptr_eq(&c, &intern(&cjk)));
        // Names that differ only past the cut are one name.
        let d = intern(&format!("{}A", "x".repeat(MAX_NAME_BYTES)));
        let e = intern(&format!("{}B", "x".repeat(MAX_NAME_BYTES)));
        assert!(Arc::ptr_eq(&d, &e));
        // The longest name that is kept is shared.
        let edge = "y".repeat(MAX_NAME_BYTES);
        assert!(Arc::ptr_eq(&intern(&edge), &intern(&edge)));
    }

    #[test]
    fn a_full_table_starts_again_and_old_names_stay_valid() {
        let first = intern("first-name");
        for i in 0..MAX_NAMES + 5 {
            intern(&format!("name-{i}"));
        }
        // The table was cleared on the way: the old name is a new allocation now, equal to the
        // one handed out before.
        let again = intern("first-name");
        assert_eq!(first, again);
        assert!(!Arc::ptr_eq(&first, &again));
        // ...and the new one is shared from here on.
        assert!(Arc::ptr_eq(&again, &intern("first-name")));
        assert!(NAMES.with(|n| n.borrow().len()) <= MAX_NAMES);
    }

    #[test]
    fn the_empty_name_is_a_name() {
        assert!(Arc::ptr_eq(&intern(""), &intern("")));
        assert_eq!(&*intern(""), "");
    }
}
