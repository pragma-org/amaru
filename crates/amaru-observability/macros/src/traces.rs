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

use proc_macro::TokenStream;
use quote::quote;

use crate::utils::{
    make_assign_macro_name, make_ident, make_instrument_macro_name, make_module_validator_name, make_record_macro_name,
    make_require_macro_name, make_schema_field_count_const_name, make_schema_public_const_name, parse_full_schema_path,
};

const TRACE_SPAN_NAME_PREFIX: &str = "__amaru_trace_span";

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
}

const SEPARATOR: &str = "::";

impl SchemaMeta {
    /// Get all categories as a Vec<String> from module_path
    fn categories(&self) -> Vec<String> {
        self.module_path.split(SEPARATOR).map(|s| s.to_string()).collect()
    }

    /// Check if this is a local schema (not from amaru_observability).
    ///
    /// Local schemas are defined with `define_local_schemas!` and their macros
    /// are NOT exported with `#[macro_export]`. They are identified by a
    /// leading `self::` or `crate::` segment in the user-supplied schema path,
    /// which `parse_macro_module` collapses to `"self"` or `"crate"`.
    fn is_local_schema(&self) -> bool {
        matches!(self.macro_module.as_str(), "" | "self" | "crate")
    }

    /// Generate a macro call as a statement (with trailing semicolon).
    ///
    /// For exported macros: `::amaru_observability::macro_name!(...);`
    /// For local macros: `macro_name!(...);`
    fn macro_call_stmt(&self, macro_ident: &syn::Ident, args: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
        if self.is_local_schema() {
            quote! { #macro_ident!(#args); }
        } else {
            quote! { ::amaru_observability::#macro_ident!(#args); }
        }
    }

    fn macro_call_expr(&self, macro_ident: &syn::Ident, args: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
        if self.is_local_schema() {
            quote! { #macro_ident!(#args) }
        } else {
            quote! { ::amaru_observability::#macro_ident!(#args) }
        }
    }
}

/// Generate required fields checker invocation.
fn generate_required_fields_check(meta: &SchemaMeta, field_names: &[String]) -> proc_macro2::TokenStream {
    let categories = meta.categories();
    let require_macro = make_require_macro_name(&categories, &meta.schema_name);
    let require_ident = make_ident(&require_macro);
    let field_idents: Vec<_> = field_names.iter().map(|n| make_ident(n)).collect();

    meta.macro_call_stmt(&require_ident, quote! { #(#field_idents),* })
}

/// Wrap code in the module validator macro.
///
/// For valid schemas: expands the body
/// For invalid schemas: produces a clear compile error and discards the body
fn wrap_in_module_validator(meta: &SchemaMeta, body: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
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

    if meta.is_local_schema() {
        quote! {{ #validator_ident!(#schema_ident => #body) }}
    } else {
        quote! {{ ::amaru_observability::#validator_ident!(#schema_ident => #body) }}
    }
}

/// Produce the fully-qualified path token stream for an exported-schema item
/// (a public constant or field-count constant), inserting `amaru_observability`
/// and/or `amaru` prefixes as needed.
///
/// - Local schemas: emit the user-supplied path verbatim (relative to `self`/`crate`).
/// - Path starts with `amaru_observability::...`: emit `::path` unchanged.
/// - Path starts with `amaru::...`: prepend `::amaru_observability::`.
/// - Path has neither prefix: prepend `::amaru_observability::amaru::`.
fn build_exported_path(meta: &SchemaMeta, path: &syn::Path) -> proc_macro2::TokenStream {
    if meta.is_local_schema() {
        return quote! { #path };
    }

    let first = path.segments.first().map(|segment| segment.ident.to_string());

    if matches!(first.as_deref(), Some("amaru_observability")) {
        return quote! { ::#path };
    }

    let mut prefixed =
        syn::Path { leading_colon: Some(Default::default()), segments: syn::punctuated::Punctuated::new() };
    prefixed.segments.push(syn::PathSegment::from(make_ident("amaru_observability")));
    if !matches!(first.as_deref(), Some("amaru")) {
        prefixed.segments.push(syn::PathSegment::from(make_ident("amaru")));
    }
    for segment in path.segments.iter() {
        prefixed.segments.push(segment.clone());
    }
    quote! { #prefixed }
}

fn build_public_const_path(meta: &SchemaMeta, schema_path: &syn::Path) -> proc_macro2::TokenStream {
    let categories = meta.categories();
    let public_const_ident = make_ident(&make_schema_public_const_name(&categories, &meta.schema_name));
    let mut public_const_path = schema_path.clone();
    if let Some(last_segment) = public_const_path.segments.last_mut() {
        last_segment.ident = public_const_ident;
    }
    build_exported_path(meta, &public_const_path)
}

fn private_emit_guard_tokens() -> proc_macro2::TokenStream {
    quote! {
        let __amaru_emit_private = {
            static __AMARU_TRACE_EMIT_PRIVATE: ::std::sync::OnceLock<bool> = ::std::sync::OnceLock::new();
            *__AMARU_TRACE_EMIT_PRIVATE.get_or_init(|| {
                ::std::env::var("AMARU_TRACE_EMIT_PRIVATE").is_ok_and(|value| {
                    let value = value.trim();
                    !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
                })
            })
        };
    }
}

/// Records fields to the current span with a schema anchor.
///
/// This macro allows recording fields to the current span outside of code that
/// created a `debug_span!`. Use this when you want to add additional context
/// to an existing span without creating a new one.
///
/// This macro does NOT create a new span - it records fields to the current span.
/// The schema constant anchors the recording and documents which schema these
/// fields belong to.
///
/// # Example
///
/// ```text
/// trace_record!(ledger::block::APPLY, error = "invalid witness");
/// ```
///
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
/// belong to. Use this inside or outside of code that enters a `debug_span!` span to
/// record fields to the current span.
///
/// # Examples
///
/// ```text
/// fn apply_block(point_slot: u64, error: Option<&str>) {
///     let _span = debug_span!(ledger::block::APPLY, point_slot = point_slot);
///     let _guard = _span.enter();
///
///     if let Some(error) = error {
///         // Record additional context (no log event)
///         trace_record!(ledger::block::APPLY, error = error);
///
///         // Record and emit a debug log event
///         trace_record!(DEBUG, ledger::block::APPLY, error = error);
///     }
/// }
/// ```
pub fn expand_trace_record(input: TokenStream) -> TokenStream {
    if crate::is_trace_no_emit() {
        return quote! { { } }.into();
    }

    // Parse using syn to properly handle commas in expressions
    use syn::{
        Token,
        parse::{Parse, ParseStream},
    };

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
                        if matches!(ident_str.as_str(), "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR")
                            && checkpoint.peek(Token![,])
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

            Ok(TraceRecordArgs { level, schema_path, field_assignments })
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
    let full_path_tokens = quote! { #schema_const_tokens };
    let path_str: String = full_path_tokens.to_string().chars().filter(|c| !c.is_whitespace()).collect();
    let (schema_name, module_path, macro_module) = parse_full_schema_path(&path_str);
    let meta = SchemaMeta { schema_name: schema_name.to_owned(), module_path, macro_module: macro_module.to_owned() };

    let public_const_path = build_public_const_path(&meta, &args.schema_path);
    let schema_const_path = build_exported_path(&meta, &args.schema_path);
    let private_emit_guard = private_emit_guard_tokens();

    // Generate the expanded code - generate the full block based on whether a level is specified
    let expanded = if let Some(level_ident) = &args.level {
        let level_str = level_ident.to_string().to_lowercase();

        // Validate level
        if !matches!(level_str.as_str(), "trace" | "debug" | "info" | "warn" | "error") {
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
                #private_emit_guard

                if #public_const_path || __amaru_emit_private {
                    let _schema = &#schema_const_path;
                    #(#field_records)*
                    tracing::#level_macro!(#(#event_fields),*);
                }
            }
        }
    } else {
        // Without level: just record to span
        quote! {
            {
                #private_emit_guard

                if #public_const_path || __amaru_emit_private {
                    // Use the schema constant to anchor the recording context
                    // This documents which schema these fields belong to
                    let _schema = &#schema_const_path;

                    // Runtime recording of all fields
                    #(#field_records)*
                }
            }
        }
    };

    expanded.into()
}

/// Emits a tracing event with a compile-time validated schema anchor.
///
/// This macro emits a log event whose `target` and `name` are derived from the
/// schema constant, exactly like the spans created by `debug_span!`. The schema
/// path is validated at compile time, and emission is gated on the schema
/// visibility (public schemas always emit; private ones only when
/// `AMARU_TRACE_EMIT_PRIVATE` is set).
///
/// Fields are validated against the schema like span fields: required fields
/// must be present, unknown fields are rejected, and plain `field = value`
/// assignments are type-checked against the declared field type. Values are
/// rendered with `Display` by default (`Debug` for `?value`), so field values
/// do not need to implement `tracing::Value`.
///
/// # Syntax
///
/// ```text
/// trace_event!(LEVEL, SCHEMA, field = value, ...);   // type-checked, Display-rendered
/// trace_event!(LEVEL, SCHEMA, field = %value, ...);  // pre-formatted, Display-rendered
/// trace_event!(LEVEL, SCHEMA, field = ?value, ...);  // pre-formatted, Debug-rendered
/// trace_event!(LEVEL, SCHEMA, value, %value, ?value) // shorthands for the above
/// ```
///
/// # Example
///
/// ```text
/// trace_event!(ERROR, stores::ledger::accounts::RESET_MANY, ?credential, reason = "no account for given credential");
/// ```
pub fn expand_trace_event(input: TokenStream) -> TokenStream {
    if crate::is_trace_no_emit() {
        return quote! { { } }.into();
    }

    use syn::{
        Token,
        parse::{Parse, ParseStream},
    };

    enum TraceEventFormatter {
        /// `field = value`: type-checked against the schema, rendered with Display
        Typed,
        /// `field = %value`: pre-formatted by the caller, rendered with Display
        Display,
        /// `field = ?value`: pre-formatted by the caller, rendered with Debug
        Debug,
        /// `field = @value`: a ready-made `tracing::Value`, recorded as-is. This is the
        /// escape hatch for dynamically-absent fields (`Option<_>` / `field::Empty`).
        Value,
    }

    struct TraceEventField {
        name: syn::Ident,
        value: syn::Expr,
        formatter: TraceEventFormatter,
    }

    struct TraceEventArgs {
        level: syn::Ident,
        schema_path: syn::Path,
        fields: Vec<TraceEventField>,
    }

    impl Parse for TraceEventArgs {
        fn parse(input: ParseStream) -> syn::Result<Self> {
            let level: syn::Ident = input.parse()?;
            input.parse::<Token![,]>()?;
            let schema_path: syn::Path = input.parse()?;

            let mut fields = Vec::new();
            while input.peek(Token![,]) {
                input.parse::<Token![,]>()?;

                if input.is_empty() {
                    break;
                }

                // Shorthands: `%ident`, `?ident` and `@ident` name the field after the variable.
                if input.peek(Token![%]) || input.peek(Token![?]) || input.peek(Token![@]) {
                    let formatter = if input.peek(Token![%]) {
                        input.parse::<Token![%]>()?;
                        TraceEventFormatter::Display
                    } else if input.peek(Token![?]) {
                        input.parse::<Token![?]>()?;
                        TraceEventFormatter::Debug
                    } else {
                        input.parse::<Token![@]>()?;
                        TraceEventFormatter::Value
                    };
                    let name: syn::Ident = input.parse()?;
                    let value = syn::parse_quote!(#name);
                    fields.push(TraceEventField { name, value, formatter });
                    continue;
                }

                let name: syn::Ident = input.parse()?;

                // Bare `ident` records the variable under its own name.
                if !input.peek(Token![=]) {
                    let value = syn::parse_quote!(#name);
                    fields.push(TraceEventField { name, value, formatter: TraceEventFormatter::Typed });
                    continue;
                }

                input.parse::<Token![=]>()?;

                let formatter = if input.peek(Token![%]) {
                    input.parse::<Token![%]>()?;
                    TraceEventFormatter::Display
                } else if input.peek(Token![?]) {
                    input.parse::<Token![?]>()?;
                    TraceEventFormatter::Debug
                } else if input.peek(Token![@]) {
                    input.parse::<Token![@]>()?;
                    TraceEventFormatter::Value
                } else {
                    TraceEventFormatter::Typed
                };
                let value: syn::Expr = input.parse()?;

                fields.push(TraceEventField { name, value, formatter });
            }

            if !input.is_empty() {
                return Err(input.error("unexpected tokens after field assignments"));
            }

            Ok(TraceEventArgs { level, schema_path, fields })
        }
    }

    let args = match syn::parse::<TraceEventArgs>(input) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };

    let level_str = args.level.to_string().to_lowercase();
    if !matches!(level_str.as_str(), "trace" | "debug" | "info" | "warn" | "error") {
        return syn::Error::new_spanned(
            &args.level,
            "Invalid tracing level. Must be one of: TRACE, DEBUG, INFO, WARN, ERROR",
        )
        .to_compile_error()
        .into();
    }
    let level_macro = syn::Ident::new(&level_str, proc_macro2::Span::call_site());

    let schema_const_tokens = &args.schema_path;
    let full_path_tokens = quote! { #schema_const_tokens };
    let path_str: String = full_path_tokens.to_string().chars().filter(|c| !c.is_whitespace()).collect();
    let (schema_name, module_path, macro_module) = parse_full_schema_path(&path_str);
    let meta = SchemaMeta { schema_name: schema_name.to_owned(), module_path, macro_module: macro_module.to_owned() };

    let categories = meta.categories();
    let target = categories.iter().take(2).map(|part| part.as_str()).collect::<Vec<_>>().join("::");
    let name = categories
        .iter()
        .skip(2)
        .map(|part| part.to_lowercase())
        .chain(std::iter::once(meta.schema_name.to_lowercase()))
        .collect::<Vec<_>>()
        .join(".");
    let name_literal = syn::LitStr::new(&name, proc_macro2::Span::call_site());
    let target_literal = syn::LitStr::new(&target, proc_macro2::Span::call_site());

    let record_macro_ident = make_ident(&make_record_macro_name(&categories, &meta.schema_name));
    let field_names: Vec<_> = args.fields.iter().map(|field| field.name.to_string()).collect();
    let required_fields_check = generate_required_fields_check(&meta, &field_names);

    let value_bindings: Vec<_> = args
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let field_name = field.name.to_string();
            let expr = &field.value;
            let value_ident = make_ident(&format!("__amaru_trace_value_{index}"));
            let formatted_ident = make_ident(&format!("__amaru_trace_formatted_{index}"));
            let (validation_mode, formatter_binding) = match field.formatter {
                TraceEventFormatter::Typed => (
                    quote! { validate_value },
                    quote! { let #formatted_ident = ::tracing::field::display(&#value_ident); },
                ),
                TraceEventFormatter::Display => (
                    quote! { validate_event_display },
                    quote! { let #formatted_ident = ::tracing::field::display(&#value_ident); },
                ),
                TraceEventFormatter::Debug => (
                    quote! { validate_event_debug },
                    quote! { let #formatted_ident = ::tracing::field::debug(&#value_ident); },
                ),
                TraceEventFormatter::Value => {
                    (quote! { validate_event_value }, quote! { let #formatted_ident = #value_ident; })
                }
            };
            let validate_value_call =
                meta.macro_call_stmt(&record_macro_ident, quote! { #field_name, &#value_ident, #validation_mode });
            quote! {
                let #value_ident = &(#expr);
                #validate_value_call
                #formatter_binding
            }
        })
        .collect();

    let event_fields: Vec<_> = args
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let field_ident = &field.name;
            let formatted_ident = make_ident(&format!("__amaru_trace_formatted_{index}"));
            quote! { #field_ident = #formatted_ident }
        })
        .collect();

    let public_const_path = build_public_const_path(&meta, &args.schema_path);
    let private_emit_guard = private_emit_guard_tokens();

    let expanded = wrap_in_module_validator(
        &meta,
        quote! {{
            #required_fields_check
            #private_emit_guard

            if #public_const_path || __amaru_emit_private {
                #(#value_bindings)*

                ::tracing::#level_macro!(
                    name: #name_literal,
                    target: #target_literal,
                    message = #name_literal
                    #(, #event_fields)*
                );
            }
        }},
    );

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
/// debug_span!(operations::database::OPENING_CHAIN_DB, path = "...")
/// debug_span!(DEBUG, ledger::block::APPLY, block_size = 1024)
/// debug_span!(INFO, consensus::VALIDATE)
/// debug_span!(parent_context: &ctx, consensus::VALIDATE)
/// ```
pub fn expand_trace_span(input: TokenStream) -> TokenStream {
    if crate::is_trace_no_emit() {
        return quote! { tracing::Span::none() }.into();
    }

    // Parse using syn to properly handle commas in expressions
    use syn::{
        Token,
        parse::{Parse, ParseStream},
    };

    struct TraceSpanArgs {
        level: Option<syn::Ident>,
        parent: Option<TraceSpanParent>,
        schema_path: syn::Path,
        fields: Vec<TraceSpanField>,
    }

    enum TraceSpanParent {
        Root,
        Span(syn::Expr),
        Context(syn::Expr),
    }

    enum TraceSpanFormatter {
        Display,
        Debug,
    }

    struct TraceSpanField {
        name: String,
        validation_expr: proc_macro2::TokenStream,
        formatter: TraceSpanFormatter,
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
                        if matches!(ident_str.as_str(), "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR")
                            && checkpoint.peek(Token![,])
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

            let parent = if input.peek(syn::Ident) {
                let checkpoint = input.fork();
                match checkpoint.parse::<syn::Ident>() {
                    Ok(ident) if ident == "root" && checkpoint.peek(Token![,]) => {
                        let _: syn::Ident = input.parse()?;
                        input.parse::<Token![,]>()?;
                        Some(TraceSpanParent::Root)
                    }
                    Ok(ident) if ident == "parent" && checkpoint.peek(Token![:]) => {
                        let _: syn::Ident = input.parse()?;
                        input.parse::<Token![:]>()?;
                        let parent_expr: syn::Expr = input.parse()?;
                        input.parse::<Token![,]>()?;
                        Some(TraceSpanParent::Span(parent_expr))
                    }
                    Ok(ident) if ident == "parent_context" && checkpoint.peek(Token![:]) => {
                        let _: syn::Ident = input.parse()?;
                        input.parse::<Token![:]>()?;
                        let parent_expr: syn::Expr = input.parse()?;
                        input.parse::<Token![,]>()?;
                        Some(TraceSpanParent::Context(parent_expr))
                    }
                    _ => None,
                }
            } else {
                None
            };

            let schema_path: syn::Path = input.parse()?;
            let mut fields = Vec::new();

            // Parse comma-separated field assignments
            // We need to handle tracing format specifiers like %name or ?value
            while input.peek(Token![,]) {
                input.parse::<Token![,]>()?; // consume comma

                if input.is_empty() {
                    break;
                }

                let field_name: syn::Ident = input.parse()?;
                input.parse::<Token![=]>()?;
                let field_name_str = field_name.to_string();

                // Check for tracing format specifiers (%, ?, or expressions)
                let (validation_expr, formatter) = if input.peek(Token![%]) {
                    // Format specifier %field
                    input.parse::<Token![%]>()?;
                    let field_ref: syn::Ident = input.parse()?;
                    (quote! { #field_ref }, TraceSpanFormatter::Display)
                } else if input.peek(Token![?]) {
                    // Format specifier ?field
                    input.parse::<Token![?]>()?;
                    let field_ref: syn::Ident = input.parse()?;
                    (quote! { #field_ref }, TraceSpanFormatter::Debug)
                } else {
                    // Regular expression
                    let value_expr: syn::Expr = input.parse()?;
                    (quote! { #value_expr }, TraceSpanFormatter::Display)
                };

                fields.push(TraceSpanField { name: field_name_str, validation_expr, formatter });
            }

            // Ensure all input has been consumed - no trailing tokens
            // This prevents silent failures where invalid input is ignored
            if !input.is_empty() {
                return Err(input.error("unexpected tokens after field assignments"));
            }

            Ok(TraceSpanArgs { level, parent, schema_path, fields })
        }
    }

    let args = match syn::parse::<TraceSpanArgs>(input) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };
    let fields = &args.fields;

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
    let full_path_tokens = quote! { #schema_const_tokens };
    let path_str: String = full_path_tokens.to_string().chars().filter(|c| !c.is_whitespace()).collect();
    let (schema_name, module_path, macro_module) = parse_full_schema_path(&path_str);
    let meta = SchemaMeta { schema_name: schema_name.to_owned(), module_path, macro_module: macro_module.to_owned() };

    let categories = meta.categories();
    let record_macro_ident = make_ident(&make_record_macro_name(&categories, &meta.schema_name));
    let assign_macro_ident = make_ident(&make_assign_macro_name(&categories, &meta.schema_name));
    let field_count_const_ident = make_ident(&make_schema_field_count_const_name(&categories, &meta.schema_name));
    let mut field_count_path = args.schema_path.clone();
    if let Some(last_segment) = field_count_path.segments.last_mut() {
        last_segment.ident = field_count_const_ident;
    }
    let field_count_path = build_exported_path(&meta, &field_count_path);
    let public_const_path = build_public_const_path(&meta, &args.schema_path);
    let private_emit_guard = private_emit_guard_tokens();

    let span_name = make_ident(TRACE_SPAN_NAME_PREFIX);
    let value_bindings: Vec<_> = fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let field_name = field.name.as_str();
            let expr = &field.validation_expr;
            let value_ident = make_ident(&format!("__amaru_trace_value_{index}"));
            let formatted_ident = make_ident(&format!("__amaru_trace_formatted_{index}"));
            let validate_value_call =
                meta.macro_call_stmt(&record_macro_ident, quote! { #field_name, &#value_ident, validate_value });
            let formatter_binding = match field.formatter {
                TraceSpanFormatter::Display => {
                    quote! { let #formatted_ident = tracing::field::display(&#value_ident); }
                }
                TraceSpanFormatter::Debug => {
                    quote! { let #formatted_ident = tracing::field::debug(&#value_ident); }
                }
            };
            quote! {
                let #value_ident = #expr;
                #validate_value_call
                #formatter_binding
            }
        })
        .collect();

    let assign_calls: Vec<_> = fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let field_name = field.name.as_str();
            let formatted_ident = make_ident(&format!("__amaru_trace_formatted_{index}"));
            meta.macro_call_stmt(
                &assign_macro_ident,
                quote! { __amaru_span_values, #field_name, &#formatted_ident as &dyn tracing::field::Value },
            )
        })
        .collect();

    let required_field_names: Vec<_> = fields.iter().map(|field| field.name.clone()).collect();
    let required_fields_check = generate_required_fields_check(&meta, &required_field_names);

    let instrument_macro_ident = make_ident(&make_instrument_macro_name(&categories, &meta.schema_name));
    let span_parent = match &args.parent {
        Some(TraceSpanParent::Root) => Some(quote! { ::tracing::Span::none() }),
        Some(TraceSpanParent::Span(parent_expr)) => Some(quote! { #parent_expr }),
        Some(TraceSpanParent::Context(_)) | None => None,
    };
    let parent_context_expr = match &args.parent {
        Some(TraceSpanParent::Context(parent_expr)) => Some(parent_expr),
        Some(TraceSpanParent::Root | TraceSpanParent::Span(_)) | None => None,
    };

    let span_expr = if level_str == "trace" {
        if let Some(parent_expr) = span_parent {
            meta.macro_call_expr(
                &instrument_macro_ident,
                quote! { parent = #parent_expr, values = &__amaru_span_values[..] },
            )
        } else {
            meta.macro_call_expr(&instrument_macro_ident, quote! { values = &__amaru_span_values[..] })
        }
    } else {
        let level_const = match level_str.as_str() {
            "debug" => quote! { tracing::Level::DEBUG },
            "info" => quote! { tracing::Level::INFO },
            "warn" => quote! { tracing::Level::WARN },
            "error" => quote! { tracing::Level::ERROR },
            _ => quote! { tracing::Level::TRACE },
        };
        if let Some(parent_expr) = span_parent {
            meta.macro_call_expr(
                &instrument_macro_ident,
                quote! { parent = #parent_expr, level = #level_const, values = &__amaru_span_values[..] },
            )
        } else {
            meta.macro_call_expr(
                &instrument_macro_ident,
                quote! { level = #level_const, values = &__amaru_span_values[..] },
            )
        }
    };
    let (opentelemetry_path, tracing_opentelemetry_path) = if meta.is_local_schema() {
        (quote! { ::opentelemetry }, quote! { ::tracing_opentelemetry })
    } else {
        (quote! { ::amaru_observability::opentelemetry }, quote! { ::amaru_observability::tracing_opentelemetry })
    };
    let parent_context_attachment = parent_context_expr
        .map(|parent_expr| {
            quote! {
                {
                    let __amaru_parent_context = #parent_expr;
                    let __amaru_otel_context = __amaru_parent_context.context();
                    let __amaru_has_valid_parent = {
                        use #opentelemetry_path::trace::TraceContextExt as _;
                        __amaru_otel_context.span().span_context().is_valid()
                    };
                    {
                        use #tracing_opentelemetry_path::OpenTelemetrySpanExt as _;
                        if let ::std::result::Result::Err(error) = #span_name.set_parent(__amaru_otel_context)
                            && __amaru_has_valid_parent
                        {
                            ::tracing::warn!(%error, "failed to set span parent context");
                        }
                    }
                }
            }
        })
        .unwrap_or_else(|| quote! {});

    let expanded = wrap_in_module_validator(
        &meta,
        quote! {{
            #required_fields_check
            #private_emit_guard

            if !#public_const_path && !__amaru_emit_private {
                ::tracing::Span::none()
            } else {
                #(#value_bindings)*

                let mut __amaru_span_values: Vec<::tracing::__macro_support::Option<&dyn ::tracing::field::Value>> = vec![
                    ::tracing::__macro_support::Option::Some(
                        &tracing::field::Empty as &dyn ::tracing::field::Value
                    );
                    #field_count_path
                ];

                #(#assign_calls)*

                let #span_name = #span_expr;
                #parent_context_attachment

                #span_name
            }
        }},
    );

    expanded.into()
}
