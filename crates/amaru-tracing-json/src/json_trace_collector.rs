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

use serde_json as json;
use serde_json::Value;
use std::sync::{Arc, RwLock};

#[repr(transparent)]
#[derive(Clone, Default)]
pub struct JsonTraceCollector(Arc<RwLock<Vec<Value>>>);

impl JsonTraceCollector {
    pub(crate) fn insert(&self, value: Value) {
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

#[derive(Default)]
pub(crate) struct JsonVisitor {
    pub(crate) fields: json::Map<String, Value>,
}

impl JsonVisitor {
    #[expect(clippy::unwrap_used)]
    fn add_field(&mut self, json_path: &str, value: Value) {
        let steps = json_path.split('.').collect::<Vec<_>>();

        if steps.is_empty() {
            return;
        }

        if steps.len() == 1 {
            self.fields.insert(json_path.to_string(), value);
            return;
        }

        // Safe because we just ensured steps is never empty
        let (root, children) = steps.split_first().unwrap();

        let mut current_value = self
            .fields
            .entry(root.to_string())
            .or_insert_with(|| json::json!({}));

        for &key in children.iter().take(children.len() - 1) {
            if !current_value.is_object() {
                *current_value = json::json!({});
            }

            // Safe because we just ensured current_value is an object
            let current_object = current_value.as_object_mut().unwrap();

            if !current_object.contains_key(key) {
                current_object.insert(key.to_string(), json::json!({}));
            }

            // Safe because we just inserted the key if it didn't exist
            current_value = current_object.get_mut(key).unwrap()
        }

        if let Some(last) = children.last() {
            if !current_value.is_object() {
                *current_value = json::json!({});
            }

            // Safe because we just ensured that current_value is always an object
            current_value
                .as_object_mut()
                .unwrap()
                .insert(last.to_string(), value);
        }
    }
}

macro_rules! record_t {
    ($title:ident, $ty:ty) => {
        fn $title(&mut self, field: &tracing::field::Field, value: $ty) {
            self.add_field(field.name(), json::json!(value));
        }
    };
}

impl tracing::field::Visit for JsonVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.add_field(field.name(), json::json!(format!("{:?}", value)))
    }

    record_t!(record_f64, f64);
    record_t!(record_i64, i64);
    record_t!(record_u64, u64);
    record_t!(record_i128, i128);
    record_t!(record_u128, u128);
    record_t!(record_bool, bool);
    record_t!(record_str, &str);

    fn record_bytes(&mut self, field: &tracing::field::Field, value: &[u8]) {
        self.add_field(field.name(), json::json!(hex::encode(value)));
    }

    fn record_error(
        &mut self,
        field: &tracing::field::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        self.add_field(field.name(), json::json!(format!("{}", value)))
    }
}
