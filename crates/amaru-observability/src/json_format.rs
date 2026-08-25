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

//! Single-pass CBOR-aware JSON log formatting.
//!
//! Builds each NDJSON line with one `serde_json` map serialization straight to the
//! writer. Event fields are visited once (CBOR → nested JSON). Span fields come from
//! a structured [`SpanJsonFields`] extension populated by [`CborJsonSpanLayer`], with a
//! `FormattedFields` string fallback when the layer is absent.
//!
//! Nested context is encoded as `span` `{name, target}`, `parents` (ancestor
//! names, outermost first), and the wrapping span's fields inlined into `fields`
//! (EDR-033). Ancestor span fields are not copied.
//!
//! This replaces the previous approach of running stock `Format<Json>`, re-parsing the
//! whole line, splicing fields, and re-serializing.

use std::{cell::RefCell, collections::BTreeMap, fmt, io};

use serde::ser::{Serialize, SerializeMap, Serializer as _};
use serde_json::{Serializer, Value as JsonValue};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
    span,
};
use tracing_subscriber::{
    Layer,
    fmt::{
        FmtContext, FormatEvent, FormatFields, FormattedFields,
        format::Writer,
        time::{FormatTime, SystemTime},
    },
    registry::LookupSpan,
};

use crate::{field::cbor_to_json, span_encode::ancestor_span_names};

// -----------------------------------------------------------------------------
// Structured span field bag (extension)
// -----------------------------------------------------------------------------

/// CBOR-aware span fields stored as a JSON object map (not a serialized string).
///
/// Keys are `'static` field names from `tracing` (no per-event `String` allocation).
/// Populated by [`CborJsonSpanLayer`]. [`CborJsonEventFormat`] prefers this over
/// re-parsing [`FormattedFields`].
#[derive(Debug, Clone, Default)]
pub struct SpanJsonFields(pub BTreeMap<&'static str, JsonValue>);

/// Layer that keeps [`SpanJsonFields`] in span extensions for zero-parse event formatting.
#[derive(Debug, Default, Clone, Copy)]
pub struct CborJsonSpanLayer;

impl CborJsonSpanLayer {
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for CborJsonSpanLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut visitor = CborJsonVisitor::default();
        attrs.record(&mut visitor);
        if let Some(span_ref) = ctx.span(id) {
            span_ref.extensions_mut().insert(SpanJsonFields(visitor.values));
        }
    }

    fn on_record(&self, id: &span::Id, values: &span::Record<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let Some(span_ref) = ctx.span(id) else {
            return;
        };
        let mut ext = span_ref.extensions_mut();
        if let Some(bag) = ext.get_mut::<SpanJsonFields>() {
            let mut visitor = CborJsonVisitor { values: std::mem::take(&mut bag.0) };
            values.record(&mut visitor);
            bag.0 = visitor.values;
        } else {
            let mut visitor = CborJsonVisitor::default();
            values.record(&mut visitor);
            ext.insert(SpanJsonFields(visitor.values));
        }
    }
}

// -----------------------------------------------------------------------------
// Field visitor + FormatFields string mirror
// -----------------------------------------------------------------------------

/// Visit that builds nested JSON values, decoding CBOR `record_bytes`.
#[derive(Default)]
pub(crate) struct CborJsonVisitor {
    pub(crate) values: BTreeMap<&'static str, JsonValue>,
}

impl Visit for CborJsonVisitor {
    fn record_f64(&mut self, field: &Field, value: f64) {
        if let Some(n) = serde_json::Number::from_f64(value) {
            self.values.insert(field.name(), JsonValue::Number(n));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.values.insert(field.name(), JsonValue::Number(value.into()));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.values.insert(field.name(), JsonValue::Number(value.into()));
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        if let Ok(v) = i64::try_from(value) {
            self.record_i64(field, v);
        } else {
            self.values.insert(field.name(), JsonValue::String(value.to_string()));
        }
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        if let Ok(v) = u64::try_from(value) {
            self.record_u64(field, v);
        } else {
            self.values.insert(field.name(), JsonValue::String(value.to_string()));
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.values.insert(field.name(), JsonValue::Bool(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.values.insert(field.name(), JsonValue::String(value.to_owned()));
    }

    fn record_bytes(&mut self, field: &Field, value: &[u8]) {
        let json = cbor_to_json(value).unwrap_or_else(|_| JsonValue::String(hex::encode(value)));
        self.values.insert(field.name(), json);
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.values.insert(field.name(), JsonValue::String(value.to_string()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let name = field.name();
        let key = name.strip_prefix("r#").unwrap_or(name);
        self.values.insert(key, JsonValue::String(format!("{value:?}")));
    }
}

/// Field formatter that stores a JSON object string in `FormattedFields`.
///
/// Used as a fallback when [`CborJsonSpanLayer`] is not installed. With the layer,
/// [`CborJsonEventFormat`] reads [`SpanJsonFields`] and does not re-parse this string.
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

    fn add_fields(
        &self,
        current: &'writer mut FormattedFields<Self>,
        fields: &tracing::span::Record<'_>,
    ) -> fmt::Result {
        if current.is_empty() {
            return self.format_fields(current.as_writer(), fields);
        }

        let mut existing: BTreeMap<String, JsonValue> =
            serde_json::from_str(current.as_str()).map_err(|_| fmt::Error)?;
        let mut visitor = CborJsonVisitor::default();
        fields.record(&mut visitor);
        for (key, value) in visitor.values {
            existing.insert(key.to_owned(), value);
        }
        current.fields = serde_json::to_string(&existing).map_err(|_| fmt::Error)?;
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Single-pass FormatEvent
// -----------------------------------------------------------------------------

/// JSON event formatter: one `SerializeMap` pass to the writer, CBOR-aware fields.
///
/// Envelope (see EDR-033):
/// - `timestamp`, `level`, `fields`, `target`
/// - wrapping-span fields inlined into `fields` (event fields win on collision)
/// - `span`: `{ name, target }` of the wrapping span
/// - `parents`: ancestor span names from outermost to the wrapping span's parent
/// - `id` on span lifecycle events; `parent_id` when the wrapping span has a parent
#[derive(Debug, Default, Clone, Copy)]
pub struct CborJsonEventFormat;

impl CborJsonEventFormat {
    pub fn new() -> Self {
        Self
    }
}

/// Adapts [`fmt::Write`] to [`io::Write`] for `serde_json::Serializer`.
struct FmtWriteAdaptor<'a> {
    fmt_write: &'a mut dyn fmt::Write,
}

impl<'a> FmtWriteAdaptor<'a> {
    fn new(fmt_write: &'a mut dyn fmt::Write) -> Self {
        Self { fmt_write }
    }
}

impl io::Write for FmtWriteAdaptor<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s = std::str::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.fmt_write.write_str(s).map_err(io::Error::other)?;
        Ok(s.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// JSON value that is either owned (event field) or borrowed from the wrapping span bag.
enum JsonRef<'a> {
    Owned(JsonValue),
    Borrowed(&'a JsonValue),
}

impl Serialize for JsonRef<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Owned(value) => value.serialize(serializer),
            Self::Borrowed(value) => value.serialize(serializer),
        }
    }
}

/// Event fields plus borrowed wrapping-span fields, for one serialize pass.
#[derive(Default)]
struct EventFields<'a> {
    values: BTreeMap<&'a str, JsonRef<'a>>,
}

impl EventFields<'_> {
    fn insert(&mut self, key: &'static str, value: JsonValue) {
        self.values.insert(key, JsonRef::Owned(value));
    }
}

impl Visit for EventFields<'_> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        if let Some(n) = serde_json::Number::from_f64(value) {
            self.insert(field.name(), JsonValue::Number(n));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field.name(), JsonValue::Number(value.into()));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field.name(), JsonValue::Number(value.into()));
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        if let Ok(v) = i64::try_from(value) {
            self.record_i64(field, v);
        } else {
            self.insert(field.name(), JsonValue::String(value.to_string()));
        }
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        if let Ok(v) = u64::try_from(value) {
            self.record_u64(field, v);
        } else {
            self.insert(field.name(), JsonValue::String(value.to_string()));
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field.name(), JsonValue::Bool(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field.name(), JsonValue::String(value.to_owned()));
    }

    fn record_bytes(&mut self, field: &Field, value: &[u8]) {
        let json = cbor_to_json(value).unwrap_or_else(|_| JsonValue::String(hex::encode(value)));
        self.insert(field.name(), json);
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.insert(field.name(), JsonValue::String(value.to_string()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let name = field.name();
        self.insert(name.strip_prefix("r#").unwrap_or(name), JsonValue::String(format!("{value:?}")));
    }
}

/// Wrapping-span fields, borrowed from [`SpanJsonFields`] when the layer is installed.
enum CurrentSpanFields<'a> {
    None,
    Bag(&'a BTreeMap<&'static str, JsonValue>),
    /// Fallback when only [`FormattedFields`] is present (parsed once, then borrowed).
    Parsed(BTreeMap<String, JsonValue>),
}

impl CurrentSpanFields<'_> {
    fn iter(&self) -> CurrentSpanIter<'_> {
        match self {
            Self::None => CurrentSpanIter::Empty,
            Self::Bag(bag) => CurrentSpanIter::Bag(bag.iter()),
            Self::Parsed(parsed) => CurrentSpanIter::Parsed(parsed.iter()),
        }
    }
}

enum CurrentSpanIter<'a> {
    Empty,
    Bag(std::collections::btree_map::Iter<'a, &'static str, JsonValue>),
    Parsed(std::collections::btree_map::Iter<'a, String, JsonValue>),
}

impl<'a> Iterator for CurrentSpanIter<'a> {
    type Item = (&'a str, &'a JsonValue);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Empty => None,
            Self::Bag(iter) => iter.next().map(|(key, value)| (*key, value)),
            Self::Parsed(iter) => iter.next().map(|(key, value)| (key.as_str(), value)),
        }
    }
}

/// Fields recorded on the wrapping span only (no ancestor walk, no clone of the bag).
fn current_span_fields<'a, N>(extensions: &'a tracing_subscriber::registry::Extensions<'a>) -> CurrentSpanFields<'a>
where
    N: for<'fmt> FormatFields<'fmt> + 'static,
{
    if let Some(bag) = extensions.get::<SpanJsonFields>() {
        return CurrentSpanFields::Bag(&bag.0);
    }
    if let Some(formatted) = extensions.get::<FormattedFields<N>>() {
        let s = formatted.as_str().trim();
        if !s.is_empty()
            && let Ok(JsonValue::Object(parsed)) = serde_json::from_str(s)
        {
            return CurrentSpanFields::Parsed(parsed.into_iter().collect());
        }
    }
    CurrentSpanFields::None
}

#[derive(serde::Serialize)]
struct SpanIdent<'a> {
    name: &'a str,
    target: &'a str,
}

impl<S, N> FormatEvent<S, N> for CborJsonEventFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(&self, ctx: &FmtContext<'_, S, N>, mut writer: Writer<'_>, event: &Event<'_>) -> fmt::Result {
        let mut timestamp = String::new();
        SystemTime.format_time(&mut Writer::new(&mut timestamp))?;

        let meta = event.metadata();

        let current = event.parent().and_then(|id| ctx.span(id)).or_else(|| ctx.lookup_current());
        let extensions = current.as_ref().map(|span_ref| span_ref.extensions());
        let span_fields =
            extensions.as_ref().map(|ext| current_span_fields::<N>(ext)).unwrap_or(CurrentSpanFields::None);

        let mut fields = EventFields::default();
        event.record(&mut fields);
        for (key, value) in span_fields.iter() {
            fields.values.entry(key).or_insert(JsonRef::Borrowed(value));
        }

        let parents = current
            .as_ref()
            .map(|leaf| ancestor_span_names(leaf.scope().from_root().map(|span| span.name())))
            .unwrap_or_else(|| Box::new(std::iter::empty()));

        let write_line = || -> Result<(), serde_json::Error> {
            let mut serializer = Serializer::new(FmtWriteAdaptor::new(&mut writer));
            let mut map = serializer.serialize_map(None)?;

            map.serialize_entry("timestamp", &timestamp)?;
            map.serialize_entry("level", meta.level().as_str())?;
            map.serialize_entry("fields", &fields.values)?;
            map.serialize_entry("target", meta.target())?;

            if let Some(ref span_ref) = current {
                map.serialize_entry(
                    "span",
                    &SpanIdent { name: span_ref.name(), target: span_ref.metadata().target() },
                )?;
                map.serialize_entry("parents", &Parents::new(parents))?;
                if meta.is_span() {
                    map.serialize_entry("id", &span_ref.id().into_u64())?;
                }
                if let Some(parent) = span_ref.parent() {
                    map.serialize_entry("parent_id", &parent.id().into_u64())?;
                }
            }

            map.end()?;
            Ok(())
        };

        write_line().map_err(|_| fmt::Error)?;
        writeln!(writer)
    }
}

struct Parents<'a>(RefCell<Box<dyn Iterator<Item = &'a str> + 'a>>);
impl Serialize for Parents<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(None)?;
        for name in self.0.borrow_mut().as_mut() {
            seq.serialize_element(name)?;
        }
        seq.end()
    }
}
impl<'a> Parents<'a> {
    fn new(iter: Box<dyn Iterator<Item = &'a str> + 'a>) -> Self {
        Self(RefCell::new(iter))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Write},
        sync::{Arc, Mutex},
    };

    use serde::Serialize;
    use tracing_subscriber::{
        filter::LevelFilter,
        fmt::{MakeWriter, format::FmtSpan},
        layer::SubscriberExt,
    };

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

    fn json_subscriber(writer: CaptureWriter) -> impl tracing::Subscriber + Send + Sync {
        tracing_subscriber::registry().with(CborJsonSpanLayer::new()).with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .event_format(CborJsonEventFormat::new())
                .fmt_fields(CborJsonFields::new())
                .with_filter(LevelFilter::DEBUG),
        )
    }

    #[test]
    fn json_event_format_emits_typed_primitives_and_cbor_arrays() {
        let writer = CaptureWriter::default();
        let subscriber = json_subscriber(writer.clone());

        tracing::subscriber::with_default(subscriber, || {
            let dirty = true;
            let version = "10.11.0";
            let peers = encode_cbor(&Peers { addresses: vec!["a:1".into(), "b:2".into()] });
            tracing::info!(
                target: "amaru::setup::build",
                version,
                git_dirty = dirty,
                peers = peers.as_ref() as &[u8],
                "build.version"
            );
        });

        let output = writer.contents();
        let json: serde_json::Value = serde_json::from_str(output.lines().next().expect("line")).expect("json");
        assert!(json.get("timestamp").is_some());
        assert_eq!(json["level"], "INFO");
        assert_eq!(json["target"], "amaru::setup::build");
        let fields = &json["fields"];
        assert_eq!(fields["version"], "10.11.0");
        assert_eq!(fields["git_dirty"], true);
        assert_eq!(fields["peers"]["addresses"], serde_json::json!(["a:1", "b:2"]));
        // Point event with no entered span: no span envelope.
        assert!(json.get("span").is_none());
        assert!(json.get("parents").is_none());
    }

    #[test]
    fn json_span_record_merges_fields_without_malformed_panic() {
        let writer = CaptureWriter::default();
        let subscriber = json_subscriber(writer.clone());

        tracing::subscriber::with_default(subscriber, || {
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
        for line in output.lines().filter(|l| !l.is_empty()) {
            let json: JsonValue =
                serde_json::from_str(line).unwrap_or_else(|e| panic!("invalid JSON line ({e}): {line}"));
            assert!(json.is_object(), "root must be object: {line}");
        }
        assert!(
            output.contains("snapshot_count") && output.contains("continuous_ranges"),
            "expected merged span fields in JSON output:\n{output}"
        );
        let last = output.lines().rfind(|l| !l.is_empty()).expect("line");
        let json: JsonValue = serde_json::from_str(last).expect("json");
        assert_eq!(json["fields"]["snapshot_count"], 3);
        assert_eq!(json["fields"]["continuous_ranges"], 1);
        assert_eq!(json["fields"]["amaru.tag.db"], true);
        assert_eq!(json["span"]["name"], "ledger.snapshots.validate");
        assert!(json["span"].get("snapshot_count").is_none(), "span is identity only");
        assert_eq!(json["parents"], serde_json::json!([]));
    }

    #[test]
    fn json_works_without_span_layer_via_formatted_fields_fallback() {
        let writer = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .event_format(CborJsonEventFormat::new())
            .fmt_fields(CborJsonFields::new())
            .with_max_level(tracing::Level::INFO)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("parent", answer = 42_u64);
            let _g = span.enter();
            tracing::info!(msg_field = "hi", "hello");
        });

        let output = writer.contents();
        let json: JsonValue = serde_json::from_str(output.lines().next().expect("line")).expect("json");
        assert_eq!(json["fields"]["message"], "hello");
        assert_eq!(json["fields"]["answer"], 42);
        assert_eq!(json["span"]["name"], "parent");
        assert_eq!(json["parents"], serde_json::json!([]));
    }

    #[test]
    fn span_lifecycle_events_include_id() {
        let writer = CaptureWriter::default();
        let subscriber = tracing_subscriber::registry().with(CborJsonSpanLayer::new()).with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer.clone())
                .with_span_events(FmtSpan::ENTER)
                .event_format(CborJsonEventFormat::new())
                .fmt_fields(CborJsonFields::new())
                .with_filter(LevelFilter::INFO),
        );

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("lifecycle", n = 1_u64);
            let _g = span.enter();
        });

        let output = writer.contents();
        let enter_line = output.lines().find(|l| l.contains("\"enter\"") || l.contains("enter")).expect("enter event");
        let json: JsonValue = serde_json::from_str(enter_line).expect("json");
        assert!(json.get("id").and_then(|v| v.as_u64()).is_some(), "expected id on span event: {enter_line}");
        assert_eq!(json["fields"]["n"], 1);
        assert_eq!(json["span"]["name"], "lifecycle");
        assert!(json.get("parent_id").is_none());
    }

    #[test]
    fn json_nested_spans_encode_ancestry() {
        let writer = CaptureWriter::default();
        let subscriber = json_subscriber(writer.clone());

        tracing::subscriber::with_default(subscriber, || {
            let outer = tracing::info_span!("epoch.transition", from = 599_u64, into = 600_u64);
            let _outer = outer.enter();
            let mid = tracing::info_span!("governance.ratify_proposals", epoch = 598_u64);
            let _mid = mid.enter();
            let inner = tracing::info_span!("ratification.round", votes = 372_u64);
            let _inner = inner.enter();
            tracing::info!(is_dormant_epoch = false, "ratification.summarize");
        });

        let output = writer.contents();
        let line = output.lines().find(|l| l.contains("ratification.summarize")).expect("event line");
        let json: JsonValue = serde_json::from_str(line).expect("json");
        assert_eq!(json["fields"]["message"], "ratification.summarize");
        assert_eq!(json["span"]["name"], "ratification.round");
        assert_eq!(json["fields"]["votes"], 372);
        assert!(json["span"].get("votes").is_none());
        assert_eq!(json["parents"], serde_json::json!(["epoch.transition", "governance.ratify_proposals"]));
        assert!(json["fields"].get("from").is_none(), "ancestor fields must not be inlined");
        assert!(json["fields"].get("epoch").is_none(), "mid-level ancestor fields must not be inlined");
        assert!(json.get("parent_id").and_then(|v| v.as_u64()).is_some());
        assert!(json.get("id").is_none(), "point events do not carry their own span id");
    }
}
