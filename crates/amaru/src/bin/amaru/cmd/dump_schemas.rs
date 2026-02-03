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

use amaru_observability::registry::SchemaEntry;
use clap::Parser;
use serde_json::{Value, json};

/// Dump all registered trace schemas as JSON Schema
#[derive(Debug, Parser)]
pub struct Args {
    /// Compact JSON output (no pretty-printing)
    #[clap(long, short = 'c')]
    compact: bool,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let entries = SchemaEntry::all();
    let output = generate_json_schema(&entries);

    let json_string = if args.compact {
        serde_json::to_string(&output)?
    } else {
        serde_json::to_string_pretty(&output)?
    };

    eprintln!("{}", json_string);
    Ok(())
}

fn generate_json_schema(entries: &[SchemaEntry]) -> Value {
    let mut schemas = Value::Object(Default::default());

    for entry in entries {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        let mut optional = Vec::new();

        for (name, ty) in entry.required_fields {
            properties.insert(name.to_string(), field_to_json_type(ty));
            required.push(Value::String(name.to_string()));
        }

        for (name, ty) in entry.optional_fields {
            properties.insert(name.to_string(), field_to_json_type(ty));
            optional.push(Value::String(name.to_string()));
        }

        let schema = json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "optional": optional,
            "additionalProperties": false,
            "name": entry.name,
            "level": entry.level,
            "target": entry.target,
            "description": entry.description,
        });

        if let Value::Object(ref mut obj) = schemas {
            obj.insert(entry.path.to_string(), schema);
        }
    }

    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "type": "object",
        "title": "Amaru Trace Schemas",
        "description": "JSON Schema definitions for all registered traces in Amaru",
        "definitions": schemas,
    })
}

/// Convert a Rust type string to a JSON Schema type
fn field_to_json_type(rust_type: &str) -> Value {
    match rust_type {
        "u64" | "u32" | "u16" | "u8" | "i64" | "i32" | "i16" | "i8" | "usize" | "isize" => {
            json!({ "type": "integer" })
        }
        "f64" | "f32" => json!({ "type": "number" }),
        "bool" => json!({ "type": "boolean" }),
        "String" | "&str" => json!({ "type": "string" }),
        other => {
            // For custom types, use a generic object schema
            json!({
                "type": "object",
                "description": format!("Custom type: {}", other)
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_field_to_json_type() {
        assert_eq!(field_to_json_type("u64"), json!({ "type": "integer" }));
        assert_eq!(field_to_json_type("String"), json!({ "type": "string" }));
        assert_eq!(field_to_json_type("bool"), json!({ "type": "boolean" }));
    }
}
