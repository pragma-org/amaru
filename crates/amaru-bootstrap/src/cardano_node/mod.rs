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

use std::time::Duration;

use amaru_kernel::{
    Epoch, EraBound, EraHistory, EraName, EraParams, EraSummary, GlobalParameters, HeaderHash, Nonce, Point,
};
use amaru_observability::warn;
use amaru_ouroboros::{Nonces, OpcertSequenceNumbers};
use anyhow::anyhow;
use minicbor::Decoder;

use crate::bootstrap::ChainState;

pub(crate) mod mempack;
pub mod tvar;
pub(super) struct StateSnapshotPrefix {
    pub point: Point,
    pub era_history: EraHistory,
    pub ledger_data_begin: usize,
}

pub(super) fn parse_state_snapshot_prefix(
    d: &mut Decoder<'_>,
    global_parameters: &GlobalParameters,
) -> anyhow::Result<StateSnapshotPrefix> {
    d.array()?;

    // version
    // <https://github.com/IntersectMBO/ouroboros-consensus/blob/617145bd1d36b4dd07ea2dfad4b840e6001ce427/ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Util/Versioned.hs#L94-L103>
    d.skip()?;

    // Hardfork states
    // <https://github.com/IntersectMBO/ouroboros-consensus/blob/617145bd1d36b4dd07ea2dfad4b840e6001ce427/ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/HardFork/Combinator/State/Types.hs#L84-L101>
    d.array()?;

    // Past eras
    let total_eras = d.array()?.ok_or_else(|| {
        anyhow!("indefinite encoding used for hard fork states; cannot figure out how many eras to decode.")
    })? as u8;
    let mut past_eras: Vec<EraSummary> = Vec::with_capacity(total_eras as usize);
    for era_tag in 1..=(total_eras - 1_u8) {
        past_eras.push(decode_partial_era_summary(d, era_tag)?)
    }

    // Current era
    let current_era = EraName::try_from(total_eras)?;
    if current_era != EraName::Conway {
        warn!(bootstrap::snapshot::UNEXPECTED_ERA, snapshot_era = current_era);
    }
    decode_current_era(d, past_eras, current_era, global_parameters)
}

pub(super) fn extract_snapshot_chain_state_after_ledger(
    d: &mut Decoder<'_>,
    point: Point,
    epoch: Epoch,
    tail: HeaderHash,
) -> anyhow::Result<ChainState> {
    d.skip()?;
    d.skip()?;

    // header state
    d.array()?;
    d.skip()?;

    // ChainDepState for Praos
    let num_eras = d
        .array()?
        .ok_or_else(|| anyhow!("chain dep state encoded as indefinite array; cannot determine numbers of eras"))?;

    // Previous, terminated, eras.
    for _ in 1..num_eras {
        d.skip()?;
    }

    // The actual PraosState
    d.array()?;
    d.skip()?;

    // versioned TickedChainDepState
    d.array()?;
    d.skip()?;
    d.array()?;

    // last slot
    d.array()?;
    d.skip()?;
    d.u64()?;
    let opcert_sequence_numbers: OpcertSequenceNumbers = d.decode()?;

    d.array()?;
    d.skip()?;
    let evolving: Nonce = d.decode()?;

    d.array()?;
    d.skip()?;
    let candidate: Nonce = d.decode()?;

    d.array()?;
    d.skip()?;
    let active: Nonce = d.decode()?;

    d.skip()?;
    d.skip()?;

    Ok(ChainState { point, nonces: Nonces { epoch, active, evolving, candidate, tail }, opcert_sequence_numbers })
}

fn decode_current_era(
    d: &mut Decoder<'_>,
    mut eras: Vec<EraSummary>,
    current_era: EraName,
    global_parameters: &GlobalParameters,
) -> anyhow::Result<StateSnapshotPrefix> {
    d.array()?;

    eras.push(EraSummary {
        start: d.decode()?,
        end: None,
        params: EraParams {
            epoch_size_slots: global_parameters.epoch_length(),
            slot_length: Duration::from_secs(1),
            era_name: current_era,
        },
    });

    // Versioned ledger state
    // https://github.com/IntersectMBO/ouroboros-consensus/blob/617145bd1d36b4dd07ea2dfad4b840e6001ce427/ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Ledger.hs#L881
    d.array()?;

    // encoding version (2)
    d.skip()?;

    // (Shelley) ledger state
    // https://github.com/IntersectMBO/ouroboros-consensus/blob/617145bd1d36b4dd07ea2dfad4b840e6001ce427/ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Ledger.hs#L890-L914
    d.array()?;

    // tip
    // https://github.com/IntersectMBO/ouroboros-consensus/blob/617145bd1d36b4dd07ea2dfad4b840e6001ce427/ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Ledger.hs#L846-L857
    // the Point is wrapped in a WithOrigin type hence the double array
    d.array()?;
    d.array()?;
    let slot = d.u64()?;
    let block_height = d.u64()?.into();
    let hash: HeaderHash = d.decode()?;

    let ledger_data_begin = d.position();

    let era_history = EraHistory::new(&eras, global_parameters.stability_window());

    Ok(StateSnapshotPrefix { point: Point::Specific(slot.into(), hash, block_height), era_history, ledger_data_begin })
}

fn decode_partial_era_summary(d: &mut minicbor::Decoder<'_>, era_tag: u8) -> anyhow::Result<EraSummary> {
    d.array()?;

    let start: EraBound = d.decode()?;

    let end: EraBound = d.decode()?;

    let era_name = EraName::try_from(era_tag)?;

    let params =
        EraParams::from_bounds(&start, &end, era_name).ok_or_else(|| anyhow!("Invalid era bounds (non-increasing)"))?;

    Ok(EraSummary { start, end: Some(end), params })
}
