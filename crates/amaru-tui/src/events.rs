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
    fmt,
    time::{Instant, SystemTime},
};

use tracing::Level;

#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    String(String),
}

impl FieldValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            Self::String(value) => value.parse().ok(),
            Self::I64(_) | Self::U64(_) | Self::F64(_) => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::U64(value) => Some(*value),
            Self::I64(value) => (*value >= 0).then_some(*value as u64),
            Self::F64(value) => (*value >= 0.0).then_some(*value as u64),
            Self::String(value) => value.parse().ok(),
            Self::Bool(_) => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::F64(value) => Some(*value),
            Self::U64(value) => Some(*value as f64),
            Self::I64(value) => Some(*value as f64),
            Self::String(value) => value.parse().ok(),
            Self::Bool(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            Self::Bool(_) | Self::I64(_) | Self::U64(_) | Self::F64(_) => None,
        }
    }
}

impl fmt::Display for FieldValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(value) => write!(f, "{value}"),
            Self::I64(value) => write!(f, "{value}"),
            Self::U64(value) => write!(f, "{value}"),
            Self::F64(value) => write!(f, "{value:.3}"),
            Self::String(value) => f.write_str(value),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryKind {
    Event,
    SpanClose,
}

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
}

#[derive(Debug, Clone, PartialEq)]
pub struct SystemSample {
    pub at: Instant,
    pub cpu_percent: f64,
    pub process_memory_bytes: u64,
    pub rss_bytes: u64,
    pub virtual_bytes: u64,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Telemetry(TelemetryRecord),
}
