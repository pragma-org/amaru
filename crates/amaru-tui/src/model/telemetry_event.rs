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

use amaru_observability::amaru::{bootstrap, consensus, ledger, mempool, protocols};

use crate::events::TelemetryRecord;

pub const CONSENSUS_TARGET: &str = consensus::chain_db::OPEN::TARGET;
pub const LEDGER_TARGET: &str = ledger::tip::UPDATE::TARGET;
pub const PROTOCOLS_TARGET: &str = protocols::peer_selection::peer::CONNECTED::TARGET;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryEvent {
    BootstrapPotsImport,
    BlockAdopt,
    EpochTransitionApply,
    EpochTransitionCompute,
    EpochTransitionRecord,
    GovernanceActivityUpdate,
    GovernanceEnacting,
    GovernanceRatifying,
    HeaderLifecycle,
    KeepaliveRoundTrip,
    MempoolStateUpdate,
    NewGovernanceUpdates,
    PeerConnected,
    PeerDisconnected,
    PeerResolved,
    PotsLoad,
    ProposalActive,
    ProposalDrop,
    ProposalSkip,
    ProtocolParametersLoad,
    ProtocolParametersRatify,
    ProtocolUpgrade,
    RatificationSummarize,
    RewardsSummarize,
    RollForward,
    StakeDistributionInitialBegin,
    StakeDistributionInitialProgress,
    StakeDistributionInitialReady,
    StateSwitchToFork,
    StakeSnapshot,
    TipUpdate,
    TransactionValidate,
}

impl TelemetryEvent {
    pub fn from_record(record: &TelemetryRecord) -> Option<Self> {
        if ledger::tip::UPDATE::matches(&record.target, &record.name) {
            Some(Self::TipUpdate)
        } else if ledger::transaction::VALIDATE::matches(&record.target, &record.name) {
            Some(Self::TransactionValidate)
        } else if ledger::state::ROLL_FORWARD::matches(&record.target, &record.name) {
            Some(Self::RollForward)
        } else if consensus::tip::ADOPT::matches(&record.target, &record.name) {
            Some(Self::BlockAdopt)
        } else if ledger::stake_distribution::INITIAL_BEGIN::matches(&record.target, &record.name) {
            Some(Self::StakeDistributionInitialBegin)
        } else if ledger::stake_distribution::INITIAL_PROGRESS::matches(&record.target, &record.name) {
            Some(Self::StakeDistributionInitialProgress)
        } else if ledger::stake_distribution::INITIAL_READY::matches(&record.target, &record.name) {
            Some(Self::StakeDistributionInitialReady)
        } else if ledger::stake_distribution::SNAPSHOT::matches(&record.target, &record.name) {
            Some(Self::StakeSnapshot)
        } else if ledger::rewards::SUMMARIZE::matches(&record.target, &record.name) {
            Some(Self::RewardsSummarize)
        } else if ledger::pots::LOAD::matches(&record.target, &record.name) {
            Some(Self::PotsLoad)
        } else if ledger::state::SWITCH_TO_FORK::matches(&record.target, &record.name) {
            Some(Self::StateSwitchToFork)
        } else if ledger::epoch_transition::COMPUTE::matches(&record.target, &record.name) {
            Some(Self::EpochTransitionCompute)
        } else if ledger::epoch_transition::RECORD::matches(&record.target, &record.name) {
            Some(Self::EpochTransitionRecord)
        } else if ledger::epoch_transition::APPLY::matches(&record.target, &record.name) {
            Some(Self::EpochTransitionApply)
        } else if bootstrap::pots::IMPORT::matches(&record.target, &record.name) {
            Some(Self::BootstrapPotsImport)
        } else if mempool::state::UPDATE::matches(&record.target, &record.name) {
            Some(Self::MempoolStateUpdate)
        } else if consensus::perf::header::LIFECYCLE::matches(&record.target, &record.name) {
            Some(Self::HeaderLifecycle)
        } else if protocols::keepalive::peer::ROUND_TRIP::matches(&record.target, &record.name) {
            Some(Self::KeepaliveRoundTrip)
        } else if protocols::peer_selection::peer::CONNECTED::matches(&record.target, &record.name) {
            Some(Self::PeerConnected)
        } else if protocols::peer_selection::peer::DISCONNECTED::matches(&record.target, &record.name) {
            Some(Self::PeerDisconnected)
        } else if protocols::peer_selection::peer::RESOLVED::matches(&record.target, &record.name) {
            Some(Self::PeerResolved)
        } else if ledger::governance_activity::UPDATE::matches(&record.target, &record.name) {
            Some(Self::GovernanceActivityUpdate)
        } else if ledger::epoch_transition::NEW_GOVERNANCE_UPDATES::matches(&record.target, &record.name) {
            Some(Self::NewGovernanceUpdates)
        } else if ledger::governance::RATIFYING::matches(&record.target, &record.name) {
            Some(Self::GovernanceRatifying)
        } else if ledger::governance::ENACTING::matches(&record.target, &record.name) {
            Some(Self::GovernanceEnacting)
        } else if ledger::proposal::ACTIVE::matches(&record.target, &record.name) {
            Some(Self::ProposalActive)
        } else if ledger::proposal::DROP::matches(&record.target, &record.name) {
            Some(Self::ProposalDrop)
        } else if ledger::proposal::SKIP::matches(&record.target, &record.name) {
            Some(Self::ProposalSkip)
        } else if ledger::protocol::UPGRADE::matches(&record.target, &record.name) {
            Some(Self::ProtocolUpgrade)
        } else if ledger::protocol_parameters::LOAD::matches(&record.target, &record.name) {
            Some(Self::ProtocolParametersLoad)
        } else if ledger::protocol_parameters::RATIFY::matches(&record.target, &record.name) {
            Some(Self::ProtocolParametersRatify)
        } else if ledger::ratification::SUMMARIZE::matches(&record.target, &record.name) {
            Some(Self::RatificationSummarize)
        } else {
            None
        }
    }
}
