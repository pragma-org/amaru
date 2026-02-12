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

/// Format the span trees to display them in a compact form
/// This supports the copying and pasting of actual values in the run_simulator_with_traces test.
pub(super) fn format_span_trees(values: &[Value]) -> String {
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
