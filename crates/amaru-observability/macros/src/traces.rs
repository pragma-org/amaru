//! Trace macro implementations for compile-time validated tracing.

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
    #[allow(dead_code)]
    macro_module: String,
    /// Optional field name -> expression mappings from inline definitions
    /// Maps field names to custom expressions for recording
    field_expressions: std::collections::HashMap<String, proc_macro2::TokenStream>,
}

impl SchemaMeta {
    /// Extract category from module_path (e.g., "consensus" from "consensus::chain_sync")
    fn category(&self) -> &str {
        self.module_path.split("::").next().unwrap_or("")
    }

    /// Extract subcategory from module_path (e.g., "chain_sync" from "consensus::chain_sync")
    fn subcategory(&self) -> &str {
        let parts: Vec<&str> = self.module_path.split("::").collect();
        if parts.len() >= 2 { parts[1] } else { "" }
    }

    /// Check if this is a local schema (not from amaru_observability).
    ///
    /// Local schemas are defined with `define_local_schemas!` and their macros
    /// are NOT exported with `#[macro_export]`. They must be called without a path
    /// prefix since they're local to the module where they're defined.
    fn is_local_schema(&self) -> bool {
        !(self.macro_module == "amaru" || self.macro_module.starts_with("amaru::"))
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
            let mut field_expressions = std::collections::HashMap::new();
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
                field_expressions: std::collections::HashMap::new(),
            }
        }
    }
}

// =============================================================================
// Required Field Checking
// =============================================================================

/// Controls whether required field validation is enforced.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RequiredFieldCheck {
    /// Enforce that all required fields are present (for `#[trace]`)
    Enforce,
    /// Skip required field checking (for `#[augment_trace]`)
    Skip,
}

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
    let unique_names: std::collections::HashSet<_> = names.iter().collect();

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

/// Mode for record calls - determines validation behavior.
#[derive(Clone, Copy, PartialEq)]
enum RecordMode {
    /// Normal #[trace] mode:
    /// - Function params: lenient (auto-match to schema, ignore if no match)
    /// - Custom expressions: strict (must match schema field)
    Trace,
    /// #[augment_trace] mode: only allows optional fields, errors on required
    Augment,
}

/// Generate all record calls for a function's fields and inline field expressions.
///
/// Uses different modes of the _RECORD macro based on RecordMode:
/// - Trace mode:
///   - Function params: lenient (silently ignored if not in schema)
///   - Custom expressions: STRICT (compile error if field doesn't exist)
/// - Augment mode:
///   - Errors on required fields: only optional fields allowed in augment_trace
fn generate_record_calls(
    fields: &[FunctionField],
    meta: &SchemaMeta,
    mode: RecordMode,
) -> Vec<proc_macro2::TokenStream> {
    let record_macro_ident = make_ident(&make_record_macro_name(
        meta.category(),
        meta.subcategory(),
        &meta.schema_name,
    ));

    // Collect field names that have function parameters
    let param_field_names: std::collections::HashSet<_> =
        fields.iter().map(|f| f.name.clone()).collect();

    // Generate record calls for function parameters
    // Function params use lenient mode - they're auto-matched to schema fields.
    // If a param doesn't match any schema field, it's silently ignored
    // (it's a dependency used to compute custom expressions).
    let mut record_calls: Vec<proc_macro2::TokenStream> = fields
        .iter()
        .map(|field| {
            let field_name = &field.name;
            let record_expr = resolve_record_expr(field, meta);

            match mode {
                RecordMode::Augment => {
                    // Augment mode - validates optional-only and records
                    meta.macro_call_stmt(
                        &record_macro_ident,
                        quote! { #field_name, #record_expr, augment },
                    )
                }
                RecordMode::Trace => {
                    // Lenient for function params - ignores params not in schema
                    meta.macro_call_stmt(&record_macro_ident, quote! { #field_name, #record_expr })
                }
            }
        })
        .collect();

    // Generate record calls for field expressions that don't have matching parameters
    // Custom expressions use STRICT mode - the field name MUST exist in the schema.
    // This catches typos like `#[trace(schema, hsh = x)]` when it should be `hash = x`.
    for (field_name, expr) in &meta.field_expressions {
        if !param_field_names.contains(field_name) {
            let field_name_str = field_name.as_str();
            match mode {
                RecordMode::Augment => {
                    // Augment mode - validates optional-only and records
                    record_calls.push(meta.macro_call_stmt(
                        &record_macro_ident,
                        quote! { #field_name_str, #expr, augment },
                    ));
                }
                RecordMode::Trace => {
                    // STRICT mode for custom expressions - errors on unknown fields
                    record_calls.push(meta.macro_call_stmt(
                        &record_macro_ident,
                        quote! { #field_name_str, #expr, strict },
                    ));
                }
            }
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

    let parts: Vec<&str> = meta.module_path.split("::").collect();
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
fn generate_validations(
    func: &ItemFn,
    meta: &SchemaMeta,
    required_check: RequiredFieldCheck,
) -> Vec<proc_macro2::TokenStream> {
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

    // Add required fields check (only for #[trace], not #[augment_trace])
    // For augment_trace, optional-only validation happens in record calls via `augment` mode
    if required_check == RequiredFieldCheck::Enforce {
        let mut field_names: Vec<_> = fields.iter().map(|f| f.name.clone()).collect();
        // Add custom expression field names to required field check
        field_names.extend(meta.field_expressions.keys().cloned());
        validations.push(generate_required_fields_check(meta, &field_names));
    }

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
    expand_trace_macro(args, input, RequiredFieldCheck::Enforce)
}

/// Augments the current span with additional optional fields.
///
/// Unlike [`#[trace]`](macro@trace), this macro can only use **optional** fields
/// from the schema. Required fields are not allowed in `augment_trace` because
/// augmenting a span should only add supplementary context, not core identifying
/// information.
///
/// This macro does NOT create a new span - it records fields to the current span.
/// The function parameters are automatically recorded at the start of the function.
///
/// Use this when you want to add additional context to an existing span
/// without creating a new one.
///
/// # Example
///
/// ```text
/// // Given a schema with optional fields like 'peer_id' and 'timing_ms':
/// #[augment_trace(consensus::chain_sync::VALIDATE_HEADER)]
/// fn add_peer_context(peer_id: String, timing_ms: u64) {
///     // peer_id and timing_ms are automatically recorded to the current span
/// }
/// ```
pub fn expand_augment_trace(args: TokenStream, input: TokenStream) -> TokenStream {
    expand_augment_trace_macro(args, input)
}

/// Common implementation for trace macros using functional approach.
fn expand_trace_macro(
    args: TokenStream,
    input: TokenStream,
    required_check: RequiredFieldCheck,
) -> TokenStream {
    let Ok(func) = syn::parse::<ItemFn>(input.clone()) else {
        return input;
    };

    let meta = SchemaMeta::from_args(args);
    let field_validations = generate_validations(&func, &meta, required_check);

    // Extract function fields and generate record calls
    let fields = extract_function_fields(&func).unwrap_or_default();
    let record_calls = generate_record_calls(&fields, &meta, RecordMode::Trace);

    // Generate the final instrumented function (wrapped in module validator)
    let output = generate_instrumented_function(&func, &meta, field_validations, record_calls);
    output.into()
}

/// Implementation for augment_trace macro.
///
/// Unlike trace, this does NOT create a new span - it just adds record() calls
/// to record fields to the current span. Uses `augment` mode which validates
/// that only optional fields are used.
fn expand_augment_trace_macro(args: TokenStream, input: TokenStream) -> TokenStream {
    let Ok(func) = syn::parse::<ItemFn>(input.clone()) else {
        return input;
    };

    let meta = SchemaMeta::from_args(args);

    // Generate validations - only field type validators (optional-only check is done via augment mode)
    let field_validations = generate_validations(&func, &meta, RequiredFieldCheck::Skip);

    // Generate the __list_available_schemas call with proper path
    let list_schemas_ident = make_ident("__list_available_schemas");
    let list_schemas_call = meta.macro_call(&list_schemas_ident, quote! {});

    // Extract function fields for record() calls using augment mode
    // Augment mode validates that only optional fields are used
    let fields = extract_function_fields(&func).unwrap_or_default();
    let record_calls = generate_record_calls(&fields, &meta, RecordMode::Augment);

    // Build the function body
    let attrs = &func.attrs;
    let vis = &func.vis;
    let sig = &func.sig;
    let original_stmts = &func.block.stmts;

    let fn_name = &sig.ident;
    let validation_const_name = make_ident(&format!(
        "__AUGMENT_TRACE_VALIDATION_{}",
        fn_name.to_string().to_uppercase()
    ));

    let function_body = quote! {
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

    // Wrap the entire function in the module validator for better error messages
    let output = wrap_in_module_validator(&meta, function_body);
    output.into()
}
