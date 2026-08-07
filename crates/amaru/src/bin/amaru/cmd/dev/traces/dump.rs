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

use std::{collections::BTreeMap, env};

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_observability::{aliases, registry::SchemaEntry};
use clap::Parser;
use serde_json::{Value, json};

/// Dump all registered trace schemas as JSON Schema
#[derive(Debug, Parser)]
pub struct Args {
    /// Compact JSON output (no pretty-printing)
    #[clap(long)]
    compact: bool,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Simple, move || run(args))
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let aliases = aliases::load_workspace_type_aliases_from(&env::current_dir()?)?;
    let output = generate_traces_json_schema(&SchemaEntry::all(), &aliases);
    let json_string =
        if args.compact { serde_json::to_string(&output)? } else { serde_json::to_string_pretty(&output)? };

    eprintln!("{}", json_string);
    Ok(())
}

fn generate_traces_json_schema(entries: &[SchemaEntry], aliases: &BTreeMap<String, String>) -> Value {
    // Only public schemas appear in the JSON output and generated documentation.
    // Private schemas are present in the registry solely for tooling (e.g. unused-schemas).
    let mut sorted_entries: Vec<_> = entries.iter().filter(|e| e.public).cloned().collect();
    sorted_entries.sort_by(|a, b| a.path.cmp(b.path));

    let schemas_map = sorted_entries
        .iter()
        .map(|entry| {
            let properties = entry
                .required_fields
                .iter()
                .chain(entry.optional_fields.iter())
                .map(|(name, ty)| (name.to_string(), field_to_json_type(ty, aliases)))
                .collect::<serde_json::Map<_, _>>();

            let required: Vec<_> =
                entry.required_fields.iter().map(|(name, _)| Value::String(name.to_string())).collect();

            let optional: Vec<_> =
                entry.optional_fields.iter().map(|(name, _)| Value::String(name.to_string())).collect();

            (
                entry.path.to_string(),
                json!({
                    "type": "object",
                    "properties": properties,
                    "required": required,
                    "optional": optional,
                    "additionalProperties": false,
                    "name": entry.name.to_lowercase(),
                    "level": entry.level,
                    "target": entry.target,
                    "description": entry.description,
                    "public": entry.public,
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();

    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "type": "object",
        "title": "Amaru Trace Schemas",
        "description": "JSON Schema definitions for all registered traces in Amaru",
        "definitions": Value::Object(schemas_map),
    })
}

/// Convert a Rust type string to a JSON Schema type
fn field_to_json_type(rust_type: &str, aliases: &BTreeMap<String, String>) -> Value {
    let resolved = aliases::resolve_type_alias(rust_type, aliases);

    match resolved.as_str() {
        "u64" | "u32" | "u16" | "u8" | "i64" | "i32" | "i16" | "i8" | "usize" | "isize" => {
            json!({ "type": "integer" })
        }
        "f64" | "f32" => json!({ "type": "number" }),
        "bool" => json!({ "type": "boolean" }),
        "String" | "&str" => json!({ "type": "string" }),
        _ => {
            json!({
                "type": "string",
                "description": format!("Custom type: {}", rust_type)
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_field_to_json_type() {
        assert_eq!(field_to_json_type("u64", &BTreeMap::new()), json!({ "type": "integer" }));
        assert_eq!(field_to_json_type("String", &BTreeMap::new()), json!({ "type": "string" }));
        assert_eq!(field_to_json_type("& str", &BTreeMap::new()), json!({ "type": "string" }));
        assert_eq!(field_to_json_type("bool", &BTreeMap::new()), json!({ "type": "boolean" }));
    }

    #[test]
    fn test_field_to_json_type_resolves_aliases() {
        let aliases = BTreeMap::from([
            ("Lovelace".to_string(), "u64".to_string()),
            ("Amount".to_string(), "Lovelace".to_string()),
            ("amaru_kernel::Lovelace".to_string(), "u64".to_string()),
        ]);

        assert_eq!(field_to_json_type("Lovelace", &aliases), json!({ "type": "integer" }));
        assert_eq!(field_to_json_type("Amount", &aliases), json!({ "type": "integer" }));
        assert_eq!(field_to_json_type("amaru_kernel::Lovelace", &aliases), json!({ "type": "integer" }));
    }

    #[test]
    fn test_resolve_type_alias_stops_on_cycles() {
        let aliases = BTreeMap::from([
            ("Amount".to_string(), "Lovelace".to_string()),
            ("Lovelace".to_string(), "Amount".to_string()),
        ]);

        assert_eq!(aliases::resolve_type_alias("Amount", &aliases), "Lovelace");
    }

    #[test]
    fn test_field_to_json_type_custom_falls_back_to_string() {
        assert_eq!(
            field_to_json_type("amaru_kernel::Whatever", &BTreeMap::new()),
            json!({
                "type": "string",
                "description": "Custom type: amaru_kernel::Whatever"
            })
        );
    }
}
