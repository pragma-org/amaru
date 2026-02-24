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

use std::sync::{Arc, RwLock};

use serde_json as json;
use serde_json::Value;

use crate::{json_visitor::JsonVisitor, trace_collect_config::TraceCollectConfig};

/// This `tracing` layer collects spans as JSON values.
///
/// It can be configured to only collect some targets by specifying 2 lists:
///
///  - `include_targets`: targets that must be collected.
///  - `exclude_targets`: targets that must *not* be collected.
///
/// The target matches an element of those lists if it starts with that element.
/// For example the target `amaru_consensus::select_chain` will be collected with the lists
///
///  - `include_targets: Some(vec!["amaru", "amaru_ledger"]))`
///  - `exclude_targets: Some(vec!["amaru_stores::rocksdb"]))`
///
pub struct JsonLayer {
    collector: JsonTraceCollector,
    include_targets: Option<Vec<String>>,
    exclude_targets: Option<Vec<String>>,
}

impl JsonLayer {
    /// Create a new `JsonLayer` with the given collector and configuration.
    pub fn new(collector: JsonTraceCollector, trace_collect_config: TraceCollectConfig) -> Self {
        Self {
            collector,
            include_targets: trace_collect_config.include_targets.clone(),
            exclude_targets: trace_collect_config.exclude_targets.clone(),
        }
    }

    /// Return true if the given target should be collected based on the include/exclude lists.
    pub fn has_target(&self, target: &str) -> bool {
        // First check if target is excluded
        if let Some(exclude_targets) = &self.exclude_targets
            && exclude_targets.iter().any(|t| target.starts_with(t))
        {
            return false;
        }

        // Then check if target is included (if include list is specified)
        if let Some(include_targets) = &self.include_targets {
            include_targets.iter().any(|t| target.starts_with(t))
        } else {
            true
        }
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
            if !self.has_target(span.metadata().target()) {
                return;
            }
            // Store the fields in the span for later use
            let mut extensions = span.extensions_mut();
            extensions.insert(visitor.fields);
        }
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = JsonVisitor::default();
        values.record(&mut visitor);

        if let Some(span) = ctx.span(id) {
            if !self.has_target(span.metadata().target()) {
                return;
            }
            let mut extensions = span.extensions_mut();
            if let Some(fields) = extensions.get_mut::<json::Map<String, Value>>() {
                // Merge the new fields into existing ones
                for (key, value) in visitor.fields {
                    fields.insert(key, value);
                }
            }
        }
    }

    fn on_enter(&self, _id: &tracing::span::Id, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        // We collect span data on close instead of enter to capture all recorded fields
    }

    fn on_close(&self, id: tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        if let Some(span) = ctx.span(&id) {
            if !self.has_target(span.metadata().target()) {
                return;
            }
            let mut span_json = json::json!({
                "target": span.metadata().target(),
            });

            // Merge user fields first so reserved keys always win
            if let Some(fields) = span.extensions().get::<json::Map<String, Value>>() {
                for (key, value) in fields {
                    span_json[key] = value.clone();
                }
            }

            // Set reserved keys after to prevent user-field clobbering
            span_json["id"] = Value::String(format!("{id:?}"));
            span_json["name"] = Value::String(span.name().to_string());
            span_json["type"] = Value::String("span".to_string());
            span_json["level"] = Value::String(format!("{}", span.metadata().level()));
            span_json["target"] = Value::String(span.metadata().target().to_string());

            // Walk up the parent chain to find the nearest collected ancestor
            // (i.e., one whose target passes our filter)
            let mut current_parent = span.parent();
            while let Some(parent) = current_parent {
                if self.has_target(parent.metadata().target()) {
                    span_json["parent_id"] = Value::String(format!("{:?}", parent.id()));
                    break;
                }
                current_parent = parent.parent();
            }

            self.collector.insert(span_json);
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        if !self.has_target(event.metadata().target()) {
            return;
        }

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

        self.collector.insert(event_json);
    }
}

/// The JsonTraceCollector is used to collect traces as JSON values.
/// It is referenced by the JsonLayer and used later on to flush the collected values to any function
/// that needs them.
#[repr(transparent)]
#[derive(Clone, Default)]
pub struct JsonTraceCollector(Arc<RwLock<Vec<Value>>>);

impl JsonTraceCollector {
    fn insert(&self, value: Value) {
        if let Ok(mut lines) = self.0.write() {
            lines.push(value);
        }
    }

    pub fn flush(&self) -> Vec<Value> {
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
