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

extern crate self as amaru_observability;

pub mod field;
pub mod json_format;
pub mod layers;
pub mod otel_log_bridge;
pub mod otel_trace_arrays;
mod record_fields;
pub mod registry;
// Include the schemas module which uses define_schemas! to generate
// the amaru module with all schema constants and validation macros
mod schemas;
mod span_duration;
pub mod span_encode;
pub mod telemetry_capture;
mod trace_context;

// Re-export the macros for convenient use
use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub use amaru_observability_macros::{define_schemas, trace_event as __trace_event, trace_record, trace_span};
pub use field::{
    DecodedField, TAG_FIELD_PREFIX, as_field_ref, as_str_value, cbor_to_any_value, cbor_to_decoded_field,
    cbor_to_trace_value, display_string_value, encode_cbor, is_tag_field_name,
};
pub use json_format::{CborJsonEventFormat, CborJsonFields, CborJsonSpanLayer, SpanJsonFields};
pub use layers::{
    CborAwareMakeVisitor, CborConsoleEventFormat, CborDiagVisitor, CborToStringVisit, HideTagFields,
    console_field_formatter,
};
pub use opentelemetry;
pub use otel_log_bridge::CborOtelLogBridge;
pub use otel_trace_arrays::{CborTraceArrayLayer, prepare_exported_attributes};
pub use record_fields::RecordFields;
/// Re-export for schema macros that require `Serialize` / `JsonSchema` on complex field types.
pub use schemars;
pub use schemas::*;
pub use serde;
pub use span_duration::{SpanDurationLayer, offer_span_elapsed};
pub use span_encode::{
    abbreviate_span_name, ancestor_span_names, format_abbreviated_span_path, write_abbreviated_span_name,
    write_abbreviated_span_path,
};
pub use telemetry_capture::{FieldValue, TelemetryCaptureLayer, TelemetryRecord, subscribe_telemetry};
pub use trace_context::{
    AcceptsParent, AttachedSpan, CarriedHeader, ChainChoice, ChainSyncProcess, FetchResume, NoParent, ParentContext,
    SchemaSpan, TraceContext,
};
pub use tracing::{self, Instrument};

static EMIT_PRIVATE_TRACES: AtomicBool = AtomicBool::new(false);

/// Emit private schemas even when this crate was not built for tests.
///
/// Stage call sites live in crates that are dependencies of a node test, so `cfg!(test)` there
/// is false. A node test that must see those spans calls this before the node runs.
pub fn enable_private_traces() {
    EMIT_PRIVATE_TRACES.store(true, Ordering::Relaxed);
}

/// Whether [`enable_private_traces`] is set for this process.
pub fn private_traces_enabled() -> bool {
    EMIT_PRIVATE_TRACES.load(Ordering::Relaxed)
}

static SPAN_DURATIONS: Mutex<BTreeMap<(String, String), u64>> = Mutex::new(BTreeMap::new());

/// Remember a span's elapsed microseconds, keyed by span name and header hash.
///
/// [`SpanDurationLayer`] records this from `on_close`, before a batch exporter runs.
pub fn note_span_duration(name: &str, header_hash: &str, micros: u64) {
    if let Ok(mut durations) = SPAN_DURATIONS.lock() {
        durations.insert((name.to_string(), header_hash.to_string()), micros);
    }
}

/// Take the elapsed time recorded by [`note_span_duration`].
pub fn take_span_duration(name: &str, header_hash: &str) -> Option<u64> {
    SPAN_DURATIONS.lock().ok().and_then(|mut durations| durations.remove(&(name.to_string(), header_hash.to_string())))
}
pub use tracing_opentelemetry;
pub use tracing_subscriber;

#[macro_export]
macro_rules! trace_event {
    ($($rest:tt)*) => {
        {
            #[allow(unused_imports)]
            use $crate::tracing;
            $crate::__trace_event!($($rest)*);
        }
    };
}

#[macro_export]
macro_rules! trace {
    ($($rest:tt)*) => {
        $crate::trace_event!(TRACE, $($rest)*);
    };
}

#[macro_export]
macro_rules! debug {
    ($($rest:tt)*) => {
        $crate::trace_event!(DEBUG, $($rest)*);
    };
}

#[macro_export]
macro_rules! info {
    ($($rest:tt)*) => {
        $crate::trace_event!(INFO, $($rest)*);
    };
}

#[macro_export]
macro_rules! warn {
    ($($rest:tt)*) => {
        $crate::trace_event!(WARN, $($rest)*);
    };
}

#[macro_export]
macro_rules! error {
    ($($rest:tt)*) => {
        $crate::trace_event!(ERROR, $($rest)*);
    };
}

#[macro_export]
macro_rules! debug_span {
    ($($rest:tt)*) => {
        $crate::trace_span!(DEBUG, $($rest)*)
    };
}

#[macro_export]
macro_rules! info_span {
    ($($rest:tt)*) => {
        $crate::trace_span!(INFO, $($rest)*)
    };
}

#[macro_export]
macro_rules! debug_record {
    ($($rest:tt)*) => {
        $crate::trace_record!(DEBUG, $($rest)*)
    };
}

#[macro_export]
macro_rules! info_record {
    ($($rest:tt)*) => {
        $crate::trace_record!(INFO, $($rest)*)
    };
}

#[macro_export]
macro_rules! error_record {
    ($($rest:tt)*) => {
        $crate::trace_record!(ERROR, $($rest)*)
    };
}

#[macro_export]
macro_rules! warn_record {
    ($($rest:tt)*) => {
        $crate::trace_record!(WARN, $($rest)*)
    };
}
