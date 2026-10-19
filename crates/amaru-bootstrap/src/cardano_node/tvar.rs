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
    iter,
    sync::Arc,
};

use amaru_kernel::{
    Credential, Epoch, EraHistory, GlobalParameters, Hash, HeaderHash, MemoizedTransactionOutput, NetworkName, Point,
    TransactionInput, cbor,
    cbor::lazy::{Checkpoint, LazyDecoder},
};
use amaru_ledger::{
    bootstrap::import_initial_snapshot_with_decoder,
    store::{Columns, Store, TransactionalContext},
};
use amaru_observability::info;
use amaru_progress_bar::{ProgressBarExt, ProgressBarFactory};
use anyhow::anyhow;

use super::{extract_snapshot_chain_state_after_ledger, mempack, parse_state_snapshot_prefix};
use crate::bootstrap::{BootstrapCancellation, ChainState, checkpoint};

#[expect(clippy::too_many_arguments)]
pub(crate) fn import_state_from_tvar(
    db: &impl Store,
    state_file: &mut impl Read,
    network: NetworkName,
    global_parameters: &GlobalParameters,
    nonce_tail: Option<HeaderHash>,
    previous_accounts: BTreeSet<Credential>,
    with_progress: &impl ProgressBarFactory,
    cancellation: &BootstrapCancellation,
) -> anyhow::Result<(Epoch, Point, EraHistory, Option<ChainState>)> {
    let decoder_control = cancellation.clone();
    let decoder_checkpoint: Arc<Checkpoint> = Arc::new(move || checkpoint(&decoder_control).map_err(Into::into));
    let mut decoder = LazyDecoder::with_checkpoint(state_file, decoder_checkpoint);
    let parsed_snapshot = decoder.with_decoder(|d| parse_state_snapshot_prefix(d, global_parameters))?;
    let point = parsed_snapshot.point;

    info!(bootstrap::snapshot::IMPORT_TVAR, point, new_epoch_state_offset = parsed_snapshot.ledger_data_begin);

    let epoch = import_initial_snapshot_with_decoder(
        db,
        &mut decoder,
        previous_accounts,
        &point,
        &parsed_snapshot.era_history,
        network,
        with_progress,
    )?;

    let chain_state = nonce_tail
        .map(|tail| decoder.with_decoder(|d| extract_snapshot_chain_state_after_ledger(d, point, epoch, tail)))
        .transpose()?;

    Ok((epoch, point, parsed_snapshot.era_history, chain_state))
}

pub(crate) fn import_utxo_from_tvar(
    utxo_file: &mut impl Read,
    db: &impl Store,
    with_progress: &impl ProgressBarFactory,
    point: &Point,
    era_history: &EraHistory,
    network: NetworkName,
    cancellation: &BootstrapCancellation,
) -> anyhow::Result<()> {
    let decoder_control = cancellation.clone();
    let decoder_checkpoint: Arc<Checkpoint> = Arc::new(move || checkpoint(&decoder_control).map_err(Into::into));
    let mut decoder = LazyDecoder::with_checkpoint(utxo_file, decoder_checkpoint);
    let protocol_parameters = db.protocol_parameters()?;

    decoder.begin_array()?;
    let (progress, actual_size) = decoder.stream_map(
        |d| {
            let input = cbor::decode_bytes(d)?;
            let output = cbor::decode_bytes(d)?;
            decode_tvar_entry(&input, &output).map_err(cbor::decode::Error::message)
        },
        |size| {
            (
                with_progress.create_for(
                    "import_utxo",
                    size.map(|size| size as usize).unwrap_or(network.estimated_utxo_size()),
                    "{spinner:.green} Importing UTxO entries {bar:40.green} [{pos:>7}/{len:7}] ({eta} remaining)",
                ),
                0_usize,
            )
        },
        |(progress, actual_size), entries| {
            let utxo: BTreeMap<_, _> = entries.into_iter().collect();
            progress.tick(utxo.len());
            *actual_size += utxo.len();
            if !utxo.is_empty() {
                db.with_transaction(|transaction| {
                    transaction.save(
                        era_history,
                        &protocol_parameters,
                        None,
                        point,
                        None,
                        Columns {
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
                    )
                })?;
                checkpoint(cancellation)?;
            }
            Ok(())
        },
    )?;

    progress.finish(|| {
        info!(bootstrap::import::UTXO, size = actual_size);
    });

    Ok(())
}

fn decode_tvar_entry(input: &[u8], output: &[u8]) -> anyhow::Result<(TransactionInput, MemoizedTransactionOutput)> {
    if input.len() != 34 {
        return Err(anyhow!("expected 34-byte TxIn key"));
    }

    let input = TransactionInput {
        transaction_id: Hash::try_from(&input[..32])?,
        index: u16::from_be_bytes([input[32], input[33]]),
    };
    let output = mempack::decode_transaction_output(output)?;

    Ok((input, output))
}
