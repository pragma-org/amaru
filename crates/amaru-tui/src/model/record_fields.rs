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

use std::time::Instant;

use crate::events::{FieldValue, TelemetryRecord};

#[derive(Debug, Clone, Copy)]
pub struct RecordFields<'record>(&'record TelemetryRecord);

impl<'record> From<&'record TelemetryRecord> for RecordFields<'record> {
    fn from(record: &'record TelemetryRecord) -> Self {
        Self(record)
    }
}

impl<'record> RecordFields<'record> {
    pub fn at(self) -> Instant {
        self.0.at
    }

    pub fn as_bool(self, name: &str) -> Option<bool> {
        self.field(name).and_then(FieldValue::as_bool)
    }

    pub fn as_f64(self, name: &str) -> Option<f64> {
        self.field(name).and_then(FieldValue::as_f64)
    }

    pub fn as_str(self, name: &str) -> Option<&'record str> {
        self.field(name).and_then(FieldValue::as_str)
    }

    pub fn as_u64(self, name: &str) -> Option<u64> {
        self.field(name).and_then(FieldValue::as_u64)
    }

    pub fn conn_id(self) -> Option<String> {
        self.as_str("conn_id").map(ToOwned::to_owned).or_else(|| self.as_u64("conn_id").map(|value| value.to_string()))
    }

    pub fn to_fields_string(self) -> String {
        self.0
            .fields
            .iter()
            .filter(|(name, _)| name.as_str() != "message")
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn field(self, name: &str) -> Option<&'record FieldValue> {
        self.0.field(name)
    }
}
