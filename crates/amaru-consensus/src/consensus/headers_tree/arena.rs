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

use indextree::{Arena, Node, NodeId};
use std::collections::BTreeSet;
use std::fmt::Debug;
use std::iter::Filter;
use std::slice::Iter;

/// Walk through the list of all nodes that are younger than each tip
/// and delete them from the arena.
///
/// For example:
///
///  0 +- 1
///    +- 2 - 3 - 4
///    +- 5 - 6
///
///  peers = alice 2, bob 3, eve 5
///
/// Then, after trimming we get:
///
///  0 +- 1
///    +- 2 - 3
///
pub(super) fn trim_arena_unused_nodes<T>(arena: &mut Arena<T>, all_tips: BTreeSet<NodeId>) {
    // list of nodes to remove with their descendants
    let mut to_remove: BTreeSet<NodeId> = BTreeSet::new();

    // find any tip that is not included in another chain
    // i.e has the tip of another chain as a descendant of its children
    for node in all_tips.iter() {
        for children in node.children(arena) {
            let mut children_descendants = children.descendants(arena);
            if !children_descendants.any(|n| all_tips.contains(&n)) {
                to_remove.insert(children);
            }
        }
    }
    for n in to_remove {
        n.remove_subtree(arena);
    }
}

/// Return the list of nodes in the arena that haven't been removed
/// For those nodes it is safe to extract the value of type T with node.get()
pub(super) fn get_arena_active_nodes<T>(
    arena: &Arena<T>,
) -> Filter<Iter<'_, Node<T>>, fn(&&Node<T>) -> bool> {
    arena.iter().filter(|n| !n.is_removed())
}

/// Pretty-print the arena starting from the root of the tree
///
/// Shows a graphical representation of the internal structure of
/// the tree.
pub(super) fn pretty_print_root<T: Debug>(arena: &Arena<T>) -> String {
    // take the first active node and retrieve the root of the tree from there
    if let Some(root) = get_arena_root(arena) {
        format!("{:?}", root.debug_pretty_print(arena))
    } else {
        "<EMPTY ARENA>".to_string()
    }
}

pub(super) fn get_arena_root<T>(arena: &Arena<T>) -> Option<NodeId> {
    // take the first active node and retrieve the root of the tree from there
    get_arena_active_nodes(arena).next().and_then(|n| {
        arena
            .get_node_id(n)
            .and_then(|tip| tip.ancestors(arena).last())
    })
}
