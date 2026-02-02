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

use crate::headers_tree::{HeadersTree, HeadersTreeState, tree::Tree};
use amaru_kernel::{HeaderHash, IsHeader, utils::string::ListToString};
use std::fmt::{Debug, Display, Formatter};

/// A displayable version of HeadersTree, for Debug and Display implementations.
/// It contains a snapshot of both the in memory data for the headers tree but also
/// its state in the database: anchor and best chain.
#[derive(Clone)]
pub struct HeadersTreeDisplay<H> {
    tree: Option<Tree<H>>,
    tree_state: HeadersTreeState,
    anchor: HeaderHash,
    best_chain: HeaderHash,
    best_chains: Vec<HeaderHash>,
    best_length: usize,
}

impl<H: IsHeader + Debug + Clone + PartialEq + Eq + 'static> From<HeadersTree<H>>
    for HeadersTreeDisplay<H>
{
    fn from(value: HeadersTree<H>) -> Self {
        let tree_state = value.clone().into_tree_state();
        HeadersTreeDisplay {
            tree: value.to_tree(),
            tree_state,
            anchor: value.anchor(),
            best_chain: value.best_chain(),
            best_chains: value.best_peers_chains().into_iter().cloned().collect(),
            best_length: value.best_length(),
        }
    }
}

impl<H: IsHeader + Clone + Debug + PartialEq + Eq + 'static> Debug for HeadersTreeDisplay<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.format(f, |tip| format!("{tip:?}"))
    }
}

impl<H: IsHeader + Debug + Clone + Display + PartialEq + Eq + 'static> Display
    for HeadersTreeDisplay<H>
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.format(f, |tip| format!("{tip}"))
    }
}

impl<H: IsHeader + Debug + Clone + PartialEq + Eq + 'static> HeadersTreeDisplay<H> {
    /// Common function to either format for Debug or Display
    pub fn format(
        &self,
        f: &mut Formatter<'_>,
        header_to_string: fn(&H) -> String,
    ) -> std::fmt::Result {
        f.write_str("HeadersTree {\n")?;
        if let Some(tree) = &self.tree {
            writeln!(
                f,
                "  headers:\n    {}",
                &tree.pretty_print_with(header_to_string)
            )?;
        };
        writeln!(f, "{}", &self.tree_state)?;
        writeln!(f, "  anchor: {}", &self.anchor)?;
        writeln!(f, "  best_chain: {}", &self.best_chain)?;
        writeln!(
            f,
            "  best_chains: [{}]",
            &self.best_chains.list_to_string(", ")
        )?;
        writeln!(f, "  best_length: {}", &self.best_length)?;
        f.write_str("}\n")
    }
}
