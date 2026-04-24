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

use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    iter,
};

use amaru_kernel::{Epoch, EraHistory, HeaderHash, NetworkName, Point, cbor, cbor::lazy::LazyDecoder};
use amaru_progress_bar::ProgressBar;
use tracing::{info, warn};

use crate::{
    bootstrap::{default_governance_activity, import_initial_snapshot},
    mempack,
    store::{self, Store, TransactionalContext},
};

/// Import a cardano-node UTxOHD ledger snapshot directory into the Amaru database.
///
/// The cardano-node stores its ledger under a per-slot-number directory (e.g.
/// `db/ledger/119183041/`) with two relevant files:
///
/// - `state`       — CBOR-encoded Ouroboros wrapper around the current and previous epoch HFC
///   states. The embedded `NewEpochState` has an **empty** UTxO map because UTxO
///   is stored in the separate table file.
/// - `tables/tvar` — CBOR-encoded UTxO table: `array(1) { map { <input>: <output>, … } }`.
///
/// This function reads the `state` file, navigates the outer Ouroboros/HFC wrapper to locate the
/// `NewEpochState` at path `root[0][1][0][6][1][1][1]`:
///
/// ```text
///   root[0]              → array(2)          Ouroboros wrapper
///     [1]                → array(2)          current + previous epoch snapshots
///       [0]              → array(7)          current epoch nonce state (Praos)
///         [6]            → array(2)          Conway era boundary
///           [1]          → array(2)          (slot, ledger_wrapper)
///             [0]        → unsigned          slot number
///             [1]        → array(3)          ledger wrapper
///               [0]      → array(1, array(3)) anchor (WithOrigin(AnnTip))
///               [1]      → array(7)          ← THE NewEpochState
///               [2]      → array(4)          stake distribution snapshot
/// ```
///
/// Then it imports UTxO from `tables/tvar`.
///
/// Returns the imported epoch and the on-chain [`Point`] (slot + block hash).
#[allow(clippy::too_many_arguments)]
pub fn import_snapshot_from_node_ledger(
    db: &impl Store,
    state_file: &mut std::fs::File,
    utxo_file: &mut std::fs::File,
    era_history: &EraHistory,
    network: NetworkName,
    with_progress: impl Fn(usize, &str) -> Box<dyn ProgressBar> + Copy,
) -> Result<(Epoch, Point), Box<dyn std::error::Error>> {
    // Read the entire state file into memory for navigation. The state file is ~33 MB, which is
    // small enough to buffer. We need the full bytes because the nonce fields (items 0-4 of the
    // telescope array) are large enough that a small head buffer causes minicbor's `skip()` to
    // fail or return wrong positions when it reaches end-of-buffer mid-element.
    //
    // After navigating to the NewEpochState offset we seek the file back to that position and
    // pass it directly to `import_initial_snapshot`, whose `LazyDecoder` reads from there.
    let head = {
        let mut buf = Vec::new();
        state_file.read_to_end(&mut buf)?;
        buf
    };

    // Navigate the Ouroboros/HFC wrapper to find (slot, hash) and the byte offset of the
    // NewEpochState.
    //
    // Actual CBOR structure (from cbor-structure schema, max-depth 4):
    //
    //   root = array(2)
    //     [0] = uint                      ← format version
    //     [1] = array(1)                  ← [current epoch HFC state]
    //       [0] = array(7)               ← telescope across eras
    //         [0..4] = [* [* uint]]      ← past era nonce fields
    //         [5] = array(2)             ← Babbage boundary
    //           [0] = [* uint]
    //           [1] = [#6.2(bstr), uint, uint]
    //         [6] = array(2)             ← Conway boundary
    //           [0] = [#6.2(bstr), uint, uint]   ← era lower bound
    //           [1] = array(2)           ← (slot, ledger_wrapper)
    //             [0] = uint             ← slot
    //             [1] = array(3)         ← ledger wrapper
    //               [0] = array(1, array(3, [uint, uint, bstr]))  ← anchor
    //               [1] = array(7)       ← NewEpochState  ← target
    //               [2] = ...
    //
    // Path: root[1][0][6][1][1][1]
    let (slot, hash, new_epoch_state_offset) = {
        let mut d = amaru_kernel::cbor::Decoder::new(&head);
        d.array()?; // root = array(2)
        d.skip()?; // root[0] = format version (uint)
        d.array()?; // root[1] = array(1) wrapping current epoch state
        d.array()?; // root[1][0] = telescope array(7)
        d.skip()?; // root[1][0][0] — nonce field
        d.skip()?; // root[1][0][1] — nonce field
        d.skip()?; // root[1][0][2] — nonce field
        d.skip()?; // root[1][0][3] — nonce field
        d.skip()?; // root[1][0][4] — nonce field
        d.skip()?; // root[1][0][5] — Babbage boundary
        d.array()?; // root[1][0][6] = Conway boundary = array(2)
        d.skip()?; // root[1][0][6][0] = era lower bound
        d.array()?; // root[1][0][6][1] = (era_start_slot, ledger_wrapper) = array(2)
        d.skip()?; // root[1][0][6][1][0] = Conway era start boundary slot (not the tip slot)
        d.array()?; // root[1][0][6][1][1] = ledger wrapper = array(3)
        // root[1][0][6][1][1][0] = anchor = array(1, array(3)) = [[tip_slot, block_no, hash]]
        d.array()?; // outer array(1)
        d.array()?; // inner array(3)
        let slot = d.u64()?; // tip slot (from anchor)
        d.skip()?; // block_no
        let hash: HeaderHash = d.decode()?; // hash
        // Decoder is now at root[1][0][6][1][1][1] = NewEpochState (array(7)).
        let offset = d.position();
        (slot, hash, offset)
    };

    let point = Point::Specific(slot.into(), hash);

    info!(
        slot = slot,
        hash = %hash,
        new_epoch_state_offset,
        "importing node ledger snapshot"
    );

    // Seek the state file to the start of the NewEpochState and pass it directly.
    // `import_initial_snapshot` creates a `LazyDecoder` that reads from the current
    // file cursor, so no tempfile or in-memory copy is needed.
    state_file.seek(SeekFrom::Start(new_epoch_state_offset as u64))?;

    // The UTxO inside the state file is empty (UTxOHD backend); has_rewards=false.
    let epoch = import_initial_snapshot(db, state_file, &point, era_history, network, with_progress, false)?;

    // Import UTxO from the tvar file.
    let mut utxo_decoder = LazyDecoder::from_file(utxo_file);
    import_node_utxo(&mut utxo_decoder, db, with_progress, &point, era_history, network)?;

    Ok((epoch, point))
}

/// Import UTxO from a cardano-node UTxOHD `tables/tvar` file.
///
/// ## tvar file format
///
/// The tvar file is CBOR with the following top-level structure:
///
/// ```text
/// array(1) {
///   map(*) {                     -- definite or indefinite-length map
///     bstr(34),                  -- key:   raw TxIn
///     bstr(n),                   -- value: mempack-encoded BabbageTxOut
///     ...
///   }
/// }
/// ```
///
/// ### Key — raw `TxIn` (34 bytes)
///
/// ```text
/// [ tx_hash   : 32 bytes ]
/// [ tx_index  :  2 bytes little-endian u16 ]
/// ```
///
/// ### Value — mempack `BabbageTxOut`
///
/// Mempack is a compact non-CBOR binary format; see the [`mempack`] module for the full layout.
/// The first byte is a tag selecting the constructor:
///
/// | Tag | Constructor                       | Fields                                        |
/// |-----|-----------------------------------|-----------------------------------------------|
/// |   0 | `TxOutCompact'`                   | compact_addr + compact_value                  |
/// |   1 | `TxOutCompactDH'`                 | compact_addr + compact_value + data_hash(32)  |
/// |   2 | `TxOut_AddrHash28_AdaOnly`        | stake_cred + addr28 + compact_coin            |
/// |   3 | `TxOut_AddrHash28_AdaOnly_DH32`   | stake_cred + addr28 + compact_coin + data_hash(32) |
/// |   4 | `TxOutCompactDatum`               | compact_addr + compact_value + inline_datum   |
/// |   5 | `TxOutCompactRefScript`           | compact_addr + compact_value + datum + script |
///
/// `compact_addr` and inline datums/scripts are varuint-length-prefixed byte strings.
/// `compact_value` is tag(0)=coin-only or tag(1)=multiasset.
/// `addr28` is a 32-byte packed representation: 24 bytes of payment hash, 4-byte LE flags word
/// (bit 0 = key payment, bit 1 = mainnet), then the last 4 bytes of the payment hash.
fn import_node_utxo(
    decoder: &mut LazyDecoder<'_>,
    db: &impl Store,
    with_progress: impl Fn(usize, &str) -> Box<dyn ProgressBar>,
    point: &Point,
    era_history: &EraHistory,
    network: NetworkName,
) -> Result<(), Box<dyn std::error::Error>> {
    let protocol_parameters = <&amaru_kernel::ProtocolParameters>::try_from(network)
        .map_err(|e| format!("no initial protocol parameters for network: {e}"))?;

    if db.iter_utxos()?.next().is_some() {
        warn!("given storage is not empty: it contains UTxO; overwriting");
    }

    let size: Option<usize> = decoder.with_decoder(|d| {
        d.array()?;
        Ok(d.map()?.map(|s| s as usize))
    })?;

    let estimated_size = size.unwrap_or(match network {
        NetworkName::Mainnet => 11_000_000,
        NetworkName::Preview => 1_500_000,
        NetworkName::Preprod => 1_500_000,
        NetworkName::Testnet(..) => 1,
    });

    let progress = with_progress(estimated_size, "  UTxO entries {bar:70} {pos:>7}/{len:7}");

    let mut actual_size: usize = 0;
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

                if size.is_some_and(|s| actual_size + chunk_size >= s) {
                    done = true;
                    break;
                }

                let mut probe = d.probe();
                let io = probe.bytes().and_then(|input| probe.bytes().map(|output| (input.to_vec(), output.to_vec())));

                if let Ok((input, output)) = io {
                    chunk_size += 1;
                    d.skip()?;
                    d.skip()?;
                    utxo.insert(
                        {
                            if input.len() != 34 {
                                Err(cbor::decode::Error::message("expected 34-byte TxIn key"))?
                            }
                            amaru_kernel::TransactionInput {
                                transaction_id: amaru_kernel::Hash::from(&input[..32]),
                                index: u16::from_le_bytes([input[32], input[33]]).into(),
                            }
                        },
                        mempack::decode_transaction_output(&output).map_err(cbor::decode::Error::message)?,
                    );
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
                protocol_parameters,
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
