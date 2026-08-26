//! Reading mermaid's `gitGraph` language — commits on branches, with merges and cherry-picks.
//!
//! Read from `packages/parser/src/language/gitGraph/gitGraph.langium` and `gitGraphAst.ts` at tag
//! `mermaid@11.17.2`.
//!
//! # The keyword has four spellings and one of them carries the direction
//!
//! ```text
//! ('gitGraph' | 'gitGraph' ':' | 'gitGraph:' | ('gitGraph' Direction ':'))
//! ```
//!
//! `gitGraph LR:` is a direction; `gitGraph LR` — without the colon — is not, and upstream refuses
//! it. konoma refuses it too rather than being helpfully lenient, because being lenient here means
//! a diagram that draws in konoma and fails on GitHub.
//!
//! # A statement's fields may come in any order, and `msg:` is optional
//!
//! `Commit: 'commit' ('id:' STRING | 'msg:'? message=STRING | 'tag:' STRING | 'type:' …)*` — a
//! bare string is the message, and every field may repeat. `tag:` accumulates; the others
//! overwrite.
//!
//! # What konoma refuses, because upstream throws
//!
//! Creating a branch that exists, merging a branch into itself, merging a branch that does not
//! exist or has no commits, merging two heads that are already the same commit, a custom merge id
//! that is taken, and cherry-picking an id that does not exist. Every one of them is a *statement
//! about the history* that cannot be drawn truthfully, and the alternative to refusing is a
//! picture that says something false about a repository.

// konoma is a binary crate, so `pub` marks nothing as used: the parts of this module's surface
// that exist for the tests and for the renderer look unreachable to rustc until every caller is
// wired. Same reasoning, and the same lint, as the sibling parsers.
#![allow(dead_code)]

use crate::preview::mermaid::chart::{
    find_header, first_word, starts_ci, Envelope, ParseError, Preamble, Source, TitleSyntax,
};

/// The keyword.
pub const KEYWORD: &str = "gitGraph";

/// mermaid's `gitGraph.mainBranchName` default.
pub const MAIN: &str = "main";

/// Which way the history runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// `gitGraph LR:` — time runs left to right. mermaid's default.
    #[default]
    LeftToRight,
    /// `gitGraph TB:` — time runs down the page.
    TopToBottom,
    /// `gitGraph BT:` — time runs up the page.
    BottomToTop,
}

/// What a commit is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommitKind {
    /// The default, and `type: NORMAL`.
    #[default]
    Normal,
    /// `type: REVERSE`.
    Reverse,
    /// `type: HIGHLIGHT`.
    Highlight,
    /// Made by `merge`. Not spellable as a `type:`.
    Merge,
    /// Made by `cherry-pick`.
    CherryPick,
}

/// One commit.
#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    /// The id, written or generated.
    pub id: String,
    /// Whether the source wrote the id itself (`commit id: "x"`).
    ///
    /// A generated id is `seq + '-' + getID()` upstream — a random string nobody wrote and nobody
    /// can refer to. Knowing which is which is what lets the drawing caption the commits an author
    /// named and leave the rest bare, instead of littering the diagram with noise.
    pub explicit_id: bool,
    /// The message, if one was written. A merge gets one made for it.
    pub message: String,
    /// Position in the whole history, which is what orders the drawing.
    pub seq: usize,
    /// What it is.
    pub kind: CommitKind,
    /// A `type:` written on a `merge`, which overrides how the merge dot is drawn but not that it
    /// is a merge.
    pub custom_type: Option<CommitKind>,
    /// Its `tag:`s, in the order written.
    pub tags: Vec<String>,
    /// Its parents' ids: one for an ordinary commit, two for a merge, none for the first.
    pub parents: Vec<String>,
    /// Which branch it was made on.
    pub branch: String,
}

/// One branch.
#[derive(Debug, Clone, PartialEq)]
pub struct Branch {
    /// Its name.
    pub name: String,
    /// `order:`, which overrides where its lane is drawn.
    pub order: Option<i64>,
}

/// A parsed git graph.
#[derive(Debug, Clone, PartialEq)]
pub struct GitGraph {
    /// Title and accessibility statements.
    pub preamble: Preamble,
    /// Which way time runs.
    pub direction: Direction,
    /// The branches, in the order they came into existence. `main` is always first.
    pub branches: Vec<Branch>,
    /// The commits, in `seq` order.
    pub commits: Vec<Commit>,
}

impl Default for GitGraph {
    fn default() -> GitGraph {
        GitGraph {
            preamble: Preamble::default(),
            direction: Direction::default(),
            branches: vec![Branch {
                name: MAIN.to_string(),
                order: None,
            }],
            commits: Vec::new(),
        }
    }
}

impl GitGraph {
    /// The lanes, in the order they are drawn: `order:` first where it was given, then the order
    /// the branches were created in. mermaid's `branchPos` sort.
    pub fn lanes(&self) -> Vec<&Branch> {
        let mut out: Vec<(usize, &Branch)> = self.branches.iter().enumerate().collect();
        out.sort_by(|(ia, a), (ib, b)| match (a.order, b.order) {
            (Some(x), Some(y)) => x.cmp(&y).then(ia.cmp(ib)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => ia.cmp(ib),
        });
        out.into_iter().map(|(_, b)| b).collect()
    }

    /// A commit by id.
    pub fn commit(&self, id: &str) -> Option<&Commit> {
        self.commits.iter().find(|c| c.id == id)
    }
}

/// Whether this source is a git graph.
pub fn is_git_graph(src: &str) -> bool {
    header_of(src).is_some()
}

/// The keyword line, with its four spellings resolved.
fn header_of(src: &str) -> Option<(Source, Direction)> {
    // `gitGraph:` is one token upstream, so the keyword scan has to accept the colon as part of
    // the word rather than as trailing text.
    let source = find_header(src, &["gitGraph:", "gitGraph"])?;
    let rest = source.header_rest.trim();
    if rest.is_empty() || rest == ":" {
        return Some((source, Direction::LeftToRight));
    }
    let (word, tail) = rest.split_once(':')?;
    if !tail.trim().is_empty() {
        return None;
    }
    let direction = match word.trim().to_ascii_uppercase().as_str() {
        "LR" => Direction::LeftToRight,
        "TB" => Direction::TopToBottom,
        "BT" => Direction::BottomToTop,
        _ => return None,
    };
    Some((source, direction))
}

/// Parses a mermaid git graph.
pub fn parse(src: &str) -> Result<GitGraph, ParseError> {
    let Some((
        Source {
            lines,
            header_index,
            front_matter_title,
            ..
        },
        direction,
    )) = header_of(src)
    else {
        return Err(ParseError::NotThisChart {
            expected: KEYWORD,
            header: first_word(src),
        });
    };

    let mut graph = GitGraph {
        direction,
        ..GitGraph::default()
    };
    let mut env = Envelope::new(front_matter_title);
    // `branches` maps a branch to the id of its head, `null` until it has one.
    let mut heads: Vec<(String, Option<String>)> = vec![(MAIN.to_string(), None)];
    let mut current = MAIN.to_string();
    let mut head: Option<String> = None;
    let mut seq = 0usize;

    for (i, line) in lines.iter().enumerate().skip(header_index + 1) {
        let number = i + 1;
        if env.read(line, TitleSyntax::Terminal) {
            continue;
        }
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let Some((keyword, rest)) = split_keyword(t) else {
            return Err(ParseError::Unexpected {
                kind: KEYWORD,
                line: number,
                text: t.to_string(),
            });
        };
        match keyword.as_str() {
            "commit" => {
                let f = read_fields(rest, number, &["id", "msg", "tag", "type"])?;
                let id = f.id.clone().unwrap_or_else(|| format!("{seq}-{seq}"));
                graph.commits.push(Commit {
                    id: id.clone(),
                    explicit_id: f.id.is_some(),
                    message: f.message.unwrap_or_default(),
                    seq,
                    kind: f.kind.unwrap_or(CommitKind::Normal),
                    custom_type: None,
                    tags: f.tags,
                    parents: head.iter().cloned().collect(),
                    branch: current.clone(),
                });
                seq += 1;
                head = Some(id.clone());
                set_head(&mut heads, &current, Some(id));
            }
            "branch" => {
                let (name, order) = read_branch(rest, number)?;
                if heads.iter().any(|(b, _)| *b == name) {
                    return Err(ParseError::Invalid {
                        line: number,
                        message: format!("branch `{name}` already exists"),
                    });
                }
                heads.push((name.clone(), head.clone()));
                graph.branches.push(Branch {
                    name: name.clone(),
                    order,
                });
                current = name;
            }
            "checkout" | "switch" => {
                let name = unquote(rest.trim());
                if !heads.iter().any(|(b, _)| *b == name) {
                    return Err(ParseError::Invalid {
                        line: number,
                        message: format!("cannot check out `{name}`: no such branch"),
                    });
                }
                head = heads
                    .iter()
                    .find(|(b, _)| *b == name)
                    .and_then(|(_, h)| h.clone());
                current = name;
            }
            "merge" => {
                let (other, rest) = split_first_word(rest);
                let other = unquote(other);
                let f = read_fields(rest, number, &["id", "tag", "type"])?;
                // Checked before the "both branches are at the same commit" test below, which
                // would otherwise catch the same source with a message that describes a symptom
                // rather than the mistake. Upstream keeps both, in this order, for the same
                // reason; `tests` pins the wording so the order cannot quietly change.
                if other == current {
                    return Err(ParseError::Invalid {
                        line: number,
                        message: format!("cannot merge `{other}` into itself"),
                    });
                }
                let Some((_, other_head)) = heads.iter().find(|(b, _)| *b == other) else {
                    return Err(ParseError::Invalid {
                        line: number,
                        message: format!("cannot merge `{other}`: no such branch"),
                    });
                };
                let Some(other_head) = other_head.clone() else {
                    return Err(ParseError::Invalid {
                        line: number,
                        message: format!("cannot merge `{other}`: it has no commits"),
                    });
                };
                if head.as_deref() == Some(other_head.as_str()) {
                    return Err(ParseError::Invalid {
                        line: number,
                        message: format!(
                            "cannot merge `{other}`: both branches are at the same commit"
                        ),
                    });
                }
                if let Some(custom) = &f.id {
                    if graph.commits.iter().any(|c| c.id == *custom) {
                        return Err(ParseError::Invalid {
                            line: number,
                            message: format!("a commit with id `{custom}` already exists"),
                        });
                    }
                }
                let explicit_id = f.id.is_some();
                let id = f.id.clone().unwrap_or_else(|| format!("{seq}-{seq}"));
                let mut parents: Vec<String> = head.iter().cloned().collect();
                if !parents.is_empty() {
                    parents.push(other_head);
                }
                graph.commits.push(Commit {
                    id: id.clone(),
                    explicit_id,
                    message: format!("merged branch {other} into {current}"),
                    seq,
                    kind: CommitKind::Merge,
                    custom_type: f.kind,
                    tags: f.tags,
                    parents,
                    branch: current.clone(),
                });
                seq += 1;
                head = Some(id.clone());
                set_head(&mut heads, &current, Some(id));
            }
            "cherry-pick" => {
                let f = read_fields(rest, number, &["id", "tag", "parent"])?;
                let Some(source_id) = f.id.clone() else {
                    return Err(ParseError::Invalid {
                        line: number,
                        message: "cherry-pick needs the id of the commit to pick".to_string(),
                    });
                };
                let Some(picked) = graph.commits.iter().find(|c| c.id == source_id) else {
                    return Err(ParseError::Invalid {
                        line: number,
                        message: format!("cannot cherry-pick `{source_id}`: no such commit"),
                    });
                };
                if picked.branch == current {
                    return Err(ParseError::Invalid {
                        line: number,
                        message: format!("`{source_id}` is already on `{current}`"),
                    });
                }
                if let Some(parent) = &f.parent {
                    if !picked.parents.iter().any(|p| p == parent) {
                        return Err(ParseError::Invalid {
                            line: number,
                            message: format!("`{parent}` is not a parent of `{source_id}`"),
                        });
                    }
                }
                let tags = if f.tags.is_empty() {
                    vec![format!("cherry-pick:{source_id}")]
                } else {
                    f.tags
                };
                let id = format!("{seq}-{seq}");
                graph.commits.push(Commit {
                    id: id.clone(),
                    explicit_id: false,
                    message: format!("cherry-picked {source_id} into {current}"),
                    seq,
                    kind: CommitKind::CherryPick,
                    custom_type: None,
                    tags,
                    parents: head.iter().cloned().collect(),
                    branch: current.clone(),
                });
                seq += 1;
                head = Some(id.clone());
                set_head(&mut heads, &current, Some(id));
            }
            _ => {
                return Err(ParseError::Unexpected {
                    kind: KEYWORD,
                    line: number,
                    text: t.to_string(),
                })
            }
        }
    }

    graph.preamble = env.preamble;
    if graph.commits.is_empty() {
        return Err(ParseError::NoData {
            kind: KEYWORD,
            wanted: "commit",
        });
    }
    Ok(graph)
}

fn set_head(heads: &mut [(String, Option<String>)], branch: &str, id: Option<String>) {
    if let Some(slot) = heads.iter_mut().find(|(b, _)| b == branch) {
        slot.1 = id;
    }
}

/// The fields a statement carries.
#[derive(Default)]
struct Fields {
    id: Option<String>,
    message: Option<String>,
    tags: Vec<String>,
    kind: Option<CommitKind>,
    parent: Option<String>,
}

/// Reads `key: "value"` pairs in any order, plus the bare string that is a `commit`'s message.
fn read_fields(rest: &str, number: usize, allowed: &[&str]) -> Result<Fields, ParseError> {
    let mut out = Fields::default();
    let mut s = rest.trim();
    while !s.is_empty() {
        // A bare string with no key is the message.
        if s.starts_with('"') || s.starts_with('\'') {
            let (value, tail) = take_string(s, number)?;
            if !allowed.contains(&"msg") {
                return Err(ParseError::Unexpected {
                    kind: KEYWORD,
                    line: number,
                    text: value,
                });
            }
            out.message = Some(value);
            s = tail.trim_start();
            continue;
        }
        let Some(colon) = s.find(':') else {
            return Err(ParseError::Unexpected {
                kind: KEYWORD,
                line: number,
                text: s.to_string(),
            });
        };
        let key = s[..colon].trim().to_ascii_lowercase();
        if !allowed.iter().any(|a| *a == key) {
            return Err(ParseError::Unexpected {
                kind: KEYWORD,
                line: number,
                text: s.to_string(),
            });
        }
        let mut tail = s[colon + 1..].trim_start();
        let value = if tail.starts_with('"') || tail.starts_with('\'') {
            let (v, t) = take_string(tail, number)?;
            tail = t.trim_start();
            v
        } else {
            let (v, t) = split_first_word(tail);
            tail = t.trim_start();
            v.to_string()
        };
        match key.as_str() {
            "id" => out.id = Some(value),
            "msg" => out.message = Some(value),
            "tag" => out.tags.push(value),
            "parent" => out.parent = Some(value),
            "type" => {
                out.kind = Some(match value.to_ascii_uppercase().as_str() {
                    "NORMAL" => CommitKind::Normal,
                    "REVERSE" => CommitKind::Reverse,
                    "HIGHLIGHT" => CommitKind::Highlight,
                    _ => {
                        return Err(ParseError::Invalid {
                            line: number,
                            message: format!("`{value}` is not one of NORMAL, REVERSE, HIGHLIGHT"),
                        })
                    }
                })
            }
            _ => unreachable!("checked above"),
        }
        s = tail;
    }
    Ok(out)
}

/// `branch name` with an optional `order: N`.
fn read_branch(rest: &str, number: usize) -> Result<(String, Option<i64>), ParseError> {
    let (name, tail) = split_first_word(rest.trim());
    let name = unquote(name);
    if name.is_empty() {
        return Err(ParseError::Unexpected {
            kind: KEYWORD,
            line: number,
            text: rest.trim().to_string(),
        });
    }
    let tail = tail.trim();
    if tail.is_empty() {
        return Ok((name, None));
    }
    let Some(value) = tail
        .strip_prefix("order:")
        .or_else(|| tail.strip_prefix("order :"))
    else {
        return Err(ParseError::Unexpected {
            kind: KEYWORD,
            line: number,
            text: tail.to_string(),
        });
    };
    let order = value
        .trim()
        .parse::<i64>()
        .map_err(|_| ParseError::BadNumber {
            line: number,
            text: value.trim().to_string(),
            why: "a branch order must be a whole number",
        })?;
    Ok((name, Some(order)))
}

/// The statement keyword. `cherry-pick` has a dash in it, so a plain whitespace split is wrong.
fn split_keyword(t: &str) -> Option<(String, &str)> {
    for word in [
        "cherry-pick",
        "checkout",
        "switch",
        "commit",
        "branch",
        "merge",
    ] {
        if starts_ci(t, word) {
            let rest = &t[word.len()..];
            if rest.is_empty() || rest.starts_with([' ', '\t']) {
                return Some((word.to_string(), rest));
            }
        }
    }
    None
}

fn split_first_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    }
}

/// Takes a `"…"` or `'…'` string off the front.
fn take_string(s: &str, number: usize) -> Result<(String, &str), ParseError> {
    let quote = s.chars().next().expect("caller checked");
    let rest = &s[quote.len_utf8()..];
    let Some(end) = rest.find(quote) else {
        return Err(ParseError::Invalid {
            line: number,
            message: format!("unclosed `{quote}`"),
        });
    };
    Ok((rest[..end].to_string(), &rest[end + quote.len_utf8()..]))
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            return v[1..v.len() - 1].to_string();
        }
    }
    v.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> GitGraph {
        parse(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
    }

    #[test]
    fn the_documented_example_reads_as_a_branch_a_merge_and_six_commits() {
        let g = ok("gitGraph\n   commit\n   commit\n   branch develop\n   checkout develop\n   commit\n   commit\n   checkout main\n   merge develop\n   commit\n   commit\n");
        assert_eq!(g.commits.len(), 7);
        assert_eq!(g.branches.len(), 2);
        assert_eq!(g.branches[1].name, "develop");
        assert_eq!(g.commits[2].branch, "develop");
        let merge = &g.commits[4];
        assert_eq!(merge.kind, CommitKind::Merge);
        assert_eq!(merge.branch, "main");
        assert_eq!(merge.parents.len(), 2, "a merge has two parents");
        assert_eq!(merge.message, "merged branch develop into main");
    }

    #[test]
    fn every_documented_commit_field_is_read_in_any_order() {
        let g = ok("gitGraph\n  commit id: \"Normal\" tag: \"v1.0.0\"\n  commit type: REVERSE id: \"Reverse\" tag: \"RC_1\" tag: \"second\"\n  commit \"a bare message\"\n  commit msg: \"a keyed message\"\n");
        assert_eq!(g.commits[0].id, "Normal");
        assert_eq!(g.commits[0].tags, ["v1.0.0"]);
        assert_eq!(g.commits[1].kind, CommitKind::Reverse);
        assert_eq!(g.commits[1].tags, ["RC_1", "second"]);
        assert_eq!(g.commits[2].message, "a bare message");
        assert_eq!(g.commits[3].message, "a keyed message");
    }

    #[test]
    fn the_direction_needs_its_colon() {
        assert_eq!(
            ok("gitGraph LR:\n  commit\n").direction,
            Direction::LeftToRight
        );
        assert_eq!(
            ok("gitGraph TB:\n  commit\n").direction,
            Direction::TopToBottom
        );
        assert_eq!(
            ok("gitGraph BT:\n  commit\n").direction,
            Direction::BottomToTop
        );
        assert_eq!(
            ok("gitGraph:\n  commit\n").direction,
            Direction::LeftToRight
        );
        assert_eq!(ok("gitGraph\n  commit\n").direction, Direction::LeftToRight);
        // `gitGraph LR` without the colon is not this grammar.
        assert!(matches!(
            parse("gitGraph LR\n  commit\n"),
            Err(ParseError::NotThisChart { .. })
        ));
    }

    #[test]
    fn branch_order_decides_which_lane_is_drawn_first() {
        let g = ok("gitGraph\n  commit\n  branch b order: 3\n  commit\n  branch a order: 1\n  commit\n  branch c\n  commit\n");
        let lanes: Vec<&str> = g.lanes().iter().map(|b| b.name.as_str()).collect();
        assert_eq!(lanes, ["a", "b", "main", "c"]);
    }

    #[test]
    fn cherry_pick_carries_a_tag_naming_what_it_picked() {
        let g = ok("gitGraph\n  commit id: \"ZERO\"\n  branch develop\n  branch release\n  commit id: \"A\"\n  checkout main\n  commit id: \"ONE\"\n  checkout develop\n  commit id: \"B\"\n  checkout main\n  merge develop id: \"MERGE\"\n  commit id: \"TWO\"\n  checkout release\n  cherry-pick id: \"MERGE\" parent: \"B\"\n");
        let picked = g.commits.last().expect("a commit");
        assert_eq!(picked.kind, CommitKind::CherryPick);
        assert_eq!(picked.branch, "release");
        assert_eq!(picked.tags, ["cherry-pick:MERGE"]);
    }

    /// Each of these is a statement about a repository that cannot be true, and each one the
    /// crate konoma replaced would draw anyway.
    ///
    /// The **message** is checked as well as the refusal for the pair that would otherwise be
    /// indistinguishable: merging a branch into itself and merging two branches that are already
    /// at the same commit are two different mistakes, and the first is the one an author made.
    #[test]
    fn an_impossible_history_is_refused_rather_than_drawn() {
        assert_eq!(
            parse("gitGraph\n  commit\n  merge main\n")
                .unwrap_err()
                .to_string(),
            "cannot merge `main` into itself at line 3"
        );
        assert_eq!(
            parse("gitGraph\n  commit\n  branch a\n  checkout main\n  merge a\n")
                .unwrap_err()
                .to_string(),
            "cannot merge `a`: both branches are at the same commit at line 5"
        );
        for src in [
            "gitGraph\n  commit\n  branch a\n  branch a\n",
            "gitGraph\n  commit\n  merge main\n",
            "gitGraph\n  commit\n  merge nope\n",
            "gitGraph\n  commit\n  branch a\n  checkout main\n  merge a\n",
            "gitGraph\n  commit\n  checkout nope\n",
            "gitGraph\n  commit\n  cherry-pick id: \"nope\"\n",
            "gitGraph\n  commit id: \"x\"\n  branch a\n  commit\n  checkout main\n  merge a id: \"x\"\n",
            "gitGraph\n  commit id: \"x\"\n  branch a\n  commit\n  cherry-pick id: \"x\" parent: \"nope\"\n",
        ] {
            assert!(
                matches!(parse(src), Err(ParseError::Invalid { .. })),
                "should be refused: {src:?} — got {:?}",
                parse(src).map(|g| g.commits.len())
            );
        }
    }

    #[test]
    fn switch_is_checkout() {
        let g = ok("gitGraph\n  commit\n  branch a\n  checkout main\n  switch a\n  commit\n");
        assert_eq!(g.commits[1].branch, "a");
    }

    #[test]
    fn an_unknown_commit_type_is_refused() {
        assert!(matches!(
            parse("gitGraph\n  commit type: SPARKLY\n"),
            Err(ParseError::Invalid { .. })
        ));
    }

    #[test]
    fn a_header_with_no_commits_is_refused() {
        assert!(matches!(
            parse("gitGraph\n"),
            Err(ParseError::NoData { .. })
        ));
    }

    #[test]
    fn the_routing_predicate_and_the_parser_agree() {
        for src in [
            "gitGraph\n  commit",
            "GITGRAPH\n  commit",
            "gitGraph:\n  commit",
            "gitGraph LR:\n  commit",
            "gitGraph",
            "gitGraphs\n  commit",
            "gantt\n  title G",
            "",
            "%% only a comment\n",
        ] {
            let refused = matches!(parse(src), Err(ParseError::NotThisChart { .. }));
            assert_eq!(is_git_graph(src), !refused, "{src:?}");
        }
    }
}
