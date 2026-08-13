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

use std::{
    collections::BTreeMap,
    time::{Instant, SystemTime},
};

use amaru_observability::RecordFields;
use tracing::Level;

use super::FieldValue;

#[derive(Debug, Clone, PartialEq)]
pub struct TelemetryRecord {
    pub level: Level,
    pub target: String,
    pub name: String,
    pub at: Instant,
    pub wall_time: SystemTime,
    pub fields: BTreeMap<String, FieldValue>,
    /// Ancestor span names, outermost first, excluding the wrapping span.
    pub parents: Vec<String>,
    /// Name of the wrapping span (`None` when the record is outside any span).
    pub span_name: Option<String>,
    pub id: Option<u64>,
    pub parent_id: Option<u64>,
}

impl TelemetryRecord {
    pub fn conn_id(&self) -> Option<String> {
        self.str("conn_id").map(ToOwned::to_owned).or_else(|| self.u64("conn_id").map(|value| value.to_string()))
    }

    pub fn field(&self, name: &str) -> Option<&FieldValue> {
        self.fields.get(name)
    }

    pub fn message(&self) -> Option<&str> {
        self.field("message").and_then(FieldValue::as_str)
    }

    pub fn primary_label(&self) -> &str {
        match self.message() {
            Some(message) if message != self.name => message,
            _ => &self.name,
        }
    }

    /// Abbreviated ancestry (`e.t:g.r`), wrapping span excluded.
    pub fn parents_label(&self) -> Option<String> {
        if self.parents.is_empty() {
            None
        } else {
            Some(amaru_observability::format_abbreviated_span_path(self.parents.iter()))
        }
    }

    /// Console-style path: abbreviated ancestors plus the wrapping span's full name.
    pub fn span_path_label(&self) -> Option<String> {
        match (self.parents_label(), self.span_name.as_deref()) {
            (Some(mut ancestors), Some(wrap)) => {
                ancestors.push(':');
                ancestors.push_str(wrap);
                Some(ancestors)
            }
            (None, Some(wrap)) => Some(wrap.to_owned()),
            (Some(ancestors), None) => Some(ancestors),
            (None, None) => None,
        }
    }

    /// Label shown in the TUI log pane: path, then the event/span label.
    ///
    /// Span-close records (`id` set) omit a repeated wrapping-span name. Point
    /// events keep their label even when it matches the wrapping span name.
    pub fn log_label(&self) -> String {
        match self.span_path_label() {
            Some(path) if self.id.is_some() && self.span_name.as_deref() == Some(self.primary_label()) => path,
            Some(path) => format!("{} {}", path, self.primary_label()),
            None => self.primary_label().to_string(),
        }
    }

    pub fn to_fields_string(&self) -> String {
        self.fields
            .iter()
            .filter(|(name, _)| name.as_str() != "message")
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl RecordFields for TelemetryRecord {
    fn bool(&self, name: &str) -> Option<bool> {
        self.field(name).and_then(FieldValue::as_bool)
    }

    fn f64(&self, name: &str) -> Option<f64> {
        self.field(name).and_then(FieldValue::as_f64)
    }

    fn i64(&self, name: &str) -> Option<i64> {
        match self.field(name) {
            Some(FieldValue::I64(value)) => Some(*value),
            Some(FieldValue::U64(value)) => i64::try_from(*value).ok(),
            Some(FieldValue::F64(value)) => Some(*value as i64),
            Some(FieldValue::String(value)) => value.parse().ok(),
            Some(FieldValue::Bool(_)) | None => None,
        }
    }

    fn str(&self, name: &str) -> Option<&str> {
        self.field(name).and_then(FieldValue::as_str)
    }

    fn u64(&self, name: &str) -> Option<u64> {
        self.field(name).and_then(FieldValue::as_u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        name: &str,
        message: Option<&str>,
        span_name: Option<&str>,
        parents: &[&str],
        id: Option<u64>,
    ) -> TelemetryRecord {
        let mut fields = BTreeMap::new();
        if let Some(message) = message {
            fields.insert("message".into(), FieldValue::String(message.into()));
        }
        TelemetryRecord {
            level: Level::INFO,
            target: "amaru::ledger".into(),
            name: name.into(),
            at: Instant::now(),
            wall_time: SystemTime::UNIX_EPOCH,
            fields,
            parents: parents.iter().map(|parent| (*parent).to_string()).collect(),
            span_name: span_name.map(str::to_string),
            id,
            parent_id: None,
        }
    }

    #[test]
    fn log_label_keeps_point_event_when_message_matches_wrapping_span() {
        let event = record(
            "event",
            Some("governance.ratify_proposals"),
            Some("governance.ratify_proposals"),
            &["epoch.transition"],
            None,
        );
        assert_eq!(event.log_label(), "e.t:governance.ratify_proposals governance.ratify_proposals");
    }

    #[test]
    fn log_label_omits_repeated_name_on_span_close() {
        let close = record(
            "governance.ratify_proposals",
            None,
            Some("governance.ratify_proposals"),
            &["epoch.transition"],
            Some(1),
        );
        assert_eq!(close.log_label(), "e.t:governance.ratify_proposals");
    }
}
