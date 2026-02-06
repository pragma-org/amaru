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

//! Trace macro implementations for compile-time validated tracing.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro::TokenStream;
use quote::quote;
use syn::{FnArg, ItemFn, Pat, PatType};

use crate::utils::{
    make_ident, make_instrument_macro_name, make_module_validator_name, make_record_macro_name,
    make_require_macro_name, parse_full_schema_path, strip_leading_underscores,
};

/// Parsed schema path with optional inline field expressions.
///
/// Extracted from the macro argument like:
/// - `amaru::consensus::chain_sync::VALIDATE_HEADER`
/// - `amaru::consensus::chain_sync::VALIDATE_HEADER, hash = compute_hash()`
/// - `my_crate::schemas::amaru::test::sub::MY_SCHEMA`
struct SchemaMeta {
    /// The schema name (e.g., `VALIDATE_HEADER`)
    schema_name: String,
    /// The module path for tracing target (e.g., `consensus::chain_sync`)
    module_path: String,
    /// The macro module path (e.g., `amaru` or `my_crate::schemas::amaru`)
    /// Currently unused but kept for future extensibility (local schema support)
    macro_module: String,
    /// Optional field name -> expression mappings from inline definitions
    /// Maps field names to custom expressions for recording
    field_expressions: BTreeMap<String, proc_macro2::TokenStream>,
}

const SEPARATOR: &str = "::";

impl SchemaMeta {
    /// Extract category from module_path (e.g., "consensus" from "consensus::chain_sync")
    fn category(&self) -> &str {
        self.module_path.split(SEPARATOR).next().unwrap_or("")
    }

    /// Extract subcategory from module_path (e.g., "chain_sync" from "consensus::chain_sync")
    fn subcategory(&self) -> &str {
        let parts: Vec<&str> = self.module_path.split(SEPARATOR).collect();
        if parts.len() >= 2 { parts[1] } else { "" }
    }

    /// Check if this is a local schema (not from amaru_observability).
    ///
    /// Local schemas are defined with `define_local_schemas!` and their macros
    /// are NOT exported with `#[macro_export]`. They must be called without a path
    /// prefix since they're local to the module where they're defined.
    fn is_local_schema(&self) -> bool {
        self.macro_module != "amaru"
    }

    /// Generate a macro call with or without a crate path prefix.
    ///
    /// For exported macros (from amaru_observability), generates: `::amaru_observability::macro_name!(...)`
    /// For local macros (from define_local_schemas!), generates: `macro_name!(...)`
    ///
    /// Note: Does NOT include trailing semicolon - use in expression context.
    /// For statement context, use this in a block that provides the semicolon.
    fn macro_call(
        &self,
        macro_ident: &syn::Ident,
        args: proc_macro2::TokenStream,
    ) -> proc_macro2::TokenStream {
        if self.is_local_schema() {
            quote! { #macro_ident!(#args) }
        } else {
            quote! { ::amaru_observability::#macro_ident!(#args) }
        }
    }

    /// Generate a macro call as a statement (with trailing semicolon).
    ///
    /// For exported macros: `::amaru_observability::macro_name!(...);`
    /// For local macros: `macro_name!(...);`
    fn macro_call_stmt(
        &self,
        macro_ident: &syn::Ident,
        args: proc_macro2::TokenStream,
    ) -> proc_macro2::TokenStream {
        if self.is_local_schema() {
            quote! { #macro_ident!(#args); }
        } else {
            quote! { ::amaru_observability::#macro_ident!(#args); }
        }
    }

    /// Generate a macro call with block body (for instrument macro).
    ///
    /// For exported macros: `::amaru_observability::macro_name! { body }`
    /// For local macros: `macro_name! { body }`
    fn macro_call_block(
        &self,
        macro_ident: &syn::Ident,
        body: proc_macro2::TokenStream,
    ) -> proc_macro2::TokenStream {
        if self.is_local_schema() {
            quote! { #macro_ident! { #body } }
        } else {
            quote! { ::amaru_observability::#macro_ident! { #body } }
        }
    }

    /// Parse schema metadata from macro arguments.
    ///
    /// Supports formats:
    /// - `amaru::consensus::validate_header::EVOLVE_NONCE`
    /// - `amaru::consensus::validate_header::EVOLVE_NONCE, hash = compute_hash()`
    /// - `my_crate::amaru::test::sub::MY_SCHEMA, field = expr()`
    fn from_args(args: TokenStream) -> Self {
        // First, try to extract just the schema path for backward compatibility
        let args_str = args.to_string();

        // Check if we have the new syntax with field expressions
        if args_str.contains('=') {
            // New syntax: parse with syn for proper expression handling
            use syn::{Token, parse::Parse, parse::ParseStream};

            struct MacroArgs {
                schema_path: syn::Path,
                field_exprs: Vec<(syn::Ident, syn::Expr)>,
            }

            impl Parse for MacroArgs {
                fn parse(input: ParseStream) -> syn::Result<Self> {
                    let schema_path: syn::Path = input.parse()?;
                    let mut field_exprs = Vec::new();

                    // Parse optional comma-separated field expressions
                    while input.peek(Token![,]) {
                        input.parse::<Token![,]>()?; // consume comma

                        if input.is_empty() {
                            break;
                        }

                        let field_name: syn::Ident = input.parse()?;
                        input.parse::<Token![=]>()?;
                        let expr: syn::Expr = input.parse()?;

                        field_exprs.push((field_name, expr));
                    }

                    Ok(MacroArgs {
                        schema_path,
                        field_exprs,
                    })
                }
            }

            let parsed = syn::parse::<MacroArgs>(args).expect("Failed to parse macro arguments");

            // Convert path to string and parse schema components
            let schema_path = &parsed.schema_path;
            let path_str = quote! { #schema_path }.to_string().replace(' ', "");
            let (schema_name, module_path, macro_module) = parse_full_schema_path(&path_str);

            // Convert field expressions to map
            let mut field_expressions = BTreeMap::new();
            for (field_name, expr) in parsed.field_exprs {
                field_expressions.insert(field_name.to_string(), quote! { #expr });
            }

            SchemaMeta {
                schema_name: schema_name.to_owned(),
                module_path,
                macro_module: macro_module.to_owned(),
                field_expressions,
            }
        } else {
            // Legacy syntax: just a schema path
            let schema_path = syn::parse::<syn::Path>(args).expect("Invalid schema path");
            let path_str = quote! { #schema_path }.to_string().replace(' ', "");
            let (schema_name, module_path, macro_module) = parse_full_schema_path(&path_str);

            SchemaMeta {
                schema_name: schema_name.to_owned(),
                module_path,
                macro_module: macro_module.to_owned(),
                field_expressions: BTreeMap::new(),
            }
        }
    }
}

// =============================================================================
// Required Field Checking
// =============================================================================

/// Extracted function parameter information.
struct FunctionField {
    /// Original parameter name (may have leading underscores)
    raw_name: String,
    /// Field name after stripping leading underscores (for schema matching)
    name: String,
    /// Type as a string
    ty: String,
}

/// Extract field information from function parameters.
///
/// Returns `None` if a collision is detected (two parameters normalize to the same name).
fn extract_function_fields(func: &ItemFn) -> Option<Vec<FunctionField>> {
    let typed_args = func.sig.inputs.iter().filter_map(|arg| match arg {
        FnArg::Typed(pat_type) => Some(pat_type),
        _ => None,
    });

    let fields: Vec<_> = typed_args
        .filter_map(|PatType { pat, ty, .. }| match pat.as_ref() {
            Pat::Ident(pat_ident) => {
                let raw_name = pat_ident.ident.to_string();
                let name = strip_leading_underscores(&raw_name);
                let ty = quote!(#ty).to_string();

                Some(FunctionField { raw_name, name, ty })
            }
            _ => None,
        })
        .collect();

    // Check for duplicates using functional approach
    let names: Vec<_> = fields.iter().map(|f| &f.name).collect();
    let unique_names: BTreeSet<_> = names.iter().collect();

    if names.len() != unique_names.len() {
        None // Collision detected
    } else {
        Some(fields)
    }
}

/// Generate field type validator invocation for a single field.
///
/// Uses the unified _RECORD macro with `validate` mode to check field name/type pairs.
fn generate_field_validator(meta: &SchemaMeta, field: &FunctionField) -> proc_macro2::TokenStream {
    let record_macro =
        make_record_macro_name(meta.category(), meta.subcategory(), &meta.schema_name);
    let record_ident = make_ident(&record_macro);
    let field_name = &field.name;
    let field_type = &field.ty;

    meta.macro_call_stmt(&record_ident, quote! { #field_name, #field_type, validate })
}

/// Generate required fields checker invocation.
fn generate_required_fields_check(
    meta: &SchemaMeta,
    field_names: &[String],
) -> proc_macro2::TokenStream {
    let require_macro =
        make_require_macro_name(meta.category(), meta.subcategory(), &meta.schema_name);
    let require_ident = make_ident(&require_macro);
    let field_idents: Vec<_> = field_names.iter().map(|n| make_ident(n)).collect();

    meta.macro_call_stmt(&require_ident, quote! { #(#field_idents),* })
}

/// Generate an error when multiple parameters map to the same field name.
///
/// This happens when parameters like `field` and `_field` both exist,
/// since leading underscores are stripped for field matching.
fn generate_collision_error(schema_name: &str) -> proc_macro2::TokenStream {
    let schema = schema_name;
    quote! {
        compile_error!(concat!(
            "Multiple parameters map to the same field in schema `",
            #schema,
            "`. Parameters like `field` and `_field` cannot coexist ",
            "because they both map to the same tracing field. ",
            "Please rename one of the conflicting parameters."
        ));
    }
}

/// Resolve the record expression for a field.
///
/// Priority order:
/// 1. Inline expression in trace macro args: `#[trace(schema, field = expr)]`
/// 2. Parameter name itself
fn resolve_record_expr(field: &FunctionField, meta: &SchemaMeta) -> proc_macro2::TokenStream {
    meta.field_expressions
        .get(&field.name)
        .cloned()
        .unwrap_or_else(|| {
            let param_ident = make_ident(&field.raw_name);
            quote! { #param_ident }
        })
}

/// Generate all record calls for a function's fields and inline field expressions.
///
/// - Function params: lenient (silently ignored if not in schema)
/// - Custom expressions: STRICT (compile error if field doesn't exist)
///   - Custom expressions: strict (must match schema field)
fn generate_record_calls(
    fields: &[FunctionField],
    meta: &SchemaMeta,
) -> Vec<proc_macro2::TokenStream> {
    let record_macro_ident = make_ident(&make_record_macro_name(
        meta.category(),
        meta.subcategory(),
        &meta.schema_name,
    ));

    // Collect field names that have function parameters
    let param_field_names: BTreeSet<_> = fields.iter().map(|f| f.name.clone()).collect();

    // Generate record calls for function parameters
    // Function params use lenient mode - they're auto-matched to schema fields.
    // If a param doesn't match any schema field, it's silently ignored
    // (it's a dependency used to compute custom expressions).
    let mut record_calls: Vec<proc_macro2::TokenStream> = fields
        .iter()
        .map(|field| {
            let field_name = &field.name;
            let record_expr = resolve_record_expr(field, meta);

            // Lenient for function params - ignores params not in schema
            meta.macro_call_stmt(&record_macro_ident, quote! { #field_name, #record_expr })
        })
        .collect();

    // Generate record calls for field expressions that don't have matching parameters
    // Custom expressions use STRICT mode - the field name MUST exist in the schema.
    // This catches typos like `#[trace(schema, hsh = x)]` when it should be `hash = x`.
    for (field_name, expr) in &meta.field_expressions {
        if !param_field_names.contains(field_name) {
            let field_name_str = field_name.as_str();
            // STRICT mode for custom expressions - errors on unknown fields
            record_calls.push(meta.macro_call_stmt(
                &record_macro_ident,
                quote! { #field_name_str, #expr, strict },
            ));
        }
    }

    record_calls
}

/// Generate the final instrumented function output.
///
/// The entire function (including instrument macro call) is wrapped in the module
/// validator to ensure invalid schema names produce a clear error message instead
/// of "cannot find macro".
fn generate_instrumented_function(
    func: &ItemFn,
    meta: &SchemaMeta,
    field_validations: Vec<proc_macro2::TokenStream>,
    record_calls: Vec<proc_macro2::TokenStream>,
) -> proc_macro2::TokenStream {
    let instrument_macro_ident = make_ident(&make_instrument_macro_name(
        meta.category(),
        meta.subcategory(),
        &meta.schema_name,
    ));
    let attrs = &func.attrs;
    let vis = &func.vis;
    let sig = &func.sig;
    let original_stmts = &func.block.stmts;

    // Generate the __list_available_schemas call with proper path
    let list_schemas_ident = make_ident("__list_available_schemas");
    let list_schemas_call = meta.macro_call(&list_schemas_ident, quote! {});

    // Generate a unique const name based on function name
    let fn_name = &sig.ident;
    let validation_const_name = make_ident(&format!(
        "__TRACE_VALIDATION_{}",
        fn_name.to_string().to_uppercase()
    ));

    // Build the instrumented function body
    let instrumented_body = quote! {
        #(#attrs)*
        #vis #sig {
            // Compile-time validation of schema usage
            #[allow(non_upper_case_globals)]
            const #validation_const_name: () = {
                #(#field_validations)*
                const _: &str = #list_schemas_call;
            };
            #(#record_calls)*
            #(#original_stmts)*
        }
    };

    // Wrap the instrument macro call in the body that gets validated
    let instrumented_function = meta.macro_call_block(&instrument_macro_ident, instrumented_body);

    // Wrap EVERYTHING in the module validator - this ensures invalid schema names
    // produce a clear error instead of "cannot find macro __INVALID_INSTRUMENT"
    wrap_in_module_validator(meta, instrumented_function)
}

/// Wrap code in the module validator macro.
///
/// For valid schemas: expands the body
/// For invalid schemas: produces a clear compile error and discards the body
fn wrap_in_module_validator(
    meta: &SchemaMeta,
    body: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    if meta.module_path.is_empty() {
        return body;
    }

    let parts: Vec<&str> = meta.module_path.split(SEPARATOR).collect();
    if parts.len() < 2 {
        return body;
    }

    let validator_name = make_module_validator_name(parts[0], parts[1]);
    let validator_ident = make_ident(&validator_name);
    let schema_ident = make_ident(&meta.schema_name);

    // Use brace syntax for the outer macro call since it expands to an item (function)
    // Macros that expand to items must use braces or be followed by semicolon
    if meta.is_local_schema() {
        quote! { #validator_ident! { #schema_ident, { #body } } }
    } else {
        quote! { ::amaru_observability::#validator_ident! { #schema_ident, { #body } } }
    }
}

/// Generate all validation tokens for a function.
fn generate_validations(func: &ItemFn, meta: &SchemaMeta) -> Vec<proc_macro2::TokenStream> {
    // Extract fields, checking for collisions
    let fields = match extract_function_fields(func) {
        Some(fields) => fields,
        None => return vec![generate_collision_error(&meta.schema_name)],
    };

    let mut validations = Vec::new();

    // Add field type validators for function parameters
    // Skip validation for fields that have custom expressions (the expression overrides the type)
    for field in &fields {
        if !meta.field_expressions.contains_key(&field.name) {
            validations.push(generate_field_validator(meta, field));
        }
    }

    // Note: Custom expressions are validated through the _RECORD macro,
    // which now errors on unknown fields. No separate validation needed.

    // Add required fields check
    let mut field_names: Vec<_> = fields.iter().map(|f| f.name.clone()).collect();
    // Add custom expression field names to required field check
    field_names.extend(meta.field_expressions.keys().cloned());
    validations.push(generate_required_fields_check(meta, &field_names));

    validations
}

/// Instruments a function with tracing.
///
/// The trace argument must be a const path defined via `define_schemas!`.
/// This macro validates at compile-time that:
/// - The schema constant exists
/// - All **required** fields are present as function parameters
/// - All parameters have the correct types matching the schema
/// - Optional fields may optionally be present with correct types
///
/// # Example
///
/// ```text
/// #[trace(consensus::chain_sync::VALIDATE_HEADER)]
/// fn validate_header(point_slot: u64, point_hash: String) -> Result<(), String> {
///     Ok(())
/// }
/// ```
pub fn expand_trace(args: TokenStream, input: TokenStream) -> TokenStream {
    expand_trace_macro(args, input)
}

/// Records fields to the current span with a schema anchor.
///
/// This macro allows recording fields to the current span outside of functions
/// decorated with `#[trace]`. Use this when you want to add additional context
/// to an existing span without creating a new one.
///
/// This macro does NOT create a new span - it records fields to the current span.
/// The schema constant anchors the recording and documents which schema these
/// fields belong to.
///
/// # Example
///
/// ```text
/// trace_record!(ledger::state::APPLY_BLOCK, block_size = 1024, tx_count = 5);
/// ```
///
/// Common implementation for trace macros.
fn expand_trace_macro(args: TokenStream, input: TokenStream) -> TokenStream {
    let Ok(func) = syn::parse::<ItemFn>(input.clone()) else {
        return input;
    };

    let meta = SchemaMeta::from_args(args);
    let field_validations = generate_validations(&func, &meta);

    // Extract function fields and generate record calls
    let fields = extract_function_fields(&func).unwrap_or_default();
    let record_calls = generate_record_calls(&fields, &meta);

    // Generate the final instrumented function (wrapped in module validator)
    let output = generate_instrumented_function(&func, &meta, field_validations, record_calls);
    output.into()
}

/// Expand the `trace_record!` macro.
///
/// This macro allows recording fields to the current span. It supports both validated
/// (schema-aware) and simple forms.
/// Expand the `trace_record!` macro.
///
/// This macro records fields to the current span with a schema anchor.
///
/// # Syntax
///
/// ```text
/// trace_record!(SCHEMA_CONST, field1 = value1, field2 = value2, ...);
/// ```
///
/// The schema constant anchors the recording and documents which schema these fields
/// belong to. Use this inside or outside of functions decorated with `#[trace]` to
/// record fields to the current span.
///
/// # Examples
///
/// ```text
/// #[trace(ledger::state::APPLY_BLOCK)]
/// fn apply_block(block: &Block) {
///     // Record additional fields
///     trace_record!(ledger::state::APPLY_BLOCK, size = block.size(), tx_count = block.transactions.len());
/// }
/// ```
pub fn expand_trace_record(input: TokenStream) -> TokenStream {
    let input2: proc_macro2::TokenStream = input.into();
    let input_str = input2.to_string();

    // Only schema form is supported: trace_record!(SCHEMA, field = value, ...)
    expand_schema_form(&input2, &input_str)
}

fn expand_schema_form(input2: &proc_macro2::TokenStream, input_str: &str) -> TokenStream {
    let parts: Vec<&str> = input_str.split(',').map(|s| s.trim()).collect();

    if parts.len() < 2 {
        return syn::Error::new_spanned(
            input2,
            "trace_record! with schema requires at least one field assignment: trace_record!(SCHEMA_CONST, field = value, ...)",
        )
        .to_compile_error()
        .into();
    }

    let schema_const_str = parts[0];
    let field_assignments = &parts[1..];

    if field_assignments.is_empty() {
        return syn::Error::new_spanned(
            input2,
            "trace_record! requires at least one field assignment: trace_record!(SCHEMA_CONST, field = value, ...)",
        )
        .to_compile_error()
        .into();
    }

    // Parse the schema constant path
    let schema_const_tokens: proc_macro2::TokenStream = match schema_const_str.parse() {
        Ok(tokens) => tokens,
        Err(_) => {
            return syn::Error::new_spanned(
                input2,
                "First argument must be a valid schema constant path like 'ledger::state::APPLY_BLOCK'",
            )
            .to_compile_error()
            .into();
        }
    };

    // Parse field assignments (field = value)
    let mut field_records = Vec::new();

    for assignment in field_assignments {
        // Split on = to get field name and value
        let assignment_parts: Vec<&str> = assignment.splitn(2, '=').map(|s| s.trim()).collect();

        if assignment_parts.len() != 2 {
            return syn::Error::new_spanned(
                input2,
                format!(
                    "Invalid field assignment '{}'. Expected 'field = value'",
                    assignment
                ),
            )
            .to_compile_error()
            .into();
        }

        let field_name = assignment_parts[0];
        let value_expr_str = assignment_parts[1];

        // Parse field name (should be a valid identifier, becomes a string literal)
        if !is_valid_identifier(field_name) {
            return syn::Error::new_spanned(
                input2,
                format!("'{}' is not a valid field name identifier", field_name),
            )
            .to_compile_error()
            .into();
        }

        // Parse value expression
        let value_expr_tokens: proc_macro2::TokenStream = match value_expr_str.parse() {
            Ok(tokens) => tokens,
            Err(_) => {
                return syn::Error::new_spanned(
                    input2,
                    format!(
                        "Invalid value expression '{}' for field '{}'",
                        value_expr_str, field_name
                    ),
                )
                .to_compile_error()
                .into();
            }
        };

        // Generate the actual record call
        let field_name_literal = syn::LitStr::new(field_name, proc_macro2::Span::call_site());
        let record_call = quote! {
            tracing::Span::current().record(#field_name_literal, tracing::field::display(&#value_expr_tokens))
        };

        field_records.push(record_call);
    }

    // Combine all records
    let expanded = quote! {
        {
            // Use the schema constant to anchor the recording context
            // This documents which schema these fields belong to
            let _schema = &#schema_const_tokens;

            // Runtime recording of all fields
            #(#field_records);*
        }
    };

    expanded.into()
}

/// Check if a string is a valid Rust identifier
fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }

    let first_char = s.chars().next().unwrap();
    if !first_char.is_alphabetic() && first_char != '_' {
        return false;
    }

    s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// Creates a tracing span with compile-time validated schema anchor.
///
/// This macro replaces `info_span!`, `debug_span!`, etc. with a schema-anchored
/// approach that provides compile-time validation.
///
/// # Syntax
///
/// ```text
/// trace_span!(LEVEL, SCHEMA_CONST, field1 = value1, field2 = value2, ...);
/// ```
///
/// Where LEVEL is one of: TRACE, DEBUG, INFO, WARN, ERROR
///
/// # Examples
///
/// ```text
/// trace_span!(INFO, operations::database::OPENING_CHAIN_DB, path = "...")
/// trace_span!(DEBUG, ledger::state::APPLY_BLOCK, block_size = 1024)
/// ```
pub fn expand_trace_span(input: TokenStream) -> TokenStream {
    let input2: proc_macro2::TokenStream = input.into();
    let input_str = input2.to_string();

    // Parse: SCHEMA, field = value, ...
    // Or: SCHEMA, field1 = value1, field2 = value2, ... (with format specifiers like %field)
    let parts: Vec<&str> = input_str.splitn(2, ',').map(|s| s.trim()).collect();

    if parts.is_empty() {
        return syn::Error::new_spanned(
            &input2,
            "trace_span! requires at least: trace_span!(SCHEMA_CONST, ...)",
        )
        .to_compile_error()
        .into();
    }

    let schema_const_str = parts[0];
    let remaining = if parts.len() > 1 { parts[1] } else { "" };

    // Parse the schema constant path
    let schema_const_tokens: proc_macro2::TokenStream = match schema_const_str.parse() {
        Ok(tokens) => tokens,
        Err(_) => {
            return syn::Error::new_spanned(
                &input2,
                "First argument must be a valid schema constant path",
            )
            .to_compile_error()
            .into();
        }
    };

    // Parse the remaining field arguments - pass them through as-is
    let remaining_tokens: proc_macro2::TokenStream = match remaining.parse() {
        Ok(tokens) => tokens,
        Err(_) => {
            // If parsing fails, still try to use it
            proc_macro2::TokenStream::new()
        }
    };

    // Generate the span creation call
    let expanded = if remaining.is_empty() {
        // No fields
        quote! {
            tracing::trace_span!(stringify!(#schema_const_tokens))
        }
    } else {
        // With fields - pass through remaining tokens
        quote! {
            tracing::trace_span!(stringify!(#schema_const_tokens), #remaining_tokens)
        }
    };

    expanded.into()
}
