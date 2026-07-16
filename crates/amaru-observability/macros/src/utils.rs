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

//! Shared utilities for macro implementations
//!
//! This module provides common string manipulation, identifier creation,
//! and naming convention functions used across the macro crates.
//!
use proc_macro2::Span;

/// Format a field specification as "name:type".
pub fn format_field_spec(name: &str, ty: &str) -> String {
    format!("{name}:{ty}")
}

/// Check if a string starts with an alphabetic or underscore character.
///
/// Used to identify valid Rust identifiers.
pub fn is_identifier_start(token: &str) -> bool {
    token.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
}

/// Check if a string starts with an uppercase character.
///
/// Used to identify schema names (which follow SCREAMING_SNAKE_CASE convention).
pub fn is_uppercase_identifier(token: &str) -> bool {
    token.chars().next().is_some_and(char::is_uppercase)
}

/// Check if a string is a valid Rust identifier.
///
/// A valid identifier:
/// - Starts with a letter (a-z, A-Z) or underscore (_)
/// - Contains only letters, digits (0-9), and underscores
/// - Does not contain special characters like ::, -, etc.
///
/// Used to validate schema and category names to prevent invalid Rust identifiers.
pub fn is_valid_identifier(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }

    token.chars().all(|c| c.is_alphanumeric() || c == '_')
        && token.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
}

/// Parse a schema path and extract (schema_name, module_path) using functional approach.
///
/// # Example
/// ```
/// # use amaru_observability_macros::*;
/// # fn parse_schema_path(path: &str) -> (&str, &str) {
/// #     path.rsplit_once("::").map_or(
/// #         (path, ""), // No separator: whole path is the schema name
/// #         |(parent, name)| (name, parent)
/// #     )
/// # }
/// assert_eq!(parse_schema_path("consensus::chain_sync::VALIDATE_HEADER"), ("VALIDATE_HEADER", "consensus::chain_sync"));
/// ```
pub fn parse_schema_path(path: &str) -> (&str, &str) {
    path.rsplit_once("::").map_or(
        (path, ""), // No separator: whole path is the schema name
        |(parent, name)| (name, parent),
    )
}

/// Parse a full schema path and extract the macro module path.
///
/// Local schemas are identified by a leading `self::` or `crate::` segment.
/// Exported schemas are everything up to and including the `amaru` segment;
/// when no `amaru` segment is present, the path is assumed to be an exported
/// schema whose `amaru` prefix was elided.
///
/// # Examples
/// ```ignore
/// // Full path from external crate
/// parse_macro_module("amaru_observability::schemas::amaru::ledger::state::SCHEMA")
///   -> "amaru_observability::schemas::amaru"
///
/// // Short path with import
/// parse_macro_module("amaru::ledger::state::SCHEMA")
///   -> "amaru"
///
/// // No prefix — auto-prepended `amaru`
/// parse_macro_module("ledger::state::SCHEMA")
///   -> "amaru"
///
/// // Local test schemas (require self:: or crate:: prefix)
/// parse_macro_module("self::test::sub::MY_SCHEMA")
///   -> "self"
/// parse_macro_module("crate::test::sub::MY_SCHEMA")
///   -> "crate"
/// ```
pub fn parse_macro_module(full_path: &str) -> &str {
    if full_path.starts_with("self::") || full_path == "self" {
        "self"
    } else if full_path.starts_with("crate::") || full_path == "crate" {
        "crate"
    } else if let Some(pos) = full_path.find("amaru::") {
        // Return everything up to and including "amaru"
        &full_path[..pos + 5] // "amaru" is 5 chars
    } else if full_path == "amaru" || full_path.starts_with("amaru::") {
        "amaru"
    } else {
        // No prefix: auto-prepend `amaru`. Macro helpers still live under
        // amaru_observability, so callers treat this the same as an
        // amaru-prefixed path.
        "amaru"
    }
}

/// Parse a full schema path and extract (schema_name, target_path, macro_module).
///
/// The target_path is the categories joined by `::` (e.g. `amaru::ledger::state`).
/// For exported schemas it always begins with `amaru`. The macro_module describes
/// where validation macros are defined; for local schemas it is `self`/`crate`.
///
/// # Examples
/// ```ignore
/// parse_full_schema_path("amaru::ledger::state::SCHEMA")
///   -> ("SCHEMA", "amaru::ledger::state", "amaru")
///
/// parse_full_schema_path("ledger::state::SCHEMA")        // amaru elided
///   -> ("SCHEMA", "amaru::ledger::state", "amaru")
///
/// parse_full_schema_path("SCHEMA")                       // amaru elided, no categories
///   -> ("SCHEMA", "amaru", "amaru")
///
/// parse_full_schema_path("my_crate::schemas::amaru::test::sub::MY_SCHEMA")
///   -> ("MY_SCHEMA", "amaru::test::sub", "my_crate::schemas::amaru")
///
/// parse_full_schema_path("self::test::sub::SCHEMA")      // local schema
///   -> ("SCHEMA", "test::sub", "self")
/// parse_full_schema_path("crate::test::sub::SCHEMA")     // local schema
///   -> ("SCHEMA", "test::sub", "crate")
/// ```
pub fn parse_full_schema_path(full_path: &str) -> (&str, String, &str) {
    let macro_module = parse_macro_module(full_path);

    // Local schemas: strip the self::/crate:: marker, then use the remainder
    // as both the categories source and the public-const path root.
    if matches!(macro_module, "self" | "crate") {
        let stripped =
            full_path.strip_prefix("self::").or_else(|| full_path.strip_prefix("crate::")).unwrap_or(full_path);
        let (schema_name, target_path) = parse_schema_path(stripped);
        return (schema_name, target_path.to_string(), macro_module);
    }

    if let Some(amaru_pos) = full_path.find("amaru::") {
        // Keep "amaru::..." for the categories path.
        let after_crate_prefix = &full_path[amaru_pos..];
        let (schema_name, target_path) = parse_schema_path(after_crate_prefix);
        (schema_name, target_path.to_string(), macro_module)
    } else {
        // No prefix at all: auto-prepend `amaru` to the categories so the
        // generated identifiers match the amaru-prefixed form.
        let (schema_name, target_path) = parse_schema_path(full_path);
        let prefixed = if target_path.is_empty() { "amaru".to_string() } else { format!("amaru::{target_path}") };
        (schema_name, prefixed, macro_module)
    }
}

/// Create a Rust identifier from a string.
pub fn make_ident(name: &str) -> syn::Ident {
    syn::Ident::new(name, Span::call_site())
}

/// Generate a namespace prefix from categories.
///
/// Convention: joins categories with double underscores in uppercase
/// Examples:
/// - ["consensus", "chain_sync"] → `CONSENSUS__CHAIN_SYNC__`
/// - ["ledger"] → `LEDGER__`
/// - [] → ""
pub fn make_macro_namespace(categories: &[String]) -> String {
    if categories.is_empty() {
        String::new()
    } else {
        categories.iter().map(|c| c.to_uppercase()).collect::<Vec<_>>().join("__") + "__"
    }
}

/// Generate a required fields checker macro name for a schema.
///
/// Convention: `__{CATEGORIES}__{SCHEMA_NAME}_REQUIRE`
pub fn make_require_macro_name(categories: &[String], schema_name: &str) -> String {
    let namespace = make_macro_namespace(categories);
    format!("__{namespace}{schema_name}_REQUIRE")
}

/// Generate a required field checker helper macro name for a schema field.
///
/// Convention: `__{CATEGORIES}__{SCHEMA_NAME}_CHECK_{FIELD_NAME}`
pub fn make_required_field_check_macro_name(categories: &[String], schema_name: &str, field_name: &str) -> String {
    let namespace = make_macro_namespace(categories);
    format!("__{namespace}{schema_name}_CHECK_{}", field_name.to_uppercase())
}

/// Generate a module validator macro name.
///
/// Convention: `__VALIDATE_{CATEGORIES}` (uppercase, joined by underscores)
pub fn make_module_validator_name(categories: &[String]) -> String {
    if categories.is_empty() {
        "__VALIDATE".to_string()
    } else {
        format!("__VALIDATE_{}", categories.iter().map(|c| c.to_uppercase()).collect::<Vec<_>>().join("_"))
    }
}

/// Generate a schema field constant name.
///
/// Convention: `__{CATEGORIES}__{SCHEMA_NAME}_SCHEMA_FIELDS`
pub fn make_schema_field_const_name(categories: &[String], schema_name: &str) -> String {
    let namespace = make_macro_namespace(categories);
    format!("__{namespace}{schema_name}_SCHEMA_FIELDS")
}

/// Generate a schema field count constant name.
///
/// Convention: `__{CATEGORIES}__{SCHEMA_NAME}_FIELD_COUNT`
pub fn make_schema_field_count_const_name(categories: &[String], schema_name: &str) -> String {
    let namespace = make_macro_namespace(categories);
    format!("__{namespace}{schema_name}_FIELD_COUNT")
}

/// Generate a schema visibility constant name.
///
/// Convention: `__{CATEGORIES}__{SCHEMA_NAME}_PUBLIC`
pub fn make_schema_public_const_name(categories: &[String], schema_name: &str) -> String {
    let namespace = make_macro_namespace(categories);
    format!("__{namespace}{schema_name}_PUBLIC")
}

/// Generate a schema validation registry constant name.
///
/// Convention: `_SCHEMA_{CATEGORIES}__{SCHEMA_NAME}`
#[allow(dead_code)]
pub fn make_registry_const_name(categories: &[String], schema_name: &str) -> String {
    let namespace = make_macro_namespace(categories);
    format!("_SCHEMA_{namespace}{schema_name}")
}

/// Generate a schema instrument helper macro name.
///
/// Convention: `__{CATEGORIES}__{SCHEMA_NAME}_INSTRUMENT`
pub fn make_instrument_macro_name(categories: &[String], schema_name: &str) -> String {
    let namespace = make_macro_namespace(categories);
    format!("__{namespace}{schema_name}_INSTRUMENT")
}

/// Generate a schema field assignment helper macro name.
///
/// Convention: `__{CATEGORIES}__{SCHEMA_NAME}_ASSIGN`
pub fn make_assign_macro_name(categories: &[String], schema_name: &str) -> String {
    let namespace = make_macro_namespace(categories);
    format!("__{namespace}{schema_name}_ASSIGN")
}

/// Generate a schema field record helper macro name.
///
/// Convention: `__{CATEGORIES}__{SCHEMA_NAME}_RECORD`
pub fn make_record_macro_name(categories: &[String], schema_name: &str) -> String {
    let namespace = make_macro_namespace(categories);
    format!("__{namespace}{schema_name}_RECORD")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_operations() {
        let path = "consensus::chain_sync::VALIDATE_HEADER";
        assert_eq!(parse_schema_path(path), ("VALIDATE_HEADER", "consensus::chain_sync"));
    }

    #[test]
    fn test_path_edge_cases() {
        assert_eq!(parse_schema_path("SCHEMA"), ("SCHEMA", ""));
    }

    #[test]
    fn test_is_identifier_start() {
        assert!(is_identifier_start("field"));
        assert!(is_identifier_start("_field"));
        assert!(is_identifier_start("Field"));
        assert!(!is_identifier_start("123"));
        assert!(!is_identifier_start(""));
    }

    #[test]
    fn test_is_uppercase_identifier() {
        assert!(is_uppercase_identifier("SCHEMA"));
        assert!(is_uppercase_identifier("Schema"));
        assert!(!is_uppercase_identifier("schema"));
        assert!(!is_uppercase_identifier("_schema"));
        assert!(!is_uppercase_identifier(""));
    }

    #[test]
    fn test_parse_macro_module() {
        assert_eq!(parse_macro_module("amaru::ledger::state::SCHEMA"), "amaru");
        assert_eq!(
            parse_macro_module("amaru_observability::amaru::ledger::state::SCHEMA"),
            "amaru_observability::amaru"
        );
        assert_eq!(parse_macro_module("ledger::state::SCHEMA"), "amaru");
        assert_eq!(parse_macro_module("SCHEMA"), "amaru");
        assert_eq!(parse_macro_module("self::test::sub::SCHEMA"), "self");
        assert_eq!(parse_macro_module("crate::test::sub::SCHEMA"), "crate");
    }

    #[test]
    fn test_parse_full_schema_path_amaru_prefixed() {
        let (name, target, module) = parse_full_schema_path("amaru::ledger::state::SCHEMA");
        assert_eq!(name, "SCHEMA");
        assert_eq!(target, "amaru::ledger::state");
        assert_eq!(module, "amaru");
    }

    #[test]
    fn test_parse_full_schema_path_no_prefix() {
        let (name, target, module) = parse_full_schema_path("ledger::state::SCHEMA");
        assert_eq!(name, "SCHEMA");
        assert_eq!(target, "amaru::ledger::state");
        assert_eq!(module, "amaru");
    }

    #[test]
    fn test_parse_full_schema_path_bare_schema() {
        let (name, target, module) = parse_full_schema_path("SCHEMA");
        assert_eq!(name, "SCHEMA");
        assert_eq!(target, "amaru");
        assert_eq!(module, "amaru");
    }

    #[test]
    fn test_parse_full_schema_path_local_self() {
        let (name, target, module) = parse_full_schema_path("self::test::sub::SCHEMA");
        assert_eq!(name, "SCHEMA");
        assert_eq!(target, "test::sub");
        assert_eq!(module, "self");
    }

    #[test]
    fn test_parse_full_schema_path_local_crate() {
        let (name, target, module) = parse_full_schema_path("crate::test::sub::SCHEMA");
        assert_eq!(name, "SCHEMA");
        assert_eq!(target, "test::sub");
        assert_eq!(module, "crate");
    }

    #[test]
    fn test_is_valid_identifier() {
        // Valid identifiers
        assert!(is_valid_identifier("SCHEMA"));
        assert!(is_valid_identifier("schema"));
        assert!(is_valid_identifier("_schema"));
        assert!(is_valid_identifier("Schema123"));
        assert!(is_valid_identifier("_123"));
        assert!(is_valid_identifier("MY_CONSTANT"));

        // Invalid identifiers
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("123")); // starts with digit
        assert!(!is_valid_identifier("MY::SCHEMA")); // contains ::
        assert!(!is_valid_identifier("MY-SCHEMA")); // contains -
        assert!(!is_valid_identifier("MY SCHEMA")); // contains space
        assert!(!is_valid_identifier("MY.SCHEMA")); // contains .
        assert!(!is_valid_identifier("MY@SCHEMA")); // contains @
    }
}
