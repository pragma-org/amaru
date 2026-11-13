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

use std::collections::BTreeMap;

use amaru_kernel::{
    Anchor, Credential, Epoch, GovernanceAction, Hash, Network, PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
    PREPROD_ERA_HISTORY, Proposal, ProposalId, ProposalPointer, ProposalsRootsRc, RatificationStatus, RewardAccount,
    TransactionPointer,
};
use amaru_ledger::{
    StakeDistribution, epoch_transition::GovernanceUpdates, governance::ratification::RatificationContext,
    store::columns::proposals,
};
use amaru_observability::TelemetryRecord;
use serde_json::{Value, json};

pub const FILTER: &str = "off,amaru::ledger=debug";

/// Expire a governance proposal and refund its deposit through the real epoch-transition logic.
pub fn run_ledger_fixture() {
    let epoch = Epoch::from(200);
    let parameters = PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone();
    let distribution = StakeDistribution { epoch, ..StakeDistribution::default() };
    let account = Credential::KeyHash(Hash::from([0xcd; 28]));
    let id = ProposalId { transaction_id: Hash::from([0xab; 32]), proposal_index: 0 };
    let deposit = parameters.gov_action_deposit;
    let proposal = proposals::Row {
        proposed_in: ProposalPointer {
            transaction: TransactionPointer {
                slot: PREPROD_ERA_HISTORY
                    .epoch_bounds(epoch - u64::from(parameters.gov_action_lifetime))
                    .unwrap()
                    .start,
                transaction_index: 0,
            },
            proposal_index: 0,
        },
        valid_until: epoch,
        proposal: Proposal {
            deposit,
            reward_account: RewardAccount::new(Network::Testnet, account),
            gov_action: GovernanceAction::Information,
            anchor: Anchor { content_hash: Hash::from([0; 32]), url: "https://example.com/proposal".parse().unwrap() },
        },
    };
    let context = RatificationContext {
        epoch,
        treasury: 0,
        stake_distribution: &distribution,
        protocol_parameters: parameters.clone(),
        withdrawals: BTreeMap::new(),
        constitutional_committee: None,
        constitutional_committee_update: None,
        new_constitution: None,
        votes: BTreeMap::new(),
    };
    let updates = GovernanceUpdates::new(
        ProposalsRootsRc::default(),
        [(id, proposal)].into_iter(),
        &PREPROD_ERA_HISTORY,
        &parameters,
        context,
    )
    .unwrap();
    assert_eq!(updates.pruned_proposals, BTreeMap::from([(id, RatificationStatus::NotRatified)]));
    assert_eq!(updates.deposit_refunds, BTreeMap::from([(account, deposit)]));
    assert!(updates.is_dormant_epoch);
    assert!(updates.treasury_withdrawals.is_empty());
}

/// Assign IDs in first-seen order, preserving parent relationships in the snapshot.
fn normalize_id(ids: &mut BTreeMap<u64, usize>, id: u64) -> usize {
    assert_ne!(id, 0);
    let next = ids.len() + 1;
    *ids.entry(id).or_insert(next)
}

fn validate_timestamp(timestamp: &str) {
    assert!(timestamp.contains('T') && timestamp.ends_with('Z'), "invalid timestamp: {timestamp}");
}

fn validate_duration(duration: &str) {
    let end = duration.find(|c: char| !c.is_ascii_digit() && c != '.').expect("duration unit");
    let value = duration[..end].parse::<f64>().expect("numeric duration");
    assert!(value.is_finite() && value >= 0.0);
    assert!(["ns", "µs", "ms", "s"].contains(&&duration[end..]), "invalid duration: {duration}");
}

pub fn normalize_text(output: &str) -> Value {
    let mut ids = BTreeMap::new();
    let lines: Vec<_> = output
        .lines()
        .map(|line| {
            assert!(!line.contains('\u{1b}'), "plain output must not contain ANSI");
            let (timestamp, rest) = line.split_once(' ').expect("timestamp");
            validate_timestamp(timestamp);
            let mut normalized = format!("<timestamp> {rest}");
            for key in ["id", "parent_id", "time.busy", "time.idle"] {
                let needle = format!(" {key}=");
                if let Some(start) = normalized.rfind(&needle) {
                    let start = start + needle.len();
                    let end = normalized[start..].find(' ').map_or(normalized.len(), |len| start + len);
                    let value = &normalized[start..end];
                    let replacement = if key.starts_with("time.") {
                        validate_duration(value);
                        "<duration>".to_string()
                    } else {
                        let Ok(span_id) = value.parse() else { continue };
                        normalize_id(&mut ids, span_id).to_string()
                    };
                    normalized.replace_range(start..end, &replacement);
                }
            }
            if line.contains(" close ") {
                assert!(line.contains(" time.busy=") && line.contains(" time.idle="));
            }
            normalized
        })
        .collect();
    json!(lines)
}

pub fn normalize_json(output: &str) -> Value {
    let mut ids = BTreeMap::new();
    let records: Vec<Value> = output
        .lines()
        .map(|line| {
            let mut record: Value = serde_json::from_str(line).expect("NDJSON record");
            validate_timestamp(record["timestamp"].as_str().expect("timestamp"));
            record["timestamp"] = json!("<timestamp>");
            for key in ["id", "parent_id"] {
                if let Some(value) = record.get_mut(key) {
                    *value = json!(normalize_id(&mut ids, value.as_u64().expect("numeric span ID")));
                }
            }
            if record["fields"]["message"] == "close" {
                for key in ["time.busy", "time.idle"] {
                    validate_duration(record["fields"][key].as_str().expect("span close duration"));
                    record["fields"][key] = json!("<duration>");
                }
            }
            record
        })
        .collect();
    json!(records)
}

pub fn normalize_tui(records: Vec<TelemetryRecord>) -> Value {
    let mut ids = BTreeMap::new();
    let records: Vec<_> = records
        .into_iter()
        .map(|record| {
            let fields: BTreeMap<_, _> =
                record.fields.into_iter().map(|(key, value)| (key, format!("{value:?}"))).collect();
            json!({
                "level": record.level.as_str(), "target": record.target, "name": record.name,
                "fields": fields, "parents": record.parents, "span_name": record.span_name,
                "id": record.id.map(|id| normalize_id(&mut ids, id)),
                "parent_id": record.parent_id.map(|id| normalize_id(&mut ids, id)),
                "duration": record.duration.map(|_| "<duration>"),
            })
        })
        .collect();
    json!(records)
}
