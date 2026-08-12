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

pub mod aliases;
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
pub mod telemetry_capture;
mod trace_context;

// Re-export the macros for convenient use
pub use amaru_observability_macros::{define_schemas, trace_event as __trace_event, trace_record, trace_span};
pub use field::{
    DecodedField, as_str_value, cbor_to_any_value, cbor_to_decoded_field, cbor_to_trace_value, encode_cbor,
};
pub use json_format::{CborJsonEventFormat, CborJsonFields, CborJsonSpanLayer, SpanJsonFields};
pub use layers::{CborAwareMakeVisitor, CborDiagVisitor, CborToStringVisit, console_field_formatter};
pub use opentelemetry;
pub use otel_log_bridge::CborOtelLogBridge;
pub use otel_trace_arrays::CborTraceArrayLayer;
pub use record_fields::RecordFields;
pub use schemas::*;
/// Re-export for schema macros that require `Serialize` on complex field types.
pub use serde;
pub use telemetry_capture::{FieldValue, TelemetryCaptureLayer, TelemetryRecord, subscribe_telemetry};
pub use trace_context::TraceContext;
pub use tracing;
pub use tracing_opentelemetry;

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
