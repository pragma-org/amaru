//! This module implements a tracing layer that serializes all events and spans into CBOR.
//!
//! ## Background
//!
//! The [`tracing`] infrastructure hands each span or event site to the active [`Subscriber`](tracing::Subscriber) for the
//! current thread. In practice, subscribers are composed using the [`tracing_subscriber`] crate from
//! [`Layer`]s, which is a bit of a misnomer because these components can be stacked or sequenced, but
//! they all receive the same data (i.e. fields), meaning that this is not your grandpa's functional
//! compositionality.
//!
//! ## Approach
//!
//! Amaru will use Arnaud's "One Log" paradigm, where all logging/monitoring/observability data
//! flows through `tracing` statements. We will then attach multiple layers for different purposes,
//! e.g. one for printing textual logs to stdout, one for sending CBOR traces to a file or remote
//! service, one for capturing only the stage effects in a ringbuffer (for dumping and debugging in
//! case of failures).
//!
//! This is achieved by pairing each of these layers with an appropriate [`Filter`](tracing_subscriber::layer::Filter) to select only
//! the relevant spans and events, and then stacking the [`Filtered`](tracing_subscriber::filter::Filtered) layers on top of a
//! [`tracing_subscriber::Registry`] that is used to dispatch events to the active subscribers.

// ********************************************************
// NOTE: This is a work in progress.
// ********************************************************

#![allow(unused)]

use cbor4ii::{
    core::{
        enc::{Encode, Write},
        utils::IoWriter,
    },
    serde::{from_slice, to_writer},
};
use parking_lot::Mutex;
use std::sync::Arc;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

#[derive(Default)]
struct CborVisitor {
    fields: Vec<(&'static str, Vec<u8>)>,
}

impl CborVisitor {
    fn add_field(&mut self, name: &'static str, value: Vec<u8>) {
        self.fields.push((name, value));
    }
}

#[allow(clippy::unwrap_used)]
impl tracing::field::Visit for CborVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let mut buf = Vec::new();
        to_writer(&mut buf, &format!("{:?}", value)).unwrap();
        self.add_field(field.name(), buf);
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        let mut buf = Vec::new();
        to_writer(&mut buf, &value).unwrap();
        self.add_field(field.name(), buf);
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        let mut buf = Vec::new();
        to_writer(&mut buf, &value).unwrap();
        self.add_field(field.name(), buf);
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        let mut buf = Vec::new();
        to_writer(&mut buf, &value).unwrap();
        self.add_field(field.name(), buf);
    }

    fn record_i128(&mut self, field: &tracing::field::Field, value: i128) {
        let mut buf = Vec::new();
        to_writer(&mut buf, &value).unwrap();
        self.add_field(field.name(), buf);
    }

    fn record_u128(&mut self, field: &tracing::field::Field, value: u128) {
        let mut buf = Vec::new();
        to_writer(&mut buf, &value).unwrap();
        self.add_field(field.name(), buf);
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        let mut buf = Vec::new();
        to_writer(&mut buf, &value).unwrap();
        self.add_field(field.name(), buf);
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        let mut buf = Vec::new();
        to_writer(&mut buf, &value).unwrap();
        self.add_field(field.name(), buf);
    }

    fn record_bytes(&mut self, field: &tracing::field::Field, value: &[u8]) {
        // Assume the bytes are already CBOR encoded
        self.add_field(field.name(), value.to_vec());
    }

    fn record_error(
        &mut self,
        field: &tracing::field::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        let mut buf = Vec::new();
        to_writer(&mut buf, &format!("{}", value)).unwrap();
        self.add_field(field.name(), buf);
    }
}

pub trait CborEmitter: Send + Sync + 'static {
    fn emit(&self, value: &[u8]);
}

struct CborLayer(Box<dyn CborEmitter>);

impl CborLayer {
    pub fn new(emitter: impl CborEmitter + 'static) -> Self {
        Self(Box::new(emitter))
    }

    fn emit(&self, value: &[u8]) {
        self.0.emit(value);
    }
}

impl<S> Layer<S> for CborLayer
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        let mut visitor = CborVisitor::default();
        attrs.record(&mut visitor);

        if let Some(span) = ctx.span(id) {
            let mut extensions = span.extensions_mut();
            extensions.insert(visitor.fields);
        }
    }

    #[allow(clippy::expect_used)]
    fn on_enter(&self, id: &tracing::span::Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let ext = span.extensions();
            let fields = ext.get::<Vec<(&'static str, Vec<u8>)>>();

            let mut span_cbor = Vec::new();
            let mut writer = IoWriter::new(&mut span_cbor);

            cbor4ii::core::types::Map::bounded(
                1 + fields.map(|x| x.len()).unwrap_or_default(),
                &mut writer,
            )
            .expect("serialization should not fail");

            // Add span name
            "span_enter"
                .encode(&mut writer)
                .expect("serialization should not fail");
            span.name()
                .encode(&mut writer)
                .expect("serialization should not fail");

            if let Some(fields) = fields {
                for (key, value) in fields {
                    key.encode(&mut writer)
                        .expect("serialization should not fail");
                    writer.push(value).expect("serialization should not fail");
                }
            }

            self.emit(&span_cbor);
        }
    }

    #[allow(clippy::expect_used)]
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = CborVisitor::default();
        event.record(&mut visitor);

        let mut event_cbor = Vec::new();
        let mut writer = IoWriter::new(&mut event_cbor);

        cbor4ii::core::types::Map::bounded(visitor.fields.len(), &mut writer)
            .expect("serialization should not fail");

        for (key, value) in visitor.fields {
            key.encode(&mut writer)
                .expect("serialization should not fail");
            writer.push(&value).expect("serialization should not fail");
        }

        self.emit(&event_cbor);
    }
}

impl CborEmitter for Arc<Mutex<Vec<u8>>> {
    fn emit(&self, value: &[u8]) {
        self.lock().extend_from_slice(value);
    }
}

#[allow(clippy::disallowed_types)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, mem::take};
    use tracing::{info, info_span};
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn fix_representation() {
        #[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
        struct A {
            x: u8,
        }
        let a = A { x: 1 };
        let mut buf = Vec::new();
        cbor4ii::serde::to_writer(&mut buf, &a).unwrap();
        assert_eq!(buf, &[0xa1, 0x61, 0x78, 0x01]);
        let aa = cbor4ii::serde::from_slice::<A>(&buf).unwrap();
        assert_eq!(a, aa);
        cbor4ii::serde::to_writer(&mut buf, &a).unwrap();
        assert_eq!(buf, &[0xa1, 0x61, 0x78, 0x01, 0xa1, 0x61, 0x78, 0x01]);

        #[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
        enum B {
            Unit,
            One(u8),
            Two(u8, u8),
        }
        let b = vec![B::Unit, B::One(1), B::Two(2, 3)];
        buf.clear();
        cbor4ii::serde::to_writer(&mut buf, &b).unwrap();
        assert_eq!(
            buf,
            &[
                0x83, // array of 3 elements
                0x64, 0x55, 0x6e, 0x69, 0x74, // "Unit"
                0xa1, // map of 1 pair
                0x63, 0x4f, 0x6e, 0x65, // "One"
                0x01, // 1
                0xa1, // map of 1 pair
                0x63, 0x54, 0x77, 0x6f, // "Two"
                0x82, 0x02, 0x03, // [2, 3]
            ]
        );
        let bb = cbor4ii::serde::from_slice::<Vec<B>>(&buf).unwrap();
        assert_eq!(b, bb);

        buf.clear();
        cbor4ii::serde::to_writer(&mut buf, &()).unwrap();
        assert_eq!(buf, &[0x80]); // unit is encoded as empty array
        cbor4ii::serde::from_slice::<()>(&buf).unwrap();

        buf.clear();
        let c = vec![Some(Some("abc")), Some(None), None];
        cbor4ii::serde::to_writer(&mut buf, &c).unwrap();
        // cbor4ii doesn't distinguish between `None` and `Some(None)` (serde_cbor also doesn't)
        assert_eq!(buf, &[0x83, 0x63, 0x61, 0x62, 0x63, 0xf6, 0xf6]);
        let cc = cbor4ii::serde::from_slice::<Vec<Option<Option<&str>>>>(&buf).unwrap();
        assert_eq!(cc, vec![Some(Some("abc")), None, None]);
    }

    #[test]
    fn test_cbor_tracing() {
        let collector = Arc::new(Mutex::new(Vec::new()));
        let _guard = tracing::dispatcher::set_default(&tracing::Dispatch::new(
            tracing_subscriber::registry().with(CborLayer::new(collector.clone())),
        ));

        info_span!("test_span", field1 = 42, field2 = "value").in_scope(|| {
            info!(message = "test event", field3 = 123);
        });

        let traces = take(&mut *collector.lock());
        let mut reader = std::io::Cursor::new(traces);

        // Verify span
        let span: HashMap<String, cbor4ii::core::Value> =
            cbor4ii::serde::from_reader(&mut reader).unwrap();
        assert!(span.iter().any(|(key, _)| key == "span_enter"));
        assert!(span.iter().any(|(key, _)| key == "field1"));
        assert!(span.iter().any(|(key, _)| key == "field2"));

        // Verify event
        let event: HashMap<String, cbor4ii::core::Value> =
            cbor4ii::serde::from_reader(&mut reader).unwrap();
        assert!(event.iter().any(|(key, _)| key == "message"));
        assert!(event.iter().any(|(key, _)| key == "field3"));
    }
}
