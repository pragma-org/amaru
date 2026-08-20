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

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    iter,
    rc::Rc,
    sync::LazyLock,
};

use amaru_kernel::{
    Ballot, BallotId, BlockHeight, CertificatePointer, Constitution, ConstitutionalCommittee,
    ConstitutionalCommitteeMemberStatus, DRep, DRepRegistration, DRepState, Epoch, EraHistory, Hash, Lovelace, Network,
    NetworkName, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, Point, PoolId, PoolMetadata, PoolParams, Pots, Proposal,
    ProposalId, ProposalPointer, ProposalState, ProposalsRoots, ProposalsRootsRc, ProtocolParameters, ProtocolVersion,
    RatificationStatus, RationalNumber, Relay, Reward, RewardAccount, Slot, StakeCredential, TransactionPointer, Vote,
    Voter,
    cbor::{self, HasProtocolVersion, lazy::LazyDecoder},
    protocol_version, size,
    utils::cbor::{SerialisedAsArray, SerialisedAsSet},
};
use amaru_observability::{info, warn};
use amaru_progress_bar::ProgressBar;
use anyhow::anyhow;

use crate::{
    epoch_transition::GovernanceActivity,
    governance::ratification::{CandidateProposal, ProposalsForest},
    state::volatile::{DiffEpochReg, Resettable},
    store::{
        self, Store, StoreError, TransactionalContext,
        columns::{accounts, proposals},
    },
};

const BATCH_SIZE: usize = 1000;

static DEFAULT_CERTIFICATE_POINTER: LazyLock<CertificatePointer> = LazyLock::new(|| CertificatePointer {
    transaction: TransactionPointer { slot: Slot::from(0), transaction_index: 0 },
    certificate_index: 0,
});

#[derive(Debug, thiserror::Error)]
enum InitialSnapshotFormatError {
    #[error("invalid initial snapshot payload: expected an epoch prefix")]
    MissingEpochPrefix,

    #[error("invalid initial snapshot payload: expected epoch {expected} from the snapshot point, got {actual}")]
    UnexpectedEpoch { expected: Epoch, actual: Epoch },

    #[error("invalid initial snapshot payload: expected a previous-blocks map immediately after the epoch")]
    MissingPreviousBlocksMap,

    #[error("snapshot protocol version {}.{} is too old; minimum supported version is {}.{}", snapshot_version.major(), snapshot_version.minor(), minimum_version.major(), minimum_version.minor()
    )]
    ProtocolVersionTooOld { snapshot_version: ProtocolVersion, minimum_version: ProtocolVersion },
}

fn format_pool_state_decode_error(error: cbor::decode::Error) -> String {
    let error = error.to_string();

    if error.contains("node pool vrf key hashes") && error.contains("Invalid hash size") {
        return "snapshot uses an older unsupported pool/account encoding; regenerate snapshots with amaru snapshot create (db-analyser).".to_owned();
    }

    format!("decode pool state: {error}")
}

fn decode_initial_snapshot_prefix(d: &mut cbor::Decoder<'_>, expected_epoch: Epoch) -> anyhow::Result<Epoch> {
    use cbor::data::Type::{Map, MapIndef};

    d.array()?;

    let epoch = d.u64().map(Epoch::from).map_err(|_| InitialSnapshotFormatError::MissingEpochPrefix)?;

    if epoch != expected_epoch {
        return Err(InitialSnapshotFormatError::UnexpectedEpoch { expected: expected_epoch, actual: epoch }.into());
    }

    if !matches!(d.datatype()?, Map | MapIndef) {
        return Err(InitialSnapshotFormatError::MissingPreviousBlocksMap.into());
    }

    Ok(epoch)
}

/// The parts of a cardano-node `NewEpochState` that Amaru's initial state is built from. Fields
/// are listed in the order they appear in the payload.
struct InitialSnapshot {
    epoch: Epoch,
    block_issuers: BTreeMap<PoolId, u64>,
    treasury: i64,
    reserves: i64,
    dreps: BTreeMap<StakeCredential, DRepState>,
    cc_members: BTreeMap<StakeCredential, ConstitutionalCommitteeMemberStatus>,
    governance_activity: GovernanceActivity,
    pools: BTreeMap<PoolId, PoolParams>,
    pools_updates: BTreeMap<PoolId, PoolParams>,
    pools_retirements: BTreeMap<PoolId, Epoch>,
    fees: i64,
    proposals_roots: ProposalsRoots,
    proposals: Vec<ProposalState>,
    cc_state: Option<ConstitutionalCommittee>,
    constitution: Constitution,
    protocol_parameters: ProtocolParameters,
    enacted_proposals: Vec<ProposalState>,
    expired_proposals: BTreeSet<ProposalId>,
    donations: u64,
    delta_treasury: i64,
    delta_reserves: i64,
    unclaimed_rewards: u64,
    delta_fees: i64,
}

/// Accounts present in the map with no deposit which we pre-filled with the protocol parameters'
/// deposit. They are stashed away when importing accounts because we only know about the protocol
/// parameters later on.
struct ImportedAccounts {
    account_len: usize,
    recently_unregistered_accounts: BTreeSet<StakeCredential>,
    awaiting_default_deposit: Vec<(StakeCredential, NodeAccount)>,
}

/// Persist normalized account rows in bounded write batches.
fn save_bootstrap_account_batches(
    db: &impl Store,
    mut rows: impl Iterator<Item = (accounts::Key, accounts::Row)>,
    mut on_batch: impl FnMut(usize),
) -> Result<(), StoreError> {
    loop {
        let batch = rows.by_ref().take(BATCH_SIZE).collect::<Vec<_>>();
        if batch.is_empty() {
            return Ok(());
        }

        let size = batch.len();
        db.save_bootstrap_accounts(batch.into_iter())?;
        on_batch(size);
    }
}

fn skip_stake_snapshot_lazy(decoder: &mut LazyDecoder<'_>) -> anyhow::Result<()> {
    decoder.begin_array()?;
    decoder.skip()?;
    decoder.skip()
}

fn import_rewards(
    decoder: &mut LazyDecoder<'_>,
    db: &impl Store,
    rewards_size: usize,
    with_progress: &impl Fn(usize, &str) -> Box<dyn ProgressBar>,
) -> anyhow::Result<u64> {
    let (progress, unclaimed_rewards) = decoder.stream_map(
        |d| {
            let credential = d.decode()?;
            let SerialisedAsSet(rewards): SerialisedAsSet<Vec<Reward>> = d.decode()?;
            let amount = rewards.into_iter().fold(0, |total, reward| total + reward.amount);
            Ok((credential, amount))
        },
        |length| {
            (
                with_progress(
                    length.map(|size| size as usize).unwrap_or(rewards_size),
                    "{spinner:.green} Importing rewards {bar:40.green} [{pos:>7}/{len:7}] ({eta} remaining)",
                ),
                0_u64,
            )
        },
        |(progress, unclaimed_rewards), entries| {
            for batch in entries.chunks(BATCH_SIZE) {
                let transaction = db.create_transaction();
                for (credential, amount) in batch {
                    *unclaimed_rewards += transaction.refund(credential, *amount)?;
                }
                transaction.commit()?;
                progress.tick(batch.len());
            }
            Ok(())
        },
    )?;

    progress.clear();
    Ok(unclaimed_rewards)
}

fn import_accounts(
    decoder: &mut LazyDecoder<'_>,
    db: &impl Store,
    point: &Point,
    network: NetworkName,
    mut recently_unregistered_accounts: BTreeSet<StakeCredential>,
    with_progress: &impl Fn(usize, &str) -> Box<dyn ProgressBar>,
) -> anyhow::Result<ImportedAccounts> {
    if db.iter_accounts()?.next().is_some() {
        warn!(bootstrap::accounts::IS_NOT_EMPTY);
    }

    decoder.begin_array()?;

    let (progress, size, awaiting_default_deposit) = decoder.stream_map(
        |d| Ok((d.decode::<StakeCredential>()?, d.decode::<NodeAccount>()?)),
        |length| {
            let estimated_size = length.map(|size| size as usize).unwrap_or(match network {
                NetworkName::Mainnet => 1_500_000,
                NetworkName::Preview => 100_000,
                NetworkName::Preprod => 50_000,
                NetworkName::Testnet(_) => 1,
            });
            (
                with_progress(
                    estimated_size,
                    "{spinner:.green} Importing accounts {bar:40.green} [{pos:>7}/{len:7}] ({eta} remaining)",
                ),
                0,
                Vec::new(),
            )
        },
        |(progress, size, awaiting_default_deposit), entries| {
            progress.tick(entries.len());

            *size += entries.len();

            let (awaiting, ready): (Vec<_>, Vec<_>) = entries.into_iter().partition(|(credential, account)| {
                recently_unregistered_accounts.remove(credential);
                account.deposit == 0
            });

            awaiting_default_deposit.extend(awaiting);

            let ready = ready.into_iter().map(|(credential, account)| (credential, account.into_row(point, None)));
            save_bootstrap_account_batches(db, ready, |_| {})?;
            Ok(())
        },
    )?;

    progress.clear();

    info!(bootstrap::accounts::IMPORT, size);

    Ok(ImportedAccounts { awaiting_default_deposit, recently_unregistered_accounts, account_len: size })
}

fn import_recently_unregistered_accounts(
    db: &impl Store,
    point: &Point,
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
    recently_unregistered_accounts: BTreeSet<StakeCredential>,
) -> anyhow::Result<()> {
    if !recently_unregistered_accounts.is_empty() {
        db.with_transaction(|transaction| {
            transaction.save(
                era_history,
                protocol_parameters,
                None,
                point,
                None,
                Default::default(),
                store::Columns {
                    utxo: iter::empty(),
                    pools: iter::empty(),
                    accounts: recently_unregistered_accounts.into_iter(),
                    dreps: iter::empty(),
                    cc_members: iter::empty(),
                    proposals: iter::empty(),
                    votes: iter::empty(),
                },
                iter::empty(),
            )
        })?;
    }

    Ok(())
}

fn import_default_account_deposits(
    db: &impl Store,
    point: &Point,
    default_deposit: Lovelace,
    with_progress: &impl Fn(usize, &str) -> Box<dyn ProgressBar>,
    mut accounts: Vec<(StakeCredential, NodeAccount)>,
) -> anyhow::Result<()> {
    if accounts.is_empty() {
        return Ok(());
    }

    let progress = with_progress(
        accounts.len(),
        "{spinner:.green} Adjusting default account deposits {bar:40.green} [{pos:>7}/{len:7}] ({eta} remaining)",
    );

    let awaiting =
        accounts.drain(..).map(|(credential, account)| (credential, account.into_row(point, Some(default_deposit))));

    save_bootstrap_account_batches(db, awaiting, |size| progress.tick(size))?;

    progress.clear();
    Ok(())
}

/// (Partially) decode a cardano-node `NewEpochState` payload.
///
/// -> <https://github.com/IntersectMBO/cardano-ledger/blob/a81e6035006529ba0abc034716c2e21e7406500d/eras/shelley/impl/src/Cardano/Ledger/Shelley/LedgerState/Types.hs#L315-L345>
///
/// Account rows are persisted in bounded batches while decoding; all other snapshot state stays
/// in memory until the payload has been validated.
#[expect(clippy::too_many_arguments)]
fn decode_initial_snapshot(
    decoder: &mut LazyDecoder<'_>,
    db: &impl Store,
    previous_accounts: BTreeSet<StakeCredential>,
    point: &Point,
    expected_epoch: Epoch,
    network: NetworkName,
    era_history: &EraHistory,
    with_progress: &impl Fn(usize, &str) -> Box<dyn ProgressBar>,
) -> anyhow::Result<InitialSnapshot> {
    let epoch: Epoch = decoder.with_decoder(|d| decode_initial_snapshot_prefix(d, expected_epoch))?;

    // NOTE(INITIAL_BOOTSTRAP):
    // We use the current blocks made here as we assume that users are providing snapshots of the
    // last block of the epoch. We have no intrinsic ways to check that this is the case since we
    // do not know what the last block of an epoch is, and we can't reliably look at the number of
    // blocks either.
    decoder.skip()?; // Previous blocks made
    let block_issuers: BTreeMap<PoolId, u64> = decoder.decode()?;

    decoder.begin_array()?; // Epoch State
    decoder.begin_array()?; // Epoch State / Account State
    let treasury: i64 = decoder.decode()?;
    let reserves: i64 = decoder.decode()?;

    decoder.begin_array()?; // Epoch State / Ledger State
    decoder.begin_array()?; // Epoch State / Ledger State / Cert State
    decoder.begin_array()?; // Epoch State / Ledger State / Cert State / Voting State
    let dreps: BTreeMap<StakeCredential, DRepState> = decoder.decode()?;

    // Committee cold -> hot delegations
    let cc_members: BTreeMap<StakeCredential, ConstitutionalCommitteeMemberStatus> = decoder.decode()?;

    let dormant_epoch: Epoch = decoder.decode()?;
    let governance_activity = GovernanceActivity { consecutive_dormant_epochs: u64::from(dormant_epoch) as u32 };
    info!(bootstrap::governance_activity::IMPORT, dormant_epochs = governance_activity.consecutive_dormant_epochs);

    let pool_state_progress = with_progress(0, "{spinner:.green} Reading pool state");
    // Pool parameters contain protocol-version-sensitive byte strings, but the snapshot's
    // protocol version appears later in the payload. Keep the encoded pool state until then.
    let raw_pool_state: Vec<u8> = decoder.with_decoder(|d| {
        let start = d.position();
        d.skip()?;
        Ok(d.input()[start..d.position()].to_vec())
    })?;
    pool_state_progress.clear();

    let ImportedAccounts { recently_unregistered_accounts, awaiting_default_deposit, account_len } =
        import_accounts(decoder, db, point, network, previous_accounts, with_progress)
            .map_err(|err| anyhow!("decode accounts: {err}"))?;

    let remaining_state_progress = with_progress(0, "{spinner:.green} Reading remaining ledger state");

    decoder.skip()?; // dsPtrs
    decoder.skip()?; // dsFutureGenDelegs
    decoder.skip()?; // dsGenDelegs

    skip_embedded_utxo(decoder).map_err(|err| anyhow!("skip embedded utxo: {err}"))?;

    decoder.skip().map_err(|err| anyhow!("decode deposited: {err}"))?;
    let fees: i64 = decoder.decode().map_err(|err| anyhow!("decode fees: {err}"))?;

    decoder.begin_array()?; // Epoch State / Ledger State / UTxO State / utxosGovState
    decoder.begin_array()?; // Proposals
    decoder.begin_array()?;
    let SerialisedAsArray(root_params) = decoder.decode()?;
    let SerialisedAsArray(root_hard_fork) = decoder.decode()?;
    let SerialisedAsArray(root_cc) = decoder.decode()?;
    let SerialisedAsArray(root_constitution) = decoder.decode()?;

    let proposals_roots = ProposalsRoots {
        protocol_parameters: root_params,
        hard_fork: root_hard_fork,
        constitutional_committee: root_cc,
        constitution: root_constitution,
    };

    let proposals: Vec<ProposalState> = decoder.decode()?;

    let SerialisedAsArray(cc_state) = decoder.decode()?;

    let constitution: Constitution = decoder.decode()?;

    // Current Protocol Params — decode before any write so a stale snapshot fails cleanly.
    let protocol_parameters: ProtocolParameters = decoder.decode()?;

    protocol_version::validate(protocol_parameters.protocol_version, protocol_version::MINIMUM_SUPPORTED).map_err(
        |e| InitialSnapshotFormatError::ProtocolVersionTooOld {
            snapshot_version: e.snapshot_version,
            minimum_version: e.minimum_version,
        },
    )?;

    import_default_account_deposits(
        db,
        point,
        protocol_parameters.stake_credential_deposit,
        with_progress,
        awaiting_default_deposit,
    )?;

    let (pools, pools_updates, pools_retirements) =
        decode_node_pool_state(&mut cbor::Decoder::new(&raw_pool_state), network, protocol_parameters.protocol_version)
            .map_err(|err| anyhow!("{}", format_pool_state_decode_error(err)))?;

    decoder.skip()?; // Previous Protocol Params
    decoder.skip()?; // Future Protocol Params
    decoder.begin_array()?; // DRep Pulsing State
    decoder.begin_array()?; // Pulsing Snapshot
    decoder.skip()?; // Last epoch votes
    decoder.skip()?; // DRep distr
    decoder.skip()?; // DRep state
    decoder.skip()?; // Pool distr

    let (enacted_proposals, SerialisedAsSet(expired_proposals)): (
        Vec<ProposalState>,
        SerialisedAsSet<BTreeSet<ProposalId>>,
    ) = decoder
        .with_decoder(|d| {
            // Ratify state
            d.array()?;
            d.skip()?; // Enact state
            let enacted = d.decode()?;
            let expired = d.decode()?;
            d.skip()?; // Delayed
            Ok((enacted, expired))
        })
        .map_err(|err| anyhow!("decode ratify state: {err}"))?;

    // Epoch State / Ledger State / UTxO State / utxosStakeDistr
    decoder.skip()?;

    // Epoch State / Ledger State / UTxO State / utxosDonation
    let donations: u64 = decoder.decode()?;

    // Epoch State / Snapshots
    decoder.begin_array()?;
    skip_stake_snapshot_lazy(decoder)?; // Epoch State / Snapshots / Mark
    skip_stake_snapshot_lazy(decoder)?; // Epoch State / Snapshots / Set
    skip_stake_snapshot_lazy(decoder)?; // Epoch State / Snapshots / Go
    decoder.skip()?; // Epoch State / Snapshots / Fee
    decoder.skip()?; // Epoch State / NonMyopic

    let is_complete = decoder
        .with_decoder(|d| {
            let mut probe = d.probe();
            let is_complete = (|| -> Option<()> {
                probe.array().ok()?;
                probe.array().ok()?;
                (probe.u32().ok()? == 1).then_some(())
            })()
            .is_some();

            if is_complete {
                d.array()?;
                d.array()?;
                d.u32()?;
                d.array()?;
            }

            Ok(is_complete)
        })
        .map_err(|err| anyhow!("decode rewards update: {err}"))?;

    remaining_state_progress.clear();

    let (delta_treasury, delta_reserves, unclaimed_rewards, delta_fees) = if is_complete {
        let delta_treasury: i64 = decoder.decode()?;
        let delta_reserves: i64 = decoder.decode()?;
        let unclaimed_rewards = import_rewards(decoder, db, account_len, with_progress)?;
        let delta_fees: i64 = decoder.decode()?;
        decoder.skip()?;
        (delta_treasury, delta_reserves, unclaimed_rewards, delta_fees)
    } else {
        decoder.skip()?;
        (0_i64, 0_i64, 0_u64, 0_i64)
    };

    decoder.skip()?; // Pool distribution
    decoder.skip()?; // Stashed AVVM addresses

    import_recently_unregistered_accounts(
        db,
        point,
        era_history,
        &protocol_parameters,
        recently_unregistered_accounts,
    )?;

    Ok(InitialSnapshot {
        epoch,
        block_issuers,
        treasury,
        reserves,
        dreps,
        cc_members,
        governance_activity,
        pools,
        pools_updates,
        pools_retirements,
        fees,
        proposals_roots,
        proposals,
        cc_state,
        constitution,
        protocol_parameters,
        enacted_proposals,
        expired_proposals,
        donations,
        delta_treasury,
        delta_reserves,
        unclaimed_rewards,
        delta_fees,
    })
}

/// Bootstrap Amaru's initial state from a cardano-node `NewEpochState` payload.
#[allow(clippy::too_many_arguments)]
pub fn import_initial_snapshot(
    db: &impl Store,
    reader: &mut dyn Read,
    previous_accounts: BTreeSet<StakeCredential>,
    point: &Point,
    era_history: &EraHistory,
    network: NetworkName,
    with_progress: impl Fn(usize, &str) -> Box<dyn ProgressBar>,
) -> anyhow::Result<Epoch> {
    let mut decoder = LazyDecoder::new(reader);
    import_initial_snapshot_with_decoder(
        db,
        &mut decoder,
        previous_accounts,
        point,
        era_history,
        network,
        with_progress,
    )
}

/// Bootstrap Amaru's initial state from a decoder positioned at a cardano-node
/// `NewEpochState` payload.
#[allow(clippy::too_many_arguments)]
pub fn import_initial_snapshot_with_decoder(
    db: &impl Store,
    decoder: &mut LazyDecoder<'_>,
    previous_accounts: BTreeSet<StakeCredential>,
    point: &Point,
    era_history: &EraHistory,
    network: NetworkName,
    with_progress: impl Fn(usize, &str) -> Box<dyn ProgressBar>,
) -> anyhow::Result<Epoch> {
    let tip = point.slot_or_default();
    let expected_epoch = era_history.slot_to_epoch(tip, tip)?;

    let InitialSnapshot {
        epoch,
        block_issuers,
        treasury,
        reserves,
        dreps,
        cc_members,
        governance_activity,
        pools,
        pools_updates,
        pools_retirements,
        fees,
        proposals_roots,
        proposals,
        cc_state,
        constitution,
        protocol_parameters,
        enacted_proposals,
        expired_proposals,
        donations,
        delta_treasury,
        delta_reserves,
        unclaimed_rewards,
        delta_fees,
    } = decode_initial_snapshot(
        decoder,
        db,
        previous_accounts,
        point,
        expected_epoch,
        network,
        era_history,
        &with_progress,
    )?;

    import_protocol_parameters(db, &protocol_parameters)?;

    import_block_issuers(db, point, era_history, block_issuers)?;

    import_stake_pools(db, point, era_history, pools, pools_updates, pools_retirements)
        .map_err(|err| anyhow!("import pool state: {err}"))?;

    import_proposals_roots(db, &proposals_roots)?;

    import_proposals(db, point, era_history, &protocol_parameters, &proposals)?;

    import_recently_pruned_proposals(db, era_history, epoch, &proposals_roots, enacted_proposals, expired_proposals)?;

    import_votes(db, point, era_history, &protocol_parameters, proposals)?;

    import_pots(
        db,
        Pots {
            treasury: (treasury + delta_treasury) as u64 + unclaimed_rewards,
            reserves: (reserves - delta_reserves) as u64,
            fees: (fees - delta_fees) as u64,
            donations,
        },
    )?;

    // NOTE(INITIAL_BOOTSTRAP):
    //
    // It's important to import dreps *after* votes, because voting dreps from imported votes
    // will get their expiry updated, However:
    //
    // 1. Votes here contain ALL votes up to the snapshot; not just the ones from the ongoing
    //    epoch. So we might wrongly reset the expiry of DReps that voted in a previous epoch.
    //
    // 2. The DRep expiry is anyway stored in the drep's state, in the snapshot. So it'll be set
    //    accordingly on import.
    //
    // This may cause a few warnings on import, but they can be safely ignored.
    import_dreps(db, point, era_history, &protocol_parameters, epoch, dreps)?;

    import_constitution(db, constitution)?;

    import_constitutional_committee(db, point, era_history, &protocol_parameters, cc_state, cc_members)?;

    save_point(db, point, era_history, &protocol_parameters, governance_activity)?;

    Ok(epoch)
}

fn save_point(
    db: &impl Store,
    point: &Point,
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
    governance_activity: GovernanceActivity,
) -> anyhow::Result<()> {
    let transaction = db.create_transaction();

    transaction.save(
        era_history,
        protocol_parameters,
        Some(governance_activity),
        point,
        None,
        Default::default(),
        Default::default(),
        iter::empty(),
    )?;

    transaction.commit()?;

    Ok(())
}

fn import_protocol_parameters(db: &impl Store, protocol_parameters: &ProtocolParameters) -> anyhow::Result<()> {
    let transaction = db.create_transaction();
    transaction.set_protocol_parameters(protocol_parameters)?;
    transaction.commit()?;
    Ok(())
}

fn import_block_issuers(
    db: &impl Store,
    _point: &Point,
    era_history: &EraHistory,
    blocks: BTreeMap<PoolId, u64>,
) -> anyhow::Result<()> {
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
                era_history,
                // TODO: Unused when storing block issuers; require API change.
                &PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
                None,
                // NOTE: Synthetic keys for historical issuer counts
                //
                // These are not chain points. Each increment of `fake_slot` is a unique store key
                // used only to record that a pool issued a block. Height uses the same counter so
                // the constructed `Point` is complete and distinct.
                &Point::Specific(fake_slot.into(), Hash::new([0; 32]), BlockHeight::from(fake_slot)),
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
            )?;
            count -= 1;
            fake_slot += 1;
        }
    }
    info!(bootstrap::block_issuers::IMPORT, count = fake_slot);
    transaction.commit().map_err(Into::into)
}

fn skip_embedded_utxo(decoder: &mut LazyDecoder<'_>) -> anyhow::Result<()> {
    decoder.begin_array()?;
    decoder.skip()
}

fn import_dreps(
    db: &impl Store,
    point: &Point,
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
    epoch: Epoch,
    dreps: BTreeMap<StakeCredential, DRepState>,
) -> anyhow::Result<()> {
    let mut known_dreps = BTreeMap::new();

    let era_first_epoch = era_history.era_first_epoch(epoch).map_err(|e| StoreError::Internal(Box::new(e)))?;

    let transaction = db.create_transaction();

    transaction.with_dreps(|iterator| {
        for (drep, mut handle) in iterator {
            if epoch > era_first_epoch
                && let Some(row) = handle.borrow()
            {
                known_dreps.insert(drep, row.registered_at);
            }

            *handle.borrow_mut() = None;
        }
    })?;

    info!(bootstrap::dreps::IMPORT, size = dreps.len());

    transaction.save(
        era_history,
        protocol_parameters,
        None,
        point,
        None,
        store::Columns {
            utxo: iter::empty(),
            pools: iter::empty(),
            accounts: iter::empty(),
            dreps: dreps.into_iter().map(|(credential, state)| {
                let registered_at = known_dreps.remove(&credential).unwrap_or_else(|| CertificatePointer {
                    transaction: TransactionPointer { slot: point.slot_or_default(), ..TransactionPointer::default() },
                    certificate_index: 0,
                });

                let registration =
                    DRepRegistration { deposit: state.deposit, valid_until: state.expiry, registered_at };

                (credential, (Resettable::from(state.anchor), Some(registration)))
            }),
            cc_members: iter::empty(),
            proposals: iter::empty(),
            votes: iter::empty(),
        },
        Default::default(),
        iter::empty(),
    )?;

    Ok(transaction.commit()?)
}

fn import_proposals(
    db: &impl Store,
    point: &Point,
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
    proposals: &[ProposalState],
) -> anyhow::Result<()> {
    if db.iter_proposals()?.next().is_some() {
        warn!(bootstrap::proposals::IS_NOT_EMPTY);
    }

    let transaction = db.create_transaction();

    info!(bootstrap::proposals::IMPORT, size = proposals.len());

    transaction.save(
        era_history,
        protocol_parameters,
        None,
        point,
        None,
        store::Columns {
            utxo: iter::empty(),
            pools: iter::empty(),
            accounts: iter::empty(),
            dreps: iter::empty(),
            cc_members: iter::empty(),
            proposals: proposals
                .iter()
                .map(|proposal| -> anyhow::Result<_> {
                    let proposal_index = proposal.id.proposal_index as usize;
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
                            valid_until: proposal.expires_after,
                            proposal: proposal.procedure.clone(),
                        },
                    ))
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter(),
            votes: iter::empty(),
        },
        Default::default(),
        iter::empty(),
    )?;
    transaction.commit()?;

    Ok(())
}

/// Record the proposals pruned at the epoch boundary the snapshot sits on, using the ratification
/// outcome embedded in the snapshot (the `RatifyState` of the DRep pulser). Voting stake
/// distributions computed from this snapshot rely on these markers to exclude the deposits of
/// just-pruned proposals, and to account for just-enacted treasury withdrawals.
///
/// The ratify state only lists enacted and expired proposals. Conflicting siblings pruned by an
/// enactment are recovered by replaying the enactments through a proposals forest, mirroring the
/// regular ratification. The proposals to replay are read back from the store, so that the replay
/// sees exactly what the node will see at the next epoch boundary. Hence this must run after
/// `import_proposals`.
fn import_recently_pruned_proposals(
    db: &impl Store,
    era_history: &EraHistory,
    epoch: Epoch,
    roots: &ProposalsRoots,
    enacted: Vec<ProposalState>,
    expired: BTreeSet<ProposalId>,
) -> anyhow::Result<()> {
    let mut pruned: BTreeMap<ProposalId, RatificationStatus> =
        expired.into_iter().map(|id| (id, RatificationStatus::NotRatified)).collect();

    if !enacted.is_empty() {
        let candidates =
            db.iter_proposals()?.map(|(id, row)| (Rc::new(id), CandidateProposal::from(row))).collect::<Vec<_>>();

        // The forest's treasury is only used to refuse over-drawing withdrawals during
        // ratification. Enactments here come from the snapshot's own ratify state, so the
        // value is irrelevant.
        let mut forest = ProposalsForest::new(epoch - 1, &ProposalsRootsRc::from(roots.clone()), 0)
            .drain(era_history, candidates)
            .map_err(|err| anyhow!("replay enacted proposals: {err}"))?;
        let mut compass = forest.new_compass();

        for enacted_state in enacted {
            let id = Rc::new(enacted_state.id);
            let proposal = forest
                .get(&id)
                .cloned()
                .ok_or_else(|| anyhow!("enacted proposal {id} not found in the imported proposals"))?;
            for (pruned_id, status) in
                forest.enact(id, &proposal, &mut compass).map_err(|err| anyhow!("replay enacted proposals: {err}"))?
            {
                pruned.insert(*pruned_id, status);
            }
        }
    }

    info!(bootstrap::recently_pruned_proposals::IMPORT, size = pruned.len());

    let transaction = db.create_transaction();
    transaction.set_recently_pruned_proposals(pruned.iter().map(|(id, status)| (id, *status)))?;
    transaction.commit()?;

    Ok(())
}

fn import_stake_pools(
    db: &impl Store,
    point: &Point,
    era_history: &EraHistory,
    pools: BTreeMap<PoolId, PoolParams>,
    updates: BTreeMap<PoolId, PoolParams>,
    retirements: BTreeMap<PoolId, Epoch>,
) -> anyhow::Result<()> {
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

    info!(bootstrap::stake_pools::IMPORT, registered = state.registered.len(), retiring = state.unregistered.len());

    let transaction = db.create_transaction();
    transaction.with_pools(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })?;
    transaction.commit()?;

    let transaction = db.create_transaction();
    let protocol_parameters = &PREPROD_DEFAULT_PROTOCOL_PARAMETERS;
    transaction.save(
        era_history,
        // TODO: Unused when storing block issuers; require API change.
        protocol_parameters,
        None,
        point,
        None,
        store::Columns {
            utxo: iter::empty(),
            pools: state.registered.into_values().flat_map(move |registrations| {
                registrations
                    .into_iter()
                    .map(|r| (r, *DEFAULT_CERTIFICATE_POINTER, protocol_parameters.stake_pool_deposit))
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
    )?;
    Ok(transaction.commit()?)
}

fn import_pots(db: &impl Store, pots: Pots) -> anyhow::Result<()> {
    let transaction = db.create_transaction();
    transaction.with_pots(|mut row| {
        *row.borrow_mut() = pots;
    })?;
    transaction.commit()?;
    let Pots { treasury, reserves, fees, donations } = pots;
    info!(bootstrap::pots::IMPORT, treasury, reserves, fees, donations);
    Ok(())
}

fn import_proposals_roots(db: &impl Store, roots: &ProposalsRoots) -> anyhow::Result<()> {
    let roots_constitution = roots.constitution.as_ref().map(|s| s.to_string());
    let roots_constitutional_committee = roots.constitutional_committee.as_ref().map(|s| s.to_string());
    let roots_hard_fork = roots.hard_fork.as_ref().map(|s| s.to_string());
    let roots_protocol_parameters = roots.protocol_parameters.as_ref().map(|s| s.to_string());

    info!(
        bootstrap::proposal_roots::IMPORT,
        constitution = roots_constitution.as_deref().unwrap_or("none"),
        constitutional_committee = roots_constitutional_committee.as_deref().unwrap_or("none"),
        hard_fork = roots_hard_fork.as_deref().unwrap_or("none"),
        protocol_parameters = roots_protocol_parameters.as_deref().unwrap_or("none"),
    );

    let transaction = db.create_transaction();
    transaction.set_proposals_roots(roots)?;
    transaction.commit()?;
    Ok(())
}

fn import_constitution(db: &impl Store, constitution: Constitution) -> anyhow::Result<()> {
    let transaction = db.create_transaction();

    info!(
        bootstrap::constitution::IMPORT,
        anchor = constitution.anchor.url,
        guardrails = constitution
            .guardrail_script
            .map(|s: Hash<28>| s.to_string().chars().take(8).collect())
            .unwrap_or_else(|| "none".to_string()),
    );

    transaction.set_constitution(&constitution)?;

    transaction.commit()?;

    Ok(())
}

fn import_constitutional_committee(
    db: &impl Store,
    point: &Point,
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
    cc: Option<ConstitutionalCommittee>,
    mut hot_cold_delegations: BTreeMap<StakeCredential, ConstitutionalCommitteeMemberStatus>,
) -> anyhow::Result<()> {
    let transaction = db.create_transaction();

    transaction.with_cc_members(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })?;

    let mut cc_members = BTreeMap::new();

    let cc = match cc {
        None => {
            info!(bootstrap::constitutional_committee::IMPORT, state = "no_confidence");
            amaru_kernel::ConstitutionalCommitteeStatus::NoConfidence
        }
        Some(ConstitutionalCommittee { threshold, members }) => {
            info!(
                bootstrap::constitutional_committee::IMPORT,
                state = "trusted",
                threshold = format!("{}/{}", threshold.numerator, threshold.denominator),
                members = members.len(),
            );

            cc_members = members;

            amaru_kernel::ConstitutionalCommitteeStatus::Trusted { threshold }
        }
    };

    transaction.update_constitutional_committee(&cc, &BTreeMap::new(), &BTreeSet::new())?;

    transaction.save(
        era_history,
        protocol_parameters,
        None,
        point,
        None,
        store::Columns {
            utxo: iter::empty(),
            pools: iter::empty(),
            accounts: iter::empty(),
            dreps: iter::empty(),
            proposals: iter::empty(),
            votes: iter::empty(),
            cc_members: cc_members.into_iter().map(|(cold_cred, valid_until)| {
                (cold_cred, (Resettable::from(hot_cold_delegations.remove(&cold_cred)), Resettable::Set(valid_until)))
            }),
        },
        Default::default(),
        iter::empty(),
    )?;

    transaction.commit()?;

    Ok(())
}

fn import_votes(
    db: &impl Store,
    point: &Point,
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
    actions: Vec<ProposalState>,
) -> anyhow::Result<()> {
    let votes = actions
        .into_iter()
        .flat_map(|st| {
            let new_ballot_id = |voter| BallotId { proposal: st.id, voter };

            let mut votes = Vec::new();

            for (committee, vote) in st.committee_votes.into_iter() {
                let voter = match committee {
                    StakeCredential::KeyHash(hash) => Voter::ConstitutionalCommitteeKey(hash),
                    StakeCredential::ScriptHash(hash) => Voter::ConstitutionalCommitteeScript(hash),
                };

                let ballot = Ballot::new(vote, None);

                votes.push((new_ballot_id(voter), ballot));
            }

            for (drep, vote) in st.dreps_votes.into_iter() {
                let voter = match drep {
                    StakeCredential::KeyHash(hash) => Voter::DRepKey(hash),
                    StakeCredential::ScriptHash(hash) => Voter::DRepScript(hash),
                };

                let ballot = Ballot::new(vote, None);

                votes.push((new_ballot_id(voter), ballot));
            }

            for (pool_id, vote) in st.pools_votes.into_iter() {
                let voter = Voter::StakePoolKey(pool_id);

                let ballot = Ballot::new(vote, None);

                votes.push((new_ballot_id(voter), ballot));
            }

            votes
        })
        .collect::<Vec<_>>();

    info!(bootstrap::votes::IMPORT, size = votes.len());

    let transaction = db.create_transaction();

    transaction.save(
        era_history,
        protocol_parameters,
        None,
        point,
        None,
        store::Columns {
            utxo: iter::empty(),
            pools: iter::empty(),
            accounts: iter::empty(),
            dreps: iter::empty(),
            proposals: iter::empty(),
            cc_members: iter::empty(),
            votes: votes.into_iter(),
        },
        Default::default(),
        iter::empty(),
    )?;

    transaction.commit()?;

    Ok(())
}

// TODO: Move to Pallas
#[derive(Debug)]
#[expect(dead_code)]
struct GovActionState {
    id: ProposalId,
    committee_votes: BTreeMap<StakeCredential, Vote>,
    dreps_votes: BTreeMap<StakeCredential, Vote>,
    pools_votes: BTreeMap<PoolId, Vote>,
    proposal: Proposal,
    proposed_in: Epoch,
    expires_after: Epoch,
}

impl<'d, C: HasProtocolVersion> cbor::decode::Decode<'d, C> for GovActionState {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(7)?;
            Ok(GovActionState {
                id: d.decode_with(ctx)?,
                committee_votes: d.decode_with(ctx)?,
                dreps_votes: d.decode_with(ctx)?,
                pools_votes: d.decode_with(ctx)?,
                proposal: d.decode_with(ctx)?,
                proposed_in: d.decode_with(ctx)?,
                expires_after: d.decode_with(ctx)?,
            })
        })
    }
}

pub fn decode_node_pool_state(
    d: &mut cbor::Decoder<'_>,
    network: NetworkName,
    protocol_version: ProtocolVersion,
) -> Result<(BTreeMap<PoolId, PoolParams>, BTreeMap<PoolId, PoolParams>, BTreeMap<PoolId, Epoch>), cbor::decode::Error>
{
    d.array()?;

    let _pool_vrf_key_hashes: BTreeMap<Hash<{ size::VRF_KEY }>, u64> =
        d.decode().map_err(|err| contextualize_decode_error("node pool vrf key hashes", err))?;
    let pools = decode_node_pool_map(d, network, protocol_version, "node pools", |d, (network, protocol_version)| {
        let params: NodePoolStateParams = d.decode_with(&mut (*network, *protocol_version))?;
        Ok(params)
    })?;
    let pools_updates =
        decode_node_pool_map(d, network, protocol_version, "node pool updates", |d, (network, protocol_version)| {
            let params: NodePoolUpdateParams = d.decode_with(&mut (*network, *protocol_version))?;
            Ok(params)
        })?;
    let pools_retirements: BTreeMap<PoolId, Epoch> =
        d.decode().map_err(|err| contextualize_decode_error("node pool retirements", err))?;

    Ok((
        pools.into_iter().map(|(id, params)| (id, params.into_pool_params(id))).collect(),
        pools_updates.into_iter().map(|(id, params)| (id, params.into_pool_params(id))).collect(),
        pools_retirements,
    ))
}

fn decode_node_pool_map<T>(
    d: &mut cbor::Decoder<'_>,
    network: NetworkName,
    protocol_version: ProtocolVersion,
    field_name: &'static str,
    mut decode_value: impl FnMut(
        &mut cbor::Decoder<'_>,
        &mut (NetworkName, ProtocolVersion),
    ) -> Result<T, cbor::decode::Error>,
) -> Result<BTreeMap<PoolId, T>, cbor::decode::Error> {
    let len = d.map().map_err(|err| contextualize_decode_error(field_name, err))?;
    let mut entries = BTreeMap::new();
    let mut index = 0_u64;
    let mut protocol_version = protocol_version;

    loop {
        match len {
            Some(total) if index == total => break,
            None if d.datatype()? == cbor::data::Type::Break => {
                d.skip()?;
                break;
            }
            _ => {}
        }

        let key_offset = d.position();
        let pool_id: PoolId = d.decode_with(&mut protocol_version).map_err(|err| {
            contextualize_decode_error(format!("{field_name} key at entry {index} offset {key_offset}"), err)
        })?;
        let value_offset = d.position();
        let value = decode_value(d, &mut (network, protocol_version)).map_err(|err| {
            contextualize_decode_error(format!("{field_name} value at entry {index} offset {value_offset}"), err)
        })?;
        entries.insert(pool_id, value);
        index += 1;
    }

    Ok(entries)
}

// TODO: Reduce duplication with existing `PoolParams`
#[derive(Debug)]
struct NodePoolParams {
    vrf: Hash<{ size::VRF_KEY }>,
    pledge: Lovelace,
    cost: Lovelace,
    margin: RationalNumber,
    reward_account: RewardAccount,
    owners: BTreeSet<Hash<{ size::KEY }>>,
    relays: Vec<Relay>,
    metadata: Option<PoolMetadata>,
}

impl NodePoolParams {
    fn into_pool_params(self, id: PoolId) -> PoolParams {
        PoolParams {
            id,
            vrf: self.vrf,
            pledge: self.pledge,
            cost: self.cost,
            margin: self.margin,
            reward_account: self.reward_account,
            owners: self.owners.into_iter().collect(),
            relays: self.relays,
            metadata: self.metadata,
        }
    }
}

#[derive(Debug)]
struct NodePoolUpdateParams(NodePoolParams);

#[derive(Debug)]
struct NodePoolStateParams(NodePoolParams);

impl NodePoolUpdateParams {
    fn into_pool_params(self, id: PoolId) -> PoolParams {
        self.0.into_pool_params(id)
    }
}

impl NodePoolStateParams {
    fn into_pool_params(self, id: PoolId) -> PoolParams {
        self.0.into_pool_params(id)
    }
}

fn decode_optional_node_pool_metadata(
    d: &mut cbor::Decoder<'_>,
    len: Option<u64>,
    fields_before_metadata: u64,
    decode_metadata: impl FnOnce(&mut cbor::Decoder<'_>) -> Result<Option<PoolMetadata>, cbor::decode::Error>,
) -> Result<(Option<PoolMetadata>, u64, bool), cbor::decode::Error> {
    match len {
        Some(total) if total <= fields_before_metadata => Ok((None, fields_before_metadata, false)),
        None if d.datatype()? == cbor::data::Type::Break => {
            d.skip()?;
            Ok((None, fields_before_metadata, true))
        }
        _ => Ok((decode_metadata(d)?, fields_before_metadata + 1, false)),
    }
}

fn skip_remaining_array_fields(
    d: &mut cbor::Decoder<'_>,
    len: Option<u64>,
    consumed: u64,
    break_consumed: bool,
) -> Result<(), cbor::decode::Error> {
    match len {
        Some(total) => {
            for _ in consumed..total {
                d.skip()?;
            }
        }
        None if break_consumed => {}
        None => {
            while d.datatype()? != cbor::data::Type::Break {
                d.skip()?;
            }
            d.skip()?;
        }
    }

    Ok(())
}

fn contextualize_decode_error(context: impl Into<String>, err: cbor::decode::Error) -> cbor::decode::Error {
    if err.is_end_of_input() { err } else { cbor::decode::Error::message(format!("{}: {err}", context.into())) }
}

fn skip_node_pool_delegators(d: &mut cbor::Decoder<'_>) -> Result<(), cbor::decode::Error> {
    if d.datatype()? == cbor::data::Type::Tag {
        let found_tag = d.tag().map_err(|err| contextualize_decode_error("node pool delegators tag", err))?;

        if found_tag != cbor::data::Tag::new(258) {
            return Err(cbor::decode::Error::message(format!("unexpected node pool delegators tag: {found_tag:?}")));
        }
    }

    match d.array().map_err(|err| contextualize_decode_error("node pool delegators collection", err))? {
        Some(total) => {
            for index in 0..total {
                d.skip()
                    .map_err(|err| contextualize_decode_error(format!("node pool delegators element {index}"), err))?;
            }
        }
        None => {
            let mut index = 0_u64;

            while d.datatype()? != cbor::data::Type::Break {
                d.skip()
                    .map_err(|err| contextualize_decode_error(format!("node pool delegators element {index}"), err))?;
                index += 1;
            }
            d.skip().map_err(|err| contextualize_decode_error("node pool delegators break", err))?;
        }
    }

    Ok(())
}

impl<'b> cbor::decode::Decode<'b, (NetworkName, ProtocolVersion)> for NodePoolParams {
    fn decode(
        d: &mut cbor::Decoder<'b>,
        ctx: &mut (NetworkName, ProtocolVersion),
    ) -> Result<Self, cbor::decode::Error> {
        let (_, mut protocol_version) = *ctx;
        let len = d.array().map_err(|err| contextualize_decode_error("node pool entry", err))?;

        let vrf =
            d.decode_with(&mut protocol_version).map_err(|err| contextualize_decode_error("node pool vrf", err))?;
        let pledge =
            d.decode_with(&mut protocol_version).map_err(|err| contextualize_decode_error("node pool pledge", err))?;
        let cost =
            d.decode_with(&mut protocol_version).map_err(|err| contextualize_decode_error("node pool cost", err))?;
        let margin =
            d.decode_with(&mut protocol_version).map_err(|err| contextualize_decode_error("node pool margin", err))?;
        let reward_account = {
            let reward_account: NodeRewardAccount =
                d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool reward account", err))?;
            reward_account.0
        };
        let SerialisedAsSet(owners) =
            d.decode_with(&mut protocol_version).map_err(|err| contextualize_decode_error("node pool owners", err))?;
        let relays =
            d.decode_with(&mut protocol_version).map_err(|err| contextualize_decode_error("node pool relays", err))?;
        let (metadata, consumed, break_consumed) = decode_optional_node_pool_metadata(d, len, 7, |d| {
            d.decode_with(&mut protocol_version).map_err(|err| contextualize_decode_error("node pool metadata", err))
        })?;

        skip_remaining_array_fields(d, len, consumed, break_consumed)
            .map_err(|err| contextualize_decode_error("node pool trailing fields", err))?;

        Ok(NodePoolParams { vrf, pledge, cost, margin, reward_account, owners, relays, metadata })
    }
}

impl<'b> cbor::decode::Decode<'b, (NetworkName, ProtocolVersion)> for NodePoolUpdateParams {
    fn decode(
        d: &mut cbor::Decoder<'b>,
        ctx: &mut (NetworkName, ProtocolVersion),
    ) -> Result<Self, cbor::decode::Error> {
        let (_, mut protocol_version) = *ctx;

        let len = d.array().map_err(|err| contextualize_decode_error("node pool update entry", err))?;

        let _operator: PoolId = d
            .decode_with(&mut protocol_version)
            .map_err(|err| contextualize_decode_error("node pool update operator", err))?;

        let vrf = d
            .decode_with(&mut protocol_version)
            .map_err(|err| contextualize_decode_error("node pool update vrf", err))?;
        let pledge = d
            .decode_with(&mut protocol_version)
            .map_err(|err| contextualize_decode_error("node pool update pledge", err))?;
        let cost = d
            .decode_with(&mut protocol_version)
            .map_err(|err| contextualize_decode_error("node pool update cost", err))?;
        let margin = d
            .decode_with(&mut protocol_version)
            .map_err(|err| contextualize_decode_error("node pool update margin", err))?;
        let reward_account = {
            let reward_account: NodeRewardAccount =
                d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update reward account", err))?;
            reward_account.0
        };
        let SerialisedAsSet(owners) = d
            .decode_with(&mut protocol_version)
            .map_err(|err| contextualize_decode_error("node pool update owners", err))?;
        let relays = d
            .decode_with(&mut protocol_version)
            .map_err(|err| contextualize_decode_error("node pool update relays", err))?;
        let (metadata, consumed, break_consumed) = decode_optional_node_pool_metadata(d, len, 8, |d| {
            let metadata: NodePoolUpdateMetadata = d
                .decode_with(&mut protocol_version)
                .map_err(|err| contextualize_decode_error("node pool update metadata", err))?;
            Ok(metadata.0)
        })?;

        skip_remaining_array_fields(d, len, consumed, break_consumed)
            .map_err(|err| contextualize_decode_error("node pool update trailing fields", err))?;

        Ok(NodePoolUpdateParams(NodePoolParams { vrf, pledge, cost, margin, reward_account, owners, relays, metadata }))
    }
}

impl<'b> cbor::decode::Decode<'b, (NetworkName, ProtocolVersion)> for NodePoolStateParams {
    fn decode(
        d: &mut cbor::Decoder<'b>,
        ctx: &mut (NetworkName, ProtocolVersion),
    ) -> Result<Self, cbor::decode::Error> {
        let (_, mut protocol_version) = *ctx;
        let len = d.array().map_err(|err| contextualize_decode_error("node pool entry", err))?;

        let vrf =
            d.decode_with(&mut protocol_version).map_err(|err| contextualize_decode_error("node pool vrf", err))?;
        let pledge =
            d.decode_with(&mut protocol_version).map_err(|err| contextualize_decode_error("node pool pledge", err))?;
        let cost =
            d.decode_with(&mut protocol_version).map_err(|err| contextualize_decode_error("node pool cost", err))?;
        let margin = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool margin", err))?;
        let reward_account = {
            let reward_account: NodeRewardAccount =
                d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool reward account", err))?;
            reward_account.0
        };
        let SerialisedAsSet(owners) =
            d.decode_with(&mut protocol_version).map_err(|err| contextualize_decode_error("node pool owners", err))?;
        let relays =
            d.decode_with(&mut protocol_version).map_err(|err| contextualize_decode_error("node pool relays", err))?;
        let (metadata, consumed, _) = decode_optional_node_pool_metadata(d, len, 7, |d| {
            d.decode_with(&mut protocol_version)
                .map(|SerialisedAsArray(option)| option)
                .map_err(|err| contextualize_decode_error("node pool metadata", err))
        })?;

        d.skip().map_err(|err| {
            contextualize_decode_error(format!("node pool deposit (len={len:?}, consumed={consumed})"), err)
        })?;

        let consumed = consumed + 1;
        let (consumed, break_consumed) = match len {
            Some(total) if total <= consumed => (consumed, false),
            None if d.datatype()? == cbor::data::Type::Break => {
                d.skip()?;
                (consumed, true)
            }
            _ => {
                skip_node_pool_delegators(d).map_err(|err| {
                    contextualize_decode_error(format!("node pool delegators (len={len:?}, consumed={consumed})"), err)
                })?;
                (consumed + 1, false)
            }
        };

        skip_remaining_array_fields(d, len, consumed, break_consumed)
            .map_err(|err| contextualize_decode_error("node pool trailing fields", err))?;

        Ok(NodePoolStateParams(NodePoolParams { vrf, pledge, cost, margin, reward_account, owners, relays, metadata }))
    }
}

struct NodePoolUpdateMetadata(Option<PoolMetadata>);

impl<'b> cbor::decode::Decode<'b, ProtocolVersion> for NodePoolUpdateMetadata {
    #[allow(clippy::wildcard_enum_match_arm)]
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut ProtocolVersion) -> Result<Self, cbor::decode::Error> {
        match d.datatype()? {
            cbor::data::Type::Null => {
                d.skip()?;
                Ok(Self(None))
            }
            cbor::data::Type::Array | cbor::data::Type::ArrayIndef => {
                let mut probe = d.probe();
                let len = probe.array()?;
                if len == Some(0) {
                    d.array()?;
                    Ok(Self(None))
                } else if matches!(probe.datatype()?, cbor::data::Type::String | cbor::data::Type::StringIndef) {
                    let metadata: PoolMetadata = d.decode_with(ctx)?;
                    Ok(Self(Some(metadata)))
                } else {
                    let SerialisedAsArray(metadata) = d.decode_with(ctx)?;
                    Ok(Self(metadata))
                }
            }
            other => Err(cbor::decode::Error::type_mismatch(other)),
        }
    }
}

// TODO: reduce duplication with kernel's Account
#[derive(Debug)]
struct NodeAccount {
    rewards: Lovelace,
    deposit: Lovelace,
    pool: Option<PoolId>,
    drep: Option<DRep>,
}

impl NodeAccount {
    fn into_row(self, point: &Point, default_deposit: Option<Lovelace>) -> accounts::Row {
        accounts::Row {
            pool: self.pool.map(|pool| (pool, *DEFAULT_CERTIFICATE_POINTER)),
            deposit: default_deposit.unwrap_or(self.deposit),
            // NewEpochState has no DRep registration pointer. Place imported delegations after
            // registrations so they are considered valid.
            drep: self.drep.map(|drep| {
                (
                    drep,
                    CertificatePointer {
                        transaction: TransactionPointer {
                            slot: point.slot_or_default(),
                            ..TransactionPointer::default()
                        },
                        // NOTE: accounts initial bootstrap
                        //
                        // We use an index strictly larger than DRep registration
                        // certificates, to ensure that the imported delegations are
                        // considered valid (happened after DRep existence).
                        certificate_index: 1,
                    },
                )
            }),
            rewards: self.rewards,
        }
    }
}

impl<'b, C: HasProtocolVersion> cbor::decode::Decode<'b, C> for NodeAccount {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        let rewards = d.decode_with(ctx)?;
        let deposit = d.decode_with(ctx)?;
        let pool = d.decode_with(ctx)?;
        let drep = d.decode_with(ctx)?;

        Ok(NodeAccount { rewards, deposit, pool, drep })
    }
}

struct NodeRewardAccount(RewardAccount);

impl<'b> cbor::decode::Decode<'b, (NetworkName, ProtocolVersion)> for NodeRewardAccount {
    #[allow(clippy::wildcard_enum_match_arm)]
    fn decode(
        d: &mut cbor::Decoder<'b>,
        ctx: &mut (NetworkName, ProtocolVersion),
    ) -> Result<Self, cbor::decode::Error> {
        let (network_name, protocol_version) = ctx;
        let network: Network = (*network_name).into();
        match d.datatype()? {
            cbor::data::Type::Bytes | cbor::data::Type::BytesIndef => Ok(Self(d.decode_with(protocol_version)?)),
            cbor::data::Type::Array | cbor::data::Type::ArrayIndef => {
                let credential: StakeCredential = d.decode_with(protocol_version)?;
                Ok(Self(RewardAccount::new(network, credential)))
            }
            other => Err(cbor::decode::Error::type_mismatch(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use amaru_kernel::{Bytes, Epoch, Hash, NetworkName, StakeCredential, cbor, protocol_version, to_cbor};

    use super::{
        NodeRewardAccount, decode_initial_snapshot_prefix, decode_optional_node_pool_metadata,
        skip_remaining_array_fields,
    };

    #[test]
    fn accepts_new_epoch_state_prefix() {
        let bytes = to_cbor(&(2_u64, BTreeMap::<u8, u8>::new()));
        let mut decoder = cbor::Decoder::new(&bytes);

        let epoch = decode_initial_snapshot_prefix(&mut decoder, Epoch::from(2)).unwrap();

        assert_eq!(epoch, Epoch::from(2));
    }

    #[test]
    fn rejects_unexpected_epoch_prefix() {
        let bytes = to_cbor(&(2_u64, BTreeMap::<u8, u8>::new()));
        let mut decoder = cbor::Decoder::new(&bytes);

        let err = decode_initial_snapshot_prefix(&mut decoder, Epoch::from(42)).unwrap_err();

        assert!(err.to_string().contains("expected epoch 42"));
    }

    #[test]
    fn rejects_cardano_node_wrapper_shape() {
        let bytes = to_cbor(&(2_u64, vec![0_u8]));
        let mut decoder = cbor::Decoder::new(&bytes);

        let err = decode_initial_snapshot_prefix(&mut decoder, Epoch::from(2)).unwrap_err();

        assert!(err.to_string().contains("previous-blocks map"));
    }

    #[test]
    fn missing_optional_metadata_in_definite_arrays_is_treated_as_nothing() {
        let bytes = [0x82, 0x01, 0x02];
        let mut decoder = cbor::Decoder::new(&bytes);
        let len = decoder.array().unwrap();

        assert_eq!(decoder.u8().unwrap(), 1);
        assert_eq!(decoder.u8().unwrap(), 2);

        let (metadata, consumed, break_consumed) =
            decode_optional_node_pool_metadata(&mut decoder, len, 2, |_| Ok(None)).unwrap();

        assert!(metadata.is_none());
        assert_eq!(consumed, 2);
        assert!(!break_consumed);

        skip_remaining_array_fields(&mut decoder, len, consumed, break_consumed).unwrap();
        assert!(decoder.datatype().is_err());
    }

    #[test]
    fn missing_optional_metadata_in_indefinite_arrays_consumes_break() {
        let bytes = [0x9f, 0x01, 0x02, 0xff];
        let mut decoder = cbor::Decoder::new(&bytes);
        let len = decoder.array().unwrap();

        assert_eq!(decoder.u8().unwrap(), 1);
        assert_eq!(decoder.u8().unwrap(), 2);

        let (metadata, consumed, break_consumed) =
            decode_optional_node_pool_metadata(&mut decoder, len, 2, |_| Ok(None)).unwrap();

        assert!(metadata.is_none());
        assert_eq!(consumed, 2);
        assert!(break_consumed);

        skip_remaining_array_fields(&mut decoder, len, consumed, break_consumed).unwrap();
        assert!(decoder.datatype().is_err());
    }

    #[test]
    fn node_reward_account_bytes_preserve_embedded_network() {
        let reward_account =
            Bytes::from(hex::decode("e0e3af434a5516854f20191807cc5ea85b57b4fd0f050f3eab28af19ee").unwrap());
        let bytes = cbor::to_vec(&reward_account).unwrap();
        let mut decoder = cbor::Decoder::new(bytes.as_slice());
        let network = NetworkName::Mainnet;
        let protocol_version = protocol_version::MINIMUM_SUPPORTED;

        let decoded: NodeRewardAccount = decoder.decode_with(&mut (network, protocol_version)).unwrap();

        assert_eq!(decoded.0.to_vec(), *reward_account);
    }

    #[test]
    fn node_reward_account_credential_decodes_to_snapshot_network_reward_account() {
        let credential = StakeCredential::KeyHash(Hash::new(
            hex::decode("e3af434a5516854f20191807cc5ea85b57b4fd0f050f3eab28af19ee").unwrap().try_into().unwrap(),
        ));
        let bytes = cbor::to_vec(credential).unwrap();
        let mut decoder = cbor::Decoder::new(bytes.as_slice());
        let network = NetworkName::Mainnet;
        let protocol_version = protocol_version::MINIMUM_SUPPORTED;

        let decoded: NodeRewardAccount = decoder.decode_with(&mut (network, protocol_version)).unwrap();

        assert_eq!(
            decoded.0.to_vec(),
            hex::decode("e1e3af434a5516854f20191807cc5ea85b57b4fd0f050f3eab28af19ee").unwrap()
        );
    }
}
