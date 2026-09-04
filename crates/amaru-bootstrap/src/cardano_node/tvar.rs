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
use amaru_progress_bar::ProgressBarFactory;
use anyhow::anyhow;

use super::{extract_snapshot_chain_state_after_ledger, mempack, parse_state_snapshot_prefix};
use crate::bootstrap::{BootstrapCancellation, ChainState, checkpoint};

#[expect(clippy::too_many_arguments)]
pub fn import_snapshot_from_tvar<S, F, State, Utxo>(
    db: &S,
    state_file: &mut State,
    utxo_file: &mut Utxo,
    network: NetworkName,
    global_parameters: &GlobalParameters,
    nonce_tail: Option<HeaderHash>,
    previous_accounts: BTreeSet<Credential>,
    with_progress: F,
) -> anyhow::Result<(Epoch, Point, Option<ChainState>)>
where
    S: Store,
    F: ProgressBarFactory,
    State: Read,
    Utxo: Read,
{
    let cancellation = BootstrapCancellation::new();
    let (epoch, point, era_history, chain_state) = import_state_from_tvar(
        db,
        state_file,
        network,
        global_parameters,
        nonce_tail,
        previous_accounts,
        &with_progress,
        &cancellation,
    )?;

    import_utxo_from_tvar(utxo_file, db, &with_progress, &point, &era_history, network, &cancellation)?;

    Ok((epoch, point, chain_state))
}

#[expect(clippy::too_many_arguments)]
pub(crate) fn import_state_from_tvar<S, State>(
    db: &S,
    state_file: &mut State,
    network: NetworkName,
    global_parameters: &GlobalParameters,
    nonce_tail: Option<HeaderHash>,
    previous_accounts: BTreeSet<Credential>,
    with_progress: &impl ProgressBarFactory,
    cancellation: &BootstrapCancellation,
) -> anyhow::Result<(Epoch, Point, EraHistory, Option<ChainState>)>
where
    S: Store,
    State: Read,
{
    let decoder_control = cancellation.clone();
    let checkpoint: Arc<Checkpoint> = Arc::new(move || checkpoint(&decoder_control).map_err(Into::into));
    let mut decoder = LazyDecoder::with_checkpoint(state_file, checkpoint);
    let parsed_snapshot = decoder.with_decoder(|d| parse_state_snapshot_prefix(d, global_parameters))?;
    let point = Point::Specific(parsed_snapshot.slot.into(), parsed_snapshot.hash, parsed_snapshot.block_height);

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
        .map(|tail| decoder.with_decoder(|d| extract_snapshot_chain_state_after_ledger(d, point, tail)))
        .transpose()?;

    Ok((epoch, point, parsed_snapshot.era_history, chain_state))
}

pub(crate) fn import_utxo_from_tvar<S, Utxo>(
    utxo_file: &mut Utxo,
    db: &S,
    with_progress: &impl ProgressBarFactory,
    point: &Point,
    era_history: &EraHistory,
    network: NetworkName,
    cancellation: &BootstrapCancellation,
) -> anyhow::Result<()>
where
    S: Store,
    Utxo: Read,
{
    let decoder_control = cancellation.clone();
    let checkpoint: Arc<Checkpoint> = Arc::new(move || checkpoint(&decoder_control).map_err(Into::into));
    let mut decoder = LazyDecoder::with_checkpoint(utxo_file, checkpoint);
    import_tvar_utxo(&mut decoder, db, with_progress, point, era_history, network)
}

fn import_tvar_utxo<S>(
    decoder: &mut LazyDecoder<'_>,
    db: &S,
    with_progress: &impl ProgressBarFactory,
    point: &Point,
    era_history: &EraHistory,
    network: NetworkName,
) -> anyhow::Result<()>
where
    S: Store,
{
    let protocol_parameters = db.protocol_parameters()?;

    let size: Option<usize> = decoder.with_decoder(|d| {
        d.array()?;
        Ok(d.map()?.map(|len| len as usize))
    })?;

    let estimated_size = size.unwrap_or(network.estimated_utxo_size());

    let progress = with_progress.create_for(
        "import_utxo",
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
            decoder.checkpoint()?;
        }

        if done {
            break;
        }
    }

    progress.finish();
    info!(bootstrap::import::UTXO, size = actual_size);

    Ok(())
}

fn decode_tvar_entry(input: &[u8], output: &[u8]) -> anyhow::Result<(TransactionInput, MemoizedTransactionOutput)> {
    if input.len() != 34 {
        return Err(anyhow!("expected 34-byte TxIn key"));
    }

    let input = TransactionInput {
        transaction_id: Hash::from(&input[..32]),
        index: u16::from_be_bytes([input[32], input[33]]).into(),
    };
    let output = mempack::decode_transaction_output(output)?;

    Ok((input, output))
}
