use serde_json as json;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard};
use tracing::Dispatch;
use tracing_subscriber::layer::SubscriberExt;

#[repr(transparent)]
#[derive(Clone, Default)]
pub struct JsonTraceCollector(Arc<RwLock<Vec<json::Value>>>);

impl JsonTraceCollector {
    fn insert(&self, value: json::Value) {
        if let Ok(mut lines) = self.0.write() {
            lines.push(value);
        }
    }

    pub fn get_traces(
        &self,
    ) -> Result<
        RwLockReadGuard<'_, Vec<json::Value>>,
        PoisonError<RwLockReadGuard<'_, Vec<json::Value>>>,
    > {
        self.0.read()
    }
}

#[derive(Default)]
struct JsonVisitor {
    fields: json::Map<String, json::Value>,
}

impl JsonVisitor {
    #[allow(clippy::unwrap_used)]
    fn add_field(&mut self, json_path: &str, value: json::Value) {
        let steps = json_path.split('.').collect::<Vec<_>>();

        if steps.is_empty() {
            return;
        }

        if steps.len() == 1 {
            self.fields.insert(json_path.to_string(), value);
            return;
        }

        // Safe because we just ensured steps is never empty
        let (root, children) = steps.split_first().unwrap();

        let mut current_value = self
            .fields
            .entry(root.to_string())
            .or_insert_with(|| json::json!({}));

        for &key in children.iter().take(children.len() - 1) {
            if !current_value.is_object() {
                *current_value = json::json!({});
            }

            // Safe because we just ensured current_value is an object
            let current_object = current_value.as_object_mut().unwrap();

            if !current_object.contains_key(key) {
                current_object.insert(key.to_string(), json::json!({}));
            }

            // Safe because we just inserted the key if it didn't exist
            current_value = current_object.get_mut(key).unwrap()
        }

        if let Some(last) = children.last() {
            if !current_value.is_object() {
                *current_value = json::json!({});
            }

            // Safe because we just ensured that current_value is always an object
            current_value
                .as_object_mut()
                .unwrap()
                .insert(last.to_string(), value);
        }
    }
}

macro_rules! record_t {
    ($title:ident, $ty:ty) => {
        fn $title(&mut self, field: &tracing::field::Field, value: $ty) {
            self.add_field(field.name(), json::json!(value));
        }
    };
}

impl tracing::field::Visit for JsonVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.add_field(field.name(), json::json!(format!("{:?}", value)))
    }

    record_t!(record_f64, f64);
    record_t!(record_i64, i64);
    record_t!(record_u64, u64);
    record_t!(record_i128, i128);
    record_t!(record_u128, u128);
    record_t!(record_bool, bool);
    record_t!(record_str, &str);

    fn record_bytes(&mut self, field: &tracing::field::Field, value: &[u8]) {
        self.add_field(field.name(), json::json!(hex::encode(value)));
    }

    fn record_error(
        &mut self,
        field: &tracing::field::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        self.add_field(field.name(), json::json!(format!("{}", value)))
    }
}

struct JsonLayer(JsonTraceCollector);

impl JsonLayer {
    pub fn new(collector: JsonTraceCollector) -> Self {
        Self(collector)
    }
}

impl<S> tracing_subscriber::Layer<S> for JsonLayer
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = JsonVisitor::default();
        attrs.record(&mut visitor);

        if let Some(span) = ctx.span(id) {
            // Store the fields in the span for later use
            let mut extensions = span.extensions_mut();
            extensions.insert(visitor.fields);
        }
    }

    fn on_enter(&self, id: &tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut span_json = json::json!({
                "name": span.name(),
            });

            if let Some(fields) = span.extensions().get::<json::Map<String, json::Value>>() {
                for (key, value) in fields {
                    span_json[key] = value.clone();
                }
            }

            self.0.insert(span_json);
        }
    }
}

pub fn with_tracing<F, R>(test_fn: F) -> R
where
    F: FnOnce(&JsonTraceCollector) -> R,
{
    let collector = JsonTraceCollector::default();
    let layer = JsonLayer::new(collector.clone());
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = Dispatch::new(subscriber);
    let _guard = tracing::dispatcher::set_default(&dispatch);
    test_fn(&collector)
}

#[derive(Debug)]
// This is just to provide additional context when a test fails due to invalid tracing
// Not actually used in code (except for Debug), so we have to allow dead_code
#[allow(dead_code)]
pub enum InvalidTrace {
    TraceLengthMismatch {
        expected: usize,
        actual: usize,
    },
    InvalidTrace {
        expected: serde_json::Value,
        actual: serde_json::Value,
        index: usize,
    },
}

pub fn verify_traces(
    actual_traces: Vec<json::Value>,
    expected_traces: Vec<json::Value>,
) -> Result<(), InvalidTrace> {
    let actual_len = actual_traces.len();
    let expected_len = expected_traces.len();

    for (index, (actual, expected)) in actual_traces
        .into_iter()
        .zip(expected_traces.into_iter())
        .enumerate()
    {
        if actual != expected {
            return Err(InvalidTrace::InvalidTrace {
                expected,
                actual,
                index,
            });
        }
    }

    if actual_len != expected_len {
        return Err(InvalidTrace::TraceLengthMismatch {
            expected: expected_len,
            actual: actual_len,
        });
    }

    Ok(())
}
