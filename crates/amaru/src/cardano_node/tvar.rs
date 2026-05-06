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
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    iter,
};

use amaru_kernel::{
    Epoch, EraHistory, Hash, HeaderHash, MemoizedTransactionOutput, NetworkName, Point, TransactionInput, cbor,
    cbor::lazy::LazyDecoder,
};
use amaru_ledger::{
    bootstrap::{default_governance_activity, import_initial_snapshot},
    store::{self, Store, TransactionalContext},
};
use amaru_progress_bar::ProgressBar;
use tracing::{info, warn};

use super::{
    decode_node_accounts, decode_node_pool_state, mempack, parse_state_snapshot, parse_state_snapshot_with_nonces,
};
use crate::bootstrap::InitialNonces;

pub fn import_snapshot_from_tvar<S, F>(
    db: &S,
    state_file: &mut std::fs::File,
    utxo_file: &mut std::fs::File,
    network: NetworkName,
    nonce_tail: Option<HeaderHash>,
    with_progress: F,
) -> Result<(Epoch, Point, Option<InitialNonces>), Box<dyn std::error::Error>>
where
    S: Store,
    F: Fn(usize, &str) -> Box<dyn ProgressBar> + Copy,
{
    let state_head = read_state_snapshot(state_file)?;
    let (parsed_snapshot, initial_nonces) = if let Some(tail) = nonce_tail {
        let (parsed_snapshot, initial_nonces) =
            parse_state_snapshot_with_nonces(minicbor::Decoder::new(&state_head), &network, tail)?;
        (parsed_snapshot, Some(initial_nonces))
    } else {
        let mut decoder = minicbor::Decoder::new(&state_head);
        (parse_state_snapshot(&mut decoder, &network)?, None)
    };
    let point = Point::Specific(parsed_snapshot.slot.into(), parsed_snapshot.hash);
    let era_history = parsed_snapshot.era_history;
    let new_epoch_state_offset = parsed_snapshot.ledger_data_begin;

    info!(point = %point, new_epoch_state_offset, "importing state snapshot with external utxo source");

    state_file.seek(SeekFrom::Start(new_epoch_state_offset as u64))?;

    let epoch = import_initial_snapshot(
        db,
        state_file,
        &point,
        &era_history,
        network,
        with_progress,
        decode_node_pool_state,
        0,
        decode_node_accounts,
        2,
        false,
    )?;

    import_utxo_from_tvar(utxo_file, db, with_progress, &point, &era_history, network)?;

    Ok((epoch, point, initial_nonces))
}

fn read_state_snapshot(file: &mut std::fs::File) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn import_utxo_from_tvar<S, F>(
    utxo_file: &mut std::fs::File,
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
    let mut decoder = LazyDecoder::from_file(utxo_file);
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

    if db.iter_utxos()?.next().is_some() {
        warn!("given storage is not empty: it contains UTxO; overwriting");
    }

    let size: Option<usize> = decoder.with_decoder(|d| {
        d.array()?;
        Ok(d.map()?.map(|len| len as usize))
    })?;

    let estimated_size = size.unwrap_or(match network {
        NetworkName::Mainnet => 11_000_000,
        NetworkName::Preview => 1_500_000,
        NetworkName::Preprod => 1_500_000,
        NetworkName::Testnet(..) => 1,
    });

    let progress = with_progress(estimated_size, "  UTxO entries {bar:70} {pos:>7}/{len:7}");

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
                let io = probe.bytes().and_then(|input| probe.bytes().map(|output| (input.to_vec(), output.to_vec())));

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
            let transaction = db.create_transaction();
            transaction.save(
                era_history,
                &protocol_parameters,
                &mut default_governance_activity(),
                point,
                None,
                store::Columns {
                    utxo: utxo.into_iter(),
                    pools: iter::empty(),
                    accounts: iter::empty(),
                    dreps: iter::empty(),
                    cc_members: iter::empty(),
                    proposals: iter::empty(),
                    votes: iter::empty(),
                },
                Default::default(),
                iter::empty(),
            )?;
            transaction.commit()?;
        }

        if done {
            break;
        }
    }

    info!(size = actual_size, "utxo");
    progress.clear();

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
