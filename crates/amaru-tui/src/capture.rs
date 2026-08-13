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

//! TUI telemetry capture is built on [`amaru_observability::TelemetryCaptureLayer`].

use std::collections::BTreeMap;

use amaru_observability::{FieldValue as ObsFieldValue, TelemetryRecord as ObsTelemetryRecord};

use crate::events::{FieldValue, TelemetryRecord};

/// Convert an observability capture record into the TUI event model.
pub fn from_observability(record: ObsTelemetryRecord) -> TelemetryRecord {
    TelemetryRecord {
        level: record.level,
        target: record.target,
        name: record.name,
        // Prefer span open time when duration is known so duration widgets stay coherent.
        at: record.duration.map(|d| record.at.checked_sub(d).unwrap_or(record.at)).unwrap_or(record.at),
        wall_time: record.wall_time,
        fields: convert_fields(record.fields),
        parents: record.parents,
        span_name: record.span_name,
        id: record.id,
        parent_id: record.parent_id,
    }
}

fn convert_fields(fields: BTreeMap<String, ObsFieldValue>) -> BTreeMap<String, FieldValue> {
    fields.into_iter().map(|(k, v)| (k, convert_field(v))).collect()
}

fn convert_field(value: ObsFieldValue) -> FieldValue {
    match value {
        ObsFieldValue::Bool(v) => FieldValue::Bool(v),
        ObsFieldValue::I64(v) => FieldValue::I64(v),
        ObsFieldValue::U64(v) => FieldValue::U64(v),
        ObsFieldValue::F64(v) => FieldValue::F64(v),
        ObsFieldValue::Str(v) | ObsFieldValue::Debug(v) => FieldValue::String(v),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;

    use amaru_kernel::{Epoch, NULL_HASH32, Slot, TransactionId};
    use amaru_observability::{TelemetryCaptureLayer, amaru::ledger, info, info_span};
    use tracing_subscriber::prelude::*;

    use super::*;
    use crate::events::Message;

    #[test]
    fn event_telemetry_drops_tag_fields() {
        let (tx, rx) = sync_channel(1);
        let subscriber = tracing_subscriber::registry().with(TelemetryCaptureLayer::new(tx));

        tracing::subscriber::with_default(subscriber, || {
            info!(ledger::transaction::VALIDATE, transaction_id = TransactionId::new(NULL_HASH32),);
        });

        let record = from_observability(rx.recv().expect("telemetry event"));
        assert_eq!(record.target, ledger::transaction::VALIDATE::TARGET);
        assert_eq!(record.name, ledger::transaction::VALIDATE::NAME);
        assert_eq!(
            record.fields.get(ledger::transaction::VALIDATE::FIELD_TRANSACTION_ID),
            Some(&FieldValue::String(TransactionId::new(NULL_HASH32).to_string()))
        );
        assert!(!record.fields.keys().any(|k| k.starts_with("amaru.tag.")));
    }

    #[test]
    fn span_close_emits_record() {
        let (tx, rx) = sync_channel(4);
        let subscriber = tracing_subscriber::registry().with(TelemetryCaptureLayer::new(tx));

        tracing::subscriber::with_default(subscriber, || {
            let span = info_span!(
                ledger::tip::UPDATE,
                slot = Slot::new(42),
                header_hash = NULL_HASH32,
                block_height = 1u64,
                tx_count = 0usize,
                epoch = Epoch::new(1),
                slot_in_epoch = Slot::new(1),
                density = 0.0f64,
                current_kes_period = 0u64,
                remaining_kes_periods = 0u64,
            );
            let _g = span.enter();
        });

        let record = from_observability(rx.recv().expect("span close"));
        assert_eq!(record.target, ledger::tip::UPDATE::TARGET);
        // Slot is a Serialize newtype → CBOR `record_bytes` → decoded back to U64
        // (handled in TelemetryCaptureLayer, not in this thin converter).
        assert_eq!(
            record.fields.get(ledger::tip::UPDATE::FIELD_SLOT),
            Some(&FieldValue::U64(42)),
            "slot must be typed U64 after CBOR decode, not raw bytes Debug"
        );
        assert_eq!(record.fields.get(ledger::tip::UPDATE::FIELD_EPOCH), Some(&FieldValue::U64(1)));
        let _ = Message::Telemetry(record);
    }

    #[test]
    fn event_inside_nested_spans_keeps_parents() {
        let (tx, rx) = sync_channel(4);
        let subscriber = tracing_subscriber::registry().with(TelemetryCaptureLayer::new(tx));

        tracing::subscriber::with_default(subscriber, || {
            let outer = info_span!(ledger::epoch_transition::COMPUTE, from = Epoch::new(599), into = Epoch::new(600),);
            let _outer = outer.enter();
            let inner = info_span!(ledger::governance::RATIFY_PROPOSALS, epoch = Epoch::new(598),);
            let _inner = inner.enter();
            info!(ledger::ratification::SUMMARIZE, is_dormant_epoch = false);
        });

        let records: Vec<_> = rx.try_iter().map(from_observability).collect();
        let event = records.iter().find(|r| r.name == ledger::ratification::SUMMARIZE::NAME).expect("summarize event");
        assert_eq!(event.parents, vec![ledger::epoch_transition::COMPUTE::NAME.to_string()]);
        assert_eq!(event.span_name.as_deref(), Some(ledger::governance::RATIFY_PROPOSALS::NAME));
        let expected_ancestors =
            amaru_observability::format_abbreviated_span_path([ledger::epoch_transition::COMPUTE::NAME]);
        assert_eq!(event.parents_label().as_deref(), Some(expected_ancestors.as_str()));
        let expected_path = format!("{expected_ancestors}:{}", ledger::governance::RATIFY_PROPOSALS::NAME);
        assert_eq!(event.span_path_label().as_deref(), Some(expected_path.as_str()));
        assert!(event.parent_id.is_some());
    }
}
