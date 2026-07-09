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
    sync::LazyLock,
};

use amaru_kernel::{
    Account, Ballot, BallotId, Bytes, CertificatePointer, ComparableProposalId, Constitution, ConstitutionalCommittee,
    ConstitutionalCommitteeMemberStatus, DRep, DRepRegistration, DRepState, Epoch, EraHistory, Hash, Lovelace, Network,
    NetworkName, Nullable, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, Point, PoolId, PoolMetadata, PoolParams, Proposal,
    ProposalId, ProposalPointer, ProposalState, ProtocolParameters, RationalNumber, Relay, Reward, RewardAccount, Set,
    Slot, StakeCredential, StakePayload, StrictMaybe, TransactionPointer, Vote, Voter,
    cbor::{self, lazy::LazyDecoder},
    new_stake_address, protocol_version, reward_account_to_stake_credential, size,
};
use amaru_progress_bar::ProgressBar;
use tracing::{info, warn};

use crate::{
    epoch_transition::GovernanceActivity,
    governance::ratification::ProposalsRoots,
    state::{diff_bind::Resettable, diff_epoch_reg::DiffEpochReg},
    store::{self, Store, StoreError, TransactionalContext, columns::proposals},
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

    #[error("snapshot protocol version {}.{} is too old; minimum supported version is {}.{}", snapshot_version.0, snapshot_version.1, minimum_version.0, minimum_version.1)]
    ProtocolVersionTooOld {
        snapshot_version: amaru_kernel::ProtocolVersion,
        minimum_version: amaru_kernel::ProtocolVersion,
    },
}

fn format_pool_state_decode_error(error: Box<dyn std::error::Error>) -> String {
    let error = error.to_string();

    if error.contains("node pool vrf key hashes") && error.contains("Invalid hash size") {
        return "snapshot uses an older unsupported pool/account encoding; regenerate snapshots with amaru create-bootstrap-snapshots (db-analyser).".to_owned();
    }

    format!("decode pool state: {error}")
}

fn decode_initial_snapshot_prefix(
    d: &mut cbor::Decoder<'_>,
    expected_epoch: Epoch,
) -> Result<Epoch, Box<dyn std::error::Error>> {
    use cbor::data::Type::{Map, MapIndef};

    d.array()?;

    let epoch = d
        .u64()
        .map(Epoch::from)
        .map_err(|_| Box::new(InitialSnapshotFormatError::MissingEpochPrefix) as Box<dyn std::error::Error>)?;

    if epoch != expected_epoch {
        return Err(Box::new(InitialSnapshotFormatError::UnexpectedEpoch { expected: expected_epoch, actual: epoch }));
    }

    if !matches!(d.datatype()?, Map | MapIndef) {
        return Err(Box::new(InitialSnapshotFormatError::MissingPreviousBlocksMap));
    }

    Ok(epoch)
}

/// (Partially) decode a cardano-node `NewEpochState` payload.
///
/// -> <https://github.com/IntersectMBO/cardano-ledger/blob/a81e6035006529ba0abc034716c2e21e7406500d/eras/shelley/impl/src/Cardano/Ledger/Shelley/LedgerState/Types.hs#L315-L345>
///
/// We rely on data present in these to bootstrap Amaru's initial state.
#[allow(clippy::too_many_arguments)]
pub fn import_initial_snapshot(
    db: &impl Store,
    reader: &mut dyn Read,
    point: &Point,
    era_history: &EraHistory,
    network: NetworkName,
    with_progress: impl Fn(usize, &str) -> Box<dyn ProgressBar>,
) -> Result<Epoch, Box<dyn std::error::Error>> {
    let mut decoder = LazyDecoder::new(reader);
    let tip = point.slot_or_default();
    let expected_epoch = era_history.slot_to_epoch(tip, tip)?;

    let epoch: Epoch = decoder.with_decoder(|d| decode_initial_snapshot_prefix(d, expected_epoch))?;

    // NOTE(INITIAL_BOOTSTRAP):
    // We use the current blocks made here as we assume that users are providing snapshots of the
    // last block of the epoch. We have no intrinsic ways to check that this is the case since we
    // do not know what the last block of an epoch is, and we can't reliably look at the number of
    // blocks either.
    let block_issuers: BTreeMap<PoolId, u64> = decoder.with_decoder(|d| {
        // Previous blocks made
        d.skip()?;

        Ok(d.decode()?)
    })?;

    let (treasury, reserves): (i64, i64) = decoder.with_decoder(|d| {
        // Epoch State
        d.array()?;

        // Epoch State / Account State
        d.array()?;

        Ok((d.decode()?, d.decode()?))
    })?;

    let dreps: BTreeMap<StakeCredential, DRepState> = decoder.with_decoder(|d| {
        // Epoch State / Ledger State
        d.array()?;

        // Epoch State / Ledger State / Cert State
        d.array()?;

        // Epoch State / Ledger State / Cert State / Voting State
        d.array()?;

        Ok(d.decode()?)
    })?;

    // Committee cold -> hot delegations
    let cc_members: BTreeMap<StakeCredential, ConstitutionalCommitteeMemberStatus> = decoder.decode()?;

    let governance_activity: GovernanceActivity = decoder.with_decoder(|d| {
        // Dormant Epoch
        let dormant_epoch: Epoch = d.decode()?;
        let governance_activity = GovernanceActivity { consecutive_dormant_epochs: u64::from(dormant_epoch) as u32 };
        info!(dormant_epochs = governance_activity.consecutive_dormant_epochs, "governance activity");
        Ok(governance_activity)
    })?;

    let (pools, pools_updates, pools_retirements) =
        decoder.with_decoder(|d| Ok(decode_node_pool_state(d, network)?)).map_err(format_pool_state_decode_error)?;

    let accounts =
        decoder.with_decoder(|d| Ok(decode_node_accounts(d)?)).map_err(|err| format!("decode accounts: {err}"))?;

    skip_embedded_utxo(&mut decoder).map_err(|err| format!("skip embedded utxo: {err}"))?;

    let fees: i64 = decoder
        .with_decoder(|d| {
            let _deposited: u64 = d.decode()?;
            Ok(d.decode()?)
        })
        .map_err(|err| format!("decode fees: {err}"))?;

    let (root_params, root_hard_fork, root_cc, root_constitution) = decoder.with_decoder(|d| {
        // Epoch State / Ledger State / UTxO State / utxosGovState
        d.array()?;

        // Proposals
        d.array()?;
        d.array()?;
        Ok((d.decode()?, d.decode()?, d.decode()?, d.decode()?))
    })?;

    let proposals: Vec<ProposalState> = decoder.decode()?;

    let cc_state: StrictMaybe<ConstitutionalCommittee> = decoder.decode()?;

    let constitution: Constitution = decoder.decode()?;

    // Current Protocol Params — decode before any write so a stale snapshot fails cleanly.
    let pparams: ProtocolParameters = decoder.decode()?;

    protocol_version::validate(pparams.protocol_version, protocol_version::MINIMUM_SUPPORTED).map_err(|e| {
        InitialSnapshotFormatError::ProtocolVersionTooOld {
            snapshot_version: e.snapshot_version,
            minimum_version: e.minimum_version,
        }
    })?;

    import_block_issuers(db, point, era_history, block_issuers)?;
    import_stake_pools(db, point, era_history, epoch, pools, pools_updates, pools_retirements)
        .map_err(|err| format!("import pool state: {err}"))?;
    import_proposals_roots(db, root_params, root_hard_fork, root_cc, root_constitution)?;
    let protocol_parameters = import_protocol_parameters(db, pparams)?;

    import_proposals(db, point, era_history, &protocol_parameters, &proposals)?;

    import_votes(db, point, era_history, &protocol_parameters, proposals)?;

    decoder.skip()?; // Previous Protocol Params
    decoder.skip()?; // Future Protocol Params
    decoder.with_decoder(|d| {
        d.array()?; // DRep Pulsing State
        d.array()?; // Pulsing Snapshot
        Ok(d.skip()?) // Last epoch votes
    })?;
    decoder.skip()?; // DRep distr
    decoder.skip()?; // DRep state
    decoder.skip()?; // Pool distr
    decoder.skip()?; // Ratify state

    // Epoch State / Ledger State / UTxO State / utxosStakeDistr
    decoder.skip()?;

    // Epoch State / Ledger State / UTxO State / utxosDonation
    decoder.skip()?;

    // Epoch State / Snapshots
    decoder.with_decoder(|d| {
        d.array()?;
        Ok(())
    })?;
    decoder.skip()?; // Epoch State / Snapshots / Mark
    decoder.skip()?; // Epoch State / Snapshots / Set
    decoder.skip()?; // Epoch State / Snapshots / Go
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
        .map_err(|err| format!("decode rewards update: {err}"))?;

    let (delta_treasury, delta_reserves, mut rewards, delta_fees) = if is_complete {
        let delta_treasury: i64 = decoder.decode()?;
        let delta_reserves: i64 = decoder.decode()?;
        let rewards: BTreeMap<StakeCredential, Set<Reward>> = decoder.decode()?;
        let delta_fees: i64 = decoder.decode()?;
        decoder.skip()?;
        (delta_treasury, delta_reserves, rewards, delta_fees)
    } else {
        (0_i64, 0_i64, BTreeMap::new(), 0_i64)
    };

    import_accounts(db, &with_progress, point, era_history, &protocol_parameters, accounts, &mut rewards)?;

    let unclaimed_rewards = rewards
        .into_iter()
        .fold(0_u64, |total, (_, rewards)| total + rewards.into_iter().fold(0, |inner, reward| inner + reward.amount));

    import_pots(
        db,
        (treasury + delta_treasury) as u64 + unclaimed_rewards,
        (reserves - delta_reserves) as u64,
        (fees - delta_fees) as u64,
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
) -> Result<(), Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();

    transaction.save(
        era_history,
        protocol_parameters,
        governance_activity,
        point,
        None,
        Default::default(),
        Default::default(),
        iter::empty(),
    )?;

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
    _point: &Point,
    era_history: &EraHistory,
    blocks: BTreeMap<PoolId, u64>,
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
                era_history,
                // TODO: Unused when storing block issuers; require API change.
                &PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
                GovernanceActivity::default(),
                &Point::Specific(fake_slot.into(), Hash::new([0; 32])),
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
    info!(count = fake_slot, "block_issuers");
    transaction.commit().map_err(Into::into)
}

fn skip_embedded_utxo(decoder: &mut LazyDecoder<'_>) -> Result<(), Box<dyn std::error::Error>> {
    decoder.with_decoder(|d| {
        d.array()?;
        d.skip()?;
        Ok(())
    })
}

fn import_dreps(
    db: &impl Store,
    point: &Point,
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
    epoch: Epoch,
    dreps: BTreeMap<StakeCredential, DRepState>,
) -> Result<(), Box<dyn std::error::Error>> {
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

    info!(size = dreps.len(), "dreps");

    transaction.save(
        era_history,
        protocol_parameters,
        GovernanceActivity::default(),
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

                (credential, (Resettable::from(Option::from(state.anchor)), Some(registration)))
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
) -> Result<(), Box<dyn std::error::Error>> {
    if db.iter_proposals()?.next().is_some() {
        warn!("given storage is not empty: it contains proposals; overwriting");
    }

    let transaction = db.create_transaction();

    info!(size = proposals.len(), "proposals");

    transaction.save(
        era_history,
        protocol_parameters,
        GovernanceActivity::default(),
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
                .map(|proposal| -> Result<_, Box<dyn std::error::Error>> {
                    let proposal_index = proposal.id.action_index as usize;
                    Ok((
                        ComparableProposalId::from(proposal.id.clone()),
                        proposals::Value {
                            proposed_in: ProposalPointer {
                                transaction: TransactionPointer {
                                    slot: era_history.epoch_bounds(proposal.proposed_in)?.start,
                                    transaction_index: 0,
                                },
                                proposal_index,
                            },
                            valid_until: proposal.proposed_in + protocol_parameters.gov_action_lifetime,
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

fn import_stake_pools(
    db: &impl Store,
    point: &Point,
    era_history: &EraHistory,
    epoch: Epoch,
    pools: BTreeMap<PoolId, PoolParams>,
    updates: BTreeMap<PoolId, PoolParams>,
    retirements: BTreeMap<PoolId, Epoch>,
) -> Result<(), Box<dyn std::error::Error>> {
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

    info!(registered = state.registered.len(), retiring = state.unregistered.len(), "stake_pools",);
    let transaction = db.create_transaction();
    transaction.with_pools(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })?;
    transaction.commit()?;

    let transaction = db.create_transaction();
    transaction.save(
        era_history,
        // TODO: Unused when storing block issuers; require API change.
        &PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
        GovernanceActivity::default(),
        point,
        None,
        store::Columns {
            utxo: iter::empty(),
            pools: state.registered.into_values().flat_map(move |registrations| {
                registrations.into_iter().map(|r| (r, *DEFAULT_CERTIFICATE_POINTER, epoch)).collect::<Vec<_>>()
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

fn import_pots(db: &impl Store, treasury: u64, reserves: u64, fees: u64) -> Result<(), Box<dyn std::error::Error>> {
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
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
    accounts: BTreeMap<StakeCredential, Account>,
    rewards_updates: &mut BTreeMap<StakeCredential, Set<Reward>>,
) -> Result<(), Box<dyn std::error::Error>> {
    if db.iter_accounts()?.next().is_some() {
        warn!("given storage is not empty: it contains accounts; overwriting");
    }

    let transaction = db.create_transaction();

    let mut credentials = accounts
        .into_iter()
        .map(|(credential, Account { rewards_and_deposit, pool, drep, .. })| {
            let (rewards, deposit) = Option::<(Lovelace, Lovelace)>::from(rewards_and_deposit)
                .unwrap_or((0, protocol_parameters.stake_credential_deposit));

            let rewards_update = match rewards_updates.remove(&credential) {
                None => 0,
                Some(set) => set.iter().fold(0, |total, update| total + update.amount),
            };

            (
                credential,
                (
                    Resettable::from(Option::<PoolId>::from(pool).map(|pool| (pool, *DEFAULT_CERTIFICATE_POINTER))),
                    //No slot to retrieve. All registrations coming from snapshot are considered valid.
                    Resettable::from(Option::<DRep>::from(drep).map(|drep| {
                        (
                            drep,
                            CertificatePointer {
                                transaction: TransactionPointer {
                                    slot: point.slot_or_default(),
                                    ..TransactionPointer::default()
                                },
                                // NOTE(INITIAL_BOOTSTRAP):
                                //
                                // We use an index strictly larger than DRep registration
                                // certificates, to ensure that the imported delegations are
                                // considered valid (happened after DRep existence).
                                certificate_index: 1,
                            },
                        )
                    })),
                    Some(deposit),
                    rewards + rewards_update,
                ),
            )
        })
        .collect::<Vec<_>>();

    info!(size = credentials.len(), "credentials");

    let progress = with_progress(credentials.len(), "Accounts [{pos:>7}/{len:7}] {bar:40.green} ({eta} remaining)");

    while !credentials.is_empty() {
        let n = std::cmp::min(BATCH_SIZE, credentials.len());
        let chunk = credentials.drain(0..n);

        transaction.save(
            era_history,
            protocol_parameters,
            GovernanceActivity::default(),
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
        )?;

        progress.tick(n);
    }

    transaction.commit()?;
    progress.clear();

    Ok(())
}

fn import_proposals_roots(
    db: &impl Store,
    protocol_parameters: StrictMaybe<ComparableProposalId>,
    hard_fork: StrictMaybe<ComparableProposalId>,
    constitutional_committee: StrictMaybe<ComparableProposalId>,
    constitution: StrictMaybe<ComparableProposalId>,
) -> Result<(), Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();

    let roots = ProposalsRoots {
        protocol_parameters: Option::from(protocol_parameters),
        hard_fork: Option::from(hard_fork),
        constitutional_committee: Option::from(constitutional_committee),
        constitution: Option::from(constitution),
    };

    let roots_constitution = roots.constitution.as_ref().map(|s| s.to_string());
    let roots_constitutional_committee = roots.constitutional_committee.as_ref().map(|s| s.to_string());
    let roots_hard_fork = roots.hard_fork.as_ref().map(|s| s.to_string());
    let roots_protocol_parameters = roots.protocol_parameters.as_ref().map(|s| s.to_string());

    info!(
        constitution = roots_constitution.as_deref().unwrap_or("none"),
        constitutional_committee = roots_constitutional_committee.as_deref().unwrap_or("none"),
        hard_fork = roots_hard_fork.as_deref().unwrap_or("none"),
        protocol_parameters = roots_protocol_parameters.as_deref().unwrap_or("none"),
        "proposal roots"
    );

    transaction.set_proposals_roots(&roots)?;
    transaction.commit()?;

    Ok(())
}

fn import_constitution(db: &impl Store, constitution: Constitution) -> Result<(), Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();

    info!(
        anchor = constitution.anchor.url,
        guardrails = Option::from(constitution.guardrail_script.clone())
            .map(|s: Hash<28>| s.to_string().chars().take(8).collect())
            .unwrap_or_else(|| "none".to_string()),
        "constitution"
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
    cc: StrictMaybe<ConstitutionalCommittee>,
    mut hot_cold_delegations: BTreeMap<StakeCredential, ConstitutionalCommitteeMemberStatus>,
) -> Result<(), Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();

    transaction.with_cc_members(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })?;

    let mut cc_members = BTreeMap::new();

    let cc = match cc {
        StrictMaybe::Nothing => {
            info!(state = "no confidence", "constitutional committee");
            amaru_kernel::ConstitutionalCommitteeStatus::NoConfidence
        }
        StrictMaybe::Just(ConstitutionalCommittee { threshold, members }) => {
            info!(
                state = "trusted",
                threshold = format!("{}/{}", threshold.numerator, threshold.denominator),
                members = members.len(),
                "constitutional committee"
            );

            cc_members = members;

            amaru_kernel::ConstitutionalCommitteeStatus::Trusted { threshold }
        }
    };

    transaction.update_constitutional_committee(&cc, BTreeMap::new(), BTreeSet::new())?;

    transaction.save(
        era_history,
        protocol_parameters,
        GovernanceActivity::default(),
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
                let hot_cred = match hot_cold_delegations.remove(&cold_cred) {
                    Some(ConstitutionalCommitteeMemberStatus::DelegatedToHotCredential(hot_cred)) => {
                        Resettable::Set(hot_cred)
                    }
                    None | Some(ConstitutionalCommitteeMemberStatus::Resigned(..)) => Resettable::Reset,
                };

                (cold_cred, (hot_cred, Resettable::Set(valid_until)))
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
) -> Result<(), Box<dyn std::error::Error>> {
    let votes = actions
        .into_iter()
        .flat_map(|st| {
            let new_ballot_id = |voter| BallotId { proposal: ComparableProposalId::from(st.id.clone()), voter };

            let mut votes = Vec::new();

            for (committee, vote) in st.committee_votes.into_iter() {
                let voter = match committee {
                    StakeCredential::AddrKeyhash(hash) => Voter::ConstitutionalCommitteeKey(hash),
                    StakeCredential::ScriptHash(hash) => Voter::ConstitutionalCommitteeScript(hash),
                };

                let ballot = Ballot::new(vote, None);

                votes.push((new_ballot_id(voter), ballot));
            }

            for (drep, vote) in st.dreps_votes.into_iter() {
                let voter = match drep {
                    StakeCredential::AddrKeyhash(hash) => Voter::DRepKey(hash),
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

    info!(size = votes.len(), "votes");

    let transaction = db.create_transaction();

    transaction.save(
        era_history,
        protocol_parameters,
        GovernanceActivity::default(),
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

impl<'d, C> cbor::decode::Decode<'d, C> for GovActionState {
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
) -> Result<(BTreeMap<PoolId, PoolParams>, BTreeMap<PoolId, PoolParams>, BTreeMap<PoolId, Epoch>), cbor::decode::Error>
{
    d.array()?;

    let mut node_network = network;
    let _pool_vrf_key_hashes: BTreeMap<Hash<{ size::VRF_KEY }>, u64> =
        d.decode().map_err(|err| contextualize_decode_error("node pool vrf key hashes", err))?;
    let pools = decode_node_pool_map(d, &mut node_network, "node pools", |d, network| {
        let params: NodePoolStateParams = d.decode_with(network)?;
        Ok(params)
    })?;
    let pools_updates = decode_node_pool_map(d, &mut node_network, "node pool updates", |d, network| {
        let params: NodePoolUpdateParams = d.decode_with(network)?;
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
    network: &mut NetworkName,
    field_name: &'static str,
    mut decode_value: impl FnMut(&mut cbor::Decoder<'_>, &mut NetworkName) -> Result<T, cbor::decode::Error>,
) -> Result<BTreeMap<PoolId, T>, cbor::decode::Error> {
    let len = d.map().map_err(|err| contextualize_decode_error(field_name, err))?;
    let mut entries = BTreeMap::new();
    let mut index = 0_u64;

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
        let pool_id: PoolId = d.decode_with(network).map_err(|err| {
            contextualize_decode_error(format!("{field_name} key at entry {index} offset {key_offset}"), err)
        })?;
        let value_offset = d.position();
        let value = decode_value(d, network).map_err(|err| {
            contextualize_decode_error(format!("{field_name} value at entry {index} offset {value_offset}"), err)
        })?;
        entries.insert(pool_id, value);
        index += 1;
    }

    Ok(entries)
}

pub fn decode_node_accounts(
    d: &mut cbor::Decoder<'_>,
) -> Result<BTreeMap<StakeCredential, Account>, cbor::decode::Error> {
    d.array()?;
    let accounts: BTreeMap<StakeCredential, NodeAccount> = d.decode()?;
    let mut pointers: BTreeMap<StakeCredential, Set<(u64, u64, u64)>> = d.decode()?;
    d.skip()?; // dsFutureGenDelegs
    d.skip()?; // dsGenDelegs

    Ok(accounts
        .into_iter()
        .map(|(credential, account)| {
            let pointers = pointers.remove(&credential).unwrap_or_else(|| Vec::new().into());
            (credential, account.into_account(pointers))
        })
        .collect())
}

#[derive(Debug)]
struct NodePoolParams {
    vrf: Hash<{ size::VRF_KEY }>,
    pledge: Lovelace,
    cost: Lovelace,
    margin: RationalNumber,
    reward_account: RewardAccount,
    owners: Set<Hash<{ size::KEY }>>,
    relays: Vec<Relay>,
    metadata: StrictMaybe<PoolMetadata>,
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
            owners: self.owners,
            relays: self.relays,
            metadata: match self.metadata {
                StrictMaybe::Nothing => Nullable::Null,
                StrictMaybe::Just(metadata) => Nullable::Some(metadata),
            },
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
    decode_metadata: impl FnOnce(&mut cbor::Decoder<'_>) -> Result<StrictMaybe<PoolMetadata>, cbor::decode::Error>,
) -> Result<(StrictMaybe<PoolMetadata>, u64, bool), cbor::decode::Error> {
    match len {
        Some(total) if total <= fields_before_metadata => Ok((StrictMaybe::Nothing, fields_before_metadata, false)),
        None if d.datatype()? == cbor::data::Type::Break => {
            d.skip()?;
            Ok((StrictMaybe::Nothing, fields_before_metadata, true))
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

impl<'b> cbor::decode::Decode<'b, NetworkName> for NodePoolParams {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut NetworkName) -> Result<Self, cbor::decode::Error> {
        let len = d.array().map_err(|err| contextualize_decode_error("node pool entry", err))?;

        let vrf = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool vrf", err))?;
        let pledge = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool pledge", err))?;
        let cost = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool cost", err))?;
        let margin = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool margin", err))?;
        let reward_account = {
            let reward_account: NodeRewardAccount =
                d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool reward account", err))?;
            reward_account.0
        };
        let owners = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool owners", err))?;
        let relays = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool relays", err))?;
        let (metadata, consumed, break_consumed) = decode_optional_node_pool_metadata(d, len, 7, |d| {
            d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool metadata", err))
        })?;

        skip_remaining_array_fields(d, len, consumed, break_consumed)
            .map_err(|err| contextualize_decode_error("node pool trailing fields", err))?;

        Ok(NodePoolParams { vrf, pledge, cost, margin, reward_account, owners, relays, metadata })
    }
}

impl<'b> cbor::decode::Decode<'b, NetworkName> for NodePoolUpdateParams {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut NetworkName) -> Result<Self, cbor::decode::Error> {
        let len = d.array().map_err(|err| contextualize_decode_error("node pool update entry", err))?;

        let _operator: PoolId =
            d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update operator", err))?;

        let vrf = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update vrf", err))?;
        let pledge = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update pledge", err))?;
        let cost = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update cost", err))?;
        let margin = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update margin", err))?;
        let reward_account = {
            let reward_account: NodeRewardAccount =
                d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update reward account", err))?;
            reward_account.0
        };
        let owners = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update owners", err))?;
        let relays = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update relays", err))?;
        let (metadata, consumed, break_consumed) = decode_optional_node_pool_metadata(d, len, 8, |d| {
            let metadata: NodePoolUpdateMetadata =
                d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update metadata", err))?;
            Ok(metadata.0)
        })?;

        skip_remaining_array_fields(d, len, consumed, break_consumed)
            .map_err(|err| contextualize_decode_error("node pool update trailing fields", err))?;

        Ok(NodePoolUpdateParams(NodePoolParams { vrf, pledge, cost, margin, reward_account, owners, relays, metadata }))
    }
}

impl<'b> cbor::decode::Decode<'b, NetworkName> for NodePoolStateParams {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut NetworkName) -> Result<Self, cbor::decode::Error> {
        let len = d.array().map_err(|err| contextualize_decode_error("node pool entry", err))?;

        let vrf = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool vrf", err))?;
        let pledge = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool pledge", err))?;
        let cost = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool cost", err))?;
        let margin = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool margin", err))?;
        let reward_account = {
            let reward_account: NodeRewardAccount =
                d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool reward account", err))?;
            reward_account.0
        };
        let owners = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool owners", err))?;
        let relays = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool relays", err))?;
        let (metadata, consumed, _) = decode_optional_node_pool_metadata(d, len, 7, |d| {
            d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool metadata", err))
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

struct NodePoolUpdateMetadata(StrictMaybe<PoolMetadata>);

impl<'b> cbor::decode::Decode<'b, NetworkName> for NodePoolUpdateMetadata {
    #[allow(clippy::wildcard_enum_match_arm)]
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut NetworkName) -> Result<Self, cbor::decode::Error> {
        match d.datatype()? {
            cbor::data::Type::Null => {
                d.skip()?;
                Ok(Self(StrictMaybe::Nothing))
            }
            cbor::data::Type::Array | cbor::data::Type::ArrayIndef => {
                let mut probe = d.probe();
                let len = probe.array()?;
                if len == Some(0) {
                    d.array()?;
                    Ok(Self(StrictMaybe::Nothing))
                } else if matches!(probe.datatype()?, cbor::data::Type::String | cbor::data::Type::StringIndef) {
                    let metadata: PoolMetadata = d.decode_with(ctx)?;
                    Ok(Self(StrictMaybe::Just(metadata)))
                } else {
                    let metadata: StrictMaybe<PoolMetadata> = d.decode_with(ctx)?;
                    Ok(Self(metadata))
                }
            }
            other => Err(cbor::decode::Error::type_mismatch(other)),
        }
    }
}

#[derive(Debug)]
struct NodeAccount {
    rewards: Lovelace,
    deposit: Lovelace,
    pool: Nullable<PoolId>,
    drep: Nullable<DRep>,
}

impl NodeAccount {
    fn into_account(self, pointers: Set<(u64, u64, u64)>) -> Account {
        Account {
            rewards_and_deposit: if self.rewards == 0 && self.deposit == 0 {
                StrictMaybe::Nothing
            } else {
                StrictMaybe::Just((self.rewards, self.deposit))
            },
            pointers,
            pool: match self.pool {
                Nullable::Some(pool) => StrictMaybe::Just(pool),
                Nullable::Null | Nullable::Undefined => StrictMaybe::Nothing,
            },
            drep: match self.drep {
                Nullable::Some(drep) => StrictMaybe::Just(drep),
                Nullable::Null | Nullable::Undefined => StrictMaybe::Nothing,
            },
        }
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for NodeAccount {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;

        Ok(NodeAccount {
            rewards: d.decode_with(ctx)?,
            deposit: d.decode_with(ctx)?,
            pool: d.decode_with(ctx)?,
            drep: d.decode_with(ctx)?,
        })
    }
}

struct NodeRewardAccount(RewardAccount);

impl<'b> cbor::decode::Decode<'b, NetworkName> for NodeRewardAccount {
    #[allow(clippy::wildcard_enum_match_arm)]
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut NetworkName) -> Result<Self, cbor::decode::Error> {
        match d.datatype()? {
            cbor::data::Type::Bytes | cbor::data::Type::BytesIndef => {
                let reward_account: RewardAccount = d.decode_with(ctx)?;
                reward_account_to_stake_credential(&reward_account)
                    .ok_or_else(|| cbor::decode::Error::message("unexpected malformed node reward account bytes"))?;

                Ok(Self(reward_account))
            }
            cbor::data::Type::Array | cbor::data::Type::ArrayIndef => {
                let credential = d.decode_with(ctx)?;
                let network: Network = (*ctx).into();
                let payload = match credential {
                    StakeCredential::AddrKeyhash(hash) => StakePayload::Stake(hash),
                    StakeCredential::ScriptHash(hash) => StakePayload::Script(hash),
                };

                Ok(Self(Bytes::from(new_stake_address(network, payload).to_vec())))
            }
            other => Err(cbor::decode::Error::type_mismatch(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use amaru_kernel::{Bytes, Epoch, Hash, NetworkName, StakeCredential, StrictMaybe, cbor, to_cbor};

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
            decode_optional_node_pool_metadata(&mut decoder, len, 2, |_| Ok(StrictMaybe::Nothing)).unwrap();

        assert!(matches!(metadata, StrictMaybe::Nothing));
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
            decode_optional_node_pool_metadata(&mut decoder, len, 2, |_| Ok(StrictMaybe::Nothing)).unwrap();

        assert!(matches!(metadata, StrictMaybe::Nothing));
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
        let mut network = NetworkName::Mainnet;

        let decoded: NodeRewardAccount = decoder.decode_with(&mut network).unwrap();

        assert_eq!(decoded.0, reward_account);
    }

    #[test]
    fn node_reward_account_credential_decodes_to_snapshot_network_reward_account() {
        let credential = StakeCredential::AddrKeyhash(Hash::new(
            hex::decode("e3af434a5516854f20191807cc5ea85b57b4fd0f050f3eab28af19ee").unwrap().try_into().unwrap(),
        ));
        let bytes = cbor::to_vec(&credential).unwrap();
        let mut decoder = cbor::Decoder::new(bytes.as_slice());
        let mut network = NetworkName::Mainnet;

        let decoded: NodeRewardAccount = decoder.decode_with(&mut network).unwrap();

        assert_eq!(
            decoded.0,
            Bytes::from(hex::decode("e1e3af434a5516854f20191807cc5ea85b57b4fd0f050f3eab28af19ee").unwrap())
        );
    }
}
