//! Shared names for the scene model: the typefaces and the language tags of the runs of a deck are
//! a handful of distinct strings repeated on every run, so a run holds an `Arc<str>` from here
//! instead of a `String` of its own.
//!
//! The table is per thread (a deck is read on one thread) and bounded: a deck full of distinct
//! names fills it, and it starts again empty (the names already handed out stay valid, they are
//! just no longer shared with the ones that come after). A name over [`MAX_NAME_BYTES`] is not
//! kept in the table at all.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Arc;

/// Most distinct names the table keeps before it starts again.
pub const MAX_NAMES: usize = 1024;

/// Longest name the table keeps (a typeface or language tag is a few bytes to a few dozen).
pub const MAX_NAME_BYTES: usize = 64;

thread_local! {
    static NAMES: RefCell<HashSet<Arc<str>>> = RefCell::new(HashSet::new());
}

/// The shared `Arc<str>` for `s`: the same allocation for equal names, while the table holds
/// them.
pub fn intern(s: &str) -> Arc<str> {
    if s.len() > MAX_NAME_BYTES {
        return Arc::from(s);
    }
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
    fn a_long_name_is_not_kept_but_still_equal() {
        let long = "x".repeat(MAX_NAME_BYTES + 1);
        let a = intern(&long);
        let b = intern(&long);
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(a, b);
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
