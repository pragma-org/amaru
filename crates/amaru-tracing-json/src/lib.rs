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

use crate::tree::Tree;
use assert_json_diff::assert_json_eq;
use serde_json as json;
use serde_json::{Value, to_string_pretty};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use tracing::Dispatch;
use tracing_subscriber::layer::SubscriberExt;

pub mod tree;

#[repr(transparent)]
#[derive(Clone, Default)]
pub struct JsonTraceCollector(Arc<RwLock<Vec<json::Value>>>);

impl JsonTraceCollector {
    fn insert(&self, value: json::Value) {
        if let Ok(mut lines) = self.0.write() {
            lines.push(value);
        }
    }

    pub fn flush(&self) -> Vec<json::Value> {
        match self.0.write() {
            Ok(mut traces) => {
                let lines = traces.clone();
                traces.clear();
                lines
            }
            // The RwLock can only get poisoned should the thread panic while pushing a new line
            // onto the stack. In case this happen, we'll likely be missing traces which should be
            // caught by assertions down the line anyway. So it is fine here to simply return the
            // 'possibly corrupted' data.
            Err(err) => err.into_inner().clone(),
        }
    }
}

#[derive(Default)]
struct JsonVisitor {
    fields: json::Map<String, json::Value>,
}

impl JsonVisitor {
    #[expect(clippy::unwrap_used)]
    fn add_field(&mut self, json_path: &str, value: json::Value) {
        let steps = json_path.split('.').collect::<Vec<_>>();

        if steps.is_empty() {
            return;
        }

        if steps.len() == 1 {
            self.fields.insert(json_path.to_string(), value);
            return;
        }

        // Safe because we just ensured steps is never empty
        let (root, children) = steps.split_first().unwrap();

        let mut current_value = self
            .fields
            .entry(root.to_string())
            .or_insert_with(|| json::json!({}));

        for &key in children.iter().take(children.len() - 1) {
            if !current_value.is_object() {
                *current_value = json::json!({});
            }

            // Safe because we just ensured current_value is an object
            let current_object = current_value.as_object_mut().unwrap();

            if !current_object.contains_key(key) {
                current_object.insert(key.to_string(), json::json!({}));
            }

            // Safe because we just inserted the key if it didn't exist
            current_value = current_object.get_mut(key).unwrap()
        }

        if let Some(last) = children.last() {
            if !current_value.is_object() {
                *current_value = json::json!({});
            }

            // Safe because we just ensured that current_value is always an object
            current_value
                .as_object_mut()
                .unwrap()
                .insert(last.to_string(), value);
        }
    }
}

macro_rules! record_t {
    ($title:ident, $ty:ty) => {
        fn $title(&mut self, field: &tracing::field::Field, value: $ty) {
            self.add_field(field.name(), json::json!(value));
        }
    };
}

impl tracing::field::Visit for JsonVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.add_field(field.name(), json::json!(format!("{:?}", value)))
    }

    record_t!(record_f64, f64);
    record_t!(record_i64, i64);
    record_t!(record_u64, u64);
    record_t!(record_i128, i128);
    record_t!(record_u128, u128);
    record_t!(record_bool, bool);
    record_t!(record_str, &str);

    fn record_bytes(&mut self, field: &tracing::field::Field, value: &[u8]) {
        self.add_field(field.name(), json::json!(hex::encode(value)));
    }

    fn record_error(
        &mut self,
        field: &tracing::field::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        self.add_field(field.name(), json::json!(format!("{}", value)))
    }
}

pub struct JsonLayer(JsonTraceCollector);

impl JsonLayer {
    pub fn new(collector: JsonTraceCollector) -> Self {
        Self(collector)
    }
}

impl<S> tracing_subscriber::Layer<S> for JsonLayer
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = JsonVisitor::default();
        attrs.record(&mut visitor);

        if let Some(span) = ctx.span(id) {
            // Store the fields in the span for later use
            let mut extensions = span.extensions_mut();
            extensions.insert(visitor.fields);
        }
    }

    fn on_enter(&self, id: &tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut span_json = json::json!({
                "id": Value::String(format!("{id:?}")),
                "name": span.name().to_string(),
                "type": "span".to_string(),
                "level": format!("{}", span.metadata().level()),
            });

            if let Some(parent) = span.parent() {
                span_json["parent_id"] = Value::String(format!("{:?}", parent.id()));
            };

            if let Some(fields) = span.extensions().get::<json::Map<String, Value>>() {
                for (key, value) in fields {
                    span_json[key] = value.clone();
                }
            }

            self.0.insert(span_json);
        }
    }

    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = JsonVisitor::default();
        event.record(&mut visitor);

        let name = visitor
            .fields
            .remove("message")
            .and_then(|value| value.as_str().map(|s| s.to_string()))
            .unwrap_or_default();

        let mut event_json = json::json!({
            "name": name,
            "type": "event",
            "level": format!("{}", event.metadata().level()),
        });

        for (key, value) in visitor.fields {
            event_json[key] = value.clone();
        }

        self.0.insert(event_json);
    }
}

/// Run a function `run` while collecting tracing data emitted during its execution
/// and assert that the collected data matches `expected`.
/// The collected data is stripped of ids before comparison and the result of `run` is returned.
pub fn assert_trace<F, R>(run: F, expected: Vec<Value>) -> R
where
    F: FnOnce() -> R,
{
    let (result, collected) = collect(run);
    let collected: Vec<_> = collected.into_iter().map(strip_ids).collect();

    if collected != expected {
        eprintln!(
            "collected traces:\n  - {}",
            collected
                .iter()
                .map(serde_json::to_string)
                .collect::<Result<Vec<_>, _>>()
                .map(|vec| vec.join("\n  - "))
                .unwrap_or_else(|e| format!("error: invalid JSON traces: {e}"))
        )
    }

    // use assert_json_eq! to get better error messages on mismatch
    assert_json_eq!(Value::Array(collected), Value::Array(expected));
    result
}

/// Run a function `run` while collecting tracing data emitted during its execution.
/// Assert that the collected spans form a list of trees.
/// `expected` is a list of expected root spans with their children.
/// The result of `run` is returned.
pub fn assert_spans_trees<F, R>(run: F, expected: Vec<Value>) -> R
where
    F: FnOnce() -> R,
{
    let (result, collected) = collect(run);
    let actual = as_trees(collected);

    if actual.len() != expected.len() {
        eprintln!(
            "actual spans tree count: {}, expected: {}",
            actual.len(),
            expected.len()
        );

        eprintln!("actual\n");
        for actual in actual.iter() {
            eprintln!("{}\n", to_string_pretty(actual).unwrap_or_default());
        }

        eprintln!("expected\n");
        for expected in expected.iter() {
            eprintln!("{}\n", to_string_pretty(expected).unwrap_or_default());
        }

        assert_eq!(
            actual.len(),
            expected.len(),
            "incorrect number of root spans"
        );
    }

    // make comparisons tree by tree
    for (actual, expected) in actual.iter().zip(expected.iter()) {
        eprintln!(
            "actual:\n {}\n\nexpected:\n {}\n",
            to_string_pretty(actual).unwrap_or_default(),
            to_string_pretty(expected).unwrap_or_default()
        );
        assert_json_eq!(actual, expected);
    }
    result
}

/// Convert a list of collected tracing data into a list of trees
/// Each tree represents a root span and its children.
///
/// The parent-child relationships are determined using the `id` and `parent_id` fields.
fn as_trees(collected: Vec<Value>) -> Vec<Value> {
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
fn is_span(item: &Value) -> bool {
    if let Some(t) = item.get("type")
        && t == "span"
    {
        true
    } else {
        false
    }
}

/// Collect tracing data emitted during the execution of `run` and also return the result of `run`.
fn collect<F, R>(run: F) -> (R, Vec<Value>)
where
    F: FnOnce() -> R,
{
    let collector = JsonTraceCollector::default();
    let layer = JsonLayer::new(collector.clone());
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = Dispatch::new(subscriber);
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let result = run();
    let collected = collector.flush();
    (result, collected)
}

/// Remove ids from collected data
fn strip_ids(mut value: Value) -> Value {
    if let Value::Object(ref mut map) = value {
        map.remove("id");
        map.remove("parent_id");
    }
    value
}

/// Keep only span name and children from collected data
/// and remove the `_span` suffix from span names
fn strip_span(mut value: Value) -> Value {
    if let Value::Object(ref mut map) = value {
        map.retain(|key, _value| ["name", "children"].contains(&(key.as_str())));
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tracing::{info, info_span};

    #[test]
    fn check_simple_tracing() {
        assert_eq!(
            assert_trace(
                || {
                    info_span!("foo").in_scope(|| {
                        info!(a = 1, "basic");
                        info!(a.foo = 1, a.bar = 2, "nested_fields");
                        "result"
                    })
                },
                vec![
                    json!({ "name": "foo", "type": "span", "level": "INFO" }),
                    json!({ "name": "basic", "a": 1, "type": "event", "level": "INFO" }),
                    json!({ "name": "nested_fields", "a": { "foo": 1, "bar": 2 }, "level": "INFO", "type": "event" }),
                ],
            ),
            "result"
        );
    }

    #[test]
    fn check_spans_tree_is_ok() {
        assert_spans_trees(
            || info_span!("foo").in_scope(|| info_span!("bar").in_scope(|| {})),
            vec![json!({
                "name": "foo",
                "children": [
                    json!({ "name": "bar"})]
            })],
        )
    }

    #[test]
    #[should_panic]
    fn check_spans_tree_is_ko() {
        assert_spans_trees(
            || info_span!("foo").in_scope(|| info_span!("bar").in_scope(|| {})),
            vec![json!({
                "name": "foo",
                "children": [
                    json!({ "name": "bar2"})]
            })],
        )
    }
}
