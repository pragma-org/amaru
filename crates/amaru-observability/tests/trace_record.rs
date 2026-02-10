// Copyright 2026 PRAGMA
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

use amaru_observability::define_schemas;
use amaru_observability::trace_record;

define_schemas! {
    test {
        example {
            /// Test event for validation
            ProcessEvent {
                required: [
                    event_type: &str,
                    timestamp: u64,
                ],
                optional: [
                    status: &str,
                    error_code: u32,
                    duration_ms: u64,
                    message: &str,
                    details: &str,
                ],
            }
        }
    }
}

#[test]
fn test_trace_record_single_field_with_schema() {
    // Single optional field recording with schema reference
    let span = tracing::info_span!("test_span");
    let _guard = span.enter();

    trace_record!(amaru::test::example::ProcessEvent, status = "active");
}

#[test]
fn test_trace_record_multiple_fields() {
    // Multiple optional field recordings with schema reference
    let span = tracing::info_span!("test_span");
    let _guard = span.enter();

    // Records multiple fields to the span, anchored by the schema constant
    trace_record!(
        amaru::test::example::ProcessEvent,
        status = "completed",
        error_code = 0,
        duration_ms = 42
    );
}

#[test]
fn test_trace_record_with_expressions() {
    // Record with computed values
    let span = tracing::info_span!("test_span");
    let _guard = span.enter();

    let status_str = String::from("running");
    let duration: u64 = 100;

    trace_record!(
        amaru::test::example::ProcessEvent,
        status = status_str.as_str(),
        duration_ms = duration
    );
}

#[test]
fn test_trace_record_with_format_macro() {
    // Record with format! macro (contains commas in the expression)
    let span = tracing::info_span!("test_span");
    let _guard = span.enter();

    let a = "hello";
    let b = "world";

    trace_record!(
        amaru::test::example::ProcessEvent,
        message = &format!("{}, {}", a, b)
    );
}

#[test]
fn test_trace_record_with_function_call_multiple_args() {
    // Record with function call containing multiple arguments
    let span = tracing::info_span!("test_span");
    let _guard = span.enter();

    fn join_strings(s1: &str, s2: &str, sep: &str) -> String {
        format!("{}{}{}", s1, sep, s2)
    }

    trace_record!(
        amaru::test::example::ProcessEvent,
        message = &join_strings("error", "code", ": ")
    );
}

#[test]
fn test_trace_record_with_tuple_construction() {
    // Record with tuple literal (contains commas)
    let span = tracing::info_span!("test_span");
    let _guard = span.enter();

    let status = "failed";
    let code = 500u32;

    trace_record!(
        amaru::test::example::ProcessEvent,
        status = status,
        error_code = code
    );
}

#[test]
fn test_trace_record_with_array_construction() {
    // Record with array literal (contains commas)
    let span = tracing::info_span!("test_span");
    let _guard = span.enter();

    let details_array = ["item1", "item2", "item3"];
    let details_str = details_array.join(", ");

    trace_record!(amaru::test::example::ProcessEvent, details = &details_str);
}

#[test]
fn test_trace_record_with_method_chain() {
    // Record with method chaining and multiple arguments
    let span = tracing::info_span!("test_span");
    let _guard = span.enter();

    let message = "  error  ".trim().to_uppercase();

    trace_record!(amaru::test::example::ProcessEvent, message = &message);
}

#[test]
fn test_trace_record_with_complex_nested_expr() {
    // Record with complex nested expression containing multiple commas
    let span = tracing::info_span!("test_span");
    let _guard = span.enter();

    let items = vec!["a", "b", "c"];
    let filtered = items.iter().filter(|s| s.len() > 0).collect::<Vec<_>>();

    trace_record!(
        amaru::test::example::ProcessEvent,
        details = &format!("Items: {} total", filtered.len())
    );
}
