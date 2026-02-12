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

use crate::collect::{as_trees, collect, strip_ids_and_target};
use crate::trace_collect_config::TraceCollectConfig;
use assert_json_diff::assert_json_eq;
use pretty_assertions::Comparison;
use serde_json::Value;

/// Run a function `run` while collecting tracing data emitted during its execution
/// and assert that the collected data matches `expected`.
/// The collected data is stripped of ids before comparison and the result of `run` is returned.
pub fn assert_trace<F, R>(run: F, expected: Vec<Value>) -> R
where
    F: FnOnce() -> R,
{
    let (result, collected) = collect(run, TraceCollectConfig::default());
    let collected: Vec<_> = collected.into_iter().map(strip_ids_and_target).collect();

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
///
/// The configuration provides a way to control emitted spans and filter targets (included/excluded)
#[expect(clippy::panic)]
pub fn assert_spans_trees<F, R>(
    run: F,
    expected: Vec<Value>,
    trace_collect_config: TraceCollectConfig,
) -> R
where
    F: FnOnce() -> R,
{
    let (result, collected) = collect(run, trace_collect_config);
    let actual = as_trees(collected);

    if actual.len() != expected.len() {
        panic!(
            "incorrect number of root spans\n{}\n\nactual spans trees:\n{}",
            Comparison::new(&actual, &expected),
            format_span_trees(&actual)
        );
    }

    for (index, (actual_tree, expected_tree)) in actual.iter().zip(expected.iter()).enumerate() {
        if actual_tree != expected_tree {
            panic!(
                "span tree at index {index} differs\n{}\n\nactual spans trees:\n{}",
                Comparison::new(actual_tree, expected_tree),
                format_span_trees(&actual)
            );
        }
    }
    result
}

/// Format the span trees to display them in a compact form
fn format_span_trees(values: &[Value]) -> String {
    if values.is_empty() {
        return "vec![]".to_string();
    }

    let mut rendered = String::from("vec![\n");
    for value in values {
        let lines = format_value_compact_lines(value, 6);
        rendered.push_str("    json!(\n");
        for line in lines {
            rendered.push_str(&line);
            rendered.push('\n');
        }
        rendered.push_str("    ),\n");
    }
    rendered.push(']');
    rendered
}

fn format_value_compact_lines(value: &Value, indent: usize) -> Vec<String> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            vec![format!("{}{}", spaces(indent), format_scalar(value))]
        }
        Value::Array(items) => format_array_lines(items, indent),
        Value::Object(map) => format_object_lines(map, indent),
    }
}

fn format_array_lines(items: &[Value], indent: usize) -> Vec<String> {
    if items.is_empty() {
        return vec![format!("{}[]", spaces(indent))];
    }

    if items.iter().all(is_scalar) {
        let elements = items
            .iter()
            .map(format_scalar)
            .collect::<Vec<_>>()
            .join(", ");
        return vec![format!("{}[{}]", spaces(indent), elements)];
    }

    let mut lines = vec![format!("{}[", spaces(indent))];
    for (index, item) in items.iter().enumerate() {
        let mut item_lines = format_value_compact_lines(item, indent + 2);
        if index + 1 < items.len()
            && let Some(last) = item_lines.last_mut()
        {
            last.push(',');
        }
        lines.extend(item_lines);
    }
    lines.push(format!("{}]", spaces(indent)));
    lines
}

fn format_object_lines(map: &serde_json::Map<String, Value>, indent: usize) -> Vec<String> {
    if map.is_empty() {
        return vec![format!("{}{{}}", spaces(indent))];
    }

    if map.values().all(is_scalar) {
        let entries = map
            .iter()
            .map(|(key, value)| format!("\"{}\": {}", key, format_scalar(value)))
            .collect::<Vec<_>>()
            .join(", ");
        return vec![format!("{}{{ {} }}", spaces(indent), entries)];
    }

    let mut lines = vec![format!("{}{{", spaces(indent))];
    let last_index = map.len() - 1;
    for (index, (key, value)) in map.iter().enumerate() {
        let prefix = format!("{}\"{}\": ", spaces(indent + 2), key);
        let value_lines = format_value_compact_lines(value, indent + 2);

        if value_lines.len() == 1 {
            let mut line = prefix;
            line.push_str(value_lines[0].trim_start());
            if index != last_index {
                line.push(',');
            }
            lines.push(line);
        } else {
            let mut iter = value_lines.into_iter();
            let mut line = prefix;
            if let Some(first_line) = iter.next() {
                line.push_str(first_line.trim_start());
            }
            lines.push(line);

            let mut remaining: Vec<String> = iter.collect();
            if index != last_index {
                if let Some(last) = remaining.last_mut() {
                    last.push(',');
                } else if let Some(last_line) = lines.last_mut() {
                    last_line.push(',');
                }
            }
            lines.extend(remaining);
        }
    }
    lines.push(format!("{}{}", spaces(indent), "}"));
    lines
}

fn is_scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

fn format_scalar(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(boolean) => boolean.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(string) => {
            // make sure that the string is formatted with proper escaping
            serde_json::to_string(string).unwrap_or_else(|_| "\"<invalid>\"".to_string())
        }
        Value::Array(_) | Value::Object(_) => {
            unreachable!("format_scalar called on non-scalar value")
        }
    }
}

fn spaces(count: usize) -> String {
    " ".repeat(count)
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
            || {
                info_span!(target: "test", "foo")
                    .in_scope(|| info_span!(target: "test", "bar").in_scope(|| {}))
            },
            vec![json!({
                "target": "test",
                "name": "foo",
                "children": [
                    json!({ "name": "bar", "target": "test"})]
            })],
            TraceCollectConfig::default(),
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
            TraceCollectConfig::default(),
        )
    }
}
