//! Parent/child rules for hierarchical post types, as pure functions.
//!
//! Nothing here touches the database: the service loads a type's rows as
//! [`Node`]s, asks these functions whether a change is legal and what paths
//! result, then writes. That keeps the rules testable without a database
//! and makes "moves are validated as a whole" cheap: the final tree is
//! checked, not each step.

use std::collections::{BTreeMap, BTreeSet};

/// Deepest nesting a hierarchical type allows (the root is depth 1).
pub const MAX_DEPTH: usize = 8;

/// Slugs no root-level entry of a hierarchical type may take: they are
/// sub-routes of the type's URL base (`/docs/search`).
pub const RESERVED_ROOT_SLUGS: &[&str] = &["search"];

/// Most moves one reorder request may carry.
pub const MAX_MOVES: usize = 500;

/// The part of a row the rules need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Row id.
    pub id: i64,
    /// Parent row, `None` at the root.
    pub parent_id: Option<i64>,
    /// Order among siblings.
    pub position: i32,
    /// URL segment.
    pub slug: String,
    /// The stored path, when the row has one. Only an orphan's is read: see
    /// [`path_of`].
    pub path: Option<String>,
}

/// One requested change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    /// The row to move.
    pub id: i64,
    /// Its new parent.
    pub parent_id: Option<i64>,
    /// Its new position.
    pub position: i32,
}

fn by_id(nodes: &[Node]) -> BTreeMap<i64, &Node> {
    nodes.iter().map(|n| (n.id, n)).collect()
}

/// The chain from `id` up to its root, `id` first. An orphan (an entry
/// whose parent is not in `nodes`: trashed, deleted or of another type)
/// ends the chain as a root, the way [`preorder`] shows it. `None` when
/// `id` is not in `nodes` or the chain loops.
fn chain<'a>(map: &BTreeMap<i64, &'a Node>, id: i64) -> Option<Vec<&'a Node>> {
    let mut out = Vec::new();
    let mut cur = Some(id);
    while let Some(c) = cur {
        let Some(n) = map.get(&c) else {
            if out.is_empty() {
                return None;
            }
            break;
        };
        if out.len() >= map.len() {
            return None; // a loop
        }
        out.push(*n);
        cur = n.parent_id;
    }
    Some(out)
}

/// How deep `id` sits (a root entry, or an orphan, is 1), or `None` when
/// `id` is unknown or the chain loops.
#[must_use]
pub fn depth_of(nodes: &[Node], id: i64) -> Option<usize> {
    chain(&by_id(nodes), id).map(|c| c.len())
}

/// The part of an orphan's stored path above it (`a` for `a/b`): the
/// address of the parent it lost, which its subtree keeps. `None` for an
/// entry with a live parent or at the root, a stored path with no parent
/// part, or an import's throwaway path (`~<id>/<slug>`).
fn orphan_prefix<'a>(map: &BTreeMap<i64, &Node>, head: &'a Node) -> Option<&'a str> {
    let parent = head.parent_id?;
    if map.contains_key(&parent) {
        return None;
    }
    let (prefix, _) = head.path.as_deref()?.rsplit_once('/')?;
    (!prefix.is_empty() && !prefix.starts_with('~')).then_some(prefix)
}

/// Slugs from the root down to `id`, joined by `/`.
///
/// An orphan's subtree keeps the address of the parent it lost: the chain
/// ends at the orphan, and the part of the orphan's stored path above it is
/// put in front. A trashed group `a` leaves its child `b` at `a/b` and a
/// grandchild `c` at `a/b/c` however they are reordered or renamed, until
/// the orphan is moved under a live parent.
#[must_use]
pub fn path_of(nodes: &[Node], id: i64) -> Option<String> {
    let map = by_id(nodes);
    let chain = chain(&map, id)?;
    let prefix = chain.last().and_then(|head| orphan_prefix(&map, head));
    let mut parts: Vec<&str> = prefix.into_iter().collect();
    parts.extend(chain.iter().rev().map(|n| n.slug.as_str()));
    Some(parts.join("/"))
}

/// Refuses `id` having a parent that is not among `nodes`. A write checks
/// this for the entries it places, so it cannot create an orphan, while an
/// orphan already in the type (its parent trashed earlier) does not block
/// writes elsewhere.
///
/// # Errors
/// A message naming the entry and the missing parent.
pub fn check_parent(nodes: &[Node], id: i64) -> Result<(), String> {
    check_parent_in(&by_id(nodes), id)
}

fn check_parent_in(map: &BTreeMap<i64, &Node>, id: i64) -> Result<(), String> {
    match map.get(&id).and_then(|n| n.parent_id) {
        Some(p) if !map.contains_key(&p) => Err(format!(
            "entry {id} has a parent ({p}) that is not in this type"
        )),
        _ => Ok(()),
    }
}

/// `id` and everything below it.
#[must_use]
pub fn subtree_ids(nodes: &[Node], id: i64) -> Vec<i64> {
    let mut out = vec![id];
    let mut i = 0;
    while i < out.len() {
        let cur = out[i];
        for n in nodes.iter().filter(|n| n.parent_id == Some(cur)) {
            if !out.contains(&n.id) {
                out.push(n.id);
            }
        }
        i += 1;
    }
    out
}

fn walk_preorder(
    kids: &BTreeMap<Option<i64>, Vec<&Node>>,
    parent: Option<i64>,
    depth: usize,
    out: &mut Vec<(i64, usize)>,
) {
    for n in kids.get(&parent).into_iter().flatten() {
        out.push((n.id, depth));
        walk_preorder(kids, Some(n.id), depth + 1, out);
    }
}

/// Reading order: parents before children, siblings by (position, id).
#[must_use]
pub fn preorder(nodes: &[Node]) -> Vec<(i64, usize)> {
    let ids: BTreeSet<i64> = nodes.iter().map(|n| n.id).collect();
    let mut kids: BTreeMap<Option<i64>, Vec<&Node>> = BTreeMap::new();
    for n in nodes {
        // An orphan (its parent is not in the set) is shown as a root
        // rather than lost.
        let key = n.parent_id.filter(|p| ids.contains(p));
        kids.entry(key).or_default().push(n);
    }
    for v in kids.values_mut() {
        v.sort_by(|a, b| a.position.cmp(&b.position).then(a.id.cmp(&b.id)));
    }
    let mut out = Vec::with_capacity(nodes.len());
    walk_preorder(&kids, None, 1, &mut out);
    out
}

/// Checks a whole tree: no loops, nothing deeper than [`MAX_DEPTH`], no
/// reserved slug at the root, no two siblings sharing a slug.
///
/// An orphan counts as a root for depth and is not itself an error: a
/// type's tree with one old orphan would otherwise refuse every write to
/// any entry of it. Writes that place an entry check it with
/// [`check_parent`].
///
/// # Errors
/// A message naming the offending entry.
pub fn validate_tree(nodes: &[Node]) -> Result<(), String> {
    for n in nodes {
        match depth_of(nodes, n.id) {
            None => return Err(format!("entry {} would become its own ancestor", n.id)),
            Some(d) if d > MAX_DEPTH => {
                return Err(format!(
                    "entry {} would sit {d} levels deep; the limit is {MAX_DEPTH}",
                    n.id
                ))
            }
            Some(_) => {}
        }
        if n.parent_id.is_none() && RESERVED_ROOT_SLUGS.contains(&n.slug.as_str()) {
            return Err(format!(
                "the slug {:?} is reserved at the top level of this type",
                n.slug
            ));
        }
    }
    let mut seen: BTreeSet<(Option<i64>, &str)> = BTreeSet::new();
    for n in nodes {
        if !seen.insert((n.parent_id, n.slug.as_str())) {
            return Err(format!(
                "two entries under the same parent use the slug {:?}",
                n.slug
            ));
        }
    }
    Ok(())
}

/// Applies `moves` to a copy of `nodes` and validates the result.
///
/// # Errors
/// Too many moves, an unknown id, or an illegal final tree.
pub fn apply_moves(nodes: &[Node], moves: &[Move]) -> Result<Vec<Node>, String> {
    if moves.len() > MAX_MOVES {
        return Err(format!("at most {MAX_MOVES} moves per request"));
    }
    let mut out = nodes.to_vec();
    for m in moves {
        let node = out
            .iter_mut()
            .find(|n| n.id == m.id)
            .ok_or_else(|| format!("entry {} is not in this type", m.id))?;
        node.parent_id = m.parent_id;
        node.position = m.position;
    }
    let map = by_id(&out);
    for m in moves {
        check_parent_in(&map, m.id)?;
    }
    validate_tree(&out)?;
    Ok(out)
}

/// The paths that changed between two states of one tree: `(id, old, new)`.
#[must_use]
pub fn changed_paths(before: &[Node], after: &[Node]) -> Vec<(i64, String, String)> {
    after
        .iter()
        .filter_map(|n| {
            let old = path_of(before, n.id)?;
            let new = path_of(after, n.id)?;
            (old != new).then_some((n.id, old, new))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(id: i64, parent: Option<i64>, pos: i32, slug: &str) -> Node {
        Node {
            id,
            parent_id: parent,
            position: pos,
            slug: slug.to_owned(),
            path: None,
        }
    }

    /// `n` with a stored path.
    fn at(node: Node, path: &str) -> Node {
        Node {
            path: Some(path.to_owned()),
            ..node
        }
    }

    fn sample() -> Vec<Node> {
        vec![
            n(1, None, 20, "guides"),
            n(2, Some(1), 10, "install"),
            n(3, Some(2), 10, "linux"),
            n(4, None, 10, "intro"),
        ]
    }

    #[test]
    fn paths_depths_and_subtrees() {
        let t = sample();
        assert_eq!(path_of(&t, 3).as_deref(), Some("guides/install/linux"));
        assert_eq!(depth_of(&t, 3), Some(3));
        assert_eq!(depth_of(&t, 4), Some(1));
        let mut sub = subtree_ids(&t, 1);
        sub.sort_unstable();
        assert_eq!(sub, [1, 2, 3]);
    }

    #[test]
    fn preorder_follows_position_then_id_and_keeps_orphans() {
        let mut t = sample();
        t.push(n(9, Some(404), 5, "orphan"));
        let order = preorder(&t);
        assert_eq!(order, [(9, 1), (4, 1), (1, 1), (2, 2), (3, 3)]);
    }

    #[test]
    fn an_old_orphan_does_not_block_other_writes() {
        let mut t = sample();
        // 9's parent (404) was trashed: not among the type's nodes.
        t.push(n(9, Some(404), 5, "orphan"));
        t.push(n(10, Some(9), 5, "child"));
        validate_tree(&t).unwrap();
        assert_eq!(depth_of(&t, 9), Some(1));
        assert_eq!(depth_of(&t, 10), Some(2));
        assert_eq!(path_of(&t, 10).as_deref(), Some("orphan/child"));
        let moved = apply_moves(
            &t,
            &[Move {
                id: 4,
                parent_id: Some(1),
                position: 30,
            }],
        )
        .unwrap();
        assert_eq!(path_of(&moved, 4).as_deref(), Some("guides/intro"));
        // Moving the orphan itself to a live parent repairs it.
        let repaired = apply_moves(
            &t,
            &[Move {
                id: 9,
                parent_id: Some(1),
                position: 30,
            }],
        )
        .unwrap();
        assert_eq!(
            path_of(&repaired, 10).as_deref(),
            Some("guides/orphan/child")
        );
        check_parent(&t, 4).unwrap();
        assert!(check_parent(&t, 9).unwrap_err().contains("(404)"));
    }

    /// `sample()` plus group `a` (404, trashed: not in the set), its child
    /// 9 `b` stored at `a/b` and grandchild 10 `c` at `a/b/c`.
    fn with_orphans() -> Vec<Node> {
        let mut t = sample();
        t.push(at(n(9, Some(404), 5, "b"), "a/b"));
        t.push(at(n(10, Some(9), 5, "c"), "a/b/c"));
        t
    }

    #[test]
    fn an_orphan_subtree_keeps_its_stored_prefix() {
        let t = with_orphans();
        assert_eq!(path_of(&t, 9).as_deref(), Some("a/b"));
        assert_eq!(path_of(&t, 10).as_deref(), Some("a/b/c"));
        // Every path recomputed from the links (an import's rebuild) is unchanged.
        for node in &t {
            let stored = node
                .path
                .clone()
                .unwrap_or_else(|| path_of(&t, node.id).unwrap());
            assert_eq!(path_of(&t, node.id), Some(stored), "entry {}", node.id);
        }
    }

    #[test]
    fn reordering_inside_an_orphan_subtree_moves_no_url() {
        let t = with_orphans();
        let after = apply_moves(
            &t,
            &[Move {
                id: 10,
                parent_id: Some(9),
                position: 99,
            }],
        )
        .unwrap();
        assert_eq!(path_of(&after, 10).as_deref(), Some("a/b/c"));
        assert!(changed_paths(&t, &after).is_empty());
    }

    #[test]
    fn a_slug_edit_in_an_orphan_subtree_keeps_the_prefix() {
        let mut t = with_orphans();
        t[5].slug = "d".to_owned(); // entry 10
        assert_eq!(path_of(&t, 10).as_deref(), Some("a/b/d"));
        let mut t = with_orphans();
        t[4].slug = "b2".to_owned(); // entry 9, the orphan itself
        assert_eq!(path_of(&t, 9).as_deref(), Some("a/b2"));
        assert_eq!(path_of(&t, 10).as_deref(), Some("a/b2/c"));
    }

    #[test]
    fn an_entry_moved_under_an_orphan_joins_its_address() {
        let t = with_orphans();
        let after = apply_moves(
            &t,
            &[Move {
                id: 4,
                parent_id: Some(10),
                position: 0,
            }],
        )
        .unwrap();
        assert_eq!(path_of(&after, 4).as_deref(), Some("a/b/c/intro"));
    }

    #[test]
    fn re_attaching_an_orphan_computes_from_the_live_root() {
        let after = apply_moves(
            &with_orphans(),
            &[Move {
                id: 9,
                parent_id: None,
                position: 0,
            }],
        )
        .unwrap();
        assert_eq!(path_of(&after, 9).as_deref(), Some("b"));
        assert_eq!(path_of(&after, 10).as_deref(), Some("b/c"));
    }

    #[test]
    fn an_import_throwaway_path_is_not_a_prefix() {
        let t = vec![
            at(n(9, Some(404), 0, "b"), "~17/b"),
            at(n(10, None, 0, "x"), "x"),
        ];
        assert_eq!(path_of(&t, 9).as_deref(), Some("b"));
        let t = vec![at(n(9, Some(404), 0, "b"), "b")];
        assert_eq!(path_of(&t, 9).as_deref(), Some("b"));
    }

    #[test]
    fn a_write_cannot_create_an_orphan() {
        let err = apply_moves(
            &sample(),
            &[Move {
                id: 2,
                parent_id: Some(404),
                position: 0,
            }],
        )
        .unwrap_err();
        assert_eq!(err, "entry 2 has a parent (404) that is not in this type");
    }

    #[test]
    fn a_loop_is_caught() {
        let t = vec![n(1, Some(2), 0, "a"), n(2, Some(1), 0, "b")];
        assert!(depth_of(&t, 1).is_none());
        assert!(validate_tree(&t).unwrap_err().contains("own ancestor"));
    }

    #[test]
    fn moving_under_a_descendant_is_refused() {
        let err = apply_moves(
            &sample(),
            &[Move {
                id: 1,
                parent_id: Some(3),
                position: 0,
            }],
        )
        .unwrap_err();
        assert!(err.contains("own ancestor"), "{err}");
    }

    #[test]
    fn depth_is_limited() {
        let mut t = vec![n(1, None, 0, "a")];
        for i in 2..=8 {
            t.push(n(i, Some(i - 1), 0, "x"));
        }
        assert!(validate_tree(&t).is_ok());
        t.push(n(9, Some(8), 0, "too-deep"));
        assert!(validate_tree(&t).unwrap_err().contains("9 levels"));
    }

    #[test]
    fn sibling_slugs_must_differ_but_cousins_may_repeat() {
        let t = vec![
            n(1, None, 0, "a"),
            n(2, None, 1, "b"),
            n(3, Some(1), 0, "x"),
            n(4, Some(2), 0, "x"),
        ];
        assert!(validate_tree(&t).is_ok());
        let err = apply_moves(
            &t,
            &[Move {
                id: 4,
                parent_id: Some(1),
                position: 1,
            }],
        )
        .unwrap_err();
        assert!(err.contains("same parent"), "{err}");
    }

    #[test]
    fn reserved_slugs_are_refused_only_at_the_root() {
        assert!(validate_tree(&[n(1, None, 0, "search")])
            .unwrap_err()
            .contains("reserved"));
        assert!(validate_tree(&[n(1, None, 0, "a"), n(2, Some(1), 0, "search")]).is_ok());
    }

    #[test]
    fn changed_paths_cover_the_moved_subtree_only() {
        let before = sample();
        let after = apply_moves(
            &before,
            &[Move {
                id: 2,
                parent_id: None,
                position: 30,
            }],
        )
        .unwrap();
        let mut changed = changed_paths(&before, &after);
        changed.sort();
        assert_eq!(
            changed,
            [
                (2, "guides/install".to_owned(), "install".to_owned()),
                (
                    3,
                    "guides/install/linux".to_owned(),
                    "install/linux".to_owned()
                )
            ]
        );
        let reordered = apply_moves(
            &before,
            &[Move {
                id: 4,
                parent_id: None,
                position: 99,
            }],
        )
        .unwrap();
        assert!(changed_paths(&before, &reordered).is_empty());
    }

    #[test]
    fn unknown_ids_and_oversized_requests_are_refused() {
        assert!(apply_moves(
            &sample(),
            &[Move {
                id: 77,
                parent_id: None,
                position: 0
            }]
        )
        .unwrap_err()
        .contains("not in this type"));
        let many = vec![
            Move {
                id: 1,
                parent_id: None,
                position: 0
            };
            MAX_MOVES + 1
        ];
        assert!(apply_moves(&sample(), &many).is_err());
    }
}
