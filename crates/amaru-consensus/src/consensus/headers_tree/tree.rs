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

use amaru_ouroboros_traits::{HeaderHash, IsHeader};
use std::collections::BTreeMap;
use std::fmt::{Debug, Display, Formatter};

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

    /// Pretty print the tree using a custom formatting function for the node values
    pub fn pretty_print_with(&self, format: fn(&H) -> String) -> String {
        let mut out = String::new();

        // recursive helper function to build the pretty-printed string
        fn pretty_print_with_prefix_and_format<T>(
            tree: &Tree<T>,
            prefix: &str,
            is_last: bool,
            format: fn(&T) -> String,
            out: &mut String,
        ) {
            out.push_str(prefix);
            if !prefix.is_empty() {
                out.push_str(if is_last { "└── " } else { "├── " });
            }
            out.push_str(&format(&tree.value));
            out.push('\n');

            let new_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });

            for (i, child) in tree.children.iter().enumerate() {
                let last = i == tree.children.len() - 1;
                pretty_print_with_prefix_and_format(child, &new_prefix, last, format, out);
            }
        }

        pretty_print_with_prefix_and_format(self, "", true, format, &mut out);
        out
    }
}

impl<H: IsHeader + Clone + Debug + PartialEq + Eq> Tree<H> {
    /// Create a `Tree` from a map of headers, indexed by their hash.
    pub fn from(headers: &BTreeMap<HeaderHash, H>) -> Option<Self> {
        // Build parent -> children index
        let mut by_parent: BTreeMap<Option<HeaderHash>, Vec<H>> = BTreeMap::new();
        for header in headers.values() {
            by_parent
                .entry(header.parent())
                .or_default()
                .push(header.clone());
        }

        // Find a root (no parent or missing parent in the set)
        if let Some(root) = headers.values().find(|header| {
            header
                .parent()
                .is_none_or(|parent| !headers.contains_key(&parent))
        }) {
            // Recursively build the tree
            fn build<T: IsHeader + Clone>(
                root: T,
                by_parent: &BTreeMap<Option<HeaderHash>, Vec<T>>,
            ) -> Tree<T> {
                let mut tree = Tree::make_leaf(&root);
                if let Some(children) = by_parent.get(&Some(root.hash())) {
                    tree.children = children
                        .iter()
                        .cloned()
                        .map(|c| build(c, by_parent))
                        .collect();
                }
                tree
            }
            Some(build(root.clone(), &by_parent))
        } else {
            None
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl<H> Tree<H> {
    /// Return the depth of a `Tree`
    pub fn depth(&self) -> usize {
        1 + self.children.iter().map(|c| c.depth()).max().unwrap_or(0)
    }

    /// Return the size of a `Tree`
    pub fn size(&self) -> usize {
        1 + self.children.iter().map(|c| c.size()).sum::<usize>()
    }

    /// Return all the nodes of a `Tree`
    pub fn nodes(&self) -> Vec<H>
    where
        H: Clone,
    {
        let mut result = vec![self.value.clone()];
        for child in &self.children {
            result.extend(child.nodes());
        }
        result
    }

    /// Return the leaves of a `Tree`
    pub fn leaves(&self) -> Vec<H>
    where
        H: Clone,
    {
        if self.children.is_empty() {
            vec![self.value.clone()]
        } else {
            self.children.iter().flat_map(|c| c.leaves()).collect()
        }
    }

    /// Get the last child of a `Tree` to modify it (if there is one).
    pub fn get_last_child_mut(&mut self) -> Option<&mut Tree<H>> {
        self.children.last_mut()
    }
}

#[cfg(any(test, feature = "test-utils"))]
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
        // Just check the value, not the whole subtree
        if !self.children.iter().any(|c| c.value == *child) {
            self.children.push(leaf);
        }
        self
    }

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
        Ratio, any_tree_of_headers, config_begin, generate_header_tree, generate_headers_chain,
    };
    use proptest::{prop_assert_eq, proptest};

    proptest! {
        #![proptest_config(config_begin().no_shrink().with_cases(1).end())]
        #[test]
        fn test_creation_from_map(tree in any_tree_of_headers(7, Ratio(1, 5))) {
            let as_map = tree.to_map();
            if let Some(actual) = Tree::from(&as_map) {
                prop_assert_eq!(actual.size(), tree.size());
                prop_assert_eq!(actual.to_map(), as_map);
            } else {
                assert!(as_map.is_empty())
            }
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
        let tree = generate_header_tree(4, 42, Ratio(1, 10));
        let expected = r#"
BlockHeader { hash: "ede0bf92248771ce3f7295de922779309a9835eea7a82d883b371bbbfef19585", slot: 1, parent: None }
    └── BlockHeader { hash: "ade5f0649f5039e91bfb933d3a95a6abd880aadcf17bda9463dbc11c5877d146", slot: 2, parent: Some("ede0bf92248771ce3f7295de922779309a9835eea7a82d883b371bbbfef19585") }
        └── BlockHeader { hash: "feeb21de183ead0e37b2d57767ed635049dbf4b1cc5fa39b48d7a55110a46b7b", slot: 3, parent: Some("ade5f0649f5039e91bfb933d3a95a6abd880aadcf17bda9463dbc11c5877d146") }
            └── BlockHeader { hash: "c5f2d7bb37e4a2d6e2646226b50b0ca576dc90296397a1cd73c2ad85629c855c", slot: 4, parent: Some("feeb21de183ead0e37b2d57767ed635049dbf4b1cc5fa39b48d7a55110a46b7b") }
"#;
        assert_eq!(
            format!("\n{tree:?}"),
            expected,
            "\n{}{}",
            &tree.pretty_print_debug(),
            expected
        );
    }
}
