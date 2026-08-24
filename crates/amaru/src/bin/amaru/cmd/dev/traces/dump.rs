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

use std::io;

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_observability::registry::SchemaEntry;
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

async fn run(args: Args) -> anyhow::Result<()> {
    let output = generate_traces_json_schema(&SchemaEntry::all());

    if args.compact {
        serde_json::to_writer(io::stdout(), &output)
    } else {
        serde_json::to_writer_pretty(io::stdout(), &output)
    }?;

    Ok(())
}

fn generate_traces_json_schema(entries: &[SchemaEntry]) -> Value {
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
                .map(|field| (field.name.to_string(), (field.json_schema)()))
                .collect::<serde_json::Map<_, _>>();

            let required: Vec<_> =
                entry.required_fields.iter().map(|field| Value::String(field.name.to_string())).collect();

            let optional: Vec<_> =
                entry.optional_fields.iter().map(|field| Value::String(field.name.to_string())).collect();

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

#[cfg(test)]
mod tests {
    use amaru_observability::registry::SchemaEntry;

    use super::*;

    #[test]
    fn dumped_schema_uses_serialized_field_types() {
        let dump = generate_traces_json_schema(&SchemaEntry::all());
        let fork = &dump["definitions"]["amaru::ledger::state::SWITCH_TO_FORK"]["properties"];
        assert!(fork["fork_point"].get("oneOf").is_some() || fork["fork_point"]["type"] == "array");
        assert_ne!(
            fork["fork_point"].get("description").and_then(Value::as_str).unwrap_or(""),
            "Custom type: amaru_kernel::Point"
        );

        let tip = &dump["definitions"]["amaru::ledger::tip::UPDATE"]["properties"];
        assert_eq!(tip["header_hash"]["type"], "string");
        assert_eq!(tip["slot"]["type"], "integer");
    }
}
