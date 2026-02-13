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
/// - `debug: amaru::consensus::chain_sync::VALIDATE_HEADER`
/// - `debug: amaru::consensus::chain_sync::VALIDATE_HEADER, hash = compute_hash()`
struct SchemaMeta {
    /// The schema name (e.g., `VALIDATE_HEADER`)
    schema_name: String,
    /// The module path for tracing target (e.g., `consensus::chain_sync`)
    module_path: String,
    /// The macro module path (e.g., `amaru` or `my_crate::schemas::amaru`)
    /// Used to determine if this is a local schema (non-amaru prefix) or exported schema
    macro_module: String,
    /// The tracing level (trace, debug, info, warn, error). Defaults to "trace"
    level: String,
    /// Optional field name -> expression mappings from inline definitions
    /// Maps field names to custom expressions for recording
    field_expressions: BTreeMap<String, proc_macro2::TokenStream>,
}

const SEPARATOR: &str = "::";

impl SchemaMeta {
    /// Get all categories as a Vec<String> from module_path
    fn categories(&self) -> Vec<String> {
        self.module_path
            .split(SEPARATOR)
            .map(|s| s.to_string())
            .collect()
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
    /// - `DEBUG, amaru::consensus::validate_header::EVOLVE_NONCE`
    /// - `DEBUG, amaru::consensus::validate_header::EVOLVE_NONCE, hash = compute_hash()`
    /// - `my_crate::amaru::test::sub::MY_SCHEMA, field = expr()`
    ///
    /// Returns `Err(TokenStream)` with compile error if parsing fails.
    fn from_args(args: TokenStream) -> Result<Self, TokenStream> {
        // First, try to extract just the schema path for backward compatibility
        let args_str = args.to_string();

        // Check if we have level specification or field expressions by attempting to parse
        // We'll try the new syntax first if it looks promising
        let has_field_exprs = args_str.contains('=');

        // Try to parse with the new syntax that supports levels and field expressions
        // This will fail gracefully if the input doesn't match the expected format
        let try_new_syntax = has_field_exprs || {
            // Quick check: does it start with an uppercase identifier followed by comma?
            // This avoids trying the new parser for simple schema paths
            let trimmed = args_str.trim();
            trimmed.starts_with("TRACE")
                || trimmed.starts_with("DEBUG")
                || trimmed.starts_with("INFO")
                || trimmed.starts_with("WARN")
                || trimmed.starts_with("ERROR")
        };

        if try_new_syntax {
            // New syntax: parse with syn for proper expression handling
            use syn::{Token, parse::Parse, parse::ParseStream};

            struct MacroArgs {
                level: Option<syn::Ident>,
                schema_path: syn::Path,
                field_exprs: Vec<(syn::Ident, syn::Expr)>,
            }

            impl Parse for MacroArgs {
                fn parse(input: ParseStream) -> syn::Result<Self> {
                    // Check if first token is a level identifier followed by a comma
                    let level = if input.peek(syn::Ident) {
                        let checkpoint = input.fork();
                        match checkpoint.parse::<syn::Ident>() {
                            Ok(ident) => {
                                let ident_str = ident.to_string();
                                // Check if this is actually a level identifier AND it's followed by a comma
                                if matches!(
                                    ident_str.as_str(),
                                    "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR"
                                ) && checkpoint.peek(Token![,])
                                {
                                    // It's a level specification
                                    let level_ident: syn::Ident = input.parse()?;
                                    input.parse::<Token![,]>()?;
                                    Some(level_ident)
                                } else {
                                    None
                                }
                            }
                            Err(_) => None,
                        }
                    } else {
                        None
                    };

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
                        level,
                        schema_path,
                        field_exprs,
                    })
                }
            }

            match syn::parse::<MacroArgs>(args.clone()) {
                Ok(parsed) => {
                    // Validate and convert level (accept uppercase and convert to lowercase)
                    let level = if let Some(level_ident) = parsed.level {
                        let level_str = level_ident.to_string().to_lowercase();
                        match level_str.as_str() {
                            "trace" | "debug" | "info" | "warn" | "error" => level_str,
                            _ => {
                                return Err(syn::Error::new_spanned(
                                    &level_ident,
                                    "Invalid tracing level. Must be one of: TRACE, DEBUG, INFO, WARN, ERROR",
                                )
                                .to_compile_error()
                                .into());
                            }
                        }
                    } else {
                        "trace".to_string()
                    };

                    // Convert path to string and parse schema components
                    let schema_path = &parsed.schema_path;
                    let full_path_tokens = quote! { #schema_path };
                    let path_str = full_path_tokens.to_string().replace(' ', "");
                    let (schema_name, module_path, macro_module) =
                        parse_full_schema_path(&path_str);

                    // Convert field expressions to map
                    let mut field_expressions = BTreeMap::new();
                    for (field_name, expr) in parsed.field_exprs {
                        field_expressions.insert(field_name.to_string(), quote! { #expr });
                    }

                    Ok(SchemaMeta {
                        schema_name: schema_name.to_owned(),
                        module_path,
                        macro_module: macro_module.to_owned(),
                        level,
                        field_expressions,
                    })
                }
                Err(_) => {
                    // Fall back to legacy syntax if new syntax parsing fails
                    let schema_path = match syn::parse::<syn::Path>(args) {
                        Ok(p) => p,
                        Err(err) => return Err(err.to_compile_error().into()),
                    };
                    let full_path_tokens = quote! { #schema_path };
                    let path_str = full_path_tokens.to_string().replace(' ', "");
                    let (schema_name, module_path, macro_module) =
                        parse_full_schema_path(&path_str);

                    Ok(SchemaMeta {
                        schema_name: schema_name.to_owned(),
                        module_path,
                        macro_module: macro_module.to_owned(),
                        level: "trace".to_string(),
                        field_expressions: BTreeMap::new(),
                    })
                }
            }
        } else {
            // Legacy syntax: just a schema path
            let schema_path = match syn::parse::<syn::Path>(args) {
                Ok(p) => p,
                Err(err) => return Err(err.to_compile_error().into()),
            };
            let full_path_tokens = quote! { #schema_path };
            let path_str = full_path_tokens.to_string().replace(' ', "");
            let (schema_name, module_path, macro_module) = parse_full_schema_path(&path_str);

            Ok(SchemaMeta {
                schema_name: schema_name.to_owned(),
                module_path,
                macro_module: macro_module.to_owned(),
                level: "trace".to_string(),
                field_expressions: BTreeMap::new(),
            })
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
    let categories = meta.categories();
    let record_macro = make_record_macro_name(&categories, &meta.schema_name);
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
    let categories = meta.categories();
    let require_macro = make_require_macro_name(&categories, &meta.schema_name);
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
    let categories = meta.categories();
    let record_macro_ident = make_ident(&make_record_macro_name(&categories, &meta.schema_name));

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
/// The entire function (including span creation) is wrapped in the module
/// validator to ensure invalid schema names produce a clear error message instead
/// of "cannot find macro".
fn generate_instrumented_function(
    func: &ItemFn,
    meta: &SchemaMeta,
    field_validations: Vec<proc_macro2::TokenStream>,
    record_calls: Vec<proc_macro2::TokenStream>,
) -> proc_macro2::TokenStream {
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

    // Use the pre-generated instrument macro for all levels.
    // This ensures span fields are always declared (required for Span::record to work).
    let categories = meta.categories();
    let instrument_macro_name = make_instrument_macro_name(&categories, &meta.schema_name);
    let instrument_macro_ident = make_ident(&instrument_macro_name);

    let func_body = quote! {
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

    let instrumented_body = if meta.level == "trace" {
        // Default level (TRACE) - use the simple form
        meta.macro_call_block(&instrument_macro_ident, func_body)
    } else {
        // Explicit level - pass it to the instrument macro
        let level_const = match meta.level.as_str() {
            "debug" => quote! { tracing::Level::DEBUG },
            "info" => quote! { tracing::Level::INFO },
            "warn" => quote! { tracing::Level::WARN },
            "error" => quote! { tracing::Level::ERROR },
            _ => quote! { tracing::Level::TRACE },
        };
        meta.macro_call_block(
            &instrument_macro_ident,
            quote! { level = #level_const, { #func_body } },
        )
    };

    // Wrap the entire function in the module validator if needed
    if meta.module_path.is_empty() {
        instrumented_body
    } else {
        wrap_in_module_validator(meta, instrumented_body)
    }
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

    let categories: Vec<String> = parts.iter().map(|s| s.to_string()).collect();
    let validator_name = make_module_validator_name(&categories);
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
    if crate::is_trace_noop() {
        return input;
    }

    let Ok(func) = syn::parse::<ItemFn>(input.clone()) else {
        return input;
    };

    let meta = match SchemaMeta::from_args(args) {
        Ok(m) => m,
        Err(err) => return err,
    };
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
/// This macro records fields to the current span with a schema anchor, and optionally
/// emits a log event at a specified level.
///
/// # Syntax
///
/// ```text
/// trace_record!(SCHEMA_CONST, field1 = value1, field2 = value2, ...);
/// trace_record!(DEBUG, SCHEMA_CONST, field1 = value1, field2 = value2, ...);
/// ```
///
/// When a level is specified (TRACE, DEBUG, INFO, WARN, ERROR), the macro will:
/// 1. Record fields to the current span
/// 2. Emit a log event at the specified level with those fields
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
///     // Record additional fields (no log event)
///     trace_record!(ledger::state::APPLY_BLOCK, size = block.size(), tx_count = block.transactions.len());
///     
///     // Record and emit a debug log event
///     trace_record!(DEBUG, ledger::state::APPLY_BLOCK, size = block.size());
/// }
/// ```
pub fn expand_trace_record(input: TokenStream) -> TokenStream {
    if crate::is_trace_noop() {
        return quote! { { } }.into();
    }

    // Parse using syn to properly handle commas in expressions
    use syn::{Token, parse::Parse, parse::ParseStream};

    struct TraceRecordArgs {
        level: Option<syn::Ident>,
        schema_path: syn::Path,
        field_assignments: Vec<(syn::Ident, syn::Expr)>,
    }

    impl Parse for TraceRecordArgs {
        fn parse(input: ParseStream) -> syn::Result<Self> {
            // Check if first token is a level identifier followed by a comma
            let level = if input.peek(syn::Ident) {
                let checkpoint = input.fork();
                match checkpoint.parse::<syn::Ident>() {
                    Ok(ident) => {
                        let ident_str = ident.to_string();
                        // Check if this is actually a level identifier AND it's followed by a comma
                        if matches!(
                            ident_str.as_str(),
                            "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR"
                        ) && checkpoint.peek(Token![,])
                        {
                            // It's a level specification
                            let level_ident: syn::Ident = input.parse()?;
                            input.parse::<Token![,]>()?;
                            Some(level_ident)
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                }
            } else {
                None
            };

            let schema_path: syn::Path = input.parse()?;
            let mut field_assignments = Vec::new();

            // Parse comma-separated field = value pairs
            while input.peek(Token![,]) {
                input.parse::<Token![,]>()?; // consume comma

                if input.is_empty() {
                    break;
                }

                let field_name: syn::Ident = input.parse()?;
                input.parse::<Token![=]>()?;
                let value_expr: syn::Expr = input.parse()?;

                field_assignments.push((field_name, value_expr));
            }

            Ok(TraceRecordArgs {
                level,
                schema_path,
                field_assignments,
            })
        }
    }

    let args = match syn::parse::<TraceRecordArgs>(input) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };

    if args.field_assignments.is_empty() {
        return syn::Error::new_spanned(
            &args.schema_path,
            "trace_record! requires at least one field assignment: trace_record!(SCHEMA_CONST, field = value, ...)",
        )
        .to_compile_error()
        .into();
    }

    // Generate record calls and event fields for each field
    let mut field_records = Vec::new();
    let mut event_fields = Vec::new();

    for (field_name, value_expr) in &args.field_assignments {
        let field_name_str = field_name.to_string();
        let field_name_literal = syn::LitStr::new(&field_name_str, proc_macro2::Span::call_site());
        let record_call = quote! {
            tracing::Span::current().record(#field_name_literal, tracing::field::display(&#value_expr));
        };
        field_records.push(record_call);

        // Store field for event emission
        let event_field = quote! { #field_name = %#value_expr };
        event_fields.push(event_field);
    }

    let schema_const_tokens = &args.schema_path;

    // Generate the expanded code - generate the full block based on whether a level is specified
    let expanded = if let Some(level_ident) = &args.level {
        let level_str = level_ident.to_string().to_lowercase();

        // Validate level
        if !matches!(
            level_str.as_str(),
            "trace" | "debug" | "info" | "warn" | "error"
        ) {
            return syn::Error::new_spanned(
                level_ident,
                "Invalid tracing level. Must be one of: TRACE, DEBUG, INFO, WARN, ERROR",
            )
            .to_compile_error()
            .into();
        }

        // Create the level macro identifier (trace, debug, info, warn, error)
        let level_macro = syn::Ident::new(&level_str, proc_macro2::Span::call_site());

        // Generate the code once with the level macro identifier
        quote! {
            {
                let _schema = &#schema_const_tokens;
                #(#field_records)*
                tracing::#level_macro!(#(#event_fields),*);
            }
        }
    } else {
        // Without level: just record to span
        quote! {
            {
                // Use the schema constant to anchor the recording context
                // This documents which schema these fields belong to
                let _schema = &#schema_const_tokens;

                // Runtime recording of all fields
                #(#field_records)*
            }
        }
    };

    expanded.into()
}

/// Creates a tracing span with compile-time validated schema anchor.
///
/// This macro creates spans with a schema-anchored approach that provides
/// compile-time validation. Supports custom log levels.
///
/// # Example
///
/// ```text
/// trace_span!(operations::database::OPENING_CHAIN_DB, path = "...")
/// trace_span!(DEBUG, ledger::state::APPLY_BLOCK, block_size = 1024)
/// trace_span!(INFO, consensus::VALIDATE)
/// ```
pub fn expand_trace_span(input: TokenStream) -> TokenStream {
    if crate::is_trace_noop() {
        return quote! { tracing::Span::none() }.into();
    }

    // Parse using syn to properly handle commas in expressions
    use syn::{Token, parse::Parse, parse::ParseStream};

    struct TraceSpanArgs {
        level: Option<syn::Ident>,
        schema_path: syn::Path,
        field_tokens: Vec<proc_macro2::TokenStream>,
    }

    impl Parse for TraceSpanArgs {
        fn parse(input: ParseStream) -> syn::Result<Self> {
            // Check if first token is a level identifier followed by a comma
            let level = if input.peek(syn::Ident) {
                let checkpoint = input.fork();
                match checkpoint.parse::<syn::Ident>() {
                    Ok(ident) => {
                        let ident_str = ident.to_string();
                        // Check if this is actually a level identifier AND it's followed by a comma
                        if matches!(
                            ident_str.as_str(),
                            "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR"
                        ) && checkpoint.peek(Token![,])
                        {
                            // It's a level specification
                            let level_ident: syn::Ident = input.parse()?;
                            input.parse::<Token![,]>()?;
                            Some(level_ident)
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                }
            } else {
                None
            };

            let schema_path: syn::Path = input.parse()?;
            let mut field_tokens = Vec::new();

            // Parse comma-separated field assignments
            // We need to handle tracing format specifiers like %name or ?value
            while input.peek(Token![,]) {
                input.parse::<Token![,]>()?; // consume comma

                if input.is_empty() {
                    break;
                }

                let field_name: syn::Ident = input.parse()?;
                input.parse::<Token![=]>()?;

                // Check for tracing format specifiers (%, ?, or expressions)
                let value_token = if input.peek(Token![%]) {
                    // Format specifier %field
                    input.parse::<Token![%]>()?;
                    let field_ref: syn::Ident = input.parse()?;
                    quote! { %#field_ref }
                } else if input.peek(Token![?]) {
                    // Format specifier ?field
                    input.parse::<Token![?]>()?;
                    let field_ref: syn::Ident = input.parse()?;
                    quote! { ?#field_ref }
                } else {
                    // Regular expression
                    let value_expr: syn::Expr = input.parse()?;
                    quote! { &#value_expr }
                };

                let field_token = quote! {
                    #field_name = #value_token
                };
                field_tokens.push(field_token);
            }

            // Ensure all input has been consumed - no trailing tokens
            // This prevents silent failures where invalid input is ignored
            if !input.is_empty() {
                return Err(input.error("unexpected tokens after field assignments"));
            }

            Ok(TraceSpanArgs {
                level,
                schema_path,
                field_tokens,
            })
        }
    }

    let args = match syn::parse::<TraceSpanArgs>(input) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };

    // Validate and convert level (accept uppercase and convert to lowercase)
    let level_str = if let Some(level_ident) = &args.level {
        let level_str = level_ident.to_string().to_lowercase();
        match level_str.as_str() {
            "trace" | "debug" | "info" | "warn" | "error" => level_str,
            _ => {
                return syn::Error::new_spanned(
                    level_ident,
                    "Invalid tracing level. Must be one of: TRACE, DEBUG, INFO, WARN, ERROR",
                )
                .to_compile_error()
                .into();
            }
        }
    } else {
        "trace".to_string()
    };

    let schema_const_tokens = &args.schema_path;
    let level_macro = syn::Ident::new(
        &format!("{}_span", level_str),
        proc_macro2::Span::call_site(),
    );

    // Generate the span creation call
    // Uses the schema constant value directly (not stringified) for:
    // 1. Compile-time validation that the constant exists
    // 2. Proper &'static str value as span name
    let expanded = if args.field_tokens.is_empty() {
        // No fields - just schema constant as span name
        quote! {
            tracing::#level_macro!(#schema_const_tokens)
        }
    } else {
        // With fields - pass them directly to tracing::{level}_span!
        let field_tokens = &args.field_tokens;
        quote! {
            tracing::#level_macro!(#schema_const_tokens, #(#field_tokens),*)
        }
    };

    expanded.into()
}
