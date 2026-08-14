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

//! CBOR-aware `tracing-subscriber` field formatters for console (and re-exports for JSON).
//!
//! Every `record_bytes` payload is treated as CBOR (project convention). Primitives
//! recorded via typed visit methods pass through unchanged. Text scalars are
//! unwrapped so the console does not Debug-quote diagnostic quotes.
//!
//! Compose [`console_field_formatter`] with [`CborConsoleEventFormat`] so nested
//! spans render as an abbreviated path (`e.t:g.r`) plus the wrapping span's
//! name and fields (EDR-033).
//!
//! JSON NDJSON formatting lives in [`crate::json_format`].

use std::fmt;

use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{
    field::{MakeVisitor, VisitFmt, VisitOutput},
    fmt::{
        FmtContext, FormatEvent, FormatFields, FormattedFields,
        format::{DefaultFields, Writer},
        time::{FormatTime, SystemTime},
    },
    registry::LookupSpan,
};

pub use crate::json_format::{CborJsonEventFormat, CborJsonFields, CborJsonSpanLayer, SpanJsonFields};
use crate::{
    field::{DecodedField, cbor_to_decoded_field, is_tag_field_name},
    span_encode::{ancestor_span_names, write_abbreviated_span_path},
};

// -----------------------------------------------------------------------------
// Console: decode CBOR bytes to native visit types
// -----------------------------------------------------------------------------

/// [`MakeVisitor`] adapter: decode CBOR `record_bytes` before delegating.
///
/// Scalars become the matching typed visit (`record_str` / `record_u64` / …);
/// maps and arrays stay diagnostic text. Prefer [`console_field_formatter`]
/// over composing this type by hand.
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

/// Visit wrapper that decodes CBOR `record_bytes` onto the inner visitor.
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
        record_decoded_cbor(&mut self.0, field, value);
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
// OTEL best-effort helper
// -----------------------------------------------------------------------------

/// Visit wrapper that decodes CBOR `record_bytes` onto the inner visitor.
///
/// Scalars keep their native type. Maps and arrays become diagnostic text
/// because classic OTEL trace attributes cannot hold nested maps.
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
        record_decoded_cbor(&mut self.0, field, value);
    }
    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.0.record_error(field, value);
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.record_debug(field, value);
    }
}

/// Dispatch a CBOR `record_bytes` payload onto `visitor` using native types.
///
/// Text scalars are recorded as `record_str` **without** RFC 8610 diagnostic quotes,
/// so the console `DefaultFields` formatter does not wrap them in a second pair of
/// quotes (`header_hash="abc"` rather than `header_hash="\"abc\""`).
fn record_decoded_cbor(visitor: &mut impl Visit, field: &Field, bytes: &[u8]) {
    match cbor_to_decoded_field(bytes) {
        DecodedField::Bool(value) => visitor.record_bool(field, value),
        DecodedField::I64(value) => visitor.record_i64(field, value),
        DecodedField::U64(value) => visitor.record_u64(field, value),
        DecodedField::F64(value) => visitor.record_f64(field, value),
        DecodedField::Text(text) => visitor.record_str(field, &text),
    }
}

// -----------------------------------------------------------------------------
// Hide `amaru.tag.*` from human-facing field formatters
// -----------------------------------------------------------------------------

/// Wraps a field formatter so that `amaru.tag.*` fields are skipped.
///
/// Tags classify spans for `EnvFilter` / OpenTelemetry backends. They have no
/// value in the human-readable console log, and because formatters may append
/// the fields of every span in scope, an inherited tag would otherwise repeat
/// once per nested span.
#[derive(Debug, Clone)]
pub struct HideTagFields<N>(pub N);

impl<'writer, N> MakeVisitor<Writer<'writer>> for HideTagFields<N>
where
    N: MakeVisitor<Writer<'writer>>,
{
    type Visitor = HideTagVisitor<N::Visitor>;

    fn make_visitor(&self, target: Writer<'writer>) -> Self::Visitor {
        HideTagVisitor(self.0.make_visitor(target))
    }
}

/// Forwards every recorded value to the inner visitor except `amaru.tag.*`.
pub struct HideTagVisitor<V>(V);

impl<V: Visit> Visit for HideTagVisitor<V> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        if !is_tag_field_name(field.name()) {
            self.0.record_f64(field, value);
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if !is_tag_field_name(field.name()) {
            self.0.record_i64(field, value);
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if !is_tag_field_name(field.name()) {
            self.0.record_u64(field, value);
        }
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        if !is_tag_field_name(field.name()) {
            self.0.record_i128(field, value);
        }
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        if !is_tag_field_name(field.name()) {
            self.0.record_u128(field, value);
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if !is_tag_field_name(field.name()) {
            self.0.record_bool(field, value);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if !is_tag_field_name(field.name()) {
            self.0.record_str(field, value);
        }
    }

    fn record_bytes(&mut self, field: &Field, value: &[u8]) {
        if !is_tag_field_name(field.name()) {
            self.0.record_bytes(field, value);
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        if !is_tag_field_name(field.name()) {
            self.0.record_error(field, value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if !is_tag_field_name(field.name()) {
            self.0.record_debug(field, value);
        }
    }
}

impl<Out, V: VisitOutput<Out>> VisitOutput<Out> for HideTagVisitor<V> {
    fn finish(self) -> Out {
        self.0.finish()
    }
}

impl<V: VisitFmt> VisitFmt for HideTagVisitor<V> {
    fn writer(&mut self) -> &mut dyn fmt::Write {
        self.0.writer()
    }
}

/// Console field stack: decode CBOR, then hide schema tags.
///
/// Compose with [`CborConsoleEventFormat`] (EDR-033).
pub fn console_field_formatter() -> HideTagFields<CborAwareMakeVisitor<DefaultFields>> {
    HideTagFields(CborAwareMakeVisitor(DefaultFields::new()))
}

// -----------------------------------------------------------------------------
// Console event format: abbreviated path + wrapping span only
// -----------------------------------------------------------------------------

/// Compact console event format (EDR-033).
///
/// Prints `e.t:g.r` for the span stack, then target, the wrapping span's full
/// name, the event fields, the wrapping span's fields, and `id` / `parent_id`.
/// Ancestor span fields are omitted so lines stay short.
#[derive(Debug, Clone, Copy)]
pub struct CborConsoleEventFormat {
    ansi: Option<bool>,
}

impl Default for CborConsoleEventFormat {
    fn default() -> Self {
        Self::new()
    }
}

impl CborConsoleEventFormat {
    pub fn new() -> Self {
        Self { ansi: None }
    }

    pub fn with_ansi(mut self, ansi: bool) -> Self {
        self.ansi = Some(ansi);
        self
    }
}

fn write_level(writer: &mut Writer<'_>, level: &tracing::Level, ansi: bool) -> fmt::Result {
    if ansi {
        let color = match *level {
            tracing::Level::ERROR => "\x1b[31m",
            tracing::Level::WARN => "\x1b[33m",
            tracing::Level::INFO => "\x1b[32m",
            tracing::Level::DEBUG => "\x1b[34m",
            tracing::Level::TRACE => "\x1b[35m",
        };
        write!(writer, "{color}{level:>5}\x1b[0m ")
    } else {
        write!(writer, "{level:>5} ")
    }
}

impl<S, N> FormatEvent<S, N> for CborConsoleEventFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(&self, ctx: &FmtContext<'_, S, N>, mut writer: Writer<'_>, event: &Event<'_>) -> fmt::Result {
        let ansi = self.ansi.unwrap_or_else(|| writer.has_ansi_escapes());

        SystemTime.format_time(&mut writer)?;
        write!(writer, " ")?;

        let meta = event.metadata();
        write_level(&mut writer, meta.level(), ansi)?;

        write!(writer, "{}: ", meta.target())?;

        let current = event.parent().and_then(|id| ctx.span(id)).or_else(|| ctx.lookup_current());
        let names_from_root: Vec<&str> =
            current.as_ref().map(|leaf| leaf.scope().from_root().map(|span| span.name()).collect()).unwrap_or_default();
        let wrapping_name = names_from_root.last().copied();
        let mut ancestors = ancestor_span_names(names_from_root).peekable();

        if ancestors.peek().is_some() {
            write_abbreviated_span_path(&mut writer, ancestors)?;
            write!(writer, ":")?;
        }

        if let Some(name) = wrapping_name {
            write!(writer, "{name} ")?;
        }

        ctx.format_fields(writer.by_ref(), event)?;

        if let Some(ref span_ref) = current {
            {
                let extensions = span_ref.extensions();
                if let Some(fields) = extensions.get::<FormattedFields<N>>()
                    && !fields.is_empty()
                {
                    write!(writer, " {fields}")?;
                }
            }
            if meta.is_span() {
                write!(writer, " id={}", span_ref.id().into_u64())?;
            }
            if let Some(parent) = span_ref.parent() {
                write!(writer, " parent_id={}", parent.id().into_u64())?;
            }
        }

        writeln!(writer)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Write},
        sync::{Arc, Mutex},
    };

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

    #[test]
    fn console_cbor_text_scalar_is_not_double_quoted() {
        let writer = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_ansi(false)
            .fmt_fields(console_field_formatter())
            .event_format(CborConsoleEventFormat::new().with_ansi(false))
            .with_max_level(tracing::Level::INFO)
            .finish();

        let hash = "3bc8f4b70575ead11872b035b6b95561b43bc7db5b5c0e304ba65e1f65eab5f2";
        tracing::subscriber::with_default(subscriber, || {
            let encoded = encode_cbor(&hash);
            tracing::info!(header_hash = encoded.as_ref() as &[u8], "tip.adopt");
        });

        let output = writer.contents();
        assert!(
            output.contains(&format!("header_hash=\"{hash}\"")),
            "expected a single-quoted hex string, got: {output}"
        );
        assert!(
            !output.contains(&format!(r#"header_hash="\"{hash}\"""#)),
            "must not Debug-quote diagnostic quotes: {output}"
        );
    }

    #[test]
    fn console_hides_tag_fields_from_nested_spans() {
        let writer = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_ansi(false)
            .fmt_fields(console_field_formatter())
            .event_format(CborConsoleEventFormat::new().with_ansi(false))
            .with_max_level(tracing::Level::INFO)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let outer = tracing::info_span!("outer", "amaru.tag.cpu" = true);
            let _outer = outer.enter();
            let inner = tracing::info_span!("inner", "amaru.tag.cpu" = true, transaction_id = "abc");
            let _inner = inner.enter();
            tracing::info!("hello");
        });

        let output = writer.contents();
        assert!(!output.contains("amaru.tag"), "tag markers must be hidden from the console: {output}");
        assert!(output.contains(" transaction_id=\"abc\""), "ordinary span fields must be kept: {output}");
        assert!(output.contains(" o:inner "), "abbreviated ancestors exclude the wrapping span: {output}");
    }
}
