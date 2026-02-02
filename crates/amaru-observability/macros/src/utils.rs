//! Shared utilities for macro implementations
//!
//! This module provides common string manipulation, identifier creation,
//! and naming convention functions used across the macro crates.
//! 
use proc_macro2::Span;


/// Strip leading underscores from a string.
///
/// This is used to normalize field names like `_field_name` to `field_name`.
pub fn strip_leading_underscores(s: &str) -> String {
    s.trim_start_matches('_').to_string()
}

/// Format a field specification as "name:type".
pub fn format_field_spec(name: &str, ty: &str) -> String {
    format!("{name}:{ty}")
}

/// Check if a string starts with an alphabetic or underscore character.
///
/// Used to identify valid Rust identifiers.
pub fn is_identifier_start(token: &str) -> bool {
    token
        .chars()
        .next()
        .map_or(false, |c| c.is_alphabetic() || c == '_')
}

/// Check if a string starts with an uppercase character.
///
/// Used to identify schema names (which follow SCREAMING_SNAKE_CASE convention).
pub fn is_uppercase_identifier(token: &str) -> bool {
    token.chars().next().map_or(false, char::is_uppercase)
}

/// Extract the last component from a colon-separated path.
///
/// # Example
/// ```
/// # fn path_last_component(path: &str) -> &str { path.rsplit_once("::").map_or(path, |(_, name)| name) }
/// assert_eq!(path_last_component("consensus::chain_sync::VALIDATE_HEADER"), "VALIDATE_HEADER");
/// assert_eq!(path_last_component("SCHEMA"), "SCHEMA");
/// ```
#[allow(dead_code)]
pub fn path_last_component(path: &str) -> &str {
    path.rsplit_once("::").map_or(path, |(_, name)| name)
}

/// Extract all but the last component from a colon-separated path.
///
/// # Example
/// ```
/// # fn path_parent(path: &str) -> &str { path.rsplit_once("::").map_or("", |(parent, _)| parent) }
/// assert_eq!(path_parent("consensus::chain_sync::VALIDATE_HEADER"), "consensus::chain_sync");
/// assert_eq!(path_parent("SCHEMA"), "");
/// ```
#[allow(dead_code)]
pub fn path_parent(path: &str) -> &str {
    path.rsplit_once("::").map_or("", |(parent, _)| parent)
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
/// The macro module is everything up to and including the `amaru` segment.
/// This allows resolution of validation macros from any crate.
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
/// // Local test schemas
/// parse_macro_module("crate::my_schemas::amaru::test::sub::MY_SCHEMA")
///   -> "crate::my_schemas::amaru"
/// ```
pub fn parse_macro_module(full_path: &str) -> &str {
    // Find the position of "amaru::" in the path
    if let Some(pos) = full_path.find("amaru::") {
        // Return everything up to and including "amaru"
        &full_path[..pos + 5] // "amaru" is 5 chars
    } else if full_path.starts_with("amaru") {
        "amaru"
    } else {
        // Fallback: no amaru found, use the parent of the schema
        let (_, module_path) = parse_schema_path(full_path);
        // Find the first segment
        module_path.split("::").next().unwrap_or("amaru")
    }
}

/// Parse a full schema path and extract (schema_name, target_path, macro_module).
///
/// The target_path is category::subcategory (for the tracing target).
/// The macro_module is the path to where validation macros are defined.
///
/// # Examples
/// ```ignore
/// parse_full_schema_path("amaru::ledger::state::SCHEMA")
///   -> ("SCHEMA", "ledger::state", "amaru")
///
/// parse_full_schema_path("my_crate::schemas::amaru::test::sub::MY_SCHEMA")
///   -> ("MY_SCHEMA", "test::sub", "my_crate::schemas::amaru")
///
/// parse_full_schema_path("test::sub::SCHEMA")  // local schema
///   -> ("SCHEMA", "test::sub", "test")
/// ```
pub fn parse_full_schema_path(full_path: &str) -> (&str, String, &str) {
    let macro_module = parse_macro_module(full_path);

    // Check if this is an "amaru" path (exported from amaru_observability)
    let is_amaru_path = full_path.contains("amaru::");

    if is_amaru_path {
        // For amaru paths, strip macro_module prefix to get target
        let after_amaru = if full_path.len() > macro_module.len() + 2 {
            &full_path[macro_module.len() + 2..] // Skip "::"
        } else {
            full_path
        };

        // Parse the remaining path to get schema_name and target
        let (schema_name, target_path) = parse_schema_path(after_amaru);

        (schema_name, target_path.to_string(), macro_module)
    } else {
        // For local schemas, use the full parent path as target
        // test::sub::SCHEMA -> ("SCHEMA", "test::sub", "test")
        let (schema_name, target_path) = parse_schema_path(full_path);

        (schema_name, target_path.to_string(), macro_module)
    }
}

/// Create a Rust identifier from a string.
pub fn make_ident(name: &str) -> syn::Ident {
    syn::Ident::new(name, Span::call_site())
}

/// Generate a namespace prefix from category and subcategory.
///
/// Convention: `{CATEGORY}__{SUBCATEGORY}__` (double underscores, uppercase)
/// Example: `consensus::chain_sync` → `CONSENSUS__CHAIN_SYNC__`
pub fn make_macro_namespace(category: &str, subcategory: &str) -> String {
    if category.is_empty() && subcategory.is_empty() {
        String::new()
    } else if subcategory.is_empty() {
        format!("{}__", category.to_uppercase())
    } else {
        format!(
            "{}__{}_",
            category.to_uppercase(),
            subcategory.to_uppercase()
        )
    }
}

/// Generate a required fields checker macro name for a schema.
///
/// Convention: `__{CATEGORY}__{SUBCATEGORY}__{SCHEMA_NAME}_REQUIRE`
pub fn make_require_macro_name(category: &str, subcategory: &str, schema_name: &str) -> String {
    let namespace = make_macro_namespace(category, subcategory);
    format!("__{namespace}{schema_name}_REQUIRE")
}

/// Generate a module validator macro name.
///
/// Convention: `__VALIDATE_{CATEGORY}_{SUBCATEGORY}` (uppercase)
pub fn make_module_validator_name(category: &str, subcategory: &str) -> String {
    format!(
        "__VALIDATE_{}_{}",
        category.to_uppercase(),
        subcategory.to_uppercase()
    )
}

/// Generate a schema field constant name.
///
/// Convention: `__{CATEGORY}__{SUBCATEGORY}__{SCHEMA_NAME}_SCHEMA_FIELDS`
pub fn make_schema_field_const_name(
    category: &str,
    subcategory: &str,
    schema_name: &str,
) -> String {
    let namespace = make_macro_namespace(category, subcategory);
    format!("__{namespace}{schema_name}_SCHEMA_FIELDS")
}

/// Generate a schema validation registry constant name.
///
/// Convention: `_SCHEMA_{CATEGORY}__{SUBCATEGORY}__{SCHEMA_NAME}`
pub fn make_registry_const_name(category: &str, subcategory: &str, schema_name: &str) -> String {
    let namespace = make_macro_namespace(category, subcategory);
    format!("_SCHEMA_{namespace}{schema_name}")
}

/// Generate a schema instrument helper macro name.
///
/// Convention: `__{CATEGORY}__{SUBCATEGORY}__{SCHEMA_NAME}_INSTRUMENT`
pub fn make_instrument_macro_name(category: &str, subcategory: &str, schema_name: &str) -> String {
    let namespace = make_macro_namespace(category, subcategory);
    format!("__{namespace}{schema_name}_INSTRUMENT")
}

/// Generate a schema field record helper macro name.
///
/// Convention: `__{CATEGORY}__{SUBCATEGORY}__{SCHEMA_NAME}_RECORD`
pub fn make_record_macro_name(category: &str, subcategory: &str, schema_name: &str) -> String {
    let namespace = make_macro_namespace(category, subcategory);
    format!("__{namespace}{schema_name}_RECORD")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_leading_underscores() {
        assert_eq!(strip_leading_underscores("_field"), "field");
        assert_eq!(strip_leading_underscores("__field"), "field");
        assert_eq!(strip_leading_underscores("field"), "field");
        assert_eq!(strip_leading_underscores("___"), "");
    }

    #[test]
    fn test_path_operations() {
        let path = "consensus::chain_sync::VALIDATE_HEADER";
        assert_eq!(path_last_component(path), "VALIDATE_HEADER");
        assert_eq!(path_parent(path), "consensus::chain_sync");
        assert_eq!(
            parse_schema_path(path),
            ("VALIDATE_HEADER", "consensus::chain_sync")
        );
    }

    #[test]
    fn test_path_edge_cases() {
        assert_eq!(path_last_component("SCHEMA"), "SCHEMA");
        assert_eq!(path_parent("SCHEMA"), "");
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
}
