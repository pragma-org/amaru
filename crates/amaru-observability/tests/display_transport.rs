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

use amaru_kernel::{Epoch, Hash, Peer, Slot};
use amaru_observability::{FieldValue, TelemetryCaptureLayer, amaru, info};
use tracing_subscriber::prelude::*;

#[test]
fn peer_and_hash_schema_fields_are_emitted_as_plain_strings() {
    let (tx, rx) = std::sync::mpsc::sync_channel(4);
    let subscriber = tracing_subscriber::registry().with(TelemetryCaptureLayer::new(tx));

    let peer = Peer::new("1.2.3.4:3001");
    let header_hash = Hash::<32>::from([0xabu8; 32]);

    tracing::subscriber::with_default(subscriber, || {
        info!(amaru::protocols::manager::peer::ADD, peer = peer.clone());
        info!(
            amaru::ledger::tip::UPDATE,
            slot = Slot::from(42),
            header_hash = header_hash,
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
