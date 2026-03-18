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
    make_require_macro_name, make_schema_field_count_const_name, make_schema_public_const_name,
    parse_full_schema_path,
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
    /// are NOT exported with `#[macro_export]`. They must be called without a path
    /// prefix since they're local to the module where they're defined.
    fn is_local_schema(&self) -> bool {
        self.macro_module != "amaru" && !self.macro_module.ends_with("::amaru")
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

/// Records fields to the current span with a schema anchor.
///
/// This macro allows recording fields to the current span outside of code that
/// created a `trace_span!`. Use this when you want to add additional context
/// to an existing span without creating a new one.
///
/// This macro does NOT create a new span - it records fields to the current span.
/// The schema constant anchors the recording and documents which schema these
/// fields belong to.
///
/// # Example
///
/// ```text
/// trace_record!(ledger::state::APPLY_BLOCK, error = "invalid witness");
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
/// belong to. Use this inside or outside of code that enters a `trace_span!` span to
/// record fields to the current span.
///
/// # Examples
///
/// ```text
/// fn apply_block(point_slot: u64, error: Option<&str>) {
///     let _span = trace_span!(ledger::state::APPLY_BLOCK, point_slot = point_slot);
///     let _guard = _span.enter();
///
///     if let Some(error) = error {
///         // Record additional context (no log event)
///         trace_record!(ledger::state::APPLY_BLOCK, error = error);
///
///         // Record and emit a debug log event
///         trace_record!(DEBUG, ledger::state::APPLY_BLOCK, error = error);
///     }
/// }
/// ```
pub fn expand_trace_record(input: TokenStream) -> TokenStream {
    if crate::is_trace_noop() {
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

    let categories = meta.categories();
    let public_const_ident = make_ident(&make_schema_public_const_name(&categories, &meta.schema_name));
    let mut public_const_path = args.schema_path.clone();
    if let Some(last_segment) = public_const_path.segments.last_mut() {
        last_segment.ident = public_const_ident;
    }
    let public_const_path = if meta.is_local_schema() {
        quote! { #public_const_path }
    } else {
        let needs_prefix = public_const_path.segments.first().map(|segment| segment.ident == "amaru").unwrap_or(false);

        if needs_prefix {
            let mut prefixed_path =
                syn::Path { leading_colon: Some(Default::default()), segments: syn::punctuated::Punctuated::new() };
            prefixed_path.segments.push(syn::PathSegment::from(make_ident("amaru_observability")));
            for segment in public_const_path.segments.iter() {
                prefixed_path.segments.push(segment.clone());
            }
            quote! { #prefixed_path }
        } else {
            quote! { ::#public_const_path }
        }
    };

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
                let __amaru_emit_private = {
                    static __AMARU_TRACE_EMIT_PRIVATE: ::std::sync::OnceLock<bool> = ::std::sync::OnceLock::new();
                    *__AMARU_TRACE_EMIT_PRIVATE.get_or_init(|| {
                        ::std::env::var("AMARU_TRACE_EMIT_PRIVATE").is_ok_and(|value| {
                            let value = value.trim();
                            !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
                        })
                    })
                };

                if #public_const_path || __amaru_emit_private {
                    let _schema = &#schema_const_tokens;
                    #(#field_records)*
                    tracing::#level_macro!(#(#event_fields),*);
                }
            }
        }
    } else {
        // Without level: just record to span
        quote! {
            {
                let __amaru_emit_private = {
                    static __AMARU_TRACE_EMIT_PRIVATE: ::std::sync::OnceLock<bool> = ::std::sync::OnceLock::new();
                    *__AMARU_TRACE_EMIT_PRIVATE.get_or_init(|| {
                        ::std::env::var("AMARU_TRACE_EMIT_PRIVATE").is_ok_and(|value| {
                            let value = value.trim();
                            !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
                        })
                    })
                };

                if #public_const_path || __amaru_emit_private {
                    // Use the schema constant to anchor the recording context
                    // This documents which schema these fields belong to
                    let _schema = &#schema_const_tokens;

                    // Runtime recording of all fields
                    #(#field_records)*
                }
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
    use syn::{
        Token,
        parse::{Parse, ParseStream},
    };

    struct TraceSpanArgs {
        level: Option<syn::Ident>,
        schema_path: syn::Path,
        fields: Vec<TraceSpanField>,
    }

    struct TraceSpanField {
        name: String,
        validation_expr: proc_macro2::TokenStream,
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
                let validation_expr = if input.peek(Token![%]) {
                    // Format specifier %field
                    input.parse::<Token![%]>()?;
                    let field_ref: syn::Ident = input.parse()?;
                    quote! { #field_ref }
                } else if input.peek(Token![?]) {
                    // Format specifier ?field
                    input.parse::<Token![?]>()?;
                    let field_ref: syn::Ident = input.parse()?;
                    quote! { #field_ref }
                } else {
                    // Regular expression
                    let value_expr: syn::Expr = input.parse()?;
                    quote! { #value_expr }
                };

                fields.push(TraceSpanField { name: field_name_str, validation_expr });
            }

            // Ensure all input has been consumed - no trailing tokens
            // This prevents silent failures where invalid input is ignored
            if !input.is_empty() {
                return Err(input.error("unexpected tokens after field assignments"));
            }

            Ok(TraceSpanArgs { level, schema_path, fields })
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
    let field_count_path = if meta.is_local_schema() {
        quote! { #field_count_path }
    } else {
        let needs_prefix = field_count_path.segments.first().map(|segment| segment.ident == "amaru").unwrap_or(false);

        if needs_prefix {
            let mut prefixed_path =
                syn::Path { leading_colon: Some(Default::default()), segments: syn::punctuated::Punctuated::new() };
            prefixed_path.segments.push(syn::PathSegment::from(make_ident("amaru_observability")));
            for segment in field_count_path.segments.iter() {
                prefixed_path.segments.push(segment.clone());
            }
            quote! { #prefixed_path }
        } else {
            quote! { ::#field_count_path }
        }
    };
    let public_const_ident = make_ident(&make_schema_public_const_name(&categories, &meta.schema_name));
    let mut public_const_path = args.schema_path.clone();
    if let Some(last_segment) = public_const_path.segments.last_mut() {
        last_segment.ident = public_const_ident;
    }
    let public_const_path = if meta.is_local_schema() {
        quote! { #public_const_path }
    } else {
        let needs_prefix = public_const_path.segments.first().map(|segment| segment.ident == "amaru").unwrap_or(false);

        if needs_prefix {
            let mut prefixed_path =
                syn::Path { leading_colon: Some(Default::default()), segments: syn::punctuated::Punctuated::new() };
            prefixed_path.segments.push(syn::PathSegment::from(make_ident("amaru_observability")));
            for segment in public_const_path.segments.iter() {
                prefixed_path.segments.push(segment.clone());
            }
            quote! { #prefixed_path }
        } else {
            quote! { ::#public_const_path }
        }
    };

    let span_name = make_ident(TRACE_SPAN_NAME_PREFIX);
    let value_bindings: Vec<_> = args
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let field_name = field.name.as_str();
            let expr = &field.validation_expr;
            let value_ident = make_ident(&format!("__amaru_trace_value_{index}"));
            let display_ident = make_ident(&format!("__amaru_trace_display_{index}"));
            let validate_value_call =
                meta.macro_call_stmt(&record_macro_ident, quote! { #field_name, &#value_ident, validate_value });
            quote! {
                let #value_ident = #expr;
                #validate_value_call
                let #display_ident = tracing::field::display(&#value_ident);
            }
        })
        .collect();

    let assign_calls: Vec<_> = args
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let field_name = field.name.as_str();
            let display_ident = make_ident(&format!("__amaru_trace_display_{index}"));
            meta.macro_call_stmt(
                &assign_macro_ident,
                quote! { __amaru_span_values, #field_name, &#display_ident as &dyn tracing::field::Value },
            )
        })
        .collect();

    let required_field_names: Vec<_> = args.fields.iter().map(|field| field.name.clone()).collect();
    let required_fields_check = generate_required_fields_check(&meta, &required_field_names);

    let instrument_macro_ident = make_ident(&make_instrument_macro_name(&categories, &meta.schema_name));
    let span_expr = if level_str == "trace" {
        meta.macro_call_expr(&instrument_macro_ident, quote! { values = &__amaru_span_values[..] })
    } else {
        let level_const = match level_str.as_str() {
            "debug" => quote! { tracing::Level::DEBUG },
            "info" => quote! { tracing::Level::INFO },
            "warn" => quote! { tracing::Level::WARN },
            "error" => quote! { tracing::Level::ERROR },
            _ => quote! { tracing::Level::TRACE },
        };
        meta.macro_call_expr(
            &instrument_macro_ident,
            quote! { level = #level_const, values = &__amaru_span_values[..] },
        )
    };

    let expanded = wrap_in_module_validator(
        &meta,
        quote! {{
            #required_fields_check
            #(#value_bindings)*

            let __amaru_emit_private = {
                static __AMARU_TRACE_EMIT_PRIVATE: ::std::sync::OnceLock<bool> = ::std::sync::OnceLock::new();
                *__AMARU_TRACE_EMIT_PRIVATE.get_or_init(|| {
                    ::std::env::var("AMARU_TRACE_EMIT_PRIVATE").is_ok_and(|value| {
                        let value = value.trim();
                        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
                    })
                })
            };

            if !#public_const_path && !__amaru_emit_private {
                ::tracing::Span::none()
            } else {
                let mut __amaru_span_values: Vec<::tracing::__macro_support::Option<&dyn ::tracing::field::Value>> = vec![
                    ::tracing::__macro_support::Option::Some(
                        &tracing::field::Empty as &dyn ::tracing::field::Value
                    );
                    #field_count_path
                ];

                #(#assign_calls)*

                let #span_name = #span_expr;

                #span_name
            }
        }},
    );

    expanded.into()
}
