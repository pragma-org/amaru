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
    collections::{BTreeMap, BTreeSet},
    fs, iter,
    path::Path,
};

use amaru_observability::registry::SchemaEntry;
use clap::Parser;
use quote::ToTokens;
use serde_json::{Value, json};
use syn::Item;

/// Dump all registered trace schemas as JSON Schema
#[derive(Debug, Parser)]
pub struct Args {
    /// Compact JSON output (no pretty-printing)
    #[clap(long, short = 'c')]
    compact: bool,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let output = generate_traces_json_schema(&SchemaEntry::all());
    let json_string =
        if args.compact { serde_json::to_string(&output)? } else { serde_json::to_string_pretty(&output)? };

    eprintln!("{}", json_string);
    Ok(())
}

fn generate_traces_json_schema(entries: &[SchemaEntry]) -> Value {
    let aliases = load_workspace_type_aliases();

    // Sort entries by path to ensure deterministic output across builds and platforms
    let mut sorted_entries = entries.to_vec();
    sorted_entries.sort_by(|a, b| a.path.cmp(b.path));

    let schemas_map = sorted_entries
        .iter()
        .map(|entry| {
            let properties = entry
                .required_fields
                .iter()
                .chain(entry.optional_fields.iter())
                .map(|(name, ty)| (name.to_string(), field_to_json_type(ty, &aliases)))
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
                    "private": entry.private,
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

fn load_workspace_type_aliases() -> BTreeMap<String, String> {
    let mut aliases = BTreeMap::new();
    let crates_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join("crates");
    collect_type_aliases(&crates_dir, &mut aliases);
    aliases
}

fn collect_type_aliases(path: &Path, aliases: &mut BTreeMap<String, String>) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };

    entries.flatten().map(|entry| entry.path()).for_each(|entry_path| match entry_path.is_dir() {
        true => collect_type_aliases(&entry_path, aliases),
        false if is_rust_source_file(&entry_path) => {
            if let Ok(source) = fs::read_to_string(&entry_path) {
                collect_type_aliases_from_source(&source, aliases);
            }
        }
        false => {}
    });
}

fn collect_type_aliases_from_source(source: &str, aliases: &mut BTreeMap<String, String>) {
    let Ok(syntax) = syn::parse_file(source) else {
        return;
    };

    syntax.items.iter().filter_map(parse_top_level_type_alias).for_each(|(alias, target)| {
        aliases.insert(alias, target);
    });
}

fn is_rust_source_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("rs")
}

fn parse_top_level_type_alias(item: &Item) -> Option<(String, String)> {
    let Item::Type(type_alias) = item else {
        return None;
    };

    if !type_alias.generics.params.is_empty() {
        return None;
    }

    let alias = type_alias.ident.to_string();
    let target = type_alias.ty.to_token_stream().to_string();

    (!alias.is_empty() && !target.is_empty()).then_some((alias, target))
}

fn resolve_type_alias<'a>(rust_type: &'a str, aliases: &'a BTreeMap<String, String>) -> &'a str {
    iter::successors(Some(rust_type), |current| aliases.get(*current).map(String::as_str))
        .scan(BTreeSet::new(), |visited, current| visited.insert(current).then_some(current))
        .last()
        .unwrap_or(rust_type)
}

/// Convert a Rust type string to a JSON Schema type
fn field_to_json_type(rust_type: &str, aliases: &BTreeMap<String, String>) -> Value {
    let resolved = resolve_type_alias(rust_type, aliases);

    match resolved {
        "u64" | "u32" | "u16" | "u8" | "i64" | "i32" | "i16" | "i8" | "usize" | "isize" => {
            json!({ "type": "integer" })
        }
        "f64" | "f32" => json!({ "type": "number" }),
        "bool" => json!({ "type": "boolean" }),
        "String" | "&str" => json!({ "type": "string" }),
        _ => {
            // For custom types, use a generic object schema
            json!({
                "type": "object",
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
        let aliases = BTreeMap::new();

        assert_eq!(field_to_json_type("u64", &aliases), json!({ "type": "integer" }));
        assert_eq!(field_to_json_type("String", &aliases), json!({ "type": "string" }));
        assert_eq!(field_to_json_type("bool", &aliases), json!({ "type": "boolean" }));
    }

    #[test]
    fn test_field_to_json_type_resolves_aliases() {
        let aliases = BTreeMap::from([
            ("Lovelace".to_string(), "u64".to_string()),
            ("Amount".to_string(), "Lovelace".to_string()),
        ]);

        assert_eq!(field_to_json_type("Lovelace", &aliases), json!({ "type": "integer" }));
        assert_eq!(field_to_json_type("Amount", &aliases), json!({ "type": "integer" }));
    }

    #[test]
    fn test_resolve_type_alias_stops_on_cycles() {
        let aliases = BTreeMap::from([
            ("Amount".to_string(), "Lovelace".to_string()),
            ("Lovelace".to_string(), "Amount".to_string()),
        ]);

        assert_eq!(resolve_type_alias("Amount", &aliases), "Lovelace");
    }

    #[test]
    fn test_collect_type_aliases_from_source() {
        let mut aliases = BTreeMap::new();

        collect_type_aliases_from_source(
            r#"
            pub type Lovelace = u64;
            pub(crate) type DisplayName = String;
            type Amount = Lovelace;
            pub type Wrapped<T> = Vec<T>;
            "#,
            &mut aliases,
        );

        assert_eq!(aliases.get("Lovelace"), Some(&"u64".to_string()));
        assert_eq!(aliases.get("DisplayName"), Some(&"String".to_string()));
        assert_eq!(aliases.get("Amount"), Some(&"Lovelace".to_string()));
        assert!(!aliases.contains_key("Wrapped"));
    }

    #[test]
    fn test_collect_type_aliases_from_source_ignores_associated_types() {
        let mut aliases = BTreeMap::new();

        collect_type_aliases_from_source(
            r#"
            pub type Lovelace = u64;

            impl Deref for Coin {
                type Target = InnerCoin;
            }

            impl Iterator for Coins {
                type Item = Coin;
            }

            impl Validation for Context {
                type FinalState = State;
            }
            "#,
            &mut aliases,
        );

        assert_eq!(aliases, BTreeMap::from([("Lovelace".to_string(), "u64".to_string())]));
    }

    #[test]
    fn test_parse_top_level_type_alias() {
        let item: Item = syn::parse_str("pub type Lovelace = u64;").unwrap();
        assert_eq!(parse_top_level_type_alias(&item), Some(("Lovelace".to_string(), "u64".to_string())));

        let generic_item: Item = syn::parse_str("pub type Wrapped<T> = Vec<T>;").unwrap();
        assert_eq!(parse_top_level_type_alias(&generic_item), None);

        let non_alias_item: Item = syn::parse_str("struct NotAnAlias;").unwrap();
        assert_eq!(parse_top_level_type_alias(&non_alias_item), None);
    }
}
