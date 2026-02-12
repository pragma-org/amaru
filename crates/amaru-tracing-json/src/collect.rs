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

use crate::json_layer::JsonLayer;
use crate::json_trace_collector::JsonTraceCollector;
use crate::trace_collect_config::TraceCollectConfig;
use crate::tree::Tree;
use serde_json::Value;
use std::collections::BTreeMap;
use tracing::Dispatch;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;

/// Collect tracing data emitted during the execution of `run` and also return the result of `run`.
pub fn collect<F, R>(run: F, trace_collect_config: TraceCollectConfig) -> (R, Vec<Value>)
where
    F: FnOnce() -> R,
{
    let collector = JsonTraceCollector::default();
    let layer = JsonLayer::new(collector.clone(), trace_collect_config.clone());
    let subscriber = tracing_subscriber::registry().with(layer).with(
        tracing_subscriber::fmt::Layer::default()
            .with_filter(trace_collect_config.env_filter.clone()),
    );
    let dispatch = Dispatch::new(subscriber);
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let result = run();
    let collected = collector.flush();
    (result, collected)
}

/// Convert a list of collected tracing data into a list of trees
/// Each tree represents a root span and its children.
///
/// The parent-child relationships are determined using the `id` and `parent_id` fields.
pub fn as_trees(collected: Vec<Value>) -> Vec<Value> {
    let mut parent_child: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut values: BTreeMap<String, Value> = BTreeMap::new();
    let mut roots: Vec<String> = vec![];
    for item in collected {
        if !is_span(&item) {
            continue;
        }

        if let Some(id) = item.get("id") {
            let id_string = id.as_str().map(|s| s.to_string()).unwrap_or_default();
            if let Some(parent_id) = item.get("parent_id") {
                let parent_id_string = parent_id
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let children = parent_child.entry(parent_id_string).or_default();
                if !children.contains(&id_string) {
                    children.push(id_string.clone());
                };
            } else if !roots.contains(&id_string) {
                roots.push(id_string.clone());
            }
            values.insert(id_string, item);
        }
    }

    fn make_tree(
        root: String,
        by_id: &BTreeMap<String, Vec<String>>,
        values: &BTreeMap<String, Value>,
    ) -> Tree<Value> {
        let value = values.get(&root).cloned().unwrap_or_default();
        let mut tree = Tree::make_leaf(strip_span(value));
        if let Some(children) = by_id.get(&root) {
            tree.children = children
                .iter()
                .cloned()
                .map(|c| make_tree(c, by_id, values))
                .collect();
        }
        tree
    }

    let mut trees: Vec<Value> = vec![];
    for root in roots {
        let tree = make_tree(root, &parent_child, &values);
        trees.push(tree.as_json());
    }
    trees
}

/// Return true if collected trace is a span
pub fn is_span(item: &Value) -> bool {
    if let Some(t) = item.get("type")
        && t == "span"
    {
        true
    } else {
        false
    }
}

/// Remove ids and target from collected data
pub fn strip_ids_and_target(mut value: Value) -> Value {
    if let Value::Object(ref mut map) = value {
        map.remove("id");
        map.remove("parent_id");
        map.remove("target");
    }
    value
}

/// Keep span fields from collected data, removing only internal fields like id, parent_id, type, level
pub fn strip_span(mut value: Value) -> Value {
    if let Value::Object(ref mut map) = value {
        map.retain(|key, _value| !["id", "parent_id", "type", "level"].contains(&(key.as_str())));
    }
    value
}
