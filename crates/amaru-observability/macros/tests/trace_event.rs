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

//! Tests that verify runtime behavior of the trace_event! macro
//!
//! These tests verify:
//! - Correct target and event names are derived from the schema constant
//! - The requested level is used
//! - Fields are validated against the schema and rendered with Display/Debug

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use amaru_observability_macros::{define_local_schemas, trace_event};
use tracing::field::Visit;
use tracing_subscriber::layer::SubscriberExt;

define_local_schemas! {
    amaru {
        stores {
            accounts {
                /// Reset rewards counters for testing
                public RESET_MANY {
                    required credential: String
                    required reason: String
                    optional count: usize
                }
                /// Event without fields for testing
                public FLUSH {}
            }
        }
    }
}

#[derive(Debug, Clone)]
struct CapturedEvent {
    name: String,
    target: String,
    level: tracing::Level,
    values: BTreeMap<String, String>,
}

#[derive(Default)]
struct FieldValueCollector {
    values: BTreeMap<String, String>,
}

impl Visit for FieldValueCollector {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.values.insert(field.name().to_string(), format!("{:?}", value));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.values.insert(field.name().to_string(), value.to_string());
    }
}

struct EventCapturingLayer {
    captured_events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl<S> tracing_subscriber::Layer<S> for EventCapturingLayer
where
    S: tracing::Subscriber + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut collector = FieldValueCollector::default();
        event.record(&mut collector);

        self.captured_events.lock().unwrap().push(CapturedEvent {
            name: event.metadata().name().to_string(),
            target: event.metadata().target().to_string(),
            level: *event.metadata().level(),
            values: collector.values,
        });
    }
}

#[cfg(test)]
mod test {
    use tracing_subscriber::Registry;

    use super::*;

    #[test]
    fn test_event_target_name_level_and_fields() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(EventCapturingLayer { captured_events: captured.clone() });

        tracing::subscriber::with_default(subscriber, || {
            let credential = "stake-credential".to_string();
            trace_event!(ERROR, crate::amaru::stores::accounts::RESET_MANY, credential, reason = "unknown account");
        });

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1);

        assert_eq!(events[0].target, "amaru::stores");
        assert_eq!(events[0].name, "accounts.reset_many");
        assert_eq!(events[0].level, tracing::Level::ERROR);
        assert_eq!(events[0].values.get("message").map(String::as_str), Some("accounts.reset_many"));
        assert_eq!(events[0].values.get("credential").map(String::as_str), Some("stake-credential"));
        assert_eq!(events[0].values.get("reason").map(String::as_str), Some("unknown account"));
    }

    #[test]
    fn test_event_typed_and_shorthand_fields() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(EventCapturingLayer { captured_events: captured.clone() });

        tracing::subscriber::with_default(subscriber, || {
            let count = 42_usize;
            let credential = "stake-credential".to_string();
            trace_event!(
                INFO,
                crate::amaru::stores::accounts::RESET_MANY,
                credential,
                reason = "unknown account".to_string(),
                count
            );
        });

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].level, tracing::Level::INFO);
        assert_eq!(events[0].values.get("credential").map(String::as_str), Some("stake-credential"));
        assert_eq!(events[0].values.get("reason").map(String::as_str), Some("unknown account"));
        assert_eq!(events[0].values.get("count").map(String::as_str), Some("42"));
    }

    #[test]
    fn test_event_value_passthrough_records_dynamic_absence() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(EventCapturingLayer { captured_events: captured.clone() });

        tracing::subscriber::with_default(subscriber, || {
            let present: Option<usize> = Some(42);
            let absent: Option<String> = None;
            trace_event!(
                INFO,
                crate::amaru::stores::accounts::RESET_MANY,
                credential = @absent,
                reason = "unknown account".to_string(),
                count = @present
            );
        });

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].values.get("count").map(String::as_str), Some("42"));
        assert_eq!(events[0].values.get("credential"), None);
    }

    #[test]
    fn test_event_without_fields() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(EventCapturingLayer { captured_events: captured.clone() });

        tracing::subscriber::with_default(subscriber, || {
            trace_event!(WARN, crate::amaru::stores::accounts::FLUSH);
        });

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].level, tracing::Level::WARN);
        assert_eq!(events[0].values.get("message").map(String::as_str), Some("accounts.flush"));
    }
}
