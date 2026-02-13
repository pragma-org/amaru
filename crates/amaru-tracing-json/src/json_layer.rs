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

use crate::json_trace_collector::{JsonTraceCollector, JsonVisitor};
use crate::trace_collect_config::TraceCollectConfig;
use serde_json as json;
use serde_json::Value;

pub struct JsonLayer {
    collector: JsonTraceCollector,
    include_targets: Option<Vec<String>>,
    exclude_targets: Option<Vec<String>>,
}

impl JsonLayer {
    pub fn new(collector: JsonTraceCollector, trace_collect_config: TraceCollectConfig) -> Self {
        Self {
            collector,
            include_targets: trace_collect_config.include_targets.clone(),
            exclude_targets: trace_collect_config.exclude_targets.clone(),
        }
    }

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
        if let Some(span) = ctx.span(id) {
            if !self.has_target(span.metadata().target()) {
                return;
            }
            let mut visitor = JsonVisitor::default();
            attrs.record(&mut visitor);

            // Store the fields in the span for later use
            let mut extensions = span.extensions_mut();
            extensions.insert(visitor.fields);
        }
    }

    fn on_enter(&self, id: &tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            if !self.has_target(span.metadata().target()) {
                return;
            }

            let mut span_json = json::json!({
                "id": Value::String(format!("{id:?}")),
                "name": span.name().to_string(),
                "target": span.metadata().target().to_string(),
                "type": "span".to_string(),
                "level": format!("{}", span.metadata().level()),
            });

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

            if let Some(fields) = span.extensions().get::<json::Map<String, Value>>() {
                for (key, value) in fields {
                    span_json[key] = value.clone();
                }
            }

            self.collector.insert(span_json);
        }
    }

    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
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
