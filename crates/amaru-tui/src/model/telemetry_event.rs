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

use crate::events::TelemetryRecord;

pub const BOOTSTRAP_TARGET: &str = "amaru::bootstrap";
pub const CONSENSUS_TARGET: &str = "amaru::consensus";
pub const LEDGER_TARGET: &str = "amaru::ledger";
pub const MEMPOOL_TARGET: &str = "amaru::mempool";
pub const PROTOCOLS_TARGET: &str = "amaru::protocols";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryEvent {
    BootstrapPotsImport,
    EpochTransitionApply,
    EpochTransitionCompute,
    EpochTransitionRecord,
    GovernanceActivityUpdate,
    GovernanceEnacting,
    GovernanceRatifying,
    KeepaliveRoundTrip,
    MempoolStateUpdate,
    NewGovernanceUpdates,
    PeerConnected,
    PeerDisconnected,
    PotsLoad,
    ProposalActive,
    ProposalDrop,
    ProposalSkip,
    ProtocolParametersLoad,
    ProtocolParametersRatify,
    ProtocolUpgrade,
    RatificationSummarize,
    RewardsSummarize,
    StakeDistributionInitialBegin,
    StakeDistributionInitialProgress,
    StakeDistributionInitialReady,
    StateSwitchToFork,
    StakeSnapshot,
    TipUpdate,
}

impl TelemetryEvent {
    pub fn from_record(record: &TelemetryRecord) -> Option<Self> {
        match (record.target.as_str(), record.name.as_str()) {
            (LEDGER_TARGET, "tip.update") => Some(Self::TipUpdate),
            (LEDGER_TARGET, "stake_distribution.initial_begin") => Some(Self::StakeDistributionInitialBegin),
            (LEDGER_TARGET, "stake_distribution.initial_progress") => Some(Self::StakeDistributionInitialProgress),
            (LEDGER_TARGET, "stake_distribution.initial_ready") => Some(Self::StakeDistributionInitialReady),
            (LEDGER_TARGET, "stake_distribution.snapshot") => Some(Self::StakeSnapshot),
            (LEDGER_TARGET, "rewards.summarize") => Some(Self::RewardsSummarize),
            (LEDGER_TARGET, "pots.load") => Some(Self::PotsLoad),
            (LEDGER_TARGET, "state.switch_to_fork") => Some(Self::StateSwitchToFork),
            (LEDGER_TARGET, "epoch_transition.compute") => Some(Self::EpochTransitionCompute),
            (LEDGER_TARGET, "epoch_transition.record") => Some(Self::EpochTransitionRecord),
            (LEDGER_TARGET, "epoch_transition.apply") => Some(Self::EpochTransitionApply),
            (BOOTSTRAP_TARGET, "pots.import") => Some(Self::BootstrapPotsImport),
            (MEMPOOL_TARGET, "state.update") => Some(Self::MempoolStateUpdate),
            (PROTOCOLS_TARGET, "keepalive.peer.round_trip") => Some(Self::KeepaliveRoundTrip),
            (PROTOCOLS_TARGET, "peer_selection.peer.connected") => Some(Self::PeerConnected),
            (PROTOCOLS_TARGET, "peer_selection.peer.disconnected") => Some(Self::PeerDisconnected),
            (LEDGER_TARGET, "governance_activity.update") => Some(Self::GovernanceActivityUpdate),
            (LEDGER_TARGET, "epoch_transition.new_governance_updates") => Some(Self::NewGovernanceUpdates),
            (LEDGER_TARGET, "governance.ratifying") => Some(Self::GovernanceRatifying),
            (LEDGER_TARGET, "governance.enacting") => Some(Self::GovernanceEnacting),
            (LEDGER_TARGET, "proposal.active") => Some(Self::ProposalActive),
            (LEDGER_TARGET, "proposal.drop") => Some(Self::ProposalDrop),
            (LEDGER_TARGET, "proposal.skip") => Some(Self::ProposalSkip),
            (LEDGER_TARGET, "protocol.upgrade") => Some(Self::ProtocolUpgrade),
            (LEDGER_TARGET, "protocol_parameters.load") => Some(Self::ProtocolParametersLoad),
            (LEDGER_TARGET, "protocol_parameters.ratify") => Some(Self::ProtocolParametersRatify),
            (LEDGER_TARGET, "ratification.summarize") => Some(Self::RatificationSummarize),
            _ => None,
        }
    }
}
