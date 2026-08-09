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

//! CBOR-aware `tracing-subscriber` field formatters for console and JSON sinks.
//!
//! Every `record_bytes` payload is treated as CBOR (project convention). Primitives
//! recorded via typed visit methods pass through unchanged.

use std::{collections::BTreeMap, fmt};

use serde_json::Value as JsonValue;
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{
    field::{MakeVisitor, VisitFmt, VisitOutput},
    fmt::{
        FmtContext, FormatEvent, FormatFields, FormattedFields,
        format::{DefaultFields, Format, Json, Writer},
    },
    registry::LookupSpan,
};

use crate::field::{cbor_diagnostic, cbor_to_json};

// -----------------------------------------------------------------------------
// Console: decode CBOR bytes to diagnostic notation
// -----------------------------------------------------------------------------

/// [`MakeVisitor`] adapter: decode CBOR `record_bytes` to diagnostic text before
/// delegating to an inner visitor (typically [`DefaultFields`]).
///
/// Compose with tag-hiding wrappers:
/// `HideTagFields(CborAwareMakeVisitor(DefaultFields::new()))`.
#[derive(Debug, Clone)]
pub struct CborAwareMakeVisitor<N>(pub N);

impl<'a, N> MakeVisitor<Writer<'a>> for CborAwareMakeVisitor<N>
where
    N: MakeVisitor<Writer<'a>>,
{
    type Visitor = CborDiagVisitor<N::Visitor>;

    fn make_visitor(&self, target: Writer<'a>) -> Self::Visitor {
        CborDiagVisitor(self.0.make_visitor(target))
    }
}

/// Visit wrapper that turns CBOR bytes into diagnostic strings.
pub struct CborDiagVisitor<V>(pub V);

impl<V: Visit> Visit for CborDiagVisitor<V> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.0.record_f64(field, value);
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.record_i64(field, value);
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.record_u64(field, value);
    }
    fn record_i128(&mut self, field: &Field, value: i128) {
        self.0.record_i128(field, value);
    }
    fn record_u128(&mut self, field: &Field, value: u128) {
        self.0.record_u128(field, value);
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.record_bool(field, value);
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.record_str(field, value);
    }
    fn record_bytes(&mut self, field: &Field, value: &[u8]) {
        let diag = cbor_diagnostic(value);
        self.0.record_str(field, &diag);
    }
    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.0.record_error(field, value);
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.record_debug(field, value);
    }
}

impl<Out, V: VisitOutput<Out>> VisitOutput<Out> for CborDiagVisitor<V> {
    fn finish(self) -> Out {
        self.0.finish()
    }
}

impl<V: VisitFmt> VisitFmt for CborDiagVisitor<V> {
    fn writer(&mut self) -> &mut dyn fmt::Write {
        self.0.writer()
    }
}

// -----------------------------------------------------------------------------
// JSON field storage (spans) + event field maps
// -----------------------------------------------------------------------------

/// Field formatter that stores a JSON object string (compatible with the stock
/// JSON `FormatEvent`, which re-parses span `FormattedFields` as a JSON object).
///
/// CBOR `record_bytes` payloads become nested JSON values (objects/arrays/scalars).
#[derive(Debug, Default, Clone)]
pub struct CborJsonFields;

impl CborJsonFields {
    pub fn new() -> Self {
        Self
    }
}

impl<'writer> FormatFields<'writer> for CborJsonFields {
    fn format_fields<R: tracing_subscriber::field::RecordFields>(
        &self,
        mut writer: Writer<'writer>,
        fields: R,
    ) -> fmt::Result {
        let mut visitor = CborJsonVisitor::default();
        fields.record(&mut visitor);
        let s = serde_json::to_string(&visitor.values).map_err(|_| fmt::Error)?;
        write!(writer, "{s}")
    }

    /// Merge newly recorded span fields into the existing JSON object string.
    ///
    /// Stock [`tracing_subscriber::fmt::format::JsonFields`] does the same: the JSON event
    /// formatter later re-parses `FormattedFields` as a **single** object. The default
    /// `add_fields` only appends with a space, which would yield `{...} {...}` and panic
    /// in debug builds (`trailing characters`).
    fn add_fields(
        &self,
        current: &'writer mut FormattedFields<Self>,
        fields: &tracing::span::Record<'_>,
    ) -> fmt::Result {
        if current.is_empty() {
            return self.format_fields(current.as_writer(), fields);
        }

        // Parse the previously serialized object, merge, re-serialize.
        let existing: BTreeMap<String, JsonValue> = serde_json::from_str(current.as_str()).map_err(|_| fmt::Error)?;
        let mut visitor = CborJsonVisitor { values: existing };
        fields.record(&mut visitor);
        current.fields = serde_json::to_string(&visitor.values).map_err(|_| fmt::Error)?;
        Ok(())
    }
}

#[derive(Default)]
struct CborJsonVisitor {
    /// Owned keys so we can merge with objects re-parsed from `FormattedFields` text.
    values: BTreeMap<String, JsonValue>,
}

impl Visit for CborJsonVisitor {
    fn record_f64(&mut self, field: &Field, value: f64) {
        if let Some(n) = serde_json::Number::from_f64(value) {
            self.values.insert(field.name().to_owned(), JsonValue::Number(n));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.values.insert(field.name().to_owned(), JsonValue::Number(value.into()));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.values.insert(field.name().to_owned(), JsonValue::Number(value.into()));
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        if let Ok(v) = i64::try_from(value) {
            self.record_i64(field, v);
        } else {
            self.values.insert(field.name().to_owned(), JsonValue::String(value.to_string()));
        }
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        if let Ok(v) = u64::try_from(value) {
            self.record_u64(field, v);
        } else {
            self.values.insert(field.name().to_owned(), JsonValue::String(value.to_string()));
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.values.insert(field.name().to_owned(), JsonValue::Bool(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.values.insert(field.name().to_owned(), JsonValue::String(value.to_owned()));
    }

    fn record_bytes(&mut self, field: &Field, value: &[u8]) {
        let json = cbor_to_json(value).unwrap_or_else(|_| JsonValue::String(hex::encode(value)));
        self.values.insert(field.name().to_owned(), json);
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.values.insert(field.name().to_owned(), JsonValue::String(value.to_string()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.values.insert(field.name().to_owned(), JsonValue::String(format!("{value:?}")));
    }
}

// -----------------------------------------------------------------------------
// JSON FormatEvent that decodes CBOR in event fields (stock uses tracing_serde)
// -----------------------------------------------------------------------------

/// JSON event formatter with CBOR-aware event fields and span id injection.
///
/// Stock `tracing_subscriber` JSON formatting serializes event fields via
/// `tracing_serde`, which cannot decode our CBOR `record_bytes` convention.
/// This formatter records event fields with `CborJsonVisitor` instead.
pub struct CborJsonEventFormat(Format<Json>);

impl CborJsonEventFormat {
    pub fn new() -> Self {
        Self(Format::default().json().with_span_list(false))
    }

    pub fn from_inner(inner: Format<Json>) -> Self {
        Self(inner)
    }
}

impl Default for CborJsonEventFormat {
    fn default() -> Self {
        Self::new()
    }
}

impl<S, N> FormatEvent<S, N> for CborJsonEventFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(&self, ctx: &FmtContext<'_, S, N>, mut writer: Writer<'_>, event: &Event<'_>) -> fmt::Result {
        // Build a stock JSON line first for timestamp/level/target/span metadata.
        let mut buf = String::new();
        self.0.format_event(ctx, Writer::new(&mut buf), event)?;

        // Replace the "fields" object with a CBOR-aware encoding of the event.
        let mut visitor = CborJsonVisitor::default();
        event.record(&mut visitor);
        let fields_json = serde_json::to_string(&visitor.values).map_err(|_| fmt::Error)?;

        if let Ok(mut root) = serde_json::from_str::<JsonValue>(&buf)
            && let Some(obj) = root.as_object_mut()
        {
            if let Ok(fields_val) = serde_json::from_str(&fields_json) {
                obj.insert("fields".to_string(), fields_val);
            }

            // Inject recorded span fields + ids (same role as SpanJsonFormat).
            if let Some(current) = ctx.lookup_current() {
                let extensions = current.extensions();
                if let Some(span_fields) = extensions.get::<FormattedFields<N>>() {
                    let s = span_fields.as_str().trim();
                    if let Ok(JsonValue::Object(span_obj)) = serde_json::from_str(s) {
                        for (k, v) in span_obj {
                            obj.entry(k).or_insert(v);
                        }
                    }
                }
                if event.metadata().is_span() {
                    obj.insert("id".to_string(), JsonValue::Number(current.id().into_u64().into()));
                }
                if let Some(parent) = current.parent() {
                    obj.insert("parent_id".to_string(), JsonValue::Number(parent.id().into_u64().into()));
                }
            }

            let rewritten = serde_json::to_string(&root).map_err(|_| fmt::Error)?;
            writer.write_str(&rewritten)?;
            writeln!(writer)?;
            return Ok(());
        }

        // Fallback: original buffer if re-parse fails.
        writer.write_str(&buf)
    }
}

// -----------------------------------------------------------------------------
// OTEL best-effort helper
// -----------------------------------------------------------------------------

/// Visit wrapper that stringifies CBOR bytes as diagnostic notation.
///
/// Use when collecting attributes for OTEL: nested structure becomes a string
/// (OTEL trace attributes cannot hold nested maps). Typed primitives pass through.
pub struct CborToStringVisit<V>(pub V);

impl<V: Visit> Visit for CborToStringVisit<V> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.0.record_f64(field, value);
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.record_i64(field, value);
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.record_u64(field, value);
    }
    fn record_i128(&mut self, field: &Field, value: i128) {
        self.0.record_i128(field, value);
    }
    fn record_u128(&mut self, field: &Field, value: u128) {
        self.0.record_u128(field, value);
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.record_bool(field, value);
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.record_str(field, value);
    }
    fn record_bytes(&mut self, field: &Field, value: &[u8]) {
        let diag = cbor_diagnostic(value);
        self.0.record_str(field, &diag);
    }
    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.0.record_error(field, value);
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.record_debug(field, value);
    }
}

/// Convenience: console field stack with CBOR diagnostic decoding.
pub fn console_field_formatter() -> CborAwareMakeVisitor<DefaultFields> {
    CborAwareMakeVisitor(DefaultFields::new())
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Write},
        sync::{Arc, Mutex},
    };

    use serde::Serialize;
    use tracing_subscriber::fmt::MakeWriter;

    use super::*;
    use crate::field::encode_cbor;

    #[derive(Clone, Default)]
    struct CaptureWriter {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    impl CaptureWriter {
        fn contents(&self) -> String {
            String::from_utf8_lossy(&self.buffer.lock().expect("lock").clone()).into_owned()
        }
    }

    impl Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.buffer.lock().expect("lock").write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[derive(Serialize)]
    struct Peers {
        addresses: Vec<String>,
    }

    #[test]
    fn json_span_record_merges_fields_without_malformed_panic() {
        // Reproduces: debug_span!(…, tag) + span.record(…) + event inside the span.
        // Without a proper add_fields merge, FormattedFields becomes `{...} {...}` and the
        // stock JSON FormatEvent panics in debug builds when re-parsing span fields.
        let writer = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .event_format(CborJsonEventFormat::new())
            .fmt_fields(CborJsonFields::new())
            .with_max_level(tracing::Level::DEBUG)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            // Later record() only applies to fields declared at creation (Empty placeholders).
            let span = tracing::debug_span!(
                "ledger.snapshots.validate",
                amaru.tag.db = true,
                snapshot_count = tracing::field::Empty,
                continuous_ranges = tracing::field::Empty,
            );
            let _g = span.enter();
            span.record("snapshot_count", 3_u64);
            span.record("continuous_ranges", 1_u64);
            tracing::debug!("inside validate");
        });

        let output = writer.contents();
        assert!(!output.is_empty(), "expected JSON log lines, got empty");
        // Every line must parse as a single JSON value. Pre-fix, FormattedFields held
        // space-joined objects and re-parse panicked with "trailing characters".
        for line in output.lines().filter(|l| !l.is_empty()) {
            let _: JsonValue = serde_json::from_str(line).unwrap_or_else(|e| panic!("invalid JSON line ({e}): {line}"));
        }
        assert!(
            output.contains("snapshot_count") && output.contains("continuous_ranges"),
            "expected merged span fields in JSON output:\n{output}"
        );
    }

    #[test]
    fn json_event_format_emits_typed_primitives_and_cbor_arrays() {
        let writer = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .event_format(CborJsonEventFormat::new())
            .fmt_fields(CborJsonFields::new())
            .with_max_level(tracing::Level::INFO)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            // Emit via raw tracing to control exact field kinds.
            let dirty = true;
            let version = "10.11.0";
            let peers = encode_cbor(&Peers { addresses: vec!["a:1".into(), "b:2".into()] });
            tracing::info!(
                target: "amaru::setup::build",
                version = version,
                git_dirty = dirty,
                peers = peers.as_ref() as &[u8],
                "build.version"
            );
        });

        let output = writer.contents();
        let json: serde_json::Value = serde_json::from_str(output.lines().next().expect("line")).expect("json");
        let fields = &json["fields"];
        assert_eq!(fields["version"], "10.11.0");
        assert_eq!(fields["git_dirty"], true);
        // CBOR map decoded to nested JSON with a real array.
        assert_eq!(fields["peers"]["addresses"], serde_json::json!(["a:1", "b:2"]));
    }

    #[test]
    fn console_cbor_diag_renders_bytes_as_text() {
        let writer = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .fmt_fields(CborAwareMakeVisitor(DefaultFields::new()))
            .with_max_level(tracing::Level::INFO)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let peers = encode_cbor(&vec!["a:1", "b:2"]);
            tracing::info!(peers = peers.as_ref() as &[u8], "hello");
        });

        let output = writer.contents();
        assert!(output.contains("hello"), "output={output}");
        // cbor-data diagnostic for a string array must include the element text (quoted form ok).
        assert!(output.contains("a:1"), "expected decoded peers diagnostic content, got: {output}");
    }
}
