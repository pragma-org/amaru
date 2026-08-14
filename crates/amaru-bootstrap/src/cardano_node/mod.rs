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
    BlockHeight, EraBound, EraHistory, EraName, EraParams, EraSummary, GlobalParameters, HeaderHash, Nonce, Point,
};
use amaru_ouroboros::OpcertSequenceNumbers;
use anyhow::anyhow;
use minicbor::Decoder;
use tracing::warn;

use crate::bootstrap::{ChainState, InitialNonces};

pub(crate) mod mempack;
pub mod tvar;
pub struct ParsedStateSnapshot {
    pub slot: u64,
    pub hash: HeaderHash,
    pub block_height: BlockHeight,
    pub era_history: EraHistory,
    pub ledger_data_begin: usize,
    pub ledger_data_end: usize,
}

pub fn parse_state_snapshot(
    d: &mut Decoder<'_>,
    global_parameters: &GlobalParameters,
) -> Result<ParsedStateSnapshot, Box<dyn std::error::Error>> {
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
        warn!(snapshot_era = %current_era, "parsed snapshot has a current era different from 'Conway'; things may break down the line.");
    }
    decode_current_era(d, past_eras, current_era, global_parameters)
}

fn extract_snapshot_chain_state_after_prefix(
    d: &mut Decoder<'_>,
    parsed_snapshot: &ParsedStateSnapshot,
    tail: HeaderHash,
) -> Result<ChainState, Box<dyn std::error::Error>> {
    let at = Point::Specific(parsed_snapshot.slot.into(), parsed_snapshot.hash, parsed_snapshot.block_height);

    d.skip().map_err(|err| format!("skip shelley transition: {err}"))?;
    d.skip().map_err(|err| format!("skip latest peras cert round: {err}"))?;

    // header state
    d.array().map_err(|err| format!("decode header state: {err}"))?;
    d.skip().map_err(|err| format!("skip header state tip: {err}"))?;

    // ChainDepState for Praos
    let num_eras = d
        .array()
        .map_err(|err| format!("decode chain dep state: {err}"))?
        .ok_or("chain dep state encoded as indefinite array; cannot determine numbers of eras")?;

    // Previous, terminated, eras.
    for i in 1..num_eras {
        d.skip().map_err(|err| format!("skip hfc state {i}: {err}"))?;
    }

    // The actual PraosState
    d.array().map_err(|err| format!("decode praos state: {err}"))?;
    d.skip().map_err(|err| format!("skip praos era bounds: {err}"))?;

    // versioned TickedChainDepState
    d.array().map_err(|err| format!("decode ticked chain dep state: {err}"))?;
    d.skip().map_err(|err| format!("skip ticked chain dep state version: {err}"))?;
    d.array().map_err(|err| format!("decode praos payload: {err}"))?;

    // last slot
    d.array().map_err(|err| format!("decode last slot wrapper: {err}"))?;
    d.skip().map_err(|err| format!("skip last slot tag: {err}"))?;
    d.u64().map_err(|err| format!("decode last slot: {err}"))?;
    let opcert_sequence_numbers: OpcertSequenceNumbers =
        d.decode().map_err(|err| format!("decode ocert counters: {err}"))?;

    d.array().map_err(|err| format!("decode evolving nonce wrapper: {err}"))?;
    d.skip().map_err(|err| format!("skip evolving nonce tag: {err}"))?;
    let evolving: Nonce = d.decode().map_err(|err| format!("decode evolving nonce: {err}"))?;

    d.array().map_err(|err| format!("decode candidate nonce wrapper: {err}"))?;
    d.skip().map_err(|err| format!("skip candidate nonce tag: {err}"))?;
    let candidate: Nonce = d.decode().map_err(|err| format!("decode candidate nonce: {err}"))?;

    d.array().map_err(|err| format!("decode active nonce wrapper: {err}"))?;
    d.skip().map_err(|err| format!("skip active nonce tag: {err}"))?;
    let active: Nonce = d.decode().map_err(|err| format!("decode active nonce: {err}"))?;

    d.skip().map_err(|err| format!("skip lab nonce: {err}"))?;
    d.skip().map_err(|err| format!("skip last epoch nonce: {err}"))?;

    let initial_nonces = InitialNonces { at, active, evolving, candidate, tail };
    Ok(ChainState { initial_nonces, opcert_sequence_numbers })
}

pub fn parse_state_snapshot_with_chain_state(
    mut d: Decoder<'_>,
    global_parameters: &GlobalParameters,
    tail: HeaderHash,
) -> Result<(ParsedStateSnapshot, ChainState), Box<dyn std::error::Error>> {
    let parsed_snapshot =
        parse_state_snapshot(&mut d, global_parameters).map_err(|err| format!("parse state snapshot prefix: {err}"))?;
    let chain_state = extract_snapshot_chain_state_after_prefix(&mut d, &parsed_snapshot, tail)?;

    Ok((parsed_snapshot, chain_state))
}

fn decode_current_era(
    d: &mut Decoder<'_>,
    mut eras: Vec<EraSummary>,
    current_era: EraName,
    global_parameters: &GlobalParameters,
) -> Result<ParsedStateSnapshot, Box<dyn std::error::Error>> {
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
    let block_height = BlockHeight::from(d.u64()?);
    let hash: HeaderHash = d.decode()?;

    let ledger_data_begin = d.position();
    d.skip()?;
    let ledger_data_end = d.position();

    let era_history = EraHistory::new(&eras, global_parameters.stability_window());

    Ok(ParsedStateSnapshot { slot, hash, block_height, era_history, ledger_data_begin, ledger_data_end })
}

fn decode_partial_era_summary(
    d: &mut minicbor::Decoder<'_>,
    era_tag: u8,
) -> Result<EraSummary, Box<dyn std::error::Error>> {
    d.array()?;

    let start: EraBound = d.decode()?;

    let end: EraBound = d.decode()?;

    let era_name = EraName::try_from(era_tag)?;

    let params =
        EraParams::from_bounds(&start, &end, era_name).ok_or_else(|| anyhow!("Invalid era bounds (non-increasing)"))?;

    Ok(EraSummary { start, end: Some(end), params })
}
