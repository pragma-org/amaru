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

use std::time::Instant;

use amaru_observability::amaru::ledger;

use crate::events::TelemetryRecord;

#[derive(Debug, Clone, PartialEq)]
pub struct TipState {
    pub slot: u64,
    pub header_hash: String,
    pub block_height: u64,
    pub epoch: u64,
    pub slot_in_epoch: u64,
    pub density: f64,
    pub current_kes_period: u64,
    pub remaining_kes_periods: u64,
    pub updated_at: Instant,
}

impl TipState {
    pub fn from_record(record: &TelemetryRecord) -> Option<Self> {
        Some(Self {
            slot: ledger::tip::UPDATE::slot(record),
            header_hash: ledger::tip::UPDATE::header_hash(record).to_owned(),
            block_height: ledger::tip::UPDATE::block_height(record),
            epoch: ledger::tip::UPDATE::epoch(record),
            slot_in_epoch: ledger::tip::UPDATE::slot_in_epoch(record),
            density: ledger::tip::UPDATE::density(record),
            current_kes_period: ledger::tip::UPDATE::current_kes_period(record),
            remaining_kes_periods: ledger::tip::UPDATE::remaining_kes_periods(record),
            updated_at: record.at,
        })
    }
}
