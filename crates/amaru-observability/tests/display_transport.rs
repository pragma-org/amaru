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

use amaru_kernel::{BlockHeight, Epoch, Hash, Peer, Point, Slot};
use amaru_observability::{
    FieldValue, TelemetryCaptureLayer, amaru,
    field::{cbor_to_json, encode_cbor},
    info,
};
use serde_json::json;
use tracing_subscriber::prelude::*;

#[test]
fn peer_and_hash_schema_fields_are_emitted_as_plain_strings() {
    let (tx, rx) = std::sync::mpsc::sync_channel(4);
    let subscriber = tracing_subscriber::registry().with(TelemetryCaptureLayer::new(tx));

    let peer: Peer = "1.2.3.4:3001".parse().unwrap();
    let header_hash = Hash::<32>::from([0xabu8; 32]);

    tracing::subscriber::with_default(subscriber, || {
        info!(amaru::protocols::manager::peer::ADD, peer = peer);
        info!(
            amaru::ledger::tip::UPDATE,
            slot = Slot::from(42),
            header_hash,
            block_height = 99_u64,
            tx_count = 3_usize,
            epoch = Epoch::from(1),
            slot_in_epoch = Slot::from(42),
            density = 0.5_f64,
            current_kes_period = 7_u64,
            remaining_kes_periods = 20_u64
        );
    });

    let records: Vec<_> = rx.try_iter().collect();
    let peer_record =
        records.iter().find(|record| record.name == amaru::protocols::manager::peer::ADD::NAME).expect("peer event");
    let tip_record = records.iter().find(|record| record.name == amaru::ledger::tip::UPDATE::NAME).expect("tip event");

    assert_eq!(peer_record.fields.get("peer"), Some(&FieldValue::Str(peer.to_string())));
    assert_eq!(tip_record.fields.get("header_hash"), Some(&FieldValue::Str(header_hash.to_string())));
}

#[test]
fn point_encodes_as_slot_hash_height_array_with_cbor_byte_string_hash() {
    let hash = Hash::<32>::from([0xabu8; 32]);
    let point = Point::Specific(Slot::from(42), hash, BlockHeight::from(7));
    let hex = "ab".repeat(32);
    assert_eq!(cbor_to_json(&encode_cbor(&point)).expect("json"), json!([42, hex, 7]));
    assert_eq!(cbor_to_json(&encode_cbor(&Point::Origin)).expect("origin"), json!([]));
}
