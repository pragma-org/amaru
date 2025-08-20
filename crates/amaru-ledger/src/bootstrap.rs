// Copyright 2024 PRAGMA
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

use crate::{
    state::{diff_bind::Resettable, diff_epoch_reg::DiffEpochReg},
    store::{self, columns::proposals, Store, StoreError, TransactionalContext},
};
use amaru_kernel::{
    cbor, network::NetworkName, protocol_parameters::ProtocolParameters, Account,
    CertificatePointer, DRep, DRepState, Epoch, EraHistory, Lovelace, MemoizedTransactionOutput,
    Point, PoolId, PoolParams, ProposalPointer, ProposalState, ProtocolVersion, Reward, Set, Slot,
    StakeCredential, TransactionInput, TransactionPointer,
};
use amaru_progress_bar::ProgressBar;
use std::{collections::BTreeMap, fs, iter, path::PathBuf, sync::LazyLock};
use tracing::info;

const BATCH_SIZE: usize = 5000;

static DEFAULT_CERTIFICATE_POINTER: LazyLock<CertificatePointer> =
    LazyLock::new(|| CertificatePointer {
        transaction: TransactionPointer {
            slot: Slot::from(0),
            transaction_index: 0,
        },
        certificate_index: 0,
    });

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("The pparams file was missing")]
    MissingPparamsFile,
}

/// (Partially) decode a Haskell cardano-node's 'NewEpochState'
///
/// -> https://github.com/IntersectMBO/cardano-ledger/blob/a81e6035006529ba0abc034716c2e21e7406500d/eras/shelley/impl/src/Cardano/Ledger/Shelley/LedgerState/Types.hs#L315-L345
///
/// We rely on data present in these to bootstrap Amaru's initial state.
pub fn import_initial_snapshot(
    db: &(impl Store + 'static),
    bytes: &[u8],
    point: &Point,
    network: NetworkName,
    // A way to notify progress while importing. The second argument is a template argument, which
    // follows the format described in:
    //
    // https://docs.rs/indicatif/latest/indicatif/index.html#templates
    with_progress: impl Fn(usize, &str) -> Box<dyn ProgressBar>,
    // An extra directory where protocol parameters can be.
    protocol_parameters_dir: Option<&PathBuf>,
    // Assumes the presence of fully computed rewards when set.
    has_rewards: bool,
) -> Result<Epoch, Box<dyn std::error::Error>> {
    let era_history = <&EraHistory>::from(network);

    let mut d = cbor::Decoder::new(bytes);

    d.array()?;

    // EpochNo
    let epoch = Epoch::from(d.u64()?);
    let tip = point.slot_or_default();
    assert_eq!(epoch, era_history.slot_to_epoch(tip, tip)?);

    // Previous blocks made
    d.skip()?;

    // Current blocks made
    // NOTE: We use the current blocks made here as we assume that users are providing snapshots of
    // the last block of the epoch. We have no intrinsic ways to check that this is the case since
    // we do not know what the last block of an epoch is, and we can't reliably look at the number
    // of blocks either.
    import_block_issuers(db, d.decode()?, era_history)?;

    // Epoch State
    d.array()?;

    // Epoch State / Account State
    d.array()?;
    let treasury: i64 = d.decode()?;
    let reserves: i64 = d.decode()?;

    // Epoch State / Ledger State
    d.array()?;

    // Epoch State / Ledger State / Cert State
    d.array()?;

    // Epoch State / Ledger State / Cert State / Voting State
    d.array()?;

    let dreps = d.decode()?;

    // Committee
    d.skip()?;

    // Dormant Epoch
    d.skip()?;

    // Epoch State / Ledger State / Cert State / Pool State
    d.array()?;
    import_stake_pools(
        db,
        point,
        epoch,
        // Pools
        d.decode()?,
        // Updates
        d.decode()?,
        // Retirements
        d.decode()?,
        era_history,
    )?;
    // Deposits
    d.skip()?;

    // Epoch State / Ledger State / Cert State / Delegation state
    d.array()?;

    // Epoch State / Ledger State / Cert State / Delegation state / dsUnified
    d.array()?;

    // credentials
    let accounts: BTreeMap<StakeCredential, Account> = d.decode()?;

    // pointers
    d.skip()?;

    // Epoch State / Ledger State / Cert State / Delegation state / dsFutureGenDelegs
    d.skip()?;

    // Epoch State / Ledger State / Cert State / Delegation state / dsGenDelegs
    d.skip()?;

    // Epoch State / Ledger State / Cert State / Delegation state / dsIRewards
    d.skip()?;

    // Epoch State / Ledger State / UTxO State
    d.array()?;

    import_utxo(
        db,
        &with_progress,
        point,
        d.decode::<BTreeMap<TransactionInput, MemoizedTransactionOutput>>()?
            .into_iter()
            .collect::<Vec<(TransactionInput, MemoizedTransactionOutput)>>(),
        era_history,
    )?;

    let _deposited: u64 = d.decode()?;

    let fees: i64 = d.decode()?;

    // Epoch State / Ledger State / UTxO State / utxosGovState
    d.array()?;

    // Proposals
    d.array()?;
    // Proposals roots
    d.skip()?;
    let proposals: Vec<ProposalState> = d.decode()?;

    // Constitutional committee
    d.skip()?;
    // Constitution
    d.skip()?;
    // Current Protocol Params
    let pparams = if let Some(dir) = protocol_parameters_dir {
        decode_seggregated_parameters(dir, d.decode()?)?
    } else {
        d.decode()?
    };
    let protocol_parameters = import_protocol_parameters(db, pparams)?;
    import_dreps(db, era_history, point, epoch, dreps, &protocol_parameters)?;
    import_proposals(db, point, era_history, proposals, &protocol_parameters)?;

    // Previous Protocol Params
    d.skip()?;
    // Future Protocol Params
    d.skip()?;
    // DRep Pulsing State
    d.skip()?;

    // Epoch State / Ledger State / UTxO State / utxosStakeDistr
    d.skip()?;

    // Epoch State / Ledger State / UTxO State / utxosDonation
    d.skip()?;

    // Epoch State / Snapshots
    d.skip()?;
    // Epoch State / NonMyopic
    d.skip()?;

    if has_rewards {
        // Rewards Update
        d.array()?;
        d.array()?;
        assert_eq!(d.u32()?, 1, "expected complete pulsing reward state");
        d.array()?;

        let delta_treasury: i64 = d.decode()?;

        let delta_reserves: i64 = d.decode()?;

        let mut rewards: BTreeMap<StakeCredential, Set<Reward>> = d.decode()?;
        let delta_fees: i64 = d.decode()?;

        // NonMyopic
        d.skip()?;

        import_accounts(
            db,
            &with_progress,
            point,
            accounts,
            &mut rewards,
            &protocol_parameters,
            era_history,
        )?;

        let unclaimed_rewards = rewards.into_iter().fold(0, |total, (_, rewards)| {
            total + rewards.into_iter().fold(0, |inner, r| inner + r.amount)
        });

        import_pots(
            db,
            (treasury + delta_treasury) as u64 + unclaimed_rewards,
            (reserves - delta_reserves) as u64,
            (fees - delta_fees) as u64,
        )?;
    } else {
        d.skip()?;
        d.skip()?;
        d.skip()?;
    }

    save_point(db, point, network.protocol_version(epoch), era_history)?;

    Ok(epoch)
}

fn save_point(
    db: &impl Store,
    point: &Point,
    protocol_version: &ProtocolVersion,
    era_history: &EraHistory,
) -> Result<(), Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();

    transaction.save(
        point,
        None,
        Default::default(),
        Default::default(),
        iter::empty(),
        era_history,
    )?;

    transaction.set_protocol_version(protocol_version)?;

    transaction.commit()?;

    Ok(())
}

fn import_protocol_parameters(
    db: &impl Store,
    protocol_parameters: ProtocolParameters,
) -> Result<ProtocolParameters, Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();
    transaction.set_protocol_parameters(&protocol_parameters)?;
    transaction.commit()?;
    Ok(protocol_parameters)
}

fn import_block_issuers(
    db: &impl Store,
    blocks: BTreeMap<PoolId, u64>,
    era_history: &EraHistory,
) -> Result<(), Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();
    transaction.with_block_issuers(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })?;
    transaction.commit()?;

    let transaction = db.create_transaction();
    let mut fake_slot = 0;
    for (pool, mut count) in blocks.into_iter() {
        while count > 0 {
            transaction.save(
                &Point::Specific(fake_slot, vec![]),
                Some(&pool),
                store::Columns {
                    utxo: iter::empty(),
                    pools: iter::empty(),
                    accounts: iter::empty(),
                    dreps: iter::empty(),
                    cc_members: iter::empty(),
                    proposals: iter::empty(),
                    votes: iter::empty(),
                },
                Default::default(),
                iter::empty(),
                era_history,
            )?;
            count -= 1;
            fake_slot += 1;
        }
    }
    info!(count = fake_slot, "block_issuers");
    transaction.commit().map_err(Into::into)
}

fn import_utxo(
    db: &impl Store,
    with_progress: impl Fn(usize, &str) -> Box<dyn ProgressBar>,
    point: &Point,
    mut utxo: Vec<(TransactionInput, MemoizedTransactionOutput)>,
    era_history: &EraHistory,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(size = utxo.len(), "utxo");

    let transaction = db.create_transaction();
    transaction.with_utxo(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })?;

    let progress = with_progress(utxo.len(), "  UTxO entries {bar:70} {pos:>7}/{len:7}");

    while !utxo.is_empty() {
        let n = std::cmp::min(BATCH_SIZE, utxo.len());
        let chunk = utxo.drain(0..n);

        transaction.save(
            point,
            None,
            store::Columns {
                utxo: chunk,
                pools: iter::empty(),
                accounts: iter::empty(),
                dreps: iter::empty(),
                cc_members: iter::empty(),
                proposals: iter::empty(),
                votes: iter::empty(),
            },
            Default::default(),
            iter::empty(),
            era_history,
        )?;

        progress.tick(n);
    }

    transaction.commit()?;
    progress.clear();

    Ok(())
}

fn import_dreps(
    db: &impl Store,
    era_history: &EraHistory,
    point: &Point,
    epoch: Epoch,
    dreps: BTreeMap<StakeCredential, DRepState>,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), impl std::error::Error> {
    let mut known_dreps = BTreeMap::new();

    let era_first_epoch = era_history
        .era_first_epoch(epoch)
        .map_err(|e| StoreError::Internal(Box::new(e)))?;

    let transaction = db.create_transaction();

    transaction.with_dreps(|iterator| {
        for (drep, mut handle) in iterator {
            if epoch > era_first_epoch {
                if let Some(row) = handle.borrow() {
                    known_dreps.insert(drep, row.registered_at);
                }
            }
            *handle.borrow_mut() = None;
        }
    })?;

    info!(size = dreps.len(), "dreps");

    let mut active_dreps = BTreeMap::new();

    transaction.save(
        point,
        None,
        store::Columns {
            utxo: iter::empty(),
            pools: iter::empty(),
            accounts: iter::empty(),
            dreps: dreps.into_iter().map(|(credential, state)| {
                // 1. First DRep registrations in Conway are *sometimes* granted an extra epoch of
                //    expiry; because the first Conway epoch is deemed as "dormant" (no proposals
                //    in the epoch prior), and a bug in version 9 is causing new DRep registrations
                //    to benefits from this extra epoch.
                //
                // 2. We have no idea when exactly was the drep registered; but we
                //    need to pick a valid slot so that mandate calculations falls
                //    back on the correct value.
                //
                //    There are two scenarios:
                //
                //    A) Either the drep has registered before the first proposal in
                //       the epoch. In which case it would enjoy an extra epoch of
                //       expiry.
                //
                //    B) Or it has registered strictly after, such that the number of
                //       dormant epoch was already reset. In which case, no bonus
                //       applies.
                //
                //    We can assign dreps to (A) or (B) by artificially chosing the
                //    first and last slot of the epoch respectively. To know whether
                //    we shall assign them to (A) or (B), we can simply look at their
                //    mandate in the snapshot which would be one greater for dreps in
                //    group (A).
                //
                // 3. We make a strong assumption that there are proposals submitted during the
                //    very first epoch of the Conway era on this network. This is true of Preview,
                //    Preprod and Mainnet. Any custom network for which this wouldn't be true is
                //    expected to use a protocol version > 9, where this assumption doesn't matter.
                let (registration_slot, last_interaction) = if epoch == era_first_epoch {
                    let last_interaction = era_first_epoch;
                    #[allow(clippy::unwrap_used)]
                    let epoch_bound = era_history.epoch_bounds(last_interaction).unwrap();
                    if state.expiry > epoch + protocol_parameters.drep_expiry as u64 {
                        (epoch_bound.start, last_interaction)
                    } else {
                        (point.slot_or_default(), last_interaction)
                    }
                } else {
                    let last_interaction = state.expiry - protocol_parameters.drep_expiry as u64;
                    #[allow(clippy::unwrap_used)]
                    let epoch_bound = era_history.epoch_bounds(last_interaction).unwrap();
                    // start or end doesn't matter here.
                    (epoch_bound.start, last_interaction)
                };

                let registration =
                    known_dreps
                        .remove(&credential)
                        .unwrap_or_else(|| CertificatePointer {
                            transaction: TransactionPointer {
                                slot: registration_slot,
                                ..TransactionPointer::default()
                            },
                            ..CertificatePointer::default()
                        });

                #[allow(clippy::unwrap_used)]
                #[allow(clippy::disallowed_methods)]
                let registration_epoch = era_history
                    .slot_to_epoch_unchecked_horizon(registration.slot())
                    .unwrap();

                // NOTE: The 'save' method will not consider the last interaction when registering
                // or re-registering a DRep. So when needed, we must retain the 'last_interaction'
                // and set it manually afterwards.
                if last_interaction > registration_epoch {
                    active_dreps.insert(credential.clone(), last_interaction);
                }

                (
                    credential,
                    (
                        Resettable::from(Option::from(state.anchor)),
                        Some((state.deposit, registration)),
                        last_interaction,
                    ),
                )
            }),
            cc_members: iter::empty(),
            proposals: iter::empty(),
            votes: iter::empty(),
        },
        Default::default(),
        iter::empty(),
        era_history,
    )?;

    transaction.commit()?;

    let transaction = db.create_transaction();

    transaction.with_dreps(|iterator| {
        for (credential, mut row) in iterator {
            if let Some(last_interaction) = active_dreps.get(&credential) {
                if let Some(drep) = row.borrow_mut() {
                    drep.last_interaction = Some(*last_interaction);
                }
            }
        }
    })?;

    transaction.commit()
}

fn import_proposals(
    db: &impl Store,
    point: &Point,
    era_history: &EraHistory,
    proposals: Vec<ProposalState>,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();
    transaction.with_proposals(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })?;

    info!(size = proposals.len(), "proposals");

    transaction.save(
        point,
        None,
        store::Columns {
            utxo: iter::empty(),
            pools: iter::empty(),
            accounts: iter::empty(),
            dreps: iter::empty(),
            cc_members: iter::empty(),
            proposals: proposals
                .into_iter()
                .map(|proposal| -> Result<_, Box<dyn std::error::Error>> {
                    let proposal_index = proposal.id.action_index as usize;
                    Ok((
                        proposal.id,
                        proposals::Value {
                            proposed_in: ProposalPointer {
                                transaction: TransactionPointer {
                                    slot: era_history.epoch_bounds(proposal.proposed_in)?.start,
                                    transaction_index: 0,
                                },
                                proposal_index,
                            },
                            valid_until: proposal.proposed_in
                                + protocol_parameters.gov_action_lifetime as u64,
                            proposal: proposal.procedure,
                        },
                    ))
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter(),
            votes: iter::empty(),
        },
        Default::default(),
        iter::empty(),
        era_history,
    )?;
    transaction.commit()?;

    Ok(())
}

fn import_stake_pools(
    db: &impl Store,
    point: &Point,
    epoch: Epoch,
    pools: BTreeMap<PoolId, PoolParams>,
    updates: BTreeMap<PoolId, PoolParams>,
    retirements: BTreeMap<PoolId, Epoch>,
    era_history: &EraHistory,
) -> Result<(), impl std::error::Error> {
    let mut state = DiffEpochReg::default();
    for (pool, params) in pools.into_iter() {
        state.register(pool, params);
    }

    for (pool, params) in updates.into_iter() {
        state.register(pool, params);
    }

    for (pool, epoch) in retirements.into_iter() {
        state.unregister(pool, epoch);
    }

    info!(
        registered = state.registered.len(),
        retiring = state.unregistered.len(),
        "stake_pools",
    );
    let transaction = db.create_transaction();
    transaction.with_pools(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })?;
    transaction.commit()?;

    let transaction = db.create_transaction();
    transaction.save(
        point,
        None,
        store::Columns {
            utxo: iter::empty(),
            pools: state
                .registered
                .into_iter()
                .flat_map(move |(_, registrations)| {
                    registrations
                        .into_iter()
                        .map(|r| (r, epoch))
                        .collect::<Vec<_>>()
                }),
            accounts: iter::empty(),
            dreps: iter::empty(),
            cc_members: iter::empty(),
            proposals: iter::empty(),
            votes: iter::empty(),
        },
        store::Columns {
            pools: state.unregistered.into_iter(),
            utxo: iter::empty(),
            accounts: iter::empty(),
            dreps: iter::empty(),
            cc_members: iter::empty(),
            proposals: iter::empty(),
            votes: iter::empty(),
        },
        iter::empty(),
        era_history,
    )?;
    transaction.commit()
}

fn import_pots(
    db: &impl Store,
    treasury: u64,
    reserves: u64,
    fees: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();
    transaction.with_pots(|mut row| {
        let pots = row.borrow_mut();
        pots.treasury = treasury;
        pots.reserves = reserves;
        pots.fees = fees;
    })?;
    transaction.commit()?;
    info!(treasury, reserves, fees, "pots");
    Ok(())
}

fn import_accounts(
    db: &impl Store,
    with_progress: impl Fn(usize, &str) -> Box<dyn ProgressBar>,
    point: &Point,
    accounts: BTreeMap<StakeCredential, Account>,
    rewards_updates: &mut BTreeMap<StakeCredential, Set<Reward>>,
    protocol_parameters: &ProtocolParameters,
    era_history: &EraHistory,
) -> Result<(), Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();
    transaction.with_accounts(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })?;

    let mut credentials = accounts
        .into_iter()
        .map(
            |(
                credential,
                Account {
                    rewards_and_deposit,
                    pool,
                    drep,
                    ..
                },
            )| {
                let (rewards, deposit) = Option::<(Lovelace, Lovelace)>::from(rewards_and_deposit)
                    .unwrap_or((0, protocol_parameters.stake_credential_deposit));

                let rewards_update = match rewards_updates.remove(&credential) {
                    None => 0,
                    Some(set) => set.iter().fold(0, |total, update| total + update.amount),
                };

                (
                    credential,
                    (
                        Resettable::from(Option::<PoolId>::from(pool)),
                        //No slot to retrieve. All registrations coming from snapshot are considered valid.
                        Resettable::from(
                            Option::<DRep>::from(drep)
                                .map(|drep| (drep, *DEFAULT_CERTIFICATE_POINTER)),
                        ),
                        Some(deposit),
                        rewards + rewards_update,
                    ),
                )
            },
        )
        .collect::<Vec<_>>();

    info!(size = credentials.len(), "credentials");

    let progress = with_progress(credentials.len(), "  Accounts {bar:70} {pos:>7}/{len:7}");

    while !credentials.is_empty() {
        let n = std::cmp::min(BATCH_SIZE, credentials.len());
        let chunk = credentials.drain(0..n);

        transaction.save(
            point,
            None,
            store::Columns {
                utxo: iter::empty(),
                pools: iter::empty(),
                accounts: chunk,
                dreps: iter::empty(),
                cc_members: iter::empty(),
                proposals: iter::empty(),
                votes: iter::empty(),
            },
            Default::default(),
            iter::empty(),
            era_history,
        )?;

        progress.tick(n);
    }

    transaction.commit()?;
    progress.clear();

    Ok(())
}

fn decode_seggregated_parameters(
    dir: &PathBuf,
    hash: &cbor::bytes::ByteSlice,
) -> Result<ProtocolParameters, Box<dyn std::error::Error>> {
    let pparams_file_path = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .find(|path| {
            path.file_name()
                .map(|filename| filename.to_str() == Some(&hex::encode(hash.as_ref())))
                .unwrap_or(false)
        })
        .ok_or(Error::MissingPparamsFile)?;

    let pparams_file = fs::read(pparams_file_path)?;

    let pparams = cbor::Decoder::new(&pparams_file).decode()?;

    Ok(pparams)
}
