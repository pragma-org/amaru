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

use std::{
    collections::BTreeMap,
    fmt::{Debug, Display, Formatter},
};

use amaru_kernel::{HeaderHash, IsHeader};

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
        Tree { value: root.clone(), children: vec![] }
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
            by_parent.entry(header.parent()).or_default().push(header.clone());
        }

        // Find a root (no parent or missing parent in the set)
        if let Some(root) =
            headers.values().find(|header| header.parent().is_none_or(|parent| !headers.contains_key(&parent)))
        {
            // Recursively build the tree
            fn build<T: IsHeader + Clone>(root: T, by_parent: &BTreeMap<Option<HeaderHash>, Vec<T>>) -> Tree<T> {
                let mut tree = Tree::make_leaf(&root);
                if let Some(children) = by_parent.get(&Some(root.hash())) {
                    tree.children = children.iter().cloned().map(|c| build(c, by_parent)).collect();
                }
                tree
            }
            Some(build(root.clone(), &by_parent))
        } else {
            None
        }
    }
}

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

    pub fn fork_nodes(&self) -> Vec<H>
    where
        H: Clone,
    {
        let mut result = Vec::new();
        if self.children.len() > 1 {
            result.push(self.value.clone());
        }
        for child in &self.children {
            result.extend(child.fork_nodes());
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

    /// Return the branches of a `Tree`
    pub fn branches(&self) -> Vec<Vec<H>>
    where
        H: Clone,
    {
        if self.children.is_empty() {
            vec![vec![self.value.clone()]]
        } else {
            let mut result = vec![];
            for child in &self.children {
                for mut branch in child.branches() {
                    let mut new_branch = vec![self.value.clone()];
                    new_branch.append(&mut branch);
                    result.push(new_branch);
                }
            }
            result
        }
    }

    /// Get the last child of a `Tree` to modify it (if there is one).
    pub fn get_last_child_mut(&mut self) -> Option<&mut Tree<H>> {
        self.children.last_mut()
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

    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "slot": self.value.slot().to_string(),
            "hash": self.value.hash().to_string().chars().take(6).collect::<String>(),
            "children": self.children.iter().map(|child| { child.as_json() }).collect::<Vec<serde_json::Value>>()
        })
    }
}

#[cfg(test)]
mod tests {
    use proptest::{prop_assert_eq, proptest};

    use super::{
        super::{any_headers_tree, config_begin, generate_headers_chain, generate_headers_tree},
        *,
    };

    proptest! {
        #![proptest_config(config_begin().no_shrink().with_cases(1).end())]
        #[test]
        fn test_creation_from_map(tree in any_headers_tree(7)) {
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
        let tree = generate_headers_tree(45, 4);
        let expected = r#"
Header { hash: "bf0a0df5229c73815bfd5445ec687fcc7ada83c8d205227dc79d9815ddb29cda", slot: 1, height: 1, parent: None }
    └── Header { hash: "1e25b4ae5f1db94deb917061ef0632fec0b0ae3f7f229c51d5cb8ce12130da20", slot: 2, height: 2, parent: Some("bf0a0df5229c73815bfd5445ec687fcc7ada83c8d205227dc79d9815ddb29cda") }
        ├── Header { hash: "d295091521c7a2ee1834cf0427fdfc21c18117d4480684be3073eed4fa16a5c0", slot: 3, height: 3, parent: Some("1e25b4ae5f1db94deb917061ef0632fec0b0ae3f7f229c51d5cb8ce12130da20") }
        │   ├── Header { hash: "16f8c5dea1c70546782b45114ca2069711b778cff15a7fb413893dc838b3b144", slot: 4, height: 4, parent: Some("d295091521c7a2ee1834cf0427fdfc21c18117d4480684be3073eed4fa16a5c0") }
        │   └── Header { hash: "c2cd14b736e51a1563d486fd9a1b03d9d74895b0cd746cb0efced1571b3237b8", slot: 4, height: 4, parent: Some("d295091521c7a2ee1834cf0427fdfc21c18117d4480684be3073eed4fa16a5c0") }
        └── Header { hash: "c4e6395dbb415b9a16cee9146113d04619b10a8acd7368cf864a19b0b6724f12", slot: 3, height: 3, parent: Some("1e25b4ae5f1db94deb917061ef0632fec0b0ae3f7f229c51d5cb8ce12130da20") }
            └── Header { hash: "cb62f3d7c0295c384e1f11e7ef1cc9f8e4f9b7b000f9bf8219f9132961f85dac", slot: 4, height: 4, parent: Some("c4e6395dbb415b9a16cee9146113d04619b10a8acd7368cf864a19b0b6724f12") }
"#;
        assert_eq!(format!("\n{tree:?}"), expected, "\n{}{}", &tree.pretty_print_debug(), expected);
    }

    #[test]
    fn test_fork_nodes() {
        // 0
        // ├── 1
        // │   └── 3
        // │       ├── 4
        // │       └── 5
        // └── 2

        let mut root = Tree::make_leaf(&"0".to_string());
        let mut leaf1 = Tree::make_leaf(&"1".to_string());
        let leaf2 = Tree::make_leaf(&"2".to_string());
        let mut leaf3 = Tree::make_leaf(&"3".to_string());
        let leaf4 = Tree::make_leaf(&"4".to_string());
        let leaf5 = Tree::make_leaf(&"5".to_string());
        leaf3.children = vec![leaf4, leaf5];
        leaf1.children = vec![leaf3];
        root.children = vec![leaf1, leaf2];

        // 1 is not a fork because it has only one child
        assert_eq!(root.fork_nodes(), vec!["0".to_string(), "3".to_string()], "{}", root.pretty_print());
    }
}
