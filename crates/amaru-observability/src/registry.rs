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

//! Runtime schema registry for introspection and JSON schema generation
//!
//! This module provides runtime access to schema definitions for debugging,
//! JSON schema generation, and other introspection needs.

use inventory;
use schemars::{JsonSchema, r#gen::SchemaSettings};
use serde_json::Value;

/// How a schema field is rendered onto the tracing wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldRender {
    /// Typed primitive / `String`, or `Serialize + JsonSchema` (CBOR).
    Typed,
    /// `Display` → string.
    Display,
    /// `Debug` → string.
    Debug,
}

/// One field in a registered schema.
#[derive(Clone)]
pub struct SchemaFieldEntry {
    pub name: &'static str,
    pub rust_type: &'static str,
    pub render: FieldRender,
    pub json_schema: fn() -> Value,
}

impl std::fmt::Debug for SchemaFieldEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SchemaFieldEntry")
            .field("name", &self.name)
            .field("rust_type", &self.rust_type)
            .field("render", &self.render)
            .finish()
    }
}

/// JSON Schema for a `String` / Display / Debug field.
pub fn json_schema_string() -> Value {
    serde_json::json!({ "type": "string" })
}

/// JSON Schema for an integer field.
pub fn json_schema_integer() -> Value {
    serde_json::json!({ "type": "integer" })
}

/// JSON Schema for a floating-point field.
pub fn json_schema_number() -> Value {
    serde_json::json!({ "type": "number" })
}

/// JSON Schema for a boolean field.
pub fn json_schema_boolean() -> Value {
    serde_json::json!({ "type": "boolean" })
}

/// JSON Schema of `T` with subschemas inlined, matching the JSON sink form.
pub fn json_schema_for<T: JsonSchema>() -> Value {
    let mut settings = SchemaSettings::draft07();
    settings.inline_subschemas = true;
    let root = settings.into_generator().into_root_schema_for::<T>();
    #[allow(clippy::expect_used)]
    {
        serde_json::to_value(root.schema).expect("JsonSchema serializes")
    }
}

/// A schema entry in the runtime registry
#[derive(Debug, Clone)]
pub struct SchemaEntry {
    pub path: &'static str,
    pub name: &'static str,
    pub target: &'static str,
    pub level: &'static str,
    pub description: &'static str,
    pub public: bool,
    pub required_fields: &'static [SchemaFieldEntry],
    pub optional_fields: &'static [SchemaFieldEntry],
}

inventory::collect!(SchemaEntry);

impl SchemaEntry {
    /// Get all registered schemas
    pub fn all() -> Vec<SchemaEntry> {
        inventory::iter::<SchemaEntry>().cloned().collect()
    }

    /// Find a schema by path
    pub fn find(path: &str) -> Option<SchemaEntry> {
        inventory::iter::<SchemaEntry>().find(|s| s.path == path).cloned()
    }

    /// Get the number of registered schemas
    pub fn count() -> usize {
        inventory::iter::<SchemaEntry>().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_collection() {
        // This test verifies that inventory can be used at runtime
        let _count = SchemaEntry::count();
        let _all = SchemaEntry::all();
    }
}
