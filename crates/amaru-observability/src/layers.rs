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
//! recorded via typed visit methods pass through unchanged.
//!
//! JSON NDJSON formatting lives in [`crate::json_format`].

use std::fmt;

use tracing::field::{Field, Visit};
use tracing_subscriber::{
    field::{MakeVisitor, VisitFmt, VisitOutput},
    fmt::format::{DefaultFields, Writer},
};

use crate::field::cbor_diagnostic;
pub use crate::json_format::{CborJsonEventFormat, CborJsonFields, CborJsonSpanLayer, SpanJsonFields};

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
}
