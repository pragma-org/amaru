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

pub mod registry;
// Include the schemas module which uses define_schemas! to generate
// the amaru module with all schema constants and validation macros
mod schemas;
mod trace_context;

// Re-export the macros for convenient use
pub use amaru_observability_macros::{define_schemas, trace_record, trace_span};
pub use opentelemetry;
pub use schemas::*;
pub use trace_context::TraceContext;
pub use tracing;
pub use tracing_opentelemetry;

#[doc(hidden)]
#[macro_export]
macro_rules! __amaru_event_missing_metadata {
    ($macro_name:literal) => {
        compile_error!(concat!(
            "amaru_observability::",
            $macro_name,
            "! requires `target: ...` and a trailing message."
        ));
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __amaru_event {
    ($level:ident, $macro_name:literal, target: $target:expr, $message:literal, $($args:tt)+) => {
        $crate::tracing::$level!(target: $target, $message, $($args)+)
    };
    ($level:ident, $macro_name:literal, target: $target:expr, $message:literal $(,)?) => {
        $crate::tracing::$level!(target: $target, $message)
    };
    ($level:ident, $macro_name:literal, target: $target:expr, $($rest:tt)+) => {
        $crate::__amaru_event_fields!($level, $macro_name, target: $target, [] $($rest)+);
    };
    ($level:ident, $macro_name:literal, $($rest:tt)*) => {
        $crate::__amaru_event_missing_metadata!($macro_name);
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __amaru_event_fields {
    ($level:ident, $macro_name:literal, target: $target:expr, [$($fields:tt)+] , $message:literal, $($args:tt)+) => {
        $crate::tracing::$level!(target: $target, $($fields)+, $message, $($args)+)
    };
    ($level:ident, $macro_name:literal, target: $target:expr, [$($fields:tt)+] , $message:literal $(,)?) => {
        $crate::tracing::$level!(target: $target, $($fields)+, $message)
    };
    ($level:ident, $macro_name:literal, target: $target:expr, [$($fields:tt)*] $next:tt $($rest:tt)*) => {
        $crate::__amaru_event_fields!($level, $macro_name, target: $target, [$($fields)* $next] $($rest)*);
    };
    ($level:ident, $macro_name:literal, target: $target:expr, [$($fields:tt)*]) => {
        $crate::__amaru_event_missing_metadata!($macro_name);
    };
}

#[macro_export]
macro_rules! trace {
    ($($rest:tt)*) => {
        $crate::__amaru_event!(trace, "trace", $($rest)*);
    };
}

#[macro_export]
macro_rules! debug {
    ($($rest:tt)*) => {
        $crate::__amaru_event!(debug, "debug", $($rest)*);
    };
}

#[macro_export]
macro_rules! info {
    ($($rest:tt)*) => {
        $crate::__amaru_event!(info, "info", $($rest)*);
    };
}

#[macro_export]
macro_rules! warn {
    ($($rest:tt)*) => {
        $crate::__amaru_event!(warn, "warn", $($rest)*);
    };
}

#[macro_export]
macro_rules! error {
    ($($rest:tt)*) => {
        $crate::__amaru_event!(error, "error", $($rest)*);
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
