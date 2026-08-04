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

use super::{FieldValue, TelemetryKind};

#[derive(Debug, Clone, PartialEq)]
pub struct TelemetryRecord {
    pub kind: TelemetryKind,
    pub level: Level,
    pub target: String,
    pub name: String,
    pub at: Instant,
    pub wall_time: SystemTime,
    pub fields: BTreeMap<String, FieldValue>,
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
