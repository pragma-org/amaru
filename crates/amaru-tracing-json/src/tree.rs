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

use serde_json::Value;
use std::fmt::{Debug, Display, Formatter};

/// This tree structure implements parent-child relationships between nodes of type `T`.
/// It is used to represent traces with parent-child relationships between spans.
#[derive(Clone, PartialEq, Eq)]
pub struct Tree<T> {
    pub value: T,
    pub children: Vec<Tree<T>>,
}

impl<T: Display> Display for Tree<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.pretty_print())
    }
}

impl<T: Debug> Debug for Tree<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.pretty_print_debug())
    }
}

impl<T> Tree<T> {
    pub fn pretty_print(&self) -> String
    where
        T: Display,
    {
        self.pretty_print_with(|h| h.to_string())
    }

    pub fn pretty_print_debug(&self) -> String
    where
        T: Debug,
    {
        self.pretty_print_with(|h| format!("{h:?}"))
    }
}

impl<T> Tree<T> {
    /// Create a `Tree` with a single value
    pub fn make_leaf(root: T) -> Tree<T> {
        Tree {
            value: root,
            children: vec![],
        }
    }

    /// Pretty print the tree using a custom formatting function for the node values
    pub fn pretty_print_with(&self, format: fn(&T) -> String) -> String {
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

impl Tree<Value> {
    pub fn as_json(&self) -> Value {
        if self.children.is_empty() {
            self.value.clone()
        } else {
            let children_json: Vec<Value> = self.children.iter().map(|c| c.as_json()).collect();
            let mut value = self.value.clone();
            if !children_json.is_empty()
                && let Some(map) = value.as_object_mut()
            {
                map.insert("children".to_string(), Value::Array(children_json));
            };
            value
        }
    }
}
