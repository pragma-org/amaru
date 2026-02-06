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
