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

use std::collections::BTreeMap;

use proc_macro::TokenStream;
use quote::{ToTokens, quote};
use syn::{
    Attribute, Ident, Token, Type, braced,
    parse::{Parse, ParseStream},
    spanned::Spanned,
};

use crate::utils::{
    format_field_spec, make_assign_macro_name, make_ident, make_instrument_macro_name, make_module_validator_name,
    make_record_macro_name, make_require_macro_name, make_required_field_check_macro_name,
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

/// How a schema field is rendered onto the tracing wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldRender {
    /// Typed primitive / `String`, or `Serialize + JsonSchema` (CBOR).
    Typed,
    /// `Display` → string (`%Type` in the schema).
    Display,
    /// `Debug` → string (`?Type` in the schema).
    Debug,
}

/// A field within a schema definition.
#[derive(Clone)]
struct SchemaField {
    /// Field name with the span from the schema definition.
    name: Ident,
    /// Field type AST with spans from the schema definition.
    ty: Type,
    /// Rendering chosen in the schema (`%` / `?` / default).
    render: FieldRender,
}

impl SchemaField {
    fn name_str(&self) -> String {
        self.name.to_string()
    }

    fn type_str(&self) -> String {
        type_to_string(&self.ty)
    }

    /// Field-name literal carrying the definition span (go-to-definition / find-usages).
    fn name_lit(&self) -> syn::LitStr {
        syn::LitStr::new(&self.name_str(), self.name.span())
    }

    /// Rust-type literal carrying the definition span of the type tokens.
    fn type_lit(&self) -> syn::LitStr {
        syn::LitStr::new(&self.type_str(), self.ty.span())
    }

    /// How this field is transported across the `tracing` boundary.
    fn transport_kind(&self) -> FieldTransportKind {
        match self.render {
            FieldRender::Display => FieldTransportKind::DisplayStr,
            FieldRender::Debug => FieldTransportKind::DebugStr,
            FieldRender::Typed => match self.type_str().as_str() {
                "bool" => FieldTransportKind::Bool,
                "i64" | "i32" | "i16" | "i8" | "isize" => FieldTransportKind::I64,
                "u64" | "u32" | "u16" | "u8" | "usize" => FieldTransportKind::U64,
                "f64" | "f32" => FieldTransportKind::F64,
                "String" | "&str" => FieldTransportKind::Str,
                _ => FieldTransportKind::Cbor,
            },
        }
    }
}

/// Wire representation for a schema field value.
#[derive(Clone, Copy)]
enum FieldTransportKind {
    Bool,
    I64,
    U64,
    F64,
    Str,
    DisplayStr,
    DebugStr,
    /// Serialized with cbor4ii and recorded via `record_bytes`.
    Cbor,
}

/// A complete schema definition.
#[derive(Clone)]
struct Schema {
    /// Whether this schema is explicitly public.
    /// Schemas are private by default unless marked `public`.
    public: bool,
    /// Category path components (idents with original spans).
    categories: Vec<Ident>,
    /// Schema name in SCREAMING_SNAKE_CASE (ident with original span).
    name: Ident,
    /// Optional description from doc comment(s).
    description: Option<String>,
    /// Functional tags, each recorded as a boolean `amaru.tag.<name>` span attribute.
    tags: Vec<Ident>,
    /// Fields that must be present.
    required_fields: Vec<SchemaField>,
    /// Fields that may optionally be present.
    optional_fields: Vec<SchemaField>,
}

impl Schema {
    fn name_str(&self) -> String {
        self.name.to_string()
    }

    fn category_strings(&self) -> Vec<String> {
        self.categories.iter().map(|c| c.to_string()).collect()
    }

    /// Get the target path by joining categories with "::"
    fn target_path(&self) -> String {
        self.category_strings().join("::")
    }

    /// Get the full schema path including the name
    fn full_path(&self) -> String {
        if self.categories.is_empty() {
            self.name_str()
        } else {
            format!("{}::{}", self.target_path(), self.name_str())
        }
    }

    /// Get the names of all required fields.
    fn required_field_names(&self) -> Vec<String> {
        self.required_fields.iter().map(|f| f.name_str()).collect()
    }

    /// Generate the validation string format: "R|req_fields|O|opt_fields"
    fn validation_string(&self) -> String {
        let required = format_field_list(&self.required_fields);
        let optional = format_field_list(&self.optional_fields);
        format!("R|{required}|O|{optional}")
    }

    fn event_name(&self) -> String {
        self.categories
            .iter()
            .skip(2)
            .map(|part| part.to_string().to_lowercase())
            .chain(std::iter::once(self.name_str().to_lowercase()))
            .collect::<Vec<_>>()
            .join(".")
    }

    fn event_target(&self) -> String {
        self.categories.iter().take(2).map(|part| part.to_string()).collect::<Vec<_>>().join("::")
    }
}

/// Render a type to a stable string without whitespace (matches the historical format).
fn type_to_string(ty: &Type) -> String {
    ty.to_token_stream().to_string().chars().filter(|c| !c.is_whitespace()).collect()
}

// =============================================================================
// Parser (syn-based)
// =============================================================================

/// Top-level input: one or more category blocks.
struct SchemaFile {
    categories: Vec<CategoryNode>,
}

/// A nested category (`ident { ... }`).
struct CategoryNode {
    name: Ident,
    items: Vec<CategoryItem>,
}

/// Items that may appear inside a category body.
enum CategoryItem {
    Tags(Vec<Ident>),
    Category(CategoryNode),
    Schema(SchemaNode),
}

/// A schema definition (`[public] NAME { ... }`).
struct SchemaNode {
    attrs: Vec<Attribute>,
    public: bool,
    name: Ident,
    items: Vec<SchemaItem>,
}

/// Items that may appear inside a schema body.
enum SchemaItem {
    Tags(Vec<Ident>),
    Field {
        /// Field doc comments are accepted for source documentation; not emitted today.
        #[allow(dead_code)]
        attrs: Vec<Attribute>,
        required: bool,
        name: Ident,
        ty: Type,
        render: FieldRender,
    },
}

impl Parse for SchemaFile {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut categories = Vec::new();
        while !input.is_empty() {
            // Disallow bare tags / schemas / fields at the root.
            let attrs = input.call(Attribute::parse_outer)?;
            if !attrs.is_empty() {
                return Err(syn::Error::new(
                    attrs[0].span(),
                    "doc comments at the root of define_schemas! are not allowed; place them on a schema",
                ));
            }
            if input.peek(Token![pub]) || peek_keyword(input, "public") || peek_keyword(input, "tags") {
                return Err(input.error("schema definitions and tags must appear inside a category block"));
            }
            categories.push(input.parse()?);
        }
        if categories.is_empty() {
            return Err(input.error("expected at least one category block"));
        }
        Ok(SchemaFile { categories })
    }
}

impl Parse for CategoryNode {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        if is_schema_name(&name) {
            return Err(syn::Error::new(
                name.span(),
                format!(
                    "expected a category name (lowercase identifier), found schema-like name '{}'. \
                     Schema definitions must appear inside a category block.",
                    name
                ),
            ));
        }
        let content;
        braced!(content in input);
        let items = parse_category_body(&content)?;
        Ok(CategoryNode { name, items })
    }
}

fn parse_category_body(input: ParseStream) -> syn::Result<Vec<CategoryItem>> {
    let mut items = Vec::new();
    while !input.is_empty() {
        let attrs = input.call(Attribute::parse_outer)?;

        if peek_keyword(input, "tags") {
            if !attrs.is_empty() {
                return Err(syn::Error::new(attrs[0].span(), "`tags:` declarations cannot have attributes"));
            }
            items.push(CategoryItem::Tags(parse_tags_decl(input)?));
            continue;
        }

        // Optional `public` keyword before a schema.
        let public = if peek_keyword(input, "public") {
            let public_ident: Ident = input.parse()?;
            if public_ident != "public" {
                return Err(syn::Error::new(public_ident.span(), "expected `public`"));
            }
            true
        } else {
            false
        };

        let name: Ident = input.parse()?;

        if is_schema_name(&name) {
            let content;
            braced!(content in input);
            let schema_items = parse_schema_body(&content)?;
            items.push(CategoryItem::Schema(SchemaNode { attrs, public, name, items: schema_items }));
        } else {
            if public {
                return Err(syn::Error::new(
                    name.span(),
                    format!(
                        "Invalid use of `public` before category '{}'. Only schema definitions may be marked public.",
                        name
                    ),
                ));
            }
            if !attrs.is_empty() {
                return Err(syn::Error::new(
                    attrs[0].span(),
                    "doc comments on categories are not supported; place them on schema definitions",
                ));
            }
            let content;
            braced!(content in input);
            let nested = parse_category_body(&content)?;
            items.push(CategoryItem::Category(CategoryNode { name, items: nested }));
        }
    }
    Ok(items)
}

fn parse_schema_body(input: ParseStream) -> syn::Result<Vec<SchemaItem>> {
    let mut items = Vec::new();
    while !input.is_empty() {
        let attrs = input.call(Attribute::parse_outer)?;

        if peek_keyword(input, "tags") {
            if !attrs.is_empty() {
                return Err(syn::Error::new(attrs[0].span(), "`tags:` declarations cannot have attributes"));
            }
            items.push(SchemaItem::Tags(parse_tags_decl(input)?));
            continue;
        }

        let kind: Ident = input.parse()?;
        let required = match kind.to_string().as_str() {
            "required" => true,
            "optional" => false,
            other => {
                return Err(syn::Error::new(
                    kind.span(),
                    format!("expected `required`, `optional`, or `tags:` inside schema body, found '{other}'"),
                ));
            }
        };

        // Reject block syntax: required { ... }
        if input.peek(syn::token::Brace) {
            return Err(syn::Error::new(
                kind.span(),
                "Block syntax for required/optional fields is not supported. Use prefix syntax instead: \
                 `required field_name: Type` (repeat for each field)",
            ));
        }

        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;
        let render = if input.peek(Token![%]) {
            input.parse::<Token![%]>()?;
            FieldRender::Display
        } else if input.peek(Token![?]) {
            input.parse::<Token![?]>()?;
            FieldRender::Debug
        } else {
            FieldRender::Typed
        };
        let ty: Type = input.parse()?;
        // Optional trailing comma.
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }

        items.push(SchemaItem::Field { attrs, required, name, ty, render });
    }
    Ok(items)
}

fn parse_tags_decl(input: ParseStream) -> syn::Result<Vec<Ident>> {
    let tags_kw: Ident = input.parse()?;
    if tags_kw != "tags" {
        return Err(syn::Error::new(tags_kw.span(), "expected `tags`"));
    }
    input.parse::<Token![:]>()?;

    let mut tags = Vec::new();
    loop {
        // Stop if the next token cannot start a tag name (next declaration / end of body).
        if input.is_empty() || input.peek(Token![#]) || !input.peek(Ident) {
            break;
        }
        // A following item looks like `ident {` (category/schema) or a keyword field prefix.
        if input.peek2(syn::token::Brace)
            || peek_keyword(input, "required")
            || peek_keyword(input, "optional")
            || peek_keyword(input, "public")
            || peek_keyword(input, "tags")
        {
            break;
        }

        let tag: Ident = input.parse().map_err(|_| {
            syn::Error::new(input.span(), "Invalid tag. Tags must be lowercase identifiers (e.g. `tags: cpu, io`).")
        })?;

        if is_schema_name(&tag) || matches!(tag.to_string().as_str(), "required" | "optional" | "public" | "tags") {
            return Err(syn::Error::new(
                tag.span(),
                format!("Invalid tag '{}'. Tags must be lowercase identifiers (e.g. `tags: cpu, io`).", tag),
            ));
        }

        if tags.iter().any(|existing| existing == &tag) {
            return Err(syn::Error::new(tag.span(), format!("Duplicate tag '{tag}' in tags declaration")));
        }
        tags.push(tag);

        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            // Trailing comma before the next declaration is allowed.
            continue;
        }
        break;
    }

    if tags.is_empty() {
        return Err(syn::Error::new(tags_kw.span(), "expected at least one tag after `tags:`"));
    }
    Ok(tags)
}

fn peek_keyword(input: ParseStream, name: &str) -> bool {
    input.peek(Ident) && input.fork().parse::<Ident>().ok().is_some_and(|ident| ident == name)
}

fn is_schema_name(ident: &Ident) -> bool {
    ident.to_string().chars().next().is_some_and(char::is_uppercase)
}

fn docs_from_attrs(attrs: &[Attribute]) -> Option<String> {
    let mut lines = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("doc")
            && let syn::Meta::NameValue(meta) = &attr.meta
            && let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &meta.value
        {
            let line = s.value();
            // syn includes a leading space from `/// text`
            let trimmed = line.strip_prefix(' ').unwrap_or(&line).trim();
            if !trimmed.is_empty() {
                lines.push(trimmed.to_string());
            }
        }
    }
    if lines.is_empty() { None } else { Some(lines.join(" ")) }
}

/// Flatten the parsed AST into a list of schemas with inherited tags, collecting errors.
fn extract_schemas(file: SchemaFile) -> (Vec<Schema>, Vec<syn::Error>) {
    let mut schemas = Vec::new();
    let mut errors = Vec::new();

    for category in file.categories {
        flatten_category(category, Vec::new(), &[], &mut schemas, &mut errors);
    }

    for schema in &schemas {
        if schema.description.is_none() {
            errors.push(syn::Error::new(
                schema.name.span(),
                format!(
                    "Schema '{}' is missing a description. Add a doc comment (///) above the schema definition.",
                    schema.name
                ),
            ));
        }
    }

    (schemas, errors)
}

fn flatten_category(
    category: CategoryNode,
    mut path: Vec<Ident>,
    inherited_tags: &[Ident],
    schemas: &mut Vec<Schema>,
    errors: &mut Vec<syn::Error>,
) {
    path.push(category.name);
    let mut module_tags: Option<Vec<Ident>> = None;

    for item in category.items {
        match item {
            CategoryItem::Tags(tags) => {
                // Innermost tags declaration wins for subsequent schemas at this level.
                module_tags = Some(tags);
            }
            CategoryItem::Category(nested) => {
                let tags = module_tags.as_deref().unwrap_or(inherited_tags);
                flatten_category(nested, path.clone(), tags, schemas, errors);
            }
            CategoryItem::Schema(node) => {
                let effective_inherited = module_tags.as_deref().unwrap_or(inherited_tags);
                match build_schema(node, path.clone(), effective_inherited) {
                    Ok(schema) => schemas.push(schema),
                    Err(errs) => errors.extend(errs),
                }
            }
        }
    }
}

fn build_schema(node: SchemaNode, categories: Vec<Ident>, inherited_tags: &[Ident]) -> Result<Schema, Vec<syn::Error>> {
    let mut errors = Vec::new();
    let description = docs_from_attrs(&node.attrs);

    let mut tags: Option<Vec<Ident>> = None;
    let mut required_fields = Vec::new();
    let mut optional_fields = Vec::new();

    for item in node.items {
        match item {
            SchemaItem::Tags(t) => {
                if tags.is_some() {
                    errors.push(syn::Error::new(
                        t.first().map(|i| i.span()).unwrap_or_else(proc_macro2::Span::call_site),
                        format!("Duplicate tags declaration in schema {}", node.name),
                    ));
                } else {
                    tags = Some(t);
                }
            }
            SchemaItem::Field { attrs: _field_docs, required, name, ty, render } => {
                let name_str = name.to_string();
                if matches!(name_str.as_str(), "name" | "schema" | "message") {
                    errors.push(syn::Error::new(
                        name.span(),
                        format!(
                            "Reserved field '{}' in schema {}. The tracing macros manage this field internally.",
                            name, node.name
                        ),
                    ));
                    continue;
                }

                let duplicate =
                    required_fields.iter().chain(optional_fields.iter()).any(|f: &SchemaField| f.name == name);
                if duplicate {
                    errors.push(syn::Error::new(
                        name.span(),
                        format!("Duplicate field '{}' in schema {}", name, node.name),
                    ));
                    continue;
                }

                let field = SchemaField { name, ty, render };
                if required {
                    required_fields.push(field);
                } else {
                    optional_fields.push(field);
                }
            }
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    let tags = tags.unwrap_or_else(|| inherited_tags.to_vec());

    Ok(Schema { public: node.public, categories, name: node.name, description, tags, required_fields, optional_fields })
}

// =============================================================================
// Code Generation Helpers
// =============================================================================

/// Format field list as "name:type,name:type,...".
fn format_field_list(fields: &[SchemaField]) -> String {
    fields.iter().map(|f| format_field_spec(&f.name_str(), &f.type_str())).collect::<Vec<_>>().join(",")
}

enum AccessorKind {
    Bool,
    F64,
    I64,
    U16,
    U32,
    U64,
    Usize,
    Str,
}

impl AccessorKind {
    fn return_type(&self) -> proc_macro2::TokenStream {
        match self {
            Self::Bool => quote! { bool },
            Self::F64 => quote! { f64 },
            Self::I64 => quote! { i64 },
            Self::U16 => quote! { u16 },
            Self::U32 => quote! { u32 },
            Self::U64 => quote! { u64 },
            Self::Usize => quote! { usize },
            Self::Str => quote! { &'record str },
        }
    }

    fn optional_return_type(&self) -> proc_macro2::TokenStream {
        let return_type = self.return_type();
        quote! { ::std::option::Option<#return_type> }
    }

    fn trait_call(&self, field_name: &str) -> proc_macro2::TokenStream {
        match self {
            Self::Bool => quote! { ::amaru_observability::RecordFields::bool(record, #field_name) },
            Self::F64 => quote! { ::amaru_observability::RecordFields::f64(record, #field_name) },
            Self::I64 => quote! { ::amaru_observability::RecordFields::i64(record, #field_name) },
            Self::U16 => quote! { ::amaru_observability::RecordFields::u16(record, #field_name) },
            Self::U32 => quote! { ::amaru_observability::RecordFields::u32(record, #field_name) },
            Self::U64 => quote! { ::amaru_observability::RecordFields::u64(record, #field_name) },
            Self::Usize => quote! { ::amaru_observability::RecordFields::usize(record, #field_name) },
            Self::Str => quote! { ::amaru_observability::RecordFields::str(record, #field_name) },
        }
    }
}

fn accessor_kind(field: &SchemaField) -> AccessorKind {
    let ty = field.type_str();
    match ty.as_str() {
        "bool" => AccessorKind::Bool,
        "f64" => AccessorKind::F64,
        "i64" => AccessorKind::I64,
        "u16" => AccessorKind::U16,
        "u32" => AccessorKind::U32,
        "u64" => AccessorKind::U64,
        "usize" => AccessorKind::Usize,
        "String" => AccessorKind::Str,
        "amaru_kernel::Epoch" | "amaru_kernel::Lovelace" | "amaru_kernel::Slot" => AccessorKind::U64,
        "Epoch" | "Lovelace" | "Slot" => AccessorKind::U64,
        _ => AccessorKind::Str,
    }
}

// =============================================================================
// Macro Generation
// =============================================================================

/// Generate the required fields checker macro for a schema.
fn generate_required_fields_macro(schema: &Schema, config: &GenerationConfig) -> proc_macro2::TokenStream {
    let categories = schema.category_strings();
    let schema_name_str = schema.name_str();
    let require_macro_name = make_require_macro_name(&categories, &schema_name_str);
    let require_ident = make_ident(&require_macro_name);
    let macro_export = config.macro_export_attr();
    let crate_prefix = config.crate_prefix();

    let required_names = schema.required_field_names();

    if required_names.is_empty() {
        return quote! {
            #macro_export
            #[doc(hidden)]
            macro_rules! #require_ident {
                ($($fields:ident),* $(,)?) => {};
            }
        };
    }

    let required_list = required_names.join(", ");
    let schema_name = &schema_name_str;

    // Use the original field idents so pattern arms carry definition spans.
    let field_idents: Vec<&Ident> = schema.required_fields.iter().map(|f| &f.name).collect();

    let mut helper_macros = Vec::new();

    for field_ident in &field_idents {
        let field_name_str = field_ident.to_string();
        let helper_name =
            make_ident(&make_required_field_check_macro_name(&categories, &schema_name_str, &field_name_str));

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

    let helper_calls: Vec<_> = required_names
        .iter()
        .map(|field_name| {
            let helper_name =
                make_ident(&make_required_field_check_macro_name(&categories, &schema_name_str, field_name));
            quote! { #crate_prefix #helper_name!($($fields)*); }
        })
        .collect();

    quote! {
        #(#helper_macros)*

        #macro_export
        #[doc(hidden)]
        macro_rules! #require_ident {
            ($($fields:ident),* $(,)?) => {
                #(#helper_calls)*
            };
        }
    }
}

/// Generate the span helper macro for a schema.
fn generate_instrument_macro(schema: &Schema, config: &GenerationConfig) -> proc_macro2::TokenStream {
    let categories = schema.category_strings();
    let schema_name_str = schema.name_str();
    let macro_name = make_instrument_macro_name(&categories, &schema_name_str);
    let macro_ident = make_ident(&macro_name);
    let macro_export = config.macro_export_attr();
    let crate_prefix = config.crate_prefix();

    let full_path: Vec<String> = categories.iter().cloned().chain(std::iter::once(schema_name_str.clone())).collect();
    let target = full_path.iter().take(2).cloned().collect::<Vec<_>>().join("::");
    let name = full_path.iter().skip(2).map(|part| part.to_lowercase()).collect::<Vec<_>>().join(".");

    let all_fields: Vec<_> = schema.required_fields.iter().chain(schema.optional_fields.iter()).collect();
    let mut field_name_literals: Vec<_> = all_fields.iter().map(|field| field.name_lit()).collect();
    let schema_field_count = field_name_literals.len();
    for tag in &schema.tags {
        let tag_attr = format!("amaru.tag.{}", tag);
        field_name_literals.push(syn::LitStr::new(&tag_attr, tag.span()));
    }
    let field_count = field_name_literals.len();

    let values_setup = if schema.tags.is_empty() {
        quote! {
            let __amaru_default_values = [
                ::amaru_observability::tracing::__macro_support::Option::Some(
                    &::amaru_observability::tracing::field::Empty as &dyn ::amaru_observability::tracing::field::Value
                );
                #field_count
            ];
            let __amaru_values = __amaru_values.unwrap_or(&__amaru_default_values);
        }
    } else {
        let tag_slots = (schema_field_count..field_count).map(|slot| {
            quote! {
                __amaru_all_values[#slot] =
                    ::amaru_observability::tracing::__macro_support::Option::Some(&true as &dyn ::amaru_observability::tracing::field::Value);
            }
        });
        quote! {
            let mut __amaru_all_values = [
                ::amaru_observability::tracing::__macro_support::Option::Some(
                    &::amaru_observability::tracing::field::Empty as &dyn ::amaru_observability::tracing::field::Value
                );
                #field_count
            ];
            if let ::amaru_observability::tracing::__macro_support::Option::Some(__amaru_given) = __amaru_values {
                for (__amaru_slot, __amaru_given_value) in __amaru_all_values.iter_mut().zip(__amaru_given.iter()) {
                    *__amaru_slot = *__amaru_given_value;
                }
            }
            #(#tag_slots)*
            let __amaru_values = &__amaru_all_values[..];
        }
    };

    let span_expr = quote! {{
        use ::amaru_observability::tracing::__macro_support::Callsite as _;

        static __CALLSITE: ::amaru_observability::tracing::callsite::DefaultCallsite = {
            static META: ::amaru_observability::tracing::Metadata<'static> = ::amaru_observability::tracing::Metadata::new(
                #name,
                #target,
                $level,
                ::amaru_observability::tracing::__macro_support::Option::Some(file!()),
                ::amaru_observability::tracing::__macro_support::Option::Some(line!()),
                ::amaru_observability::tracing::__macro_support::Option::Some(module_path!()),
                ::amaru_observability::tracing::field::FieldSet::new(
                    &[#(#field_name_literals),*],
                    ::amaru_observability::tracing::callsite::Identifier(&__CALLSITE),
                ),
                ::amaru_observability::tracing::metadata::Kind::SPAN,
            );
            ::amaru_observability::tracing::callsite::DefaultCallsite::new(&META)
        };

        #values_setup

        __CALLSITE.register();

        #[allow(unused_assignments)]
        let mut interest = ::amaru_observability::tracing::subscriber::Interest::never();
        if $level <= ::amaru_observability::tracing::level_filters::STATIC_MAX_LEVEL
            && $level <= ::amaru_observability::tracing::level_filters::LevelFilter::current()
            && {
                interest = __CALLSITE.interest();
                !interest.is_never()
            }
            && ::amaru_observability::tracing::__macro_support::__is_enabled(__CALLSITE.metadata(), interest)
        {
            let meta = __CALLSITE.metadata();
            let __amaru_values = &meta.fields().value_set_all(__amaru_values);
            match __amaru_parent {
                ::std::option::Option::Some(parent) => ::amaru_observability::tracing::Span::child_of(parent.id(), meta, __amaru_values),
                ::std::option::Option::None => ::amaru_observability::tracing::Span::new(meta, __amaru_values),
            }
        } else {
            ::amaru_observability::tracing::__macro_support::__disabled_span(__CALLSITE.metadata())
        }
    }};

    quote! {
        #macro_export
        #[doc(hidden)]
        macro_rules! #macro_ident {
            (parent = $parent:expr, level = $level:expr, values = $values_expr:expr) => {
                {
                    let __amaru_parent = ::std::option::Option::Some($parent);
                    let __amaru_values = ::std::option::Option::Some($values_expr);
                    #span_expr
                }
            };
            (parent = $parent:expr, values = $values_expr:expr) => {
                #crate_prefix #macro_ident!(parent = $parent, level = ::amaru_observability::tracing::Level::TRACE, values = $values_expr)
            };
            (parent = $parent:expr, level = $level:expr) => {
                {
                    let __amaru_parent = ::std::option::Option::Some($parent);
                    let __amaru_values: ::std::option::Option<&[::amaru_observability::tracing::__macro_support::Option<&dyn ::amaru_observability::tracing::field::Value>]> = ::std::option::Option::None;
                    #span_expr
                }
            };
            (parent = $parent:expr) => {
                #crate_prefix #macro_ident!(parent = $parent, level = ::amaru_observability::tracing::Level::TRACE)
            };
            (level = $level:expr, values = $values_expr:expr) => {
                {
                    let __amaru_parent: ::std::option::Option<::amaru_observability::tracing::Span> = ::std::option::Option::None;
                    let __amaru_values = ::std::option::Option::Some($values_expr);
                    #span_expr
                }
            };
            (values = $values_expr:expr) => {
                #crate_prefix #macro_ident!(level = ::amaru_observability::tracing::Level::TRACE, values = $values_expr)
            };
            (level = $level:expr) => {
                {
                    let __amaru_parent: ::std::option::Option<::amaru_observability::tracing::Span> = ::std::option::Option::None;
                    let __amaru_values: ::std::option::Option<&[::amaru_observability::tracing::__macro_support::Option<&dyn ::amaru_observability::tracing::field::Value>]> = ::std::option::Option::None;
                    #span_expr
                }
            };
            () => {
                #crate_prefix #macro_ident!(level = ::amaru_observability::tracing::Level::TRACE)
            };
        }
    }
}

fn generate_assign_macro(schema: &Schema, config: &GenerationConfig) -> proc_macro2::TokenStream {
    let categories = schema.category_strings();
    let schema_name_str = schema.name_str();
    let macro_name = make_assign_macro_name(&categories, &schema_name_str);
    let macro_ident = make_ident(&macro_name);
    let macro_export = config.macro_export_attr();

    let all_fields: Vec<_> = schema.required_fields.iter().chain(schema.optional_fields.iter()).collect();
    let assign_patterns: Vec<_> = all_fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            // Match on a string-literal field name at the call site.
            let field_name = field.name_lit();
            quote! {
                ($values:ident, #field_name, $value:expr) => {
                    $values[#index] = ::amaru_observability::tracing::__macro_support::Option::Some($value);
                };
            }
        })
        .collect();

    quote! {
        #macro_export
        #[doc(hidden)]
        macro_rules! #macro_ident {
            #(#assign_patterns)*
            ($values:ident, $name:literal, $value:expr) => {};
        }
    }
}

fn encode_cbor_path(_config: &GenerationConfig) -> proc_macro2::TokenStream {
    quote! { ::amaru_observability::field::encode_cbor }
}

fn as_str_value_path(_config: &GenerationConfig) -> proc_macro2::TokenStream {
    quote! { ::amaru_observability::field::as_str_value }
}

fn as_field_ref_path(_config: &GenerationConfig) -> proc_macro2::TokenStream {
    quote! { ::amaru_observability::field::as_field_ref }
}

fn display_string_value_path(_config: &GenerationConfig) -> proc_macro2::TokenStream {
    quote! { ::amaru_observability::field::display_string_value }
}

fn serialize_trait_path(_config: &GenerationConfig) -> proc_macro2::TokenStream {
    // Always use the absolute path: `$crate` inside `#[macro_export]` helpers is easy to
    // mis-resolve when those helpers are invoked via `::amaru_observability::…!`.
    quote! { ::amaru_observability::serde::Serialize }
}

fn json_schema_trait_path(_config: &GenerationConfig) -> proc_macro2::TokenStream {
    quote! { ::amaru_observability::schemars::JsonSchema }
}

/// Generate the schema validation helper macro.
fn generate_record_macro(schema: &Schema, config: &GenerationConfig) -> proc_macro2::TokenStream {
    let categories = schema.category_strings();
    let schema_name_str = schema.name_str();
    let macro_name = make_record_macro_name(&categories, &schema_name_str);
    let macro_ident = make_ident(&macro_name);
    let schema_name = &schema_name_str;
    let macro_export = config.macro_export_attr();
    let encode_cbor = encode_cbor_path(config);
    let as_str_value = as_str_value_path(config);
    let as_field_ref = as_field_ref_path(config);
    let display_string_value = display_string_value_path(config);
    let serialize_trait = serialize_trait_path(config);
    let json_schema_trait = json_schema_trait_path(config);

    let all_fields: Vec<_> = schema.required_fields.iter().chain(schema.optional_fields.iter()).collect();

    let validate_value_patterns: Vec<_> = all_fields
        .iter()
        .map(|field| {
            let field_name = field.name_lit();
            match field.transport_kind() {
                FieldTransportKind::Str => quote! {
                    (#field_name, $expr:expr, validate_value) => {{
                        let __amaru_assert_type = |_: &dyn ::std::convert::AsRef<str>| {};
                        __amaru_assert_type($expr);
                    }};
                },
                FieldTransportKind::DisplayStr => {
                    let field_type = &field.ty;
                    quote! {
                        (#field_name, $expr:expr, validate_value) => {{
                            let __amaru_v = #as_field_ref::<#field_type>($expr);
                            let __amaru_assert_display = |_: &dyn ::std::fmt::Display| {};
                            __amaru_assert_display(__amaru_v);
                        }};
                    }
                }
                FieldTransportKind::DebugStr => {
                    let field_type = &field.ty;
                    quote! {
                        (#field_name, $expr:expr, validate_value) => {{
                            let __amaru_v = #as_field_ref::<#field_type>($expr);
                            let __amaru_assert_debug = |_: &dyn ::std::fmt::Debug| {};
                            __amaru_assert_debug(__amaru_v);
                        }};
                    }
                }
                FieldTransportKind::Bool
                | FieldTransportKind::I64
                | FieldTransportKind::U64
                | FieldTransportKind::F64 => {
                    let field_type = &field.ty;
                    quote! {
                        (#field_name, $expr:expr, validate_value) => {{
                            let _ = #as_field_ref::<#field_type>($expr);
                        }};
                    }
                }
                FieldTransportKind::Cbor => {
                    let field_type = &field.ty;
                    quote! {
                        (#field_name, $expr:expr, validate_value) => {{
                            let __amaru_v = #as_field_ref::<#field_type>($expr);
                            fn __amaru_assert_serialize<T: #serialize_trait + #json_schema_trait + ?Sized>(_: &T) {}
                            __amaru_assert_serialize(__amaru_v);
                        }};
                    }
                }
            }
        })
        .collect();

    // Expression-producing patterns: type-check and produce a `tracing::Value`.
    let format_typed_patterns: Vec<_> = all_fields
        .iter()
        .map(|field| {
            let field_name = field.name_lit();
            match field.transport_kind() {
                FieldTransportKind::Bool => quote! {
                    (#field_name, $expr:expr, format_typed) => {{
                        *#as_field_ref::<bool>($expr)
                    }};
                },
                FieldTransportKind::I64 => {
                    let field_type = &field.ty;
                    quote! {
                        (#field_name, $expr:expr, format_typed) => {{
                            *#as_field_ref::<#field_type>($expr)
                        }};
                    }
                }
                FieldTransportKind::U64 => {
                    let field_type = &field.ty;
                    quote! {
                        (#field_name, $expr:expr, format_typed) => {{
                            *#as_field_ref::<#field_type>($expr)
                        }};
                    }
                }
                FieldTransportKind::F64 => {
                    let field_type = &field.ty;
                    quote! {
                        (#field_name, $expr:expr, format_typed) => {{
                            *#as_field_ref::<#field_type>($expr)
                        }};
                    }
                }
                FieldTransportKind::Str => quote! {
                    (#field_name, $expr:expr, format_typed) => {{
                        #as_str_value($expr)
                    }};
                },
                FieldTransportKind::DisplayStr => {
                    let field_type = &field.ty;
                    quote! {
                        (#field_name, $expr:expr, format_typed) => {{
                            #display_string_value(#as_field_ref::<#field_type>($expr))
                        }};
                    }
                }
                FieldTransportKind::DebugStr => {
                    let field_type = &field.ty;
                    quote! {
                        (#field_name, $expr:expr, format_typed) => {{
                            ::tracing::field::debug(#as_field_ref::<#field_type>($expr))
                        }};
                    }
                }
                FieldTransportKind::Cbor => {
                    let field_type = &field.ty;
                    quote! {
                        (#field_name, $expr:expr, format_typed) => {{
                            let __amaru_v = #as_field_ref::<#field_type>($expr);
                            fn __amaru_assert_serialize<T: #serialize_trait + #json_schema_trait + ?Sized>(_: &T) {}
                            __amaru_assert_serialize(__amaru_v);
                            #encode_cbor(__amaru_v)
                        }};
                    }
                }
            }
        })
        .collect();

    let validate_exact_patterns: Vec<_> = all_fields
        .iter()
        .map(|field| {
            let field_name = field.name_lit();
            let field_type = field.type_lit();
            quote! {
                (#field_name, #field_type, validate) => {};
            }
        })
        .collect();

    let validate_wrong_type_patterns: Vec<_> = all_fields
        .iter()
        .map(|field| {
            let field_name = field.name_lit();
            let expected_type = field.type_lit();
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

    let validate_formatted_patterns: Vec<_> = all_fields
        .iter()
        .map(|field| {
            let field_name = field.name_lit();
            quote! {
                (#field_name, $expr:expr, validate_event_display) => {{
                    let __amaru_assert_display = |_: &dyn ::std::fmt::Display| {};
                    __amaru_assert_display($expr);
                }};
                (#field_name, $expr:expr, validate_event_debug) => {{
                    let __amaru_assert_debug = |_: &dyn ::std::fmt::Debug| {};
                    __amaru_assert_debug($expr);
                }};
                (#field_name, $expr:expr, validate_event_value) => {{
                    let __amaru_assert_value = |_: &dyn ::amaru_observability::tracing::field::Value| {};
                    __amaru_assert_value($expr);
                }};
            }
        })
        .collect();

    let all_field_names: Vec<_> = all_fields.iter().map(|f| f.name_str()).collect();
    let fields_list = all_field_names.join(", ");

    quote! {
        #macro_export
        #[doc(hidden)]
        macro_rules! #macro_ident {
            #(#validate_value_patterns)*
            #(#format_typed_patterns)*
            #(#validate_formatted_patterns)*
            ($name:literal, $expr:expr, validate_value) => {
                compile_error!(concat!(
                    "Unknown field '",
                    $name,
                    "' for schema ",
                    #schema_name,
                    ". Available fields: ",
                    #fields_list
                ))
            };
            ($name:literal, $expr:expr, format_typed) => {
                compile_error!(concat!(
                    "Unknown field '",
                    $name,
                    "' for schema ",
                    #schema_name,
                    ". Available fields: ",
                    #fields_list
                ))
            };
            ($name:literal, $expr:expr, validate_event_display) => {
                compile_error!(concat!(
                    "Unknown field '",
                    $name,
                    "' for schema ",
                    #schema_name,
                    ". Available fields: ",
                    #fields_list
                ))
            };
            ($name:literal, $expr:expr, validate_event_debug) => {
                compile_error!(concat!(
                    "Unknown field '",
                    $name,
                    "' for schema ",
                    #schema_name,
                    ". Available fields: ",
                    #fields_list
                ))
            };
            ($name:literal, $expr:expr, validate_event_value) => {
                compile_error!(concat!(
                    "Unknown field '",
                    $name,
                    "' for schema ",
                    #schema_name,
                    ". Available fields: ",
                    #fields_list
                ))
            };

            #(#validate_exact_patterns)*
            #(#validate_wrong_type_patterns)*
            ($name:literal, $ty:literal, validate) => {};
        }
    }
}

/// Generate a module-specific schema validator macro.
fn generate_module_validator_macro(
    categories: &[String],
    schema_names: &[Ident],
    config: &GenerationConfig,
) -> proc_macro2::TokenStream {
    let validator_name = make_module_validator_name(categories);
    let validator_ident = make_ident(&validator_name);
    let module_path = categories.join("::");
    let schemas_list = schema_names.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ");
    let macro_export = config.macro_export_attr();

    let valid_schema_patterns: Vec<_> = schema_names
        .iter()
        .map(|schema_ident| {
            quote! {
                (#schema_ident => $body:block) => {
                    $body
                };
            }
        })
        .collect();

    quote! {
        #macro_export
        #[doc(hidden)]
        macro_rules! #validator_ident {
            #(#valid_schema_patterns)*
            ($schema:ident => $body:block) => {
                {
                    compile_error!(concat!(
                        "Invalid trace in module ",
                        #module_path,
                        " : ",
                        stringify!($schema),
                        ". Expected one of: ",
                        #schemas_list
                    ))
                }
            };
        }
    }
}

/// Check whether the macro is expanding within the `amaru-observability` lib itself.
fn is_observability_lib() -> bool {
    std::env::var("CARGO_PKG_NAME").ok().as_deref() == Some("amaru-observability")
        && std::env::var("CARGO_CRATE_NAME").ok().as_deref() == Some("amaru_observability")
}

fn field_render_tokens(render: FieldRender) -> proc_macro2::TokenStream {
    match render {
        FieldRender::Typed => quote! { FieldRender::Typed },
        FieldRender::Display => quote! { FieldRender::Display },
        FieldRender::Debug => quote! { FieldRender::Debug },
    }
}

fn field_json_schema_tokens(field: &SchemaField) -> proc_macro2::TokenStream {
    match field.render {
        FieldRender::Display | FieldRender::Debug => quote! { json_schema_string },
        FieldRender::Typed => match field.type_str().as_str() {
            "bool" => quote! { json_schema_boolean },
            "u64" | "u32" | "u16" | "u8" | "i64" | "i32" | "i16" | "i8" | "usize" | "isize" => {
                quote! { json_schema_integer }
            }
            "f64" | "f32" => quote! { json_schema_number },
            "String" | "&str" => quote! { json_schema_string },
            _ => {
                let field_ty = &field.ty;
                quote! { json_schema_for::<#field_ty> }
            }
        },
    }
}

fn field_entry_tokens(field: &SchemaField) -> proc_macro2::TokenStream {
    let name = field.name_lit();
    let ty = field.type_lit();
    let render = field_render_tokens(field.render);
    let json_schema = field_json_schema_tokens(field);
    quote! {
        SchemaFieldEntry {
            name: #name,
            rust_type: #ty,
            render: #render,
            json_schema: #json_schema,
        }
    }
}

/// Generate inventory submission for runtime schema registry.
fn generate_inventory_submission(schema: &Schema, config: &GenerationConfig) -> proc_macro2::TokenStream {
    if !config.export_macros {
        return quote! {};
    }

    let schema_path = schema.full_path();
    let target_path = schema.target_path();
    let schema_name = schema.name_str();

    let required_fields_array: Vec<_> = schema.required_fields.iter().map(field_entry_tokens).collect();
    let optional_fields_array: Vec<_> = schema.optional_fields.iter().map(field_entry_tokens).collect();

    let use_stmt = if is_observability_lib() {
        quote! {
            use crate::registry::{
                FieldRender, SchemaEntry, SchemaFieldEntry, json_schema_boolean, json_schema_for, json_schema_integer,
                json_schema_number, json_schema_string,
            };
        }
    } else {
        quote! {
            use amaru_observability::registry::{
                FieldRender, SchemaEntry, SchemaFieldEntry, json_schema_boolean, json_schema_for, json_schema_integer,
                json_schema_number, json_schema_string,
            };
        }
    };

    let description = schema.description.as_deref().unwrap_or("Missing description");
    let public = schema.public;

    let source_token_anchors = schema.required_fields.iter().chain(schema.optional_fields.iter()).map(|field| {
        let ty = &field.ty;
        let name = &field.name;
        quote! {
            {
                fn __bind(_: #ty) {}
                let _ = stringify!(#name);
            }
        }
    });
    let tag_anchors = schema.tags.iter().map(|tag| {
        quote! {
            let _ = stringify!(#tag);
        }
    });
    let tokens_fn_ident =
        Ident::new(&format!("__amaru_schema_tokens_{}", schema.full_path().replace("::", "_")), schema.name.span());

    quote! {
        #[allow(dead_code, non_snake_case, unused_variables)]
        fn #tokens_fn_ident() {
            #(#source_token_anchors)*
            #(#tag_anchors)*
        }

        #[allow(non_upper_case_globals)]
        const _: () = {
            #use_stmt
            inventory::submit!(SchemaEntry {
                path: #schema_path,
                name: #schema_name,
                target: #target_path,
                level: "TRACE",
                description: #description,
                public: #public,
                required_fields: &[#(#required_fields_array),*],
                optional_fields: &[#(#optional_fields_array),*],
            });
        };
    }
}

fn record_fields_trait_path() -> proc_macro2::TokenStream {
    if is_observability_lib() {
        quote! { crate::RecordFields }
    } else {
        quote! { ::amaru_observability::RecordFields }
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

/// A tree node representing either a category module or a schema.
#[derive(Clone)]
enum TreeNode {
    Category {
        /// Original category identifier (preserves span).
        name: Ident,
        children: BTreeMap<String, TreeNode>,
    },
    Schema(Schema),
}

/// Build a tree from schemas, grouping by category paths.
fn build_category_tree(schemas: &[Schema]) -> BTreeMap<String, TreeNode> {
    let mut root = BTreeMap::new();

    for schema in schemas {
        let mut current = &mut root;

        for category in &schema.categories {
            let key = category.to_string();
            current = current
                .entry(key)
                .or_insert_with(|| TreeNode::Category { name: category.clone(), children: BTreeMap::new() })
                .as_category_mut()
                .expect("Expected category node");
        }

        current.insert(schema.name_str(), TreeNode::Schema(schema.clone()));
    }

    root
}

impl TreeNode {
    fn as_category_mut(&mut self) -> Option<&mut BTreeMap<String, TreeNode>> {
        match self {
            TreeNode::Category { children, .. } => Some(children),
            TreeNode::Schema(_) => None,
        }
    }
}

/// Build the complete module tree with all generated code.
fn build_module_tree_with_metadata(schemas: &[Schema], config: &GenerationConfig) -> proc_macro2::TokenStream {
    let tree = build_category_tree(schemas);

    let mut validation_macros = Vec::new();
    let mut inventory_submissions = Vec::new();
    let mut all_schema_names = Vec::new();
    let mut all_schema_paths = Vec::new();

    for schema in schemas {
        all_schema_names.push(schema.name_str());
        all_schema_paths.push(schema.full_path());

        validation_macros.push(generate_required_fields_macro(schema, config));
        validation_macros.push(generate_instrument_macro(schema, config));
        validation_macros.push(generate_assign_macro(schema, config));
        validation_macros.push(generate_record_macro(schema, config));

        inventory_submissions.push(generate_inventory_submission(schema, config));
    }

    let mut category_validators = Vec::new();
    collect_category_validators(&tree, &mut vec![], &mut category_validators, config);
    validation_macros.extend(category_validators);

    let modules = build_modules(&tree, config);
    let schema_help_macro = generate_schema_help_macros(&all_schema_paths, &all_schema_names, config);

    let validation_macros = if validation_macros.is_empty() {
        quote! {}
    } else {
        quote! {
            // Validation macros at crate root (required for #[macro_export])
            #[allow(unused_macros)]
            #(#validation_macros)*
        }
    };

    quote! {
        // Submit schemas to inventory for runtime registry
        #(#inventory_submissions)*

        #validation_macros

        // Schema list helper macros
        #schema_help_macro

        // Module tree containing all schema definitions.
        #(#modules)*
    }
}

/// Recursively build minimal module structures (noop mode - no validation).
fn build_modules_noop(tree: &BTreeMap<String, TreeNode>, config: &GenerationConfig) -> Vec<proc_macro2::TokenStream> {
    let mut modules = Vec::new();

    for node in tree.values() {
        match node {
            TreeNode::Category { name, children } => {
                let child_modules = build_modules_noop(children, config);
                modules.push(quote! {
                    pub mod #name {
                        #(#child_modules)*
                    }
                });
            }
            TreeNode::Schema(schema) => {
                modules.push(generate_schema_item(schema, config));
            }
        }
    }

    modules
}

/// Recursively build module structures from the category tree.
fn build_modules(tree: &BTreeMap<String, TreeNode>, config: &GenerationConfig) -> Vec<proc_macro2::TokenStream> {
    let mut modules = Vec::new();

    for node in tree.values() {
        match node {
            TreeNode::Category { name, children } => {
                let child_modules = build_modules(children, config);
                modules.push(quote! {
                    pub mod #name {
                        #(#child_modules)*
                    }
                });
            }
            TreeNode::Schema(schema) => {
                modules.push(generate_schema_item(schema, config));
            }
        }
    }

    modules
}

fn generate_schema_item(schema: &Schema, _config: &GenerationConfig) -> proc_macro2::TokenStream {
    // Preserve the original schema ident span for go-to-definition.
    let schema_ident = &schema.name;
    let schema_name = schema.event_name();
    let schema_target = schema.event_target();
    let schema_path = schema.full_path();
    let validation_string = schema.validation_string();
    let field_count = schema.required_fields.len() + schema.optional_fields.len();
    let is_public = schema.public;
    let record_fields = record_fields_trait_path();

    let field_constants = schema.required_fields.iter().chain(schema.optional_fields.iter()).map(|field| {
        let field_const_ident = Ident::new(&format!("FIELD_{}", field.name_str().to_uppercase()), field.name.span());
        let field_name_lit = field.name_lit();

        quote! {
            pub const #field_const_ident: &str = #field_name_lit;
        }
    });

    let accessors = {
        let required_accessors = schema.required_fields.iter().map(|field| {
            // Accessor method name uses the field ident from the schema (with span).
            let accessor_ident = &field.name;
            let field_name = field.name_str();
            let accessor_kind = accessor_kind(field);
            let return_type = accessor_kind.return_type();
            let field_access = accessor_kind.trait_call(&field_name);
            let message =
                format!("missing or invalid required field '{}' for schema {}", field_name, schema.full_path());

            quote! {
                pub fn #accessor_ident<'record, R>(record: &'record R) -> #return_type
                where
                    R: #record_fields + ?Sized,
                {
                    #field_access.expect(#message)
                }
            }
        });

        let optional_accessors = schema.optional_fields.iter().map(|field| {
            let accessor_ident = &field.name;
            let field_name = field.name_str();
            let accessor_kind = accessor_kind(field);
            let return_type = accessor_kind.optional_return_type();
            let field_access = accessor_kind.trait_call(&field_name);

            quote! {
                pub fn #accessor_ident<'record, R>(record: &'record R) -> #return_type
                where
                    R: #record_fields + ?Sized,
                {
                    #field_access
                }
            }
        });

        quote! {
            #(#required_accessors)*
            #(#optional_accessors)*
        }
    };

    quote! {
        #[allow(non_camel_case_types)]
        pub struct #schema_ident;

        impl #schema_ident {
            pub const NAME: &str = #schema_name;
            pub const TARGET: &str = #schema_target;
            pub const PATH: &str = #schema_path;
            pub const VALIDATION: &str = #validation_string;
            #(#field_constants)*

            #[doc(hidden)]
            pub const SCHEMA_FIELD_COUNT: usize = #field_count;

            #[doc(hidden)]
            pub const PUBLIC: bool = #is_public;

            pub fn matches(target: &str, name: &str) -> bool {
                target == Self::TARGET && name == Self::NAME
            }

            #accessors
        }
    }
}

/// Collect category validators recursively.
fn collect_category_validators(
    tree: &BTreeMap<String, TreeNode>,
    path: &mut Vec<String>,
    validators: &mut Vec<proc_macro2::TokenStream>,
    config: &GenerationConfig,
) {
    let mut schema_names_at_this_level = Vec::new();

    for node in tree.values() {
        if let TreeNode::Schema(schema) = node {
            schema_names_at_this_level.push(schema.name.clone());
        }
    }

    if !schema_names_at_this_level.is_empty() && !path.is_empty() {
        validators.push(generate_module_validator_macro(path, &schema_names_at_this_level, config));
    }

    for (name, node) in tree {
        if let TreeNode::Category { children, .. } = node {
            path.push(name.clone());
            collect_category_validators(children, path, validators, config);
            path.pop();
        }
    }
}

fn errors_to_tokens(errors: Vec<syn::Error>) -> proc_macro2::TokenStream {
    let tokens = errors.into_iter().map(|e| e.to_compile_error());
    quote! { #(#tokens)* }
}

/// Internal expansion with configurable export behavior.
fn expand_with_config(input: TokenStream, export_macros: bool) -> TokenStream {
    let input2: proc_macro2::TokenStream = input.into();

    let file = match syn::parse2::<SchemaFile>(input2) {
        Ok(file) => file,
        Err(err) => return err.to_compile_error().into(),
    };

    let (schemas, errors) = extract_schemas(file);
    let config = GenerationConfig { export_macros };

    if crate::is_trace_no_emit() {
        if !errors.is_empty() {
            return errors_to_tokens(errors).into();
        }
        let tree = build_category_tree(&schemas);
        let modules = build_modules_noop(&tree, &config);
        return quote! { #(#modules)* }.into();
    }

    let module_tree = build_module_tree_with_metadata(&schemas, &config);

    // If there are errors, include them alongside the generated code so that
    // helper macros still exist (preventing cascading "cannot find macro" noise).
    if !errors.is_empty() {
        let error_tokens = errors_to_tokens(errors);
        return quote! {
            #error_tokens
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
    use quote::quote;

    use super::*;

    fn parse_input(tokens: proc_macro2::TokenStream) -> (Vec<Schema>, Vec<String>) {
        let file = syn::parse2::<SchemaFile>(tokens).expect("parse SchemaFile");
        let (schemas, errors) = extract_schemas(file);
        (schemas, errors.into_iter().map(|e| e.to_string()).collect())
    }

    #[test]
    fn test_extract_simple_schema() {
        let tokens = quote! {
            amaru {
                consensus {
                    sync {
                        /// Validate the schema
                        VALIDATE {
                            required slot: u64
                        }
                    }
                }
            }
        };
        let (schemas, errors) = parse_input(tokens);
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name_str(), "VALIDATE");
        assert_eq!(schemas[0].category_strings(), vec!["amaru", "consensus", "sync"]);
        assert_eq!(schemas[0].required_fields.len(), 1);
        assert_eq!(schemas[0].required_fields[0].name_str(), "slot");
        assert_eq!(schemas[0].required_fields[0].type_str(), "u64");
        assert_eq!(schemas[0].description, Some("Validate the schema".to_string()));
    }

    #[test]
    fn test_extract_schema_with_optional() {
        let tokens = quote! {
            amaru {
                test {
                    sub {
                        /// Test schema
                        SCHEMA {
                            required id: String
                            optional label: String
                        }
                    }
                }
            }
        };
        let (schemas, errors) = parse_input(tokens);
        assert!(errors.is_empty());
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].required_fields.len(), 1);
        assert_eq!(schemas[0].optional_fields.len(), 1);
        assert_eq!(schemas[0].optional_fields[0].name_str(), "label");
    }

    #[test]
    fn test_extract_schema_with_qualified_type_path() {
        let tokens = quote! {
            amaru {
                test {
                    sub {
                        /// Test schema
                        SCHEMA {
                            required credential_type: amaru_kernel::CredentialKind
                        }
                    }
                }
            }
        };
        let (schemas, errors) = parse_input(tokens);
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
        assert_eq!(schemas[0].required_fields[0].type_str(), "amaru_kernel::CredentialKind");
    }

    #[test]
    fn test_extract_schema_with_qualified_generic_type() {
        let tokens = quote! {
            amaru {
                test {
                    sub {
                        /// Test schema
                        SCHEMA {
                            required credential_hash: amaru_kernel::Hash<28>
                        }
                    }
                }
            }
        };
        let (schemas, errors) = parse_input(tokens);
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
        assert_eq!(schemas[0].required_fields[0].type_str(), "amaru_kernel::Hash<28>");
    }

    #[test]
    fn test_extract_schema_with_formatters() {
        let tokens = quote! {
            amaru {
                test {
                    sub {
                        /// Test schema
                        SCHEMA {
                            required point: amaru_kernel::Point
                            required header_hash: %amaru_kernel::HeaderHash
                            optional debug_info: ?String
                        }
                    }
                }
            }
        };
        let (schemas, errors) = parse_input(tokens);
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
        assert_eq!(schemas[0].required_fields[0].type_str(), "amaru_kernel::Point");
        assert_eq!(schemas[0].required_fields[0].render, FieldRender::Typed);
        assert_eq!(schemas[0].required_fields[1].type_str(), "amaru_kernel::HeaderHash");
        assert_eq!(schemas[0].required_fields[1].render, FieldRender::Display);
        assert_eq!(schemas[0].optional_fields[0].render, FieldRender::Debug);
    }

    #[test]
    fn test_extract_multiple_schemas() {
        let tokens = quote! {
            amaru {
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
            }
        };
        let (schemas, errors) = parse_input(tokens);
        assert!(errors.is_empty());
        assert_eq!(schemas.len(), 2);
        assert_eq!(schemas[0].name_str(), "SCHEMA_A");
        assert_eq!(schemas[1].name_str(), "SCHEMA_B");
    }

    #[test]
    fn test_duplicate_field_error() {
        let tokens = quote! {
            amaru {
                cat {
                    sub {
                        /// Schema with duplicate
                        SCHEMA {
                            required x: u32
                            required x: u64
                        }
                    }
                }
            }
        };
        let (_, errors) = parse_input(tokens);
        assert!(errors.iter().any(|e| e.contains("Duplicate field 'x'")));
    }

    #[test]
    fn test_schema_validation_string() {
        let mut schema = Schema {
            public: false,
            categories: vec![
                Ident::new("cat", proc_macro2::Span::call_site()),
                Ident::new("sub", proc_macro2::Span::call_site()),
            ],
            name: Ident::new("TEST", proc_macro2::Span::call_site()),
            description: None,
            tags: Vec::new(),
            required_fields: Vec::new(),
            optional_fields: Vec::new(),
        };
        schema.required_fields.push(SchemaField {
            name: Ident::new("id", proc_macro2::Span::call_site()),
            ty: syn::parse_quote!(u64),
            render: FieldRender::Typed,
        });
        schema.optional_fields.push(SchemaField {
            name: Ident::new("name", proc_macro2::Span::call_site()),
            ty: syn::parse_quote!(String),
            render: FieldRender::Typed,
        });
        assert_eq!(schema.validation_string(), "R|id:u64|O|name:String");
    }

    #[test]
    fn test_tags_inheritance_and_override() {
        let tokens = quote! {
            amaru {
                cat {
                    tags: cpu, io
                    sub {
                        /// Schema inheriting the module tags
                        INHERITED {
                            required x: u32
                        }
                        /// Schema overriding the module tags
                        OVERRIDDEN {
                            tags: setup
                            required y: u32
                        }
                    }
                }
                other {
                    sub {
                        /// Schema without any tags
                        UNTAGGED {
                            required z: u32
                        }
                    }
                }
            }
        };
        let (schemas, errors) = parse_input(tokens);
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
        assert_eq!(schemas.len(), 3);
        assert_eq!(
            schemas[0].tags.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
            vec!["cpu".to_string(), "io".to_string()]
        );
        assert_eq!(schemas[1].tags.iter().map(|t| t.to_string()).collect::<Vec<_>>(), vec!["setup".to_string()]);
        assert!(schemas[2].tags.is_empty());
    }

    #[test]
    fn test_invalid_tag_error() {
        let tokens = quote! {
            amaru {
                cat {
                    tags: CPU
                    sub {
                        /// Schema
                        SCHEMA {
                            required x: u32
                        }
                    }
                }
            }
        };
        // Uppercase tag should fail at parse time.
        match syn::parse2::<SchemaFile>(tokens) {
            Ok(_) => panic!("expected parse error for uppercase tag"),
            Err(err) => {
                let msg = err.to_string();
                assert!(msg.contains("Invalid tag") || msg.contains("CPU"), "got: {msg}");
            }
        }
    }

    #[test]
    fn test_duplicate_tag_error() {
        let tokens = quote! {
            amaru {
                cat {
                    sub {
                        /// Schema with a duplicated tag
                        SCHEMA {
                            tags: cpu, cpu
                            required x: u32
                        }
                    }
                }
            }
        };
        match syn::parse2::<SchemaFile>(tokens) {
            Ok(_) => panic!("expected parse error for duplicate tag"),
            Err(err) => {
                let msg = err.to_string();
                assert!(msg.contains("Duplicate tag 'cpu'"), "got: {msg}");
            }
        }
    }

    #[test]
    fn test_missing_description_error() {
        let tokens = quote! {
            amaru {
                cat {
                    sub {
                        SCHEMA {
                            required x: u32
                        }
                    }
                }
            }
        };
        let (schemas, errors) = parse_input(tokens);
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
        let tokens = quote! {
            amaru {
                cat {
                    sub {
                        /// This is a test schema
                        SCHEMA {
                            required x: u32
                        }
                    }
                }
            }
        };
        let (schemas, errors) = parse_input(tokens);
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].description, Some("This is a test schema".to_string()));
    }

    #[test]
    fn test_multiline_description() {
        let tokens = quote! {
            amaru {
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
            }
        };
        let (schemas, errors) = parse_input(tokens);
        assert!(errors.is_empty());
        assert_eq!(schemas.len(), 1);
        assert_eq!(
            schemas[0].description,
            Some("This is a test schema with multiple lines of documentation".to_string())
        );
    }

    #[test]
    fn test_trailing_comma_on_field() {
        let tokens = quote! {
            amaru {
                cat {
                    sub {
                        /// Schema with trailing commas
                        SCHEMA {
                            required id: u64,
                            optional label: String,
                        }
                    }
                }
            }
        };
        let (schemas, errors) = parse_input(tokens);
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
        assert_eq!(schemas[0].required_fields.len(), 1);
        assert_eq!(schemas[0].optional_fields.len(), 1);
    }

    #[test]
    fn test_public_schema() {
        let tokens = quote! {
            amaru {
                cat {
                    sub {
                        /// Public schema
                        public PUBLIC_EVENT {
                            required x: u32
                        }
                        /// Private schema
                        PRIVATE_EVENT {}
                    }
                }
            }
        };
        let (schemas, errors) = parse_input(tokens);
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
        assert!(schemas[0].public);
        assert!(!schemas[1].public);
    }

    #[test]
    fn test_field_doc_comments_accepted() {
        let tokens = quote! {
            amaru {
                cat {
                    sub {
                        /// Schema with field docs
                        SCHEMA {
                            /// docs on a field
                            required count: u64
                        }
                    }
                }
            }
        };
        let (schemas, errors) = parse_input(tokens);
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
        assert_eq!(schemas[0].required_fields[0].name_str(), "count");
    }
}
