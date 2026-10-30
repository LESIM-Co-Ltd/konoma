//! The part of the `Space` menu's assembly that does not depend on what an operation is: the
//! candidates are folded, ordered, given a letter, gated and named here, for any operation type
//! `A` (the host's `Action`, the standalone app's own). What the candidates are and what they are
//! called stay with the caller.

use crate::keymap::KeyPress;
use crossterm::event::KeyCode;

/// The letters tried for an operation that has neither a free key of its own nor a mnemonic.
/// `q` is not among them: inside a menu it closes it.
pub const FREE_LETTERS: &str = "abcdefghijklmnoprstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// One item of a built menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item<A, L> {
    pub key: KeyPress,
    pub action: A,
    pub label: L,
}

/// `k` when it can be the menu's key for an operation: a character without Ctrl, other than `q`
/// (closes the menu) and the space (opens it).
pub fn plain_key(k: KeyPress) -> Option<KeyPress> {
    (!k.ctrl && matches!(k.code, KeyCode::Char(c) if c != 'q' && c != ' ')).then_some(k)
}

/// Adds `a` to `out`. An operation already there is not added again; it takes `key` only when it
/// has none yet, so the first plain key of an operation is the one the menu uses.
pub fn add_candidate<A: Copy + PartialEq>(
    out: &mut Vec<(A, Option<KeyPress>)>,
    a: A,
    key: Option<KeyPress>,
) {
    match out.iter_mut().find(|(o, _)| *o == a) {
        Some((_, k)) => {
            if k.is_none() {
                *k = key;
            }
        }
        None => out.push((a, key)),
    }
}

/// What a screen's `Space` menu starts from, before any of it is named, ordered or given a
/// letter: the operations the screen can run, as the sources the menu reads them from. The caller
/// fills it from its keys (konoma's keymap, a standalone app's [`crate::keymap::CoreKeys`]) and
/// from what it declares; [`Skeleton::candidates`] folds it, so every host folds the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skeleton<A> {
    /// Every key that runs an operation now, in the order the menu lists them (the keymap's
    /// display order).
    pub bound: Vec<(KeyPress, A)>,
    /// What the screen has by default but no key reaches now (moved away by `[keys]`, or lost to
    /// a conflict).
    pub defaults: Vec<A>,
    /// The operations of the fixed keys (`Enter`, `Tab`, ...), aliases left out.
    pub fixed: Vec<A>,
    /// The operations with no key by default (they used to have one, or never did): the menu is
    /// where they are always reachable.
    pub unkeyed: Vec<A>,
    /// The operations the open plugin declared, keyed or not.
    pub declared: Vec<(A, Option<KeyPress>)>,
}

impl<A: Copy + PartialEq> Skeleton<A> {
    /// The candidates: every source in the order above, an operation once, under the first plain
    /// key it has. `offered` says whether an operation can be a candidate at all (it is named,
    /// and is not a movement or a closing); one that is not takes no letter.
    pub fn candidates(&self, offered: impl Fn(A) -> bool) -> Vec<(A, Option<KeyPress>)> {
        let mut out: Vec<(A, Option<KeyPress>)> = Vec::new();
        let mut add = |a: A, key: Option<KeyPress>| {
            if offered(a) {
                // The first plain key of an operation is the one the menu uses.
                add_candidate(&mut out, a, key);
            }
        };
        for &(k, a) in &self.bound {
            add(a, plain_key(k));
        }
        for &a in &self.defaults {
            add(a, None);
        }
        for &a in &self.fixed {
            add(a, None);
        }
        for &a in &self.unkeyed {
            add(a, None);
        }
        for &(a, k) in &self.declared {
            add(a, k);
        }
        out
    }
}

/// The key each candidate gets. Keys the operations already have come first (so a habit carries
/// over), then a mnemonic letter, then the first free letter. An operation left without a letter
/// (more operations than letters) gets `None`.
pub fn assign<A: Copy>(
    cands: &[(A, Option<KeyPress>)],
    mnemonic: impl Fn(A) -> Option<char>,
) -> Vec<Option<KeyPress>> {
    let mut taken: Vec<KeyPress> = Vec::new();
    let mut chosen: Vec<Option<KeyPress>> = vec![None; cands.len()];
    for (i, (_, k)) in cands.iter().enumerate() {
        if let Some(k) = k {
            if !taken.contains(k) {
                taken.push(*k);
                chosen[i] = Some(*k);
            }
        }
    }
    for (i, (a, _)) in cands.iter().enumerate() {
        if chosen[i].is_some() {
            continue;
        }
        let pick = mnemonic(*a)
            .map(KeyPress::ch)
            .filter(|k| !taken.contains(k))
            .or_else(|| {
                FREE_LETTERS
                    .chars()
                    .map(KeyPress::ch)
                    .find(|k| !taken.contains(k))
            });
        if let Some(k) = pick {
            taken.push(k);
            chosen[i] = Some(k);
        }
    }
    chosen
}

/// Where an operation sorts in the menu: its place in the footer plan `prio`, then the operations
/// the help lists for the screen itself (`own`, in the help's order), then the ones neither names
/// (they keep the order they came in), then the rows every screen lists (`everywhere`).
pub fn rank<A: PartialEq>(a: A, prio: &[A], own: &[A], everywhere: &[A]) -> (usize, u8, usize) {
    let p = prio.iter().position(|p| *p == a).unwrap_or(usize::MAX);
    if let Some(i) = own.iter().position(|d| *d == a) {
        (p, 0, i)
    } else if let Some(i) = everywhere.iter().position(|d| *d == a) {
        (p, 2, i)
    } else {
        (p, 1, 0)
    }
}

/// The menu of `cands`: menu order first (so the free letters run along the menu for the items
/// with no key of their own), then the letters, then the items `enabled` lets through, each named
/// by `label` (`None` = not offered). The order and the letters never look at `enabled`, so a
/// letter does not move because another operation happens not to act right now.
pub fn build_items<A: Copy + PartialEq, L>(
    mut cands: Vec<(A, Option<KeyPress>)>,
    prio: &[A],
    own: &[A],
    everywhere: &[A],
    mnemonic: impl Fn(A) -> Option<char>,
    enabled: impl Fn(A) -> bool,
    label: impl Fn(A) -> Option<L>,
) -> Vec<Item<A, L>> {
    cands.sort_by_key(|(a, _)| rank(*a, prio, own, everywhere));
    let letters = assign(&cands, mnemonic);
    cands
        .iter()
        .zip(letters)
        .filter(|((a, _), _)| enabled(*a))
        .filter_map(|((a, _), key)| {
            Some(Item {
                key: key?,
                action: *a,
                label: label(*a)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    type C = Vec<(u32, Option<KeyPress>)>;
    const NONE: fn(u32) -> Option<char> = |_| None;

    fn k(c: char) -> Option<KeyPress> {
        Some(KeyPress::ch(c))
    }

    #[test]
    fn a_skeleton_folds_its_sources_in_order_and_skips_what_is_not_offered() {
        let sk = Skeleton::<u32> {
            bound: vec![
                (KeyPress::ch('x'), 1),
                (KeyPress::ctrl_ch('n'), 2),
                (KeyPress::ch('y'), 9),
            ],
            defaults: vec![1, 3],
            fixed: vec![4, 3],
            unkeyed: vec![5],
            declared: vec![(6, k('z')), (2, k('w'))],
        };
        let got = sk.candidates(|a| a != 9);
        // `2` has only a Ctrl key in `bound`, so the later plain key of its declaration is its own.
        assert_eq!(
            got,
            vec![
                (1, k('x')),
                (2, k('w')),
                (3, None),
                (4, None),
                (5, None),
                (6, k('z'))
            ]
        );
    }

    #[test]
    fn add_candidate_folds_a_repeated_operation_and_keeps_the_first_key() {
        let mut v: C = Vec::new();
        add_candidate(&mut v, 1, k('a'));
        add_candidate(&mut v, 1, k('b'));
        assert_eq!(v, vec![(1, k('a'))]);
    }

    #[test]
    fn add_candidate_takes_a_key_that_only_the_second_occurrence_has() {
        let mut v: C = Vec::new();
        add_candidate(&mut v, 1, None);
        add_candidate(&mut v, 2, k('x'));
        add_candidate(&mut v, 1, k('b'));
        add_candidate(&mut v, 1, k('c'));
        assert_eq!(v, vec![(1, k('b')), (2, k('x'))]);
    }

    #[test]
    fn plain_key_excludes_q_space_ctrl_and_non_characters() {
        assert_eq!(plain_key(KeyPress::ch('a')), k('a'));
        assert_eq!(plain_key(KeyPress::ch('Z')), k('Z'));
        assert_eq!(plain_key(KeyPress::ch('q')), None);
        assert_eq!(plain_key(KeyPress::ch(' ')), None);
        assert_eq!(plain_key(KeyPress::ctrl_ch('a')), None);
        assert_eq!(plain_key(KeyPress::key(KeyCode::Enter)), None);
    }

    #[test]
    fn assign_lets_an_existing_key_beat_a_mnemonic() {
        // 2 has the mnemonic 'x' but 1 already owns 'x': the key wins even though 1 comes later.
        let c: C = vec![(2, None), (1, k('x'))];
        let m = |a: u32| (a == 2).then_some('x');
        let got = assign(&c, m);
        assert_eq!(got[1], k('x'));
        assert_eq!(got[0], k('a'));
    }

    #[test]
    fn assign_moves_the_second_of_a_key_clash_to_its_mnemonic_or_a_free_letter() {
        let c: C = vec![(1, k('x')), (2, k('x')), (3, k('x'))];
        let m = |a: u32| (a == 2).then_some('m');
        let got = assign(&c, m);
        assert_eq!(got, vec![k('x'), k('m'), k('a')]);
    }

    #[test]
    fn assign_falls_back_to_a_free_letter_when_the_mnemonic_is_taken() {
        let c: C = vec![(1, k('m')), (2, None)];
        let m = |a: u32| (a == 2).then_some('m');
        assert_eq!(assign(&c, m), vec![k('m'), k('a')]);
    }

    #[test]
    fn free_letters_have_no_q() {
        assert!(!FREE_LETTERS.contains('q'));
        let c: C = (0..60).map(|i| (i, None)).collect();
        let got = assign(&c, NONE);
        assert!(got.iter().flatten().all(|p| p.code != KeyCode::Char('q')));
        assert!(got.iter().all(|p| p.is_some()));
    }

    #[test]
    fn assign_leaves_the_overflow_without_a_letter() {
        let n = FREE_LETTERS.chars().count();
        let c: C = (0..n as u32 + 3).map(|i| (i, None)).collect();
        let got = assign(&c, NONE);
        assert_eq!(got.iter().filter(|p| p.is_some()).count(), n);
        assert!(got[n..].iter().all(|p| p.is_none()));
    }

    #[test]
    fn rank_orders_footer_then_own_then_neither_then_everywhere() {
        let (prio, own, every) = ([10u32, 11], [20u32, 21], [30u32, 31]);
        let r = |a| rank(a, &prio, &own, &every);
        // A footer position dominates; inside one position the layer decides.
        assert!(r(10) < r(11));
        assert!(r(11) < r(20));
        assert!(r(20) < r(21));
        assert!(r(21) < r(99));
        assert!(r(99) < r(30));
        assert!(r(30) < r(31));
        // An operation in the footer sorts before layer 0 ones even when it is `everywhere`.
        let (prio, every) = ([30u32], [30u32, 31]);
        assert!(rank(30, &prio, &own, &every) < rank(20, &prio, &own, &every));
        // Layer numbers themselves.
        assert_eq!(rank(20, &[], &own, &every).1, 0);
        assert_eq!(rank(99, &[], &own, &every).1, 1);
        assert_eq!(rank(31, &[], &own, &every).1, 2);
    }

    fn named(a: u32) -> Option<String> {
        Some(format!("op{a}"))
    }

    #[test]
    fn build_items_orders_by_rank_before_giving_letters() {
        let cands: C = vec![(5, None), (3, None), (4, None)];
        let items = build_items(cands, &[3], &[4], &[5], NONE, |_| true, named);
        let got: Vec<_> = items.iter().map(|i| (i.action, i.key)).collect();
        let (a, b, c) = (KeyPress::ch('a'), KeyPress::ch('b'), KeyPress::ch('c'));
        assert_eq!(got, vec![(3, a), (4, b), (5, c)]);
    }

    #[test]
    fn the_gate_does_not_move_letters() {
        let cands: C = vec![(1, None), (2, None), (3, None)];
        let all = build_items(cands.clone(), &[], &[], &[], NONE, |_| true, named);
        let gated = build_items(cands, &[], &[], &[], NONE, |a| a != 1, named);
        assert_eq!(gated.len(), 2);
        assert_eq!(gated[0].action, 2);
        assert_eq!(gated[0].key, all[1].key);
        assert_eq!(gated[1].key, all[2].key);
    }

    #[test]
    fn an_unnamed_item_is_not_offered_and_keeps_its_letter_unused_by_others() {
        let cands: C = vec![(1, None), (2, None)];
        let items = build_items(
            cands,
            &[],
            &[],
            &[],
            NONE,
            |_| true,
            |a| (a != 1).then(|| format!("op{a}")),
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].action, 2);
        // Letters are decided before naming: 2 stays on the second letter.
        assert_eq!(items[0].key, KeyPress::ch('b'));
    }

    #[test]
    fn build_items_with_more_candidates_than_letters_drops_the_overflow() {
        let n = FREE_LETTERS.chars().count() as u32;
        let cands: C = (0..n + 5).map(|i| (i, None)).collect();
        let items = build_items(cands, &[], &[], &[], NONE, |_| true, named);
        assert_eq!(items.len() as u32, n);
    }
}
