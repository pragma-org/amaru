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

use std::{
    collections::BTreeMap,
    fmt,
    sync::mpsc::{SyncSender, TrySendError},
    time::{Instant, SystemTime},
};

use tracing::{
    Event, Level, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

use crate::events::{FieldValue, Message, TelemetryRecord};

const TAG_FIELD_PREFIX: &str = "amaru.tag.";

#[derive(Debug, Clone)]
pub struct TracingLayer {
    tx: SyncSender<Message>,
}

impl TracingLayer {
    pub fn new(tx: SyncSender<Message>) -> Self {
        Self { tx }
    }

    fn emit(&self, record: TelemetryRecord) {
        match self.tx.try_send(Message::Telemetry(record)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

#[derive(Debug, Clone)]
struct CapturedSpan {
    level: Level,
    target: String,
    name: String,
    wall_time: SystemTime,
    opened_at: Instant,
    fields: BTreeMap<String, FieldValue>,
}

impl<S> Layer<S> for TracingLayer
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);

        self.emit(TelemetryRecord {
            level: *event.metadata().level(),
            target: event.metadata().target().to_string(),
            name: event.metadata().name().to_string(),
            at: Instant::now(),
            wall_time: SystemTime::now(),
            fields: visitor.fields,
        });
    }

    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };

        let mut visitor = FieldVisitor::default();
        attrs.record(&mut visitor);

        span.extensions_mut().insert(CapturedSpan {
            level: *attrs.metadata().level(),
            target: attrs.metadata().target().to_string(),
            name: attrs.metadata().name().to_string(),
            wall_time: SystemTime::now(),
            opened_at: Instant::now(),
            fields: visitor.fields,
        });
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };

        let mut visitor = FieldVisitor::default();
        values.record(&mut visitor);

        if let Some(state) = span.extensions_mut().get_mut::<CapturedSpan>() {
            state.fields.extend(visitor.fields);
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };

        let mut extensions = span.extensions_mut();
        let Some(state) = extensions.remove::<CapturedSpan>() else {
            return;
        };

        self.emit(TelemetryRecord {
            level: state.level,
            target: state.target,
            name: state.name,
            at: state.opened_at,
            wall_time: state.wall_time,
            fields: state.fields,
        });
    }
}

#[derive(Default)]
struct FieldVisitor {
    fields: BTreeMap<String, FieldValue>,
}

impl FieldVisitor {
    fn insert(&mut self, field: &Field, value: FieldValue) {
        if field.name().starts_with(TAG_FIELD_PREFIX) {
            return;
        }

        self.fields.insert(field.name().to_string(), value);
    }
}

impl Visit for FieldVisitor {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.insert(field, FieldValue::F64(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, FieldValue::I64(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, FieldValue::U64(value));
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        self.insert(field, FieldValue::String(value.to_string()));
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        self.insert(field, FieldValue::String(value.to_string()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, FieldValue::Bool(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field, FieldValue::String(value.to_string()));
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.insert(field, FieldValue::String(value.to_string()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.insert(field, FieldValue::String(format!("{value:?}")));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;

    use amaru_kernel::{Epoch, NULL_HASH32, Slot, TransactionId};
    use amaru_observability::{amaru::ledger, info, info_span};
    use tracing_subscriber::prelude::*;

    use super::*;

    #[test]
    fn event_telemetry_drops_tag_fields() {
        let (tx, rx) = sync_channel(1);
        let subscriber = tracing_subscriber::registry().with(TracingLayer::new(tx));

        tracing::subscriber::with_default(subscriber, || {
            info!(ledger::transaction::VALIDATE, transaction_id = TransactionId::new(NULL_HASH32),);
        });

        let Message::Telemetry(record) = rx.recv().expect("telemetry event") else {
            panic!("expected telemetry event")
        };

        assert_eq!(record.target, ledger::transaction::VALIDATE::TARGET);
        assert_eq!(record.name, ledger::transaction::VALIDATE::NAME);
        assert_eq!(
            record.fields.get(ledger::transaction::VALIDATE::FIELD_TRANSACTION_ID),
            Some(&FieldValue::String(TransactionId::new(NULL_HASH32).to_string()))
        );
        assert!(record.fields.keys().all(|name| !name.starts_with(TAG_FIELD_PREFIX)));
    }

    #[test]
    fn span_telemetry_drops_tag_fields() {
        let (tx, rx) = sync_channel(1);
        let subscriber = tracing_subscriber::registry().with(TracingLayer::new(tx));

        tracing::subscriber::with_default(subscriber, || {
            let span = info_span!(
                ledger::tip::UPDATE,
                slot = Slot::from(42u64),
                header_hash = NULL_HASH32,
                block_height = 1u64,
                tx_count = 2usize,
                epoch = Epoch::from(3u64),
                slot_in_epoch = Slot::from(4u64),
                density = 0.5f64,
                current_kes_period = 5u64,
                remaining_kes_periods = 6u64,
            );
            let _guard = span.enter();
        });

        let Message::Telemetry(record) = rx.recv().expect("telemetry span") else { panic!("expected telemetry span") };

        assert_eq!(record.target, ledger::tip::UPDATE::TARGET);
        assert_eq!(record.name, ledger::tip::UPDATE::NAME);
        assert_eq!(record.fields.get(ledger::tip::UPDATE::FIELD_SLOT), Some(&FieldValue::String("42".to_string())));
        assert!(record.fields.keys().all(|name| !name.starts_with(TAG_FIELD_PREFIX)));
    }
}
