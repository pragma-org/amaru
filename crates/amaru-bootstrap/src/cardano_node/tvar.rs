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

//! Import helpers for cardano-node snapshot directories (`state` + `tables/tvar`).
//!
//! Upstream references:
//! - <https://github.com/IntersectMBO/ouroboros-consensus/blob/main/ouroboros-consensus-cardano/cddl/disk/ledger/stateFile.cddl>
//! - <https://github.com/IntersectMBO/ouroboros-consensus/blob/main/ouroboros-consensus-cardano/src/unstable-snapshot-conversion/Ouroboros/Consensus/Cardano/SnapshotConversion.hs>
//! - <https://github.com/IntersectMBO/ouroboros-consensus/blob/main/ouroboros-consensus-cardano/src/unstable-snapshot-conversion/Ouroboros/Consensus/Cardano/StreamingLedgerTables.hs>

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

use amaru_kernel::{
    Epoch, EraHistory, GlobalParameters, Hash, HeaderHash, MemoizedTransactionOutput, NetworkName, Point,
    StakeCredential, TransactionInput, cbor, cbor::lazy::LazyDecoder,
};
use amaru_ledger::{bootstrap::import_initial_snapshot_with_decoder, store::Store};
use amaru_observability::info;
use amaru_progress_bar::ProgressBar;

use super::{extract_snapshot_chain_state_after_ledger, mempack, parse_state_snapshot_prefix};
use crate::bootstrap::ChainState;

#[expect(clippy::too_many_arguments)]
pub fn import_snapshot_from_tvar<S, F, State, Utxo>(
    db: &S,
    state_file: &mut State,
    utxo_file: &mut Utxo,
    network: NetworkName,
    global_parameters: &GlobalParameters,
    nonce_tail: Option<HeaderHash>,
    recently_unregistered_accounts: &mut BTreeSet<StakeCredential>,
    with_progress: F,
) -> Result<(Epoch, Point, Option<ChainState>), Box<dyn std::error::Error>>
where
    S: Store,
    F: Fn(usize, &str) -> Box<dyn ProgressBar> + Copy,
    State: Read,
    Utxo: Read,
{
    let (epoch, point, era_history, chain_state) = import_state_from_tvar(
        db,
        state_file,
        network,
        global_parameters,
        nonce_tail,
        recently_unregistered_accounts,
        with_progress,
    )?;

    import_utxo_from_tvar(utxo_file, db, with_progress, &point, &era_history, network)?;

    Ok((epoch, point, chain_state))
}

pub(crate) fn import_state_from_tvar<S, F, State>(
    db: &S,
    state_file: &mut State,
    network: NetworkName,
    global_parameters: &GlobalParameters,
    nonce_tail: Option<HeaderHash>,
    recently_unregistered_accounts: &mut BTreeSet<StakeCredential>,
    with_progress: F,
) -> Result<(Epoch, Point, EraHistory, Option<ChainState>), Box<dyn std::error::Error>>
where
    S: Store,
    F: Fn(usize, &str) -> Box<dyn ProgressBar> + Copy,
    State: Read,
{
    let mut decoder = LazyDecoder::new(state_file);
    let parsed_snapshot = decoder.with_decoder(|d| parse_state_snapshot_prefix(d, global_parameters))?;
    let point = Point::Specific(parsed_snapshot.slot.into(), parsed_snapshot.hash);

    info!(bootstrap::snapshot::IMPORT_TVAR, point = point, new_epoch_state_offset = parsed_snapshot.ledger_data_begin);

    let epoch = import_initial_snapshot_with_decoder(
        db,
        &mut decoder,
        recently_unregistered_accounts,
        &point,
        &parsed_snapshot.era_history,
        network,
        with_progress,
    )?;

    let chain_state = nonce_tail
        .map(|tail| decoder.with_decoder(|d| extract_snapshot_chain_state_after_ledger(d, point, tail)))
        .transpose()?;

    Ok((epoch, point, parsed_snapshot.era_history, chain_state))
}

pub(crate) fn import_utxo_from_tvar<S, F, Utxo>(
    utxo_file: &mut Utxo,
    db: &S,
    with_progress: F,
    point: &Point,
    era_history: &EraHistory,
    network: NetworkName,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: Store,
    F: Fn(usize, &str) -> Box<dyn ProgressBar> + Copy,
    Utxo: Read,
{
    let mut decoder = LazyDecoder::new(utxo_file);
    import_tvar_utxo(&mut decoder, db, with_progress, point, era_history, network)
}

fn import_tvar_utxo<S, F>(
    decoder: &mut LazyDecoder<'_>,
    db: &S,
    with_progress: F,
    point: &Point,
    era_history: &EraHistory,
    network: NetworkName,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: Store,
    F: Fn(usize, &str) -> Box<dyn ProgressBar> + Copy,
{
    let protocol_parameters = db.protocol_parameters()?;

    let size: Option<usize> = decoder.with_decoder(|d| {
        d.array()?;
        Ok(d.map()?.map(|len| len as usize))
    })?;

    let estimated_size = size.unwrap_or(network.estimated_utxo_size());

    let progress = with_progress(
        estimated_size,
        "{spinner:.green} Importing UTxO entries {bar:40.green} [{pos:>7}/{len:7}] ({eta} remaining)",
    );

    let mut actual_size = 0_usize;
    loop {
        let (done, utxo) = decoder.with_decoder(|d| {
            let mut done = false;
            let mut utxo = BTreeMap::new();
            let mut chunk_size = 0;

            loop {
                if d.datatype()? == cbor::data::Type::Break {
                    d.skip()?;
                    done = true;
                    break;
                }

                if size.is_some_and(|len| actual_size + chunk_size >= len) {
                    done = true;
                    break;
                }

                let mut probe = d.probe();
                let io = cbor::decode_bytes(&mut probe).and_then(|input| {
                    cbor::decode_bytes(&mut probe).map(|output| (input.into_owned(), output.into_owned()))
                });

                if let Ok((input, output)) = io {
                    chunk_size += 1;
                    d.skip()?;
                    d.skip()?;

                    let (input, output) = decode_tvar_entry(&input, &output).map_err(cbor::decode::Error::message)?;
                    utxo.insert(input, output);
                } else if utxo.is_empty() {
                    Err(cbor::decode::Error::end_of_input())?;
                } else {
                    break;
                }
            }

            Ok((done, utxo))
        })?;

        let size = utxo.len();
        progress.tick(size);
        actual_size += size;

        if !utxo.is_empty() {
            db.save_bootstrap_utxo(era_history, &protocol_parameters, point, utxo.into_iter())?;
        }

        if done {
            break;
        }
    }

    progress.clear();
    info!(bootstrap::import::UTXO, size = actual_size);

    Ok(())
}

fn decode_tvar_entry(input: &[u8], output: &[u8]) -> Result<(TransactionInput, MemoizedTransactionOutput), String> {
    if input.len() != 34 {
        return Err("expected 34-byte TxIn key".to_string());
    }

    let input = TransactionInput {
        transaction_id: Hash::from(&input[..32]),
        index: u16::from_be_bytes([input[32], input[33]]).into(),
    };
    let output = mempack::decode_transaction_output(output)?;

    Ok((input, output))
}
