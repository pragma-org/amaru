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

use amaru_consensus::{
    effects::{Ledger, LedgerOps},
    errors::ConsensusError,
    stages::{
        adopt_chain::{self, AdoptChain},
        select_chain::SelectChainMsg,
        validate_block::{self, ValidateBlock, ValidateBlockMsg},
    },
};
use amaru_kernel::{Epoch, IsHeader, Point, RawBlock, Slot};
use amaru_protocols::{manager::ManagerMessage, store_effects::Store};
use amaru_pure_stage::{Effects, Receiver, Sender, StageGraph, StageRef};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) enum MithrilInput {
    Block(RawBlock),
    Validation { point: Point, valid: bool },
    Adopted(Point),
    StakeDistUpdated(Epoch),
    Unexpected(String),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) enum MithrilBlockEvent {
    Failed(Point, String),
    Applied(Point),
    Finished,
}

/// Sequential immutable replay into the node's validation and adoption pipeline.
/// Validation is sent before adoption is requested; this mailbox therefore persists the
/// validation result before acknowledging adoption to the caller.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct MithrilBlockSource {
    pub(crate) validate_block: StageRef<ValidateBlockMsg>,
    pub(crate) events: StageRef<MithrilBlockEvent>,
    pub(crate) tip: Point,
    pub(crate) until_slot: Option<Slot>,
    pub(crate) pending: Option<(Epoch, RawBlock)>,
}

pub(crate) async fn mithril_block_source(
    mut state: MithrilBlockSource,
    msg: MithrilInput,
    eff: Effects<MithrilInput>,
) -> MithrilBlockSource {
    let store = Store::new(eff.clone());
    let raw = match msg {
        MithrilInput::Block(raw) => Some(raw),
        MithrilInput::StakeDistUpdated(epoch) => {
            if state.pending.as_ref().is_some_and(|(target, _)| epoch >= *target) {
                state.pending.take().map(|(_, raw)| raw)
            } else {
                None
            }
        }
        MithrilInput::Validation { point, valid } => {
            match store.set_block_valid(&point.hash(), valid).await {
                Ok(()) if valid => {}
                Ok(()) => {
                    eff.send(&state.events, MithrilBlockEvent::Failed(point, "ledger rejected block".into())).await
                }
                Err(error) => eff.send(&state.events, MithrilBlockEvent::Failed(point, error.to_string())).await,
            }
            None
        }
        MithrilInput::Adopted(point) => {
            state.tip = point;
            eff.send(&state.events, MithrilBlockEvent::Applied(point)).await;
            None
        }
        MithrilInput::Unexpected(reason) => {
            eff.send(&state.events, MithrilBlockEvent::Failed(state.tip, reason)).await;
            None
        }
    };
    let Some(raw) = raw else { return state };
    let header = match raw.decode_header() {
        Ok(header) => header,
        Err(error) => {
            eff.send(&state.events, MithrilBlockEvent::Failed(state.tip, error.to_string())).await;
            return state;
        }
    };
    let point = header.point();
    if state.until_slot.is_some_and(|until| point.slot_or_default() > until) {
        eff.send(&state.events, MithrilBlockEvent::Finished).await;
        return state;
    }
    if state.tip == Point::Origin {
        eff.send(&state.events, MithrilBlockEvent::Failed(point, "replay from origin is not supported".into())).await;
        return state;
    }
    if header.parent_hash() != Some(state.tip.hash()) || header.block_height() != state.tip.block_height() + 1 {
        eff.send(&state.events, MithrilBlockEvent::Failed(point, "block does not extend the replay tip".into())).await;
        return state;
    }
    if let Err(error) = async {
        if store.get_nonces(&header.hash()).await.is_none() {
            let nonces = Ledger::new(eff.clone())
                .validate_header(&header)
                .await
                .map_err(|error| ConsensusError::InvalidHeader(point, Box::new(error)))?;
            store
                .store_validated_header(&header, &nonces)
                .await
                .map_err(|error| ConsensusError::StoreHeaderFailed(header.hash(), error))?;
        }
        Ok::<(), ConsensusError>(())
    }
    .await
    {
        match error.as_invalid_header().and_then(|source| source.missing_stake_distribution()) {
            Some(epoch) => state.pending = Some((epoch, raw)),
            None => eff.send(&state.events, MithrilBlockEvent::Failed(point, error.to_string())).await,
        }
        return state;
    }
    if let Err(error) = store.store_block(&point.hash(), &raw).await {
        eff.send(&state.events, MithrilBlockEvent::Failed(point, error.to_string())).await;
        return state;
    }
    eff.send(&state.validate_block, ValidateBlockMsg::new(point, state.tip, point.block_height())).await;
    state
}

/// Attach a Mithril source to the same validation and adoption stages as the live node.
#[expect(clippy::wildcard_enum_match_arm)]
pub(super) fn wire_mithril_stages(
    stage_graph: &mut impl StageGraph,
    ledger_tip: Point,
    security_param: u64,
    until_slot: Option<Slot>,
) -> (Sender<MithrilInput>, Receiver<MithrilBlockEvent>) {
    let (events_ref, events) = stage_graph.output::<MithrilBlockEvent>("mithril_events", 2);
    let source = stage_graph.stage("mithril_blocks", mithril_block_source);
    let manager = source.sender().contramap(|message: ManagerMessage| match message {
        ManagerMessage::NewTip(point, _) => MithrilInput::Adopted(point),
        other => MithrilInput::Unexpected(format!("unexpected manager message: {other:?}")),
    });
    let validation_results = source.sender().contramap(|message: SelectChainMsg| match message {
        SelectChainMsg::BlockValidationResult(point, valid, ..) => MithrilInput::Validation { point, valid },
        other => MithrilInput::Unexpected(format!("unexpected validation message: {other:?}")),
    });
    let adopt_chain = stage_graph.stage("adopt_chain", adopt_chain::stage);
    let adopt_chain =
        stage_graph.wire_up(adopt_chain, AdoptChain::new(manager, None, None, security_param, ledger_tip));
    let validate_block = stage_graph.stage("validate_block", validate_block::stage);
    let validate_block = stage_graph
        .wire_up(
            validate_block,
            ValidateBlock::new(adopt_chain.without_state(), validation_results, None, security_param, ledger_tip),
        )
        .without_state();
    let source = stage_graph.wire_up(
        source,
        MithrilBlockSource { validate_block, events: events_ref, tip: ledger_tip, until_slot, pending: None },
    );
    (stage_graph.input(source), events)
}
