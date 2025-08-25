// Copyright 2025 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use amaru_kernel::HEADER_HASH_SIZE;
use amaru_ouroboros_traits::IsHeader;
use itertools::Itertools;
use pallas_crypto::hash::Hash;
use std::collections::BTreeMap;
use std::fmt::{Debug, Display, Formatter};

/// Type alias for a Header hash
type HeaderHash = Hash<HEADER_HASH_SIZE>;

/// This tree structure implements parent-child relationships between nodes of type `H`.
#[derive(Clone, PartialEq, Eq)]
pub struct Tree<H> {
    pub value: H,
    pub children: Vec<Tree<H>>,
}

impl<H: IsHeader + Display> Display for Tree<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.pretty_print())
    }
}

impl<H: IsHeader + Debug> Debug for Tree<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.pretty_print_debug())
    }
}

impl<H: Display> Tree<H> {
    pub fn pretty_print(&self) -> String {
        self.pretty_print_with(|h| h.to_string())
    }
}

impl<H: Debug> Tree<H> {
    pub fn pretty_print_debug(&self) -> String {
        self.pretty_print_with(|h| format!("{h:?}"))
    }
}

impl<H> Tree<H> {
    /// Create a `Tree` with a single value
    pub fn make_leaf(root: &H) -> Tree<H>
    where
        H: Clone,
    {
        Tree {
            value: root.clone(),
            children: vec![],
        }
    }

    /// Return the depth of a `Tree`
    pub fn depth(&self) -> usize {
        1 + self.children.iter().map(|c| c.depth()).max().unwrap_or(0)
    }

    /// Return the size of a `Tree`
    pub fn size(&self) -> usize {
        1 + self.children.iter().map(|c| c.size()).sum::<usize>()
    }

    /// Get the last child of a `Tree` to modify it (if there is one).
    pub fn get_last_child_mut(&mut self) -> Option<&mut Tree<H>> {
        let l = self.children.len();
        self.children.get_mut(l - 1)
    }

    /// Pretty print the tree using a custom formatting function for the node values
    pub fn pretty_print_with(&self, format: fn(&H) -> String) -> String {
        let mut out = String::new();
        self.pretty_print_with_prefix_and_format("", true, format, &mut out);
        out
    }

    fn pretty_print_with_prefix_and_format(
        &self,
        prefix: &str,
        is_last: bool,
        format: fn(&H) -> String,
        out: &mut String,
    ) {
        out.push_str(prefix);
        if !prefix.is_empty() {
            out.push_str(if is_last { "└── " } else { "├── " });
        }
        out.push_str(&format(&self.value));
        out.push('\n');

        let new_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });

        for (i, child) in self.children.iter().enumerate() {
            let last = i == self.children.len() - 1;
            child.pretty_print_with_prefix_and_format(&new_prefix, last, format, out);
        }
    }
}

impl<H: IsHeader + Clone + PartialEq + Eq> Tree<H> {
    /// Add a child to a specific parent in the tree
    pub fn add(&mut self, parent_hash: HeaderHash, new: &H) -> bool {
        if self.value.hash() == parent_hash {
            self.add_child(new);
            return true;
        } else {
            for child in self.children.iter_mut() {
                if child.add(parent_hash, new) {
                    return true;
                }
            }
        }
        false
    }

    /// Add a child to the current `Tree`
    pub fn add_child(&mut self, child: &H) -> &mut Tree<H> {
        let leaf = Tree::make_leaf(child);
        // Only add the child if it is not already present
        if !self.children.contains(&leaf) {
            self.children.push(leaf);
        }
        self
    }

    /// Return all the hashes in the tree, starting from the root and going depth-first
    pub fn hashes(&self) -> Vec<HeaderHash> {
        let mut hashes = vec![self.value.hash()];
        for child in &self.children {
            hashes.extend(child.hashes());
        }
        hashes
    }
}

impl<H: IsHeader + Clone + Debug + PartialEq + Eq + Default> Tree<H> {
    /// Create a `Tree` from a map of headers, indexed by their hash.
    pub fn from(headers: &BTreeMap<HeaderHash, H>) -> Self {
        if let Some(root) = headers.values().find(|h| {
            if let Some(parent) = h.parent() {
                !headers.contains_key(&parent)
            } else {
                true
            }
        }) {
            let mut headers_clone = headers.clone();
            headers_clone.remove(&root.hash());
            Self::make_tree_from(root.clone(), &mut headers_clone)
        } else {
            Tree::make_leaf(&Default::default())
        }
    }

    fn make_tree_from(root: H, headers: &mut BTreeMap<HeaderHash, H>) -> Tree<H> {
        let mut tree = Tree::make_leaf(&root);
        let children: Vec<Tree<H>> = headers
            .values()
            .filter(|n| n.parent() == Some(root.hash()))
            .cloned()
            .sorted_by_key(|h| h.hash())
            .map(|c| Self::make_tree_from(c, headers))
            .collect::<Vec<_>>();
        tree.children = children;
        tree
    }
}

#[cfg(test)]
impl<H: IsHeader + Clone> Tree<H> {
    pub fn to_map(&self) -> BTreeMap<HeaderHash, H> {
        let mut map = BTreeMap::new();
        self.to_map_recursive(&mut map);
        map
    }

    fn to_map_recursive(&self, map: &mut BTreeMap<HeaderHash, H>) {
        map.insert(self.value.hash(), self.value.clone());
        for child in &self.children {
            child.to_map_recursive(map);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::headers_tree::data_generation::{
        any_tree_of_headers, config_begin, generate_headers_chain, generate_test_header_tree,
    };
    use proptest::proptest;

    proptest! {
        #![proptest_config(config_begin().no_shrink().with_cases(1).with_seed(42).end())]
        #[test]
        fn test_creation_from_map(tree in any_tree_of_headers(7)) {
            let as_map = tree.to_map();
            let actual = Tree::from(&as_map);

            assert_eq!(actual.size(), tree.size());
            assert_eq!(actual.to_map(), as_map);
        }
    }

    #[test]
    fn test_add() {
        let mut headers = generate_headers_chain(5);
        let mut tree = Tree::make_leaf(&headers[0]);
        let tail = headers.drain(1..).collect::<Vec<_>>();
        let mut parent_hash = headers[0].hash();
        for header in tail {
            tree.add(parent_hash, &header);
            parent_hash = header.hash();
        }
        assert_eq!(tree.size(), 5);
    }

    #[test]
    fn test_pretty_print() {
        let tree = generate_test_header_tree(4, 42);
        let expected = r#"
TestHeader { hash: "a22427226377cc867d51ad3f130af08ad13451de7160efa2b23076fd782de967", slot: "1", parent: "None" }
    └── TestHeader { hash: "a8810f9ea39c3a6afb780859e8d8c7bc37b78e2f9b8d68d95e831ca1477e9b21", slot: "2", parent: "a22427226377cc867d51ad3f130af08ad13451de7160efa2b23076fd782de967" }
        └── TestHeader { hash: "1e3aba7a1f21d50037ae6bd23910a1ee09ac4e992e01938152f6d2dd43970164", slot: "3", parent: "a8810f9ea39c3a6afb780859e8d8c7bc37b78e2f9b8d68d95e831ca1477e9b21" }
            ├── TestHeader { hash: "1f5583a9c9c77da5bff5c542d0b985d832a8af76ab056b7fc34f9afa1bc08781", slot: "4", parent: "1e3aba7a1f21d50037ae6bd23910a1ee09ac4e992e01938152f6d2dd43970164" }
            └── TestHeader { hash: "da3fc7b517b61024fcad5acd80e4e585902180d1eb16fd37ca2f07a37c4b3903", slot: "5", parent: "1e3aba7a1f21d50037ae6bd23910a1ee09ac4e992e01938152f6d2dd43970164" }
"#;
        assert_eq!(
            format!("\n{tree:?}"),
            expected,
            "\n{}{}",
            &tree.pretty_print(),
            expected
        );
    }
}
