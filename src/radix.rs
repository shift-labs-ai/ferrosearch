//! Compressed prefix tree (radix tree) used as the inverted index.
//!
//! This is the Rust counterpart of MiniSearch's `SearchableMap`. The original
//! stores nodes as JavaScript `Map`s where the empty-string `LEAF` key holds
//! the value and other keys hold child edges, all in insertion order. Each
//! node here keeps a single insertion-ordered `Vec` of slots — the leaf value
//! or an edge — so both original iteration orders can be replicated exactly:
//!
//! - `TreeIterator` (entries, prefix views) visits keys in *reverse*
//!   insertion order at every node.
//! - `fuzzySearch` visits keys in *forward* insertion order.

enum Slot<T> {
    Leaf(T),
    Child(Box<str>, Node<T>),
}

struct Node<T> {
    slots: Vec<Slot<T>>,
}

impl<T> Default for Node<T> {
    fn default() -> Self {
        Node { slots: Vec::new() }
    }
}

impl<T> Node<T> {
    fn value(&self) -> Option<&T> {
        self.slots.iter().find_map(|slot| match slot {
            Slot::Leaf(value) => Some(value),
            Slot::Child(..) => None,
        })
    }

    fn value_mut(&mut self) -> Option<&mut T> {
        self.slots.iter_mut().find_map(|slot| match slot {
            Slot::Leaf(value) => Some(value),
            Slot::Child(..) => None,
        })
    }

    fn child_position(&self, matches: impl Fn(&str) -> bool) -> Option<usize> {
        self.slots.iter().position(|slot| match slot {
            Slot::Leaf(_) => false,
            Slot::Child(edge, _) => matches(edge),
        })
    }

    fn child_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| matches!(slot, Slot::Child(..)))
            .count()
    }
}

pub struct RadixTree<T> {
    root: Node<T>,
    len: usize,
}

/// Length in bytes of the longest common prefix of `a` and `b`, always on a
/// character boundary.
fn common_prefix_len(a: &str, b: &str) -> usize {
    let mut len = 0;
    for (ca, cb) in a.chars().zip(b.chars()) {
        if ca != cb {
            break;
        }
        len += ca.len_utf8();
    }
    len
}

impl<T> RadixTree<T> {
    pub fn new() -> Self {
        RadixTree {
            root: Node::default(),
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn clear(&mut self) {
        self.root = Node::default();
        self.len = 0;
    }

    fn lookup(&self, key: &str) -> Option<&Node<T>> {
        let mut node = &self.root;
        let mut rest = key;
        while !rest.is_empty() {
            let position = node.child_position(|edge| rest.starts_with(edge))?;
            let Slot::Child(edge, child) = &node.slots[position] else {
                unreachable!("child_position only returns child slots");
            };
            rest = &rest[edge.len()..];
            node = child;
        }
        Some(node)
    }

    pub fn get(&self, key: &str) -> Option<&T> {
        self.lookup(key).and_then(Node::value)
    }

    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut T> {
        let mut node = &mut self.root;
        let mut rest = key;
        while !rest.is_empty() {
            let position = node.child_position(|edge| rest.starts_with(edge))?;
            let Slot::Child(edge, child) = &mut node.slots[position] else {
                unreachable!("child_position only returns child slots");
            };
            rest = &rest[edge.len()..];
            node = child;
        }
        node.value_mut()
    }

    /// Returns a mutable reference to the value at `key`, inserting the value
    /// produced by `default` if the key is absent. Counterpart of
    /// `SearchableMap.fetch`.
    pub fn fetch_with(&mut self, key: &str, default: impl FnOnce() -> T) -> &mut T {
        let node = Self::create_path(&mut self.root, key);
        if node.value().is_none() {
            node.slots.push(Slot::Leaf(default()));
            self.len += 1;
        }
        node.value_mut().expect("value was just ensured")
    }

    pub fn insert(&mut self, key: &str, value: T) {
        let node = Self::create_path(&mut self.root, key);
        match node.value_mut() {
            Some(existing) => *existing = value,
            None => {
                node.slots.push(Slot::Leaf(value));
                self.len += 1;
            }
        }
    }

    /// Walks (and builds) the path for `key`, splitting edges on partial
    /// matches, and returns the node at the end of the path. Ported from
    /// MiniSearch's `createPath`, which is on the hot path for indexing.
    /// Edge splits append the new edge and remove the old one, replicating
    /// the original's `Map` delete-and-set ordering.
    fn create_path<'a>(mut node: &'a mut Node<T>, key: &str) -> &'a mut Node<T> {
        let mut pos = 0;
        while pos < key.len() {
            let rest = &key[pos..];
            let first = rest.chars().next().expect("rest is non-empty");

            let candidate = node.child_position(|edge| edge.starts_with(first));

            let Some(position) = candidate else {
                node.slots.push(Slot::Child(rest.into(), Node::default()));
                let Some(Slot::Child(_, child)) = node.slots.last_mut() else {
                    unreachable!("slot was just pushed");
                };
                return child;
            };

            let Slot::Child(edge, _) = &node.slots[position] else {
                unreachable!("child_position only returns child slots");
            };
            let common = common_prefix_len(rest, edge);
            let full_match = common == edge.len();

            if full_match {
                // The existing edge is fully contained in the key: descend.
                let Slot::Child(_, child) = &mut node.slots[position] else {
                    unreachable!("checked above");
                };
                node = child;
            } else {
                // Partial match: split the edge with an intermediate node
                // holding the existing subtree under the non-matching suffix.
                let Slot::Child(old_edge, old_child) = node.slots.remove(position) else {
                    unreachable!("checked above");
                };
                let mut intermediate = Node::default();
                intermediate
                    .slots
                    .push(Slot::Child(old_edge[common..].into(), old_child));
                node.slots
                    .push(Slot::Child(rest[..common].into(), intermediate));
                let Some(Slot::Child(_, child)) = node.slots.last_mut() else {
                    unreachable!("slot was just pushed");
                };
                node = child;
            }
            pos += common;
        }
        node
    }

    /// Removes the value at `key`, collapsing now-redundant nodes, and
    /// returns whether a value was removed.
    pub fn remove(&mut self, key: &str) -> bool {
        let removed = Self::remove_rec(&mut self.root, key);
        if removed {
            self.len -= 1;
        }
        removed
    }

    fn remove_rec(node: &mut Node<T>, key: &str) -> bool {
        if key.is_empty() {
            let Some(position) = node
                .slots
                .iter()
                .position(|slot| matches!(slot, Slot::Leaf(_)))
            else {
                return false;
            };
            node.slots.remove(position);
            return true;
        }
        let Some(position) = node.child_position(|edge| key.starts_with(edge)) else {
            return false;
        };
        let Slot::Child(edge, child) = &mut node.slots[position] else {
            unreachable!("child_position only returns child slots");
        };
        let edge_len = edge.len();
        let removed = Self::remove_rec(child, &key[edge_len..]);
        if removed && child.value().is_none() {
            if child.slots.is_empty() {
                node.slots.remove(position);
            } else if child.child_count() == 1 && child.slots.len() == 1 {
                // Merge the child's single edge into this edge, appending the
                // merged edge like the original's delete-and-set.
                let Slot::Child(edge, mut collapsed) = node.slots.remove(position) else {
                    unreachable!("checked above");
                };
                let Some(Slot::Child(sub_edge, sub_node)) = collapsed.slots.pop() else {
                    unreachable!("single child slot checked above");
                };
                let mut merged = String::with_capacity(edge.len() + sub_edge.len());
                merged.push_str(&edge);
                merged.push_str(&sub_edge);
                node.slots
                    .push(Slot::Child(merged.into_boxed_str(), sub_node));
            }
        }
        removed
    }

    /// Depth-first traversal over all entries, yielding full keys in the
    /// order of the original `TreeIterator`: reverse insertion order at each
    /// node.
    pub fn for_each(&self, f: &mut impl FnMut(&str, &T)) {
        let mut key = String::new();
        Self::dfs(&self.root, &mut key, f);
    }

    fn dfs<'a>(node: &'a Node<T>, key: &mut String, f: &mut impl FnMut(&str, &'a T)) {
        for slot in node.slots.iter().rev() {
            match slot {
                Slot::Leaf(value) => f(key, value),
                Slot::Child(edge, child) => {
                    key.push_str(edge);
                    Self::dfs(child, key, f);
                    key.truncate(key.len() - edge.len());
                }
            }
        }
    }

    /// Depth-first traversal over all entries whose key starts with `prefix`,
    /// in `TreeIterator` order.
    pub fn for_each_prefix(&self, prefix: &str, f: &mut impl FnMut(&str, &T)) {
        let mut node = &self.root;
        let mut rest = prefix;
        'outer: while !rest.is_empty() {
            for slot in &node.slots {
                let Slot::Child(edge, child) = slot else {
                    continue;
                };
                if rest.starts_with(&**edge) {
                    node = child;
                    rest = &rest[edge.len()..];
                    continue 'outer;
                }
                if let Some(remainder) = edge.strip_prefix(rest) {
                    // The prefix ends inside this edge: every key in the
                    // subtree matches.
                    let mut key = String::from(prefix);
                    key.push_str(remainder);
                    Self::dfs(child, &mut key, f);
                    return;
                }
            }
            return;
        }
        let mut key = String::from(prefix);
        Self::dfs(node, &mut key, f);
    }

    /// Returns all `(key, edit distance)` pairs within `max_distance`
    /// (Levenshtein) of `query`, in the discovery order of the original
    /// `fuzzySearch`: forward insertion order at each node. A single
    /// Levenshtein matrix is maintained across the radix-tree traversal.
    ///
    /// Distances are measured in Unicode scalar values, not UTF-16 code
    /// units.
    pub fn fuzzy_get(&self, query: &str, max_distance: usize) -> Vec<(String, usize)> {
        let query: Vec<char> = query.chars().collect();
        // Number of columns in the Levenshtein matrix.
        let n = query.len() + 1;
        // Matching terms can never be longer than the query plus max_distance.
        let m = n + max_distance;

        let mut matrix = vec![(max_distance + 1) as u16; m * n];
        for (j, cell) in matrix.iter_mut().enumerate().take(n) {
            *cell = j as u16;
        }
        for i in 1..m {
            matrix[i * n] = i as u16;
        }

        let mut results = Vec::new();
        let mut prefix = String::new();
        Self::fuzzy_rec(
            &self.root,
            &query,
            max_distance,
            &mut results,
            &mut matrix,
            1,
            n,
            &mut prefix,
        );
        results
    }

    #[allow(clippy::too_many_arguments)]
    fn fuzzy_rec(
        node: &Node<T>,
        query: &[char],
        max_distance: usize,
        results: &mut Vec<(String, usize)>,
        matrix: &mut [u16],
        m: usize,
        n: usize,
        prefix: &mut String,
    ) {
        let offset = m * n;

        'slot: for slot in &node.slots {
            let (edge, child) = match slot {
                Slot::Leaf(_) => {
                    // A leaf: record the key if the edit distance is
                    // acceptable.
                    let distance = matrix[offset - 1] as usize;
                    if distance <= max_distance {
                        results.push((prefix.clone(), distance));
                    }
                    continue;
                }
                Slot::Child(edge, child) => (edge, child),
            };

            // Update the Levenshtein matrix for every character in the edge,
            // pruning as soon as the minimum distance in a row exceeds the
            // maximum.
            let mut i = m;
            for ch in edge.chars() {
                // Terms longer than the query plus max_distance can never be
                // within the maximum edit distance.
                if i * n >= matrix.len() {
                    continue 'slot;
                }
                let this_row = n * i;
                let prev_row = this_row - n;
                let mut min_distance = matrix[this_row];

                let jmin = i.saturating_sub(max_distance + 1);
                let jmax = (n - 1).min(i + max_distance);

                for j in jmin..jmax {
                    let different = ch != query[j];
                    let replace = matrix[prev_row + j] + u16::from(different);
                    let delete = matrix[prev_row + j + 1] + 1;
                    let insert = matrix[this_row + j] + 1;
                    let distance = replace.min(delete).min(insert);
                    matrix[this_row + j + 1] = distance;
                    if distance < min_distance {
                        min_distance = distance;
                    }
                }

                // Distance can never decrease with more characters: prune.
                if min_distance as usize > max_distance {
                    continue 'slot;
                }
                i += 1;
            }

            let len = prefix.len();
            prefix.push_str(edge);
            Self::fuzzy_rec(child, query, max_distance, results, matrix, i, n, prefix);
            prefix.truncate(len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_with(entries: &[(&str, u32)]) -> RadixTree<u32> {
        let mut tree = RadixTree::new();
        for (key, value) in entries {
            tree.insert(key, *value);
        }
        tree
    }

    #[test]
    fn insert_get_and_split_edges() {
        let tree = tree_with(&[
            ("unicorn", 1),
            ("universe", 2),
            ("university", 3),
            ("unique", 4),
        ]);
        assert_eq!(tree.len(), 4);
        assert_eq!(tree.get("unicorn"), Some(&1));
        assert_eq!(tree.get("universe"), Some(&2));
        assert_eq!(tree.get("university"), Some(&3));
        assert_eq!(tree.get("unique"), Some(&4));
        assert_eq!(tree.get("uni"), None);
        assert_eq!(tree.get("universities"), None);
    }

    #[test]
    fn remove_merges_nodes() {
        let mut tree = tree_with(&[("hello", 1), ("help", 2), ("he", 3)]);
        assert!(tree.remove("help"));
        assert!(!tree.remove("helper"));
        assert_eq!(tree.len(), 2);
        assert_eq!(tree.get("hello"), Some(&1));
        assert_eq!(tree.get("he"), Some(&3));
    }

    #[test]
    fn prefix_iteration() {
        let tree = tree_with(&[("summer", 1), ("summary", 2), ("sunny", 3), ("winter", 4)]);
        let mut found = Vec::new();
        tree.for_each_prefix("sum", &mut |key, value| {
            found.push((key.to_string(), *value))
        });
        // TreeIterator order: reverse insertion order at each node.
        assert_eq!(found, vec![("summary".into(), 2), ("summer".into(), 1)]);

        let mut all = Vec::new();
        tree.for_each_prefix("", &mut |key, value| all.push((key.to_string(), *value)));
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn fuzzy_distances() {
        let tree = tree_with(&[("hello", 1), ("hell", 2), ("help", 3), ("ciao", 4)]);
        let mut results = tree.fuzzy_get("hallo", 2);
        results.sort();
        // "help" is at distance 3 from "hallo" and must not match.
        assert_eq!(
            results,
            vec![("hell".to_string(), 2), ("hello".to_string(), 1)]
        );
        assert!(tree.fuzzy_get("xyz", 1).is_empty());
    }
}
