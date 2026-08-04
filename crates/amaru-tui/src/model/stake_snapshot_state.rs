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
pub struct StakeSnapshotState {
    pub accounts: usize,
    pub pools: usize,
    pub dreps: usize,
    pub active_stake: u64,
    pub pools_voting_stake: u64,
    pub dreps_voting_stake: u64,
    pub updated_at: Instant,
}

impl StakeSnapshotState {
    pub fn from_record(record: &TelemetryRecord) -> Option<Self> {
        Some(Self {
            accounts: ledger::stake_distribution::SNAPSHOT::accounts(record),
            pools: ledger::stake_distribution::SNAPSHOT::pools(record),
            dreps: ledger::stake_distribution::SNAPSHOT::dreps(record),
            active_stake: ledger::stake_distribution::SNAPSHOT::active_stake(record),
            pools_voting_stake: ledger::stake_distribution::SNAPSHOT::pools_voting_stake(record),
            dreps_voting_stake: ledger::stake_distribution::SNAPSHOT::dreps_voting_stake(record),
            updated_at: record.at,
        })
    }
}
