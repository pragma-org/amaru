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
pub struct ProposalActivity {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub detail: Option<String>,
    pub proposed_in: Option<u64>,
    pub valid_until: Option<u64>,
    pub constitutional_committee: Option<bool>,
    pub delegate_representatives: Option<bool>,
    pub stake_pool_operators: Option<bool>,
    pub seen_at: Instant,
}

impl ProposalActivity {
    pub fn from_record(record: &TelemetryRecord, status: &str, detail: Option<String>) -> Self {
        let mut proposal = Self {
            id: proposal_id(record).unwrap_or_else(|| "unknown".to_owned()),
            kind: proposal_kind(record).unwrap_or("unknown").to_owned(),
            status: status.to_owned(),
            detail: None,
            proposed_in: None,
            valid_until: None,
            constitutional_committee: None,
            delegate_representatives: None,
            stake_pool_operators: None,
            seen_at: record.at,
        };
        proposal.merge_from_record(record, status, detail);
        proposal
    }

    pub fn merge_from_record(&mut self, record: &TelemetryRecord, status: &str, detail: Option<String>) {
        if let Some(kind) = proposal_kind(record) {
            self.kind = kind.to_owned();
        }
        self.status = status.to_owned();
        if let Some(detail) = detail {
            self.detail = Some(detail);
        }
        if let Some(proposed_in) = proposed_in(record) {
            self.proposed_in = Some(proposed_in);
        }
        if let Some(valid_until) = valid_until(record) {
            self.valid_until = Some(valid_until);
        }
        if let Some(approved) = approved_by_constitutional_committee(record) {
            self.constitutional_committee = Some(approved);
        }
        if let Some(approved) = approved_by_dreps(record) {
            self.delegate_representatives = Some(approved);
        }
        if let Some(approved) = approved_by_pools(record) {
            self.stake_pool_operators = Some(approved);
        }
        self.seen_at = record.at;
    }
}

pub fn proposal_id(record: &TelemetryRecord) -> Option<String> {
    if ledger::governance::RATIFYING::matches(&record.target, &record.name) {
        Some(ledger::governance::RATIFYING::proposal_id(record).to_owned())
    } else if ledger::governance::ENACTING::matches(&record.target, &record.name) {
        Some(ledger::governance::ENACTING::proposal_id(record).to_owned())
    } else if ledger::proposal::ACTIVE::matches(&record.target, &record.name) {
        Some(ledger::proposal::ACTIVE::id(record).to_owned())
    } else if ledger::proposal::DROP::matches(&record.target, &record.name) {
        Some(ledger::proposal::DROP::id(record).to_owned())
    } else if ledger::proposal::SKIP::matches(&record.target, &record.name) {
        Some(ledger::proposal::SKIP::id(record).to_owned())
    } else {
        None
    }
}

fn proposal_kind(record: &TelemetryRecord) -> Option<&str> {
    if ledger::governance::RATIFYING::matches(&record.target, &record.name) {
        Some(ledger::governance::RATIFYING::proposal_kind(record))
    } else if ledger::governance::ENACTING::matches(&record.target, &record.name) {
        Some(ledger::governance::ENACTING::proposal_kind(record))
    } else if ledger::proposal::ACTIVE::matches(&record.target, &record.name) {
        Some(ledger::proposal::ACTIVE::proposal_kind(record))
    } else {
        None
    }
}

fn proposed_in(record: &TelemetryRecord) -> Option<u64> {
    if ledger::proposal::ACTIVE::matches(&record.target, &record.name) {
        Some(ledger::proposal::ACTIVE::proposed_in(record))
    } else if ledger::proposal::SKIP::matches(&record.target, &record.name) {
        ledger::proposal::SKIP::proposed_in(record).and_then(|value| value.parse().ok())
    } else {
        None
    }
}

fn valid_until(record: &TelemetryRecord) -> Option<u64> {
    ledger::proposal::ACTIVE::matches(&record.target, &record.name)
        .then(|| ledger::proposal::ACTIVE::valid_until(record))
}

fn approved_by_constitutional_committee(record: &TelemetryRecord) -> Option<bool> {
    ledger::governance::RATIFYING::approved_by_constitutional_committee(record)
}

fn approved_by_dreps(record: &TelemetryRecord) -> Option<bool> {
    ledger::governance::RATIFYING::approved_by_dreps(record)
}

fn approved_by_pools(record: &TelemetryRecord) -> Option<bool> {
    ledger::governance::RATIFYING::approved_by_pools(record)
}
