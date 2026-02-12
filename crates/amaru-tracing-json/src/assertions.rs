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
use serde_json::{Value, to_string_pretty};

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
/// A list of `targets` can be provided to filter the collected traces by their target.
/// If `targets` is None, all traces are collected.
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
        if actual != expected {
            eprintln!(
                "actual:\n {}\n\nexpected:\n {}\n",
                to_string_pretty(actual).unwrap_or_default(),
                to_string_pretty(expected).unwrap_or_default()
            );
            assert_json_eq!(actual, expected);
        }
    }
    result
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
