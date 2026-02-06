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

//! `define_schemas!` macro implementation
//!
//! This module parses schema definitions and generates:
//! - Nested const module structures for schema access
//! - Field validator macros for compile-time type checking
//! - Required field checker macros
//! - Module validator macros
//! - Record helper macros for auto-recording schema fields
//! - Inventory submissions for runtime registry

use proc_macro::TokenStream;
use quote::quote;
use std::collections::BTreeMap;

use crate::utils::{
    format_field_spec, is_identifier_start, is_uppercase_identifier, make_ident,
    make_instrument_macro_name, make_module_validator_name, make_record_macro_name,
    make_registry_const_name, make_require_macro_name, make_schema_field_const_name,
};

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for schema code generation.
#[derive(Clone, Copy)]
struct GenerationConfig {
    /// Whether to add `#[macro_export]` to generated macros.
    /// Set to `true` for schemas in libraries (like amaru_observability).
    /// Set to `false` for local/test schemas to avoid the
    /// "macro-expanded `macro_export` macros from the current crate cannot be
    /// referred to by absolute paths" error.
    export_macros: bool,
}

impl GenerationConfig {
    /// Generate macro attributes based on export configuration.
    /// For exported macros: `#[macro_export]`
    /// For local macros: `#[allow(unused_macros)]` (to suppress warnings for unused helpers)
    fn macro_export_attr(&self) -> proc_macro2::TokenStream {
        if self.export_macros {
            quote! { #[macro_export] }
        } else {
            quote! { #[allow(unused_macros)] }
        }
    }

    /// Generate the crate path prefix for macro calls.
    /// Uses `$crate::` for exported macros, nothing for local macros.
    fn crate_prefix(&self) -> proc_macro2::TokenStream {
        if self.export_macros {
            quote! { $crate:: }
        } else {
            quote! {}
        }
    }
}

// =============================================================================
// Data Structures
// =============================================================================

/// A field within a schema definition.
#[derive(Debug, Clone)]
pub struct SchemaField {
    /// Field name (e.g., "slot", "hash")
    name: String,
    /// Field type as string (e.g., "u64", "String")
    ty: String,
}

/// A complete schema definition.
#[derive(Debug, Clone)]
pub struct Schema {
    /// Top-level category (e.g., "consensus")
    category: String,
    /// Sub-category (e.g., "chain_sync")
    subcategory: String,
    /// Schema name in SCREAMING_SNAKE_CASE (e.g., "VALIDATE_HEADER")
    name: String,
    /// Optional description from doc comment
    description: Option<String>,
    /// Fields that must be present
    required_fields: Vec<SchemaField>,
    /// Fields that may optionally be present
    optional_fields: Vec<SchemaField>,
}

impl Schema {
    /// Create a new schema at the top level (no category/subcategory).
    fn new_top_level(name: &str) -> Self {
        Self::new(name, "", "")
    }

    /// Create a new schema within a category/subcategory.
    fn new_with_path(name: &str, category: &str, subcategory: &str) -> Self {
        Self::new(name, category, subcategory)
    }

    /// Create a new schema with the given path components.
    fn new(name: &str, category: &str, subcategory: &str) -> Self {
        Schema {
            category: category.to_string(),
            subcategory: subcategory.to_string(),
            name: name.to_string(),
            description: None,
            required_fields: Vec::new(),
            optional_fields: Vec::new(),
        }
    }

    /// Set the description from a doc comment.
    fn set_description(&mut self, description: String) {
        self.description = Some(description);
    }

    /// Get the names of all required fields.
    fn required_field_names(&self) -> Vec<String> {
        self.required_fields
            .iter()
            .map(|f| f.name.clone())
            .collect()
    }

    /// Generate the validation string format: "R|req_fields|O|opt_fields"
    fn validation_string(&self) -> String {
        let required = format_field_list(&self.required_fields);
        let optional = format_field_list(&self.optional_fields);
        format!("R|{required}|O|{optional}")
    }
}

// =============================================================================
// Tokenizer
// =============================================================================

/// Tokenize input string into meaningful tokens for parsing.
///
/// Splits on: `{`, `}`, `,`, `:`, and whitespace, while preserving doc comments.
/// Returns a vector of tokens in parsing order.
fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    let mut current = String::new();

    while let Some(ch) = chars.next() {
        match ch {
            // Check for doc comment start
            '/' if chars.peek() == Some(&'/') => {
                // Flush current token
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }

                // Consume the second '/'
                chars.next();

                // Check for third '/' (doc comment)
                if chars.peek() == Some(&'/') {
                    chars.next();

                    // Collect the rest of the doc comment until end of line or next '/'
                    let mut comment = String::from("///");

                    // Skip leading whitespace after ///
                    while chars.peek() == Some(&' ') {
                        chars.next();
                    }

                    // Collect comment text until we hit something that ends it
                    while let Some(&c) = chars.peek() {
                        // Stop at newlines, or at identifiers/braces that indicate end of comment
                        if c == '\n' {
                            chars.next();
                            break;
                        }
                        // Also stop if we see start of next token (uppercase letter after whitespace)
                        comment.push(c);
                        chars.next();
                    }

                    let trimmed_comment = comment.trim().to_string();
                    if trimmed_comment.len() > 3 {
                        // More than just "///"
                        tokens.push(trimmed_comment);
                    }
                } else {
                    // Regular comment (//), skip until newline
                    while let Some(&c) = chars.peek() {
                        if c == '\n' {
                            break;
                        }
                        chars.next();
                    }
                }
            }
            '{' | '}' | ',' | ':' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                tokens.push(ch.to_string());
            }
            c if c.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => {
                current.push(c);
            }
        }
    }

    // Add any remaining content
    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

// =============================================================================
// Parser
// =============================================================================

/// Parser state for tracking nested structure during tokenization.
struct ParserState {
    /// Current brace nesting depth
    depth: i32,
    /// Current category name
    category: String,
    /// Current subcategory name
    subcategory: String,
    /// Schema being built (if any)
    current_schema: Option<Schema>,
    /// Depth at which the current schema was started
    schema_depth: i32,
    /// Pending description to be applied to the next schema
    pending_description: Option<String>,
}

impl ParserState {
    fn new() -> Self {
        ParserState {
            depth: 0,
            category: String::new(),
            subcategory: String::new(),
            current_schema: None,
            schema_depth: -1,
            pending_description: None,
        }
    }

    /// Handle opening brace.
    fn open_brace(&mut self) {
        self.depth += 1;
    }

    /// Handle closing brace, potentially finalizing a schema.
    fn close_brace(&mut self, schemas: &mut Vec<Schema>) {
        // Check if we're closing a schema
        if self.schema_depth >= 0 && self.depth == self.schema_depth + 1 {
            if let Some(schema) = self.current_schema.take() {
                schemas.push(schema);
            }
            self.schema_depth = -1;
        }

        self.depth = self.depth.saturating_sub(1);
    }

    /// Try to start a new scope (category, subcategory, or schema).
    fn try_start_scope(&mut self, name: &str) {
        match self.depth {
            0 => {
                // Top level: this is a category
                self.category = name.to_string();
            }
            1 if !is_uppercase_identifier(name) => {
                // Depth 1 with lowercase: this is a subcategory
                self.subcategory = name.to_string();
            }
            1 => {
                // Depth 1 with uppercase: schema directly in category (no subcategory)
                let mut schema = Schema::new_top_level(name);
                if let Some(description) = self.pending_description.take() {
                    schema.set_description(description);
                }
                self.current_schema = Some(schema);
                self.schema_depth = self.depth;
            }
            2 if is_uppercase_identifier(name) => {
                // Depth 2 with uppercase: schema in category::subcategory
                let mut schema = Schema::new_with_path(name, &self.category, &self.subcategory);
                if let Some(description) = self.pending_description.take() {
                    schema.set_description(description);
                }
                self.current_schema = Some(schema);
                self.schema_depth = self.depth;
            }
            _ => {}
        }
    }

    /// Check if we can add a field to the current schema.
    fn can_add_field(&self) -> bool {
        self.schema_depth >= 0 && self.current_schema.is_some()
    }

    /// Add a required field to the current schema, checking for duplicates.
    fn add_required_field(&mut self, name: &str, ty: &str, errors: &mut Vec<String>) {
        self.add_field_internal(name, ty, true, errors);
    }

    /// Add an optional field to the current schema, checking for duplicates.
    fn add_optional_field(&mut self, name: &str, ty: &str, errors: &mut Vec<String>) {
        self.add_field_internal(name, ty, false, errors);
    }

    /// Add a field to the current schema, checking for duplicates.
    fn add_field_internal(
        &mut self,
        name: &str,
        ty: &str,
        is_required: bool,
        errors: &mut Vec<String>,
    ) {
        let Some(schema) = self.current_schema.as_mut() else {
            return;
        };

        // Check for duplicate field names
        let is_duplicate = schema
            .required_fields
            .iter()
            .chain(schema.optional_fields.iter())
            .any(|f| f.name == name);

        if is_duplicate {
            errors.push(format!(
                "Duplicate field '{}' in schema {}",
                name, schema.name
            ));
            return;
        }

        let field = SchemaField {
            name: name.to_string(),
            ty: ty.to_string(),
        };

        if is_required {
            schema.required_fields.push(field);
        } else {
            schema.optional_fields.push(field);
        }
    }
}

/// Parse a single token and update state.
///
/// Returns the next index to process.
fn parse_token(
    token: &str,
    tokens: &[String],
    index: usize,
    state: &mut ParserState,
    schemas: &mut Vec<Schema>,
    errors: &mut Vec<String>,
) -> usize {
    match token {
        "{" => {
            state.open_brace();
            index + 1
        }
        "}" => {
            state.close_brace(schemas);
            index + 1
        }
        "required" => {
            // Parse: required field_name: Type
            if state.can_add_field()
                && let Some((name, ty)) = try_parse_prefixed_field(tokens, index)
            {
                state.add_required_field(name, ty, errors);
                return index + 4; // Skip required, name, :, and type
            }
            index + 1
        }
        "optional" => {
            // Parse: optional field_name: Type
            if state.can_add_field()
                && let Some((name, ty)) = try_parse_prefixed_field(tokens, index)
            {
                state.add_optional_field(name, ty, errors);
                return index + 4; // Skip optional, name, :, and type
            }
            index + 1
        }
        _ if is_identifier_start(token) => {
            // Check if this starts a new scope (followed by `{`)
            if tokens.get(index + 1).map(|s| s.as_str()) == Some("{") {
                state.try_start_scope(token);
            }
            index + 1
        }
        _ => index + 1,
    }
}

/// Try to parse a prefixed field definition: `required/optional name: type`.
///
/// Returns `Some((name, type))` if the pattern matches.
fn try_parse_prefixed_field(tokens: &[String], index: usize) -> Option<(&str, &str)> {
    // tokens[index] is "required" or "optional"
    // tokens[index+1] should be the field name
    // tokens[index+2] should be ":"
    // tokens[index+3] should be the type
    let name = tokens.get(index + 1).map(|s| s.as_str())?;
    if !is_identifier_start(name) {
        return None;
    }
    if tokens.get(index + 2).map(|s| s.as_str()) != Some(":") {
        return None;
    }
    let ty = tokens
        .get(index + 3)
        .map(|s| s.as_str())
        .filter(|s| !s.is_empty())?;
    Some((name, ty))
}

/// Extract all schemas and errors from input using functional approach.
fn extract_schemas(input: &str) -> (Vec<Schema>, Vec<String>) {
    let tokens = tokenize(input);

    // Process tokens sequentially while maintaining state
    let (_, mut state, mut schemas, errors) = tokens.iter().enumerate().fold(
        (0usize, ParserState::new(), Vec::new(), Vec::new()),
        |(mut skip_until, mut state, mut schemas, mut errors), (idx, token)| {
            if idx >= skip_until {
                // Check if we're about to start a schema and attach any preceding doc comment
                // This needs to happen BEFORE parse_token creates the schema
                if is_identifier_start(token)
                    && tokens.get(idx + 1).map(|s| s.as_str()) == Some("{")
                    && is_uppercase_identifier(token)
                {
                    // This will be a schema, collect all consecutive doc comments before it
                    let mut doc_lines = Vec::new();
                    let mut look_back = idx;
                    while look_back > 0 {
                        look_back -= 1;
                        if tokens[look_back].starts_with("///") {
                            let line = tokens[look_back].trim_start_matches("///").trim();
                            if !line.is_empty() {
                                doc_lines.push(line.to_string());
                            }
                        } else {
                            break;
                        }
                    }
                    if !doc_lines.is_empty() {
                        doc_lines.reverse(); // They were collected in reverse order
                        state.pending_description = Some(doc_lines.join(" "));
                    }
                }

                skip_until =
                    parse_token(token, &tokens, idx, &mut state, &mut schemas, &mut errors);
            }
            (skip_until, state, schemas, errors)
        },
    );

    // Finalize any pending schemas
    if let Some(schema) = state.current_schema.take() {
        schemas.push(schema);
    }

    // Validate that all schemas have descriptions
    let mut missing_descriptions = Vec::new();
    for schema in &schemas {
        if schema.description.is_none() {
            missing_descriptions.push(format!(
                "Schema '{}' is missing a description. Add a doc comment (///) above the schema definition.",
                schema.name
            ));
        }
    }

    let mut all_errors = errors;
    all_errors.extend(missing_descriptions);

    (schemas, all_errors)
}

// =============================================================================
// Code Generation Helpers
// =============================================================================

/// Format field list as "name:type,name:type,...".
fn format_field_list(fields: &[SchemaField]) -> String {
    fields
        .iter()
        .map(|f| format_field_spec(&f.name, &f.ty))
        .collect::<Vec<_>>()
        .join(",")
}

// =============================================================================
// Macro Generation
// =============================================================================

/// Generate the required fields checker macro for a schema.
///
/// This generates a single recursive `{SCHEMA}_REQUIRE` macro that validates all required
/// fields are present using tt-munching. No per-field helper macros needed.
fn generate_required_fields_macro(
    schema: &Schema,
    config: &GenerationConfig,
) -> proc_macro2::TokenStream {
    let require_macro_name =
        make_require_macro_name(&schema.category, &schema.subcategory, &schema.name);
    let require_ident = make_ident(&require_macro_name);
    let macro_export = config.macro_export_attr();
    let crate_prefix = config.crate_prefix();

    let required_names = schema.required_field_names();

    if required_names.is_empty() {
        // No required fields - accept anything
        return quote! {
            #macro_export
            #[doc(hidden)]
            macro_rules! #require_ident {
                ($($fields:ident),* $(,)?) => {};
            }
        };
    }

    let required_list = required_names.join(", ");
    let schema_name = &schema.name;

    // Generate a helper macro for each required field that searches through the input list.
    // This is simpler than trying to do complex recursion with variable field names.

    let field_idents: Vec<_> = required_names.iter().map(|n| make_ident(n)).collect();

    // Build helper macro for each required field
    let mut helper_macros = Vec::new();

    for (i, field_ident) in field_idents.iter().enumerate() {
        let field_name_str = &required_names[i];
        let helper_name = make_ident(&format!(
            "__{}_CHECK_{}",
            schema.name,
            required_names[i].to_uppercase()
        ));

        helper_macros.push(quote! {
            #macro_export
            #[doc(hidden)]
            macro_rules! #helper_name {
                // Found the target field - success
                (#field_ident $($rest:tt)*) => { };
                // Different field - keep searching
                ($other:tt $($rest:tt)*) => {
                    #crate_prefix #helper_name!($($rest)*);
                };
                // Empty input - field is missing
                () => {
                    compile_error!(concat!(
                        "Missing required field '",
                        #field_name_str,
                        "' for schema ",
                        #schema_name,
                        ". Required fields: ",
                        #required_list
                    ));
                };
            }
        });
    }

    // Generate calls to each helper macro
    let helper_calls: Vec<_> = required_names
        .iter()
        .map(|field_name| {
            let helper_name = make_ident(&format!(
                "__{}_CHECK_{}",
                schema.name,
                field_name.to_uppercase()
            ));
            quote! { #crate_prefix #helper_name!($($fields)*); }
        })
        .collect();

    quote! {
        #(#helper_macros)*

        #macro_export
        #[doc(hidden)]
        macro_rules! #require_ident {
            // Entry point - dispatch to check each required field
            ($($fields:ident),* $(,)?) => {
                #(#helper_calls)*
            };
        }
    }
}

/// Generate the instrument helper macro for a schema.
///
/// This macro provides the `#[tracing::instrument]` attribute with:
/// - `level = Level::TRACE`
/// - `skip_all`
/// - `target = "module::path"`
/// - `fields(...)` with:
///   - Required fields: `field` - captures value from function param (validated to exist)
///   - Optional fields: `field = tracing::field::Empty` - set via `Span::current().record()`
fn generate_instrument_macro(
    schema: &Schema,
    category: &str,
    subcategory: &str,
    config: &GenerationConfig,
) -> proc_macro2::TokenStream {
    let macro_name = make_instrument_macro_name(category, subcategory, &schema.name);
    let macro_ident = make_ident(&macro_name);
    let macro_export = config.macro_export_attr();

    // Target is module path, name is lowercase schema name (same as registry)
    let target = format!("{category}::{subcategory}");
    let name = schema.name.to_lowercase();

    // Required fields: declare as Empty, set via Span::current().record()
    // We can't reference function params by name because:
    // 1. This macro is generated per-schema, not per-function
    // 2. Function params may have different names than schema fields
    // Users must explicitly record field values in the function body
    let required_fields: Vec<proc_macro2::TokenStream> = schema
        .required_fields
        .iter()
        .map(|f| {
            let field_ident = make_ident(&f.name);
            quote! { #field_ident = tracing::field::Empty }
        })
        .collect();

    // Optional fields: declare as Empty, set via Span::current().record()
    let optional_fields: Vec<proc_macro2::TokenStream> = schema
        .optional_fields
        .iter()
        .map(|f| {
            let field_ident = make_ident(&f.name);
            quote! { #field_ident = tracing::field::Empty }
        })
        .collect();

    // Build the fields expression
    let fields_expr = if required_fields.is_empty() && optional_fields.is_empty() {
        quote! {}
    } else if optional_fields.is_empty() {
        quote! { fields(#(#required_fields),*) }
    } else if required_fields.is_empty() {
        quote! { fields(#(#optional_fields),*) }
    } else {
        quote! { fields(#(#required_fields,)* #(#optional_fields),*) }
    };

    quote! {
        #macro_export
        #[doc(hidden)]
        macro_rules! #macro_ident {
            ($($func:tt)*) => {
                #[tracing::instrument(
                    level = tracing::Level::TRACE,
                    skip_all,
                    name = #name,
                    target = #target,
                    #fields_expr
                )]
                $($func)*
            };
        }
    }
}

/// Generate the unified record helper macro for a schema.
///
/// This macro handles multiple validation modes:
/// - Lenient mode: `_RECORD!("name", expr)` - silently ignores unknown fields (for function params)
/// - Strict mode: `_RECORD!("name", expr, strict)` - errors on unknown fields (for custom expressions)
/// - Validate mode: `_RECORD!("name", "type", validate)` - validates field name/type pair (for #[trace])
///
/// Usage:
/// - `__SCHEMA_RECORD!("field_name", expr);` → records if known, ignores if unknown
/// - `__SCHEMA_RECORD!("field_name", expr, strict);` → records if known, errors if unknown
/// - `__SCHEMA_RECORD!("field_name", "type", validate);` → validates field name/type (for #[trace])
fn generate_record_macro(schema: &Schema, config: &GenerationConfig) -> proc_macro2::TokenStream {
    let macro_name = make_record_macro_name(&schema.category, &schema.subcategory, &schema.name);
    let macro_ident = make_ident(&macro_name);
    let schema_name = &schema.name;
    let macro_export = config.macro_export_attr();

    // Generate match arms for all schema fields (required + optional)
    let all_fields: Vec<_> = schema
        .required_fields
        .iter()
        .chain(schema.optional_fields.iter())
        .collect();

    // Generate patterns for lenient mode (no mode marker)
    let lenient_field_patterns: Vec<_> = all_fields
        .iter()
        .map(|field| {
            let field_name = &field.name;
            quote! {
                (#field_name, $expr:expr) => {{
                    tracing::Span::current().record(
                        #field_name,
                        tracing::field::display(&$expr)
                    );
                }};
            }
        })
        .collect();

    // Generate patterns for strict mode (with strict marker)
    let strict_field_patterns: Vec<_> = all_fields
        .iter()
        .map(|field| {
            let field_name = &field.name;
            quote! {
                (#field_name, $expr:expr, strict) => {{
                    tracing::Span::current().record(
                        #field_name,
                        tracing::field::display(&$expr)
                    );
                }};
            }
        })
        .collect();

    // Generate patterns for validate mode (field name + type checking)
    // This replaces the old _VALIDATOR macro
    let validate_exact_patterns: Vec<_> = all_fields
        .iter()
        .map(|field| {
            let field_name = &field.name;
            let field_type = &field.ty;
            quote! {
                (#field_name, #field_type, validate) => {};
            }
        })
        .collect();

    // Generate wrong-type patterns for validate mode
    let validate_wrong_type_patterns: Vec<_> = all_fields
        .iter()
        .map(|field| {
            let field_name = &field.name;
            let expected_type = &field.ty;
            quote! {
                (#field_name, $actual_ty:literal, validate) => {
                    compile_error!(concat!(
                        "Wrong type for field '",
                        #field_name,
                        "': expected '",
                        #expected_type,
                        "', found '",
                        $actual_ty,
                        "'"
                    ));
                };
            }
        })
        .collect();

    // List all fields for error messages
    let all_field_names: Vec<_> = all_fields.iter().map(|f| f.name.as_str()).collect();
    let fields_list = all_field_names.join(", ");

    quote! {
        #macro_export
        #[doc(hidden)]
        macro_rules! #macro_ident {
            // ===== LENIENT MODE =====
            // Known fields - record the value
            #(#lenient_field_patterns)*
            // Unknown field - silently ignore (allows extra function params)
            ($name:literal, $expr:expr) => {};

            // ===== STRICT MODE =====
            // Known fields - record the value
            #(#strict_field_patterns)*
            // Unknown field - compile error (catches typos in custom expressions)
            ($name:literal, $expr:expr, strict) => {
                compile_error!(concat!(
                    "Unknown field '",
                    $name,
                    "' for schema ",
                    #schema_name,
                    ". Available fields: ",
                    #fields_list
                ));
            };

            // ===== VALIDATE MODE =====
            // Exact matches (correct name and type)
            #(#validate_exact_patterns)*
            // Wrong type patterns
            #(#validate_wrong_type_patterns)*
            // Unknown field - ignored to allow extra function parameters
            ($name:literal, $ty:literal, validate) => {};
        }
    }
}

/// Generate a module-specific schema validator macro.
///
/// This ensures only valid schemas are used within a module path.
fn generate_module_validator_macro(
    category: &str,
    subcategory: &str,
    schema_names: &[String],
    config: &GenerationConfig,
) -> proc_macro2::TokenStream {
    let validator_name = make_module_validator_name(category, subcategory);
    let validator_ident = make_ident(&validator_name);
    let module_path = format!("{category}::{subcategory}");
    let schemas_list = schema_names.join(", ");
    let macro_export = config.macro_export_attr();

    // For valid schemas, expand the body
    let valid_schema_patterns: Vec<_> = schema_names
        .iter()
        .map(|name| {
            let schema_ident = make_ident(name);
            quote! {
                (#schema_ident, { $($body:tt)* }) => {
                    $($body)*
                };
            }
        })
        .collect();

    // For invalid schemas, emit compile_error and discard the body
    // This prevents "cannot find macro" cascading errors
    quote! {
        #macro_export
        #[doc(hidden)]
        macro_rules! #validator_ident {
            #(#valid_schema_patterns)*
            ($schema:ident, { $($body:tt)* }) => {
                compile_error!(concat!(
                    "Invalid trace in module ",
                    #module_path,
                    " : ",
                    stringify!($schema),
                    ". Expected one of: ",
                    #schemas_list
                ));
            };
        }
    }
}

/// Generate inventory submission for runtime schema registry.
///
/// Note: When the macro is expanded within the `amaru-observability` crate itself,
/// we use `crate::registry::SchemaEntry` instead of `amaru_observability::registry::SchemaEntry`.
fn generate_inventory_submission(
    schema: &Schema,
    category: &str,
    subcategory: &str,
) -> proc_macro2::TokenStream {
    let schema_path = format!("{category}::{subcategory}::{}", schema.name);
    let target_path = format!("{category}::{subcategory}");
    let schema_name = schema.name.clone();

    let required_fields_array: Vec<_> = schema
        .required_fields
        .iter()
        .map(|f| {
            let name = &f.name;
            let ty = &f.ty;
            quote! { (#name, #ty) }
        })
        .collect();

    let optional_fields_array: Vec<_> = schema
        .optional_fields
        .iter()
        .map(|f| {
            let name = &f.name;
            let ty = &f.ty;
            quote! { (#name, #ty) }
        })
        .collect();

    // Use `crate::` path when inside amaru-observability lib itself, external path otherwise.
    // We check both CARGO_PKG_NAME and CARGO_CRATE_NAME because examples within the
    // amaru-observability package have the package name but a different crate name.
    //
    // This is required for local schema testing
    let is_observability_lib = std::env::var("CARGO_PKG_NAME").ok().as_deref()
        == Some("amaru-observability")
        && std::env::var("CARGO_CRATE_NAME").ok().as_deref() == Some("amaru_observability");

    let use_stmt = if is_observability_lib {
        quote! { use crate::registry::SchemaEntry; }
    } else {
        quote! { use amaru_observability::registry::SchemaEntry; }
    };

    // Description should exist if validation passed, but use a fallback for error recovery
    let description = schema
        .description
        .as_deref()
        .unwrap_or("Missing description");

    quote! {
        #[allow(non_upper_case_globals)]
        const _: () = {
            #use_stmt
            inventory::submit!(SchemaEntry {
                path: #schema_path,
                name: #schema_name,
                target: #target_path,
                level: "TRACE",
                description: #description,
                required_fields: &[#(#required_fields_array),*],
                optional_fields: &[#(#optional_fields_array),*],
            });
        };
    }
}

/// Generate global schema listing helper macros.
fn generate_schema_help_macros(
    schema_paths: &[String],
    schema_names: &[String],
    config: &GenerationConfig,
) -> proc_macro2::TokenStream {
    let macro_export = config.macro_export_attr();

    if schema_names.is_empty() {
        return quote! {
            #macro_export
            #[doc(hidden)]
            macro_rules! __list_available_schemas {
                () => { "No schemas defined" };
            }

            #macro_export
            #[doc(hidden)]
            macro_rules! __validate_schema_name {
                ($schema:ident) => {
                    compile_error!("No schemas defined");
                };
            }
        };
    }

    let schema_paths_str = schema_paths.join(", ");
    let schema_names_list = schema_names.join(", ");

    quote! {
        /// Helper macro that lists available schemas for error messages.
        /// For macro internal use only.
        #macro_export
        #[doc(hidden)]
        macro_rules! __list_available_schemas {
            () => {
                concat!("Available schemas: ", #schema_paths_str)
            };
        }

        /// Catch-all macro for invalid schema validation.
        /// For macro internal use only.
        #macro_export
        #[doc(hidden)]
        macro_rules! __validate_schema_name {
            ($schema:ident) => {
                compile_error!(concat!(
                    "Invalid schema name. Available schemas: ",
                    #schema_names_list
                ));
            };
        }
    }
}

// =============================================================================
// Module Tree Generation
// =============================================================================

/// Group schemas by category and subcategory.
fn group_schemas_by_path(schemas: &[Schema]) -> BTreeMap<String, BTreeMap<String, Vec<Schema>>> {
    schemas
        .iter()
        .cloned()
        .fold(BTreeMap::new(), |mut grouped, schema| {
            let category = schema.category.clone();
            let subcategory = schema.subcategory.clone();
            grouped
                .entry(category)
                .or_default()
                .entry(subcategory)
                .or_default()
                .push(schema);
            grouped
        })
}

/// Build the complete module tree with all generated code.
fn build_module_tree_with_metadata(
    schemas: &[Schema],
    config: &GenerationConfig,
) -> proc_macro2::TokenStream {
    let grouped = group_schemas_by_path(schemas);

    let mut outer_modules = Vec::new();
    let mut validation_consts = Vec::new();
    let mut schema_field_consts = Vec::new();
    let mut validation_macros = Vec::new();
    let mut inventory_submissions = Vec::new();
    let mut all_schema_names = Vec::new();
    let mut all_schema_paths = Vec::new();

    for (category, subcats) in grouped {
        let category_ident = make_ident(&category);
        let mut sub_modules = Vec::new();

        for (subcat_name, cat_schemas) in subcats {
            let subcat_ident = make_ident(&subcat_name);
            let mut consts_and_metadata = Vec::new();
            let mut module_schema_names = Vec::new();

            for schema in cat_schemas {
                let schema_ident = make_ident(&schema.name);
                let full_path = format!("{category}::{subcat_name}");
                let full_const_path = format!("{full_path}::{}", schema.name.to_lowercase());

                // Track names for registry and error messages
                all_schema_names.push(schema.name.clone());
                all_schema_paths.push(full_const_path.clone());
                module_schema_names.push(schema.name.clone());

                // Generate const for schema path
                consts_and_metadata.push(quote! {
                    pub const #schema_ident: &str = #full_const_path;
                });

                // Generate validation constants
                let validation_string = schema.validation_string();
                let validation_const_name =
                    make_schema_field_const_name(&category, &subcat_name, &schema.name);
                let validation_const_ident = make_ident(&validation_const_name);

                consts_and_metadata.push(quote! {
                    /// Compile-time validation constant for the #[trace] macro.
                    /// Format: R|required_field:type,...|O|optional_field:type,...
                    pub const #validation_const_ident: &str = #validation_string;
                });

                validation_consts.push((
                    (category.clone(), subcat_name.clone(), schema.name.clone()),
                    validation_string.clone(),
                ));
                schema_field_consts.push((validation_const_name, validation_string));

                // Generate required fields checker macro
                validation_macros.push(generate_required_fields_macro(&schema, config));

                // Generate instrument helper macro
                validation_macros.push(generate_instrument_macro(
                    &schema,
                    &category,
                    &subcat_name,
                    config,
                ));

                // Generate unified record helper macro (handles recording, validation, and augment modes)
                validation_macros.push(generate_record_macro(&schema, config));

                // Generate inventory submission
                inventory_submissions.push(generate_inventory_submission(
                    &schema,
                    &category,
                    &subcat_name,
                ));
            }

            // Generate module-specific schema validator macro
            validation_macros.push(generate_module_validator_macro(
                &category,
                &subcat_name,
                &module_schema_names,
                config,
            ));

            sub_modules.push(quote! {
                pub mod #subcat_ident {
                    #(#consts_and_metadata)*
                }
            });
        }

        outer_modules.push(quote! {
            pub mod #category_ident {
                #(#sub_modules)*
            }
        });
    }

    // Generate validation registry entries
    let validation_registry_entries =
        validation_consts
            .iter()
            .map(|((category, subcategory, schema_name), schema_str)| {
                let const_name = make_registry_const_name(category, subcategory, schema_name);
                let const_ident = make_ident(&const_name);
                quote! {
                    pub const #const_ident: &str = #schema_str;
                }
            });

    let schema_field_entries = schema_field_consts.iter().map(|(const_name, schema_str)| {
        let const_ident = make_ident(const_name);
        quote! {
            pub const #const_ident: &str = #schema_str;
        }
    });

    // Generate schema list helper macros
    let schema_help_macro =
        generate_schema_help_macros(&all_schema_paths, &all_schema_names, config);

    quote! {
        // Submit schemas to inventory for runtime registry
        #(#inventory_submissions)*

        // Validation macros at crate root (required for #[macro_export])
        // Note: #[macro_export] macros are automatically placed at crate root
        // and cannot be re-exported via `pub use`. Users should call them
        // without a module prefix (e.g., `__SCHEMA_INSTRUMENT!()` not `amaru::__SCHEMA_INSTRUMENT!()`)
        #[allow(unused_macros)]
        #(#validation_macros)*

        // Schema list helper macros
        #schema_help_macro

        /// The `amaru` module contains all schema definitions and metadata constants.
        ///
        /// Use `amaru::category::subcategory::SCHEMA_NAME` paths with `#[trace]`.
        /// Note: Validation macros are at crate root due to Rust's #[macro_export] behavior.
        pub mod amaru {
            #(#outer_modules)*

            /// Internal module for schema validation constants.
            /// For macro internal use only.
            pub mod __trace_schema_registry {
                #(#validation_registry_entries)*
                #(#schema_field_entries)*
            }
        }

        // Re-export amaru module contents for convenience
        pub use amaru::*;
    }
}

/// Internal expansion with configurable export behavior.
fn expand_with_config(input: TokenStream, export_macros: bool) -> TokenStream {
    let config = GenerationConfig { export_macros };

    // Convert TokenStream to proc_macro2::TokenStream for manipulation
    let input2: proc_macro2::TokenStream = input.into();

    // Convert to string - doc comments (/// ...) are preserved in the string representation
    let input_str = input2.to_string();

    let (schemas, errors) = extract_schemas(&input_str);

    // Generate the module tree (includes all macros)
    let module_tree = build_module_tree_with_metadata(&schemas, &config);

    // If there are errors, include them alongside the generated code
    // This ensures macros are defined (preventing "cannot find macro" errors)
    // while still reporting the actual errors
    if !errors.is_empty() {
        let error_msgs: Vec<_> = errors
            .iter()
            .map(|e| quote! { compile_error!(#e); })
            .collect();

        return quote! {
            #(#error_msgs)*
            #module_tree
        }
        .into();
    }

    module_tree.into()
}

/// Expand the `define_schemas!` macro.
///
/// Generated macros are exported with `#[macro_export]` for use across crates.
pub fn expand(input: TokenStream) -> TokenStream {
    expand_with_config(input, true)
}

/// Expand the `define_local_schemas!` macro.
///
/// Generated macros are NOT exported with `#[macro_export]`, making them
/// suitable for local/test use without the "macro-expanded `macro_export`
/// macros from the current crate cannot be referred to by absolute paths" error.
pub fn expand_local(input: TokenStream) -> TokenStream {
    expand_with_config(input, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_simple() {
        let tokens = tokenize("foo { bar: u64 }");
        assert_eq!(tokens, vec!["foo", "{", "bar", ":", "u64", "}"]);
    }

    #[test]
    fn test_tokenize_nested() {
        let tokens = tokenize("cat { sub { SCHEMA { required x: u32 } } }");
        assert_eq!(
            tokens,
            vec![
                "cat", "{", "sub", "{", "SCHEMA", "{", "required", "x", ":", "u32", "}", "}", "}"
            ]
        );
    }

    #[test]
    fn test_tokenize_with_commas() {
        let tokens = tokenize("a: u32, b: String");
        assert_eq!(tokens, vec!["a", ":", "u32", ",", "b", ":", "String"]);
    }

    #[test]
    fn test_extract_simple_schema() {
        let input = r#"
            consensus {
                sync {
                    /// Validate the schema
                    VALIDATE {
                        required slot: u64
                    }
                }
            }
        "#;
        let (schemas, errors) = extract_schemas(input);
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "VALIDATE");
        assert_eq!(schemas[0].category, "consensus");
        assert_eq!(schemas[0].subcategory, "sync");
        assert_eq!(schemas[0].required_fields.len(), 1);
        assert_eq!(schemas[0].required_fields[0].name, "slot");
        assert_eq!(schemas[0].required_fields[0].ty, "u64");
        assert_eq!(
            schemas[0].description,
            Some("Validate the schema".to_string())
        );
    }

    #[test]
    fn test_extract_schema_with_optional() {
        let input = r#"
            test {
                sub {
                    /// Test schema
                    SCHEMA {
                        required id: String
                        optional name: String
                    }
                }
            }
        "#;
        let (schemas, errors) = extract_schemas(input);
        assert!(errors.is_empty());
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].required_fields.len(), 1);
        assert_eq!(schemas[0].optional_fields.len(), 1);
        assert_eq!(schemas[0].optional_fields[0].name, "name");
    }

    #[test]
    fn test_extract_multiple_schemas() {
        let input = r#"
            cat {
                sub {
                    /// Schema A description
                    SCHEMA_A {
                        required a: u32
                    }
                    /// Schema B description
                    SCHEMA_B {
                        required b: u64
                    }
                }
            }
        "#;
        let (schemas, errors) = extract_schemas(input);
        assert!(errors.is_empty());
        assert_eq!(schemas.len(), 2);
        assert_eq!(schemas[0].name, "SCHEMA_A");
        assert_eq!(schemas[1].name, "SCHEMA_B");
    }

    #[test]
    fn test_duplicate_field_error() {
        let input = r#"
            cat {
                sub {
                    /// Schema with duplicate
                    SCHEMA {
                        required x: u32
                        required x: u64
                    }
                }
            }
        "#;
        let (_, errors) = extract_schemas(input);
        assert!(errors.iter().any(|e| e.contains("Duplicate field 'x'")));
    }

    #[test]
    fn test_schema_validation_string() {
        let mut schema = Schema::new_with_path("TEST", "cat", "sub");
        schema.required_fields.push(SchemaField {
            name: "id".to_string(),
            ty: "u64".to_string(),
        });
        schema.optional_fields.push(SchemaField {
            name: "name".to_string(),
            ty: "String".to_string(),
        });
        assert_eq!(schema.validation_string(), "R|id:u64|O|name:String");
    }

    #[test]
    fn test_missing_description_error() {
        let input = r#"
            cat {
                sub {
                    SCHEMA {
                        required x: u32
                    }
                }
            }
        "#;
        let (schemas, errors) = extract_schemas(input);
        assert_eq!(schemas.len(), 1);
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].contains("SCHEMA") && errors[0].contains("missing a description"),
            "Expected missing description error, got: {}",
            errors[0]
        );
    }

    #[test]
    fn test_with_description() {
        let input = r#"
            cat {
                sub {
                    /// This is a test schema
                    SCHEMA {
                        required x: u32
                    }
                }
            }
        "#;
        let (schemas, errors) = extract_schemas(input);
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
        assert_eq!(schemas.len(), 1);
        assert_eq!(
            schemas[0].description,
            Some("This is a test schema".to_string())
        );
    }

    #[test]
    fn test_multiline_description() {
        let input = r#"
            cat {
                sub {
                    /// This is a test schema
                    /// with multiple lines
                    /// of documentation
                    SCHEMA {
                        required x: u32
                    }
                }
            }
        "#;
        let (schemas, errors) = extract_schemas(input);
        assert!(errors.is_empty());
        assert_eq!(schemas.len(), 1);
        assert_eq!(
            schemas[0].description,
            Some("This is a test schema with multiple lines of documentation".to_string())
        );
    }
}
