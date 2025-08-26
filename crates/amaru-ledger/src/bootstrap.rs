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
    governance::ratification::ProposalsRootsRc,
    state::{diff_bind::Resettable, diff_epoch_reg::DiffEpochReg},
    store::{
        self, GovernanceActivity, Store, StoreError, TransactionalContext, columns::proposals,
    },
};
use amaru_kernel::{
    Account, Anchor, Ballot, BallotId, CertificatePointer, ComparableProposalId, Constitution,
    DRep, DRepRegistration, DRepState, Epoch, EraHistory, Lovelace, MemoizedTransactionOutput,
    Point, PoolId, PoolParams, Proposal, ProposalId, ProposalPointer, ProposalState, Reward,
    ScriptHash, Set, Slot, StakeCredential, StrictMaybe, TransactionInput, TransactionPointer,
    UnitInterval, Vote, Voter, cbor, heterogeneous_array, network::NetworkName,
    protocol_parameters::ProtocolParameters,
};
use amaru_progress_bar::ProgressBar;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, iter,
    path::PathBuf,
    rc::Rc,
    sync::LazyLock,
};
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
    let block_issuers = d.decode()?;

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

    // Committee cold -> hot delegations
    let cc_members = d.decode()?;

    // Dormant Epoch
    let dormant_epoch: Epoch = d.decode()?;
    let governance_activity = GovernanceActivity {
        consecutive_dormant_epochs: u64::from(dormant_epoch) as u32,
    };
    info!(
        dormant_epochs = governance_activity.consecutive_dormant_epochs,
        "governance activity"
    );

    // Epoch State / Ledger State / Cert State / Pool State
    d.array()?;

    let pools = d.decode()?;
    let pools_updates = d.decode()?;
    let pools_retirements = d.decode()?;

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

    let utxo = d
        .decode::<BTreeMap<TransactionInput, MemoizedTransactionOutput>>()?
        .into_iter()
        .collect::<Vec<(TransactionInput, MemoizedTransactionOutput)>>();

    let _deposited: u64 = d.decode()?;

    let fees: i64 = d.decode()?;

    // Epoch State / Ledger State / UTxO State / utxosGovState
    d.array()?;

    // Proposals
    d.array()?;
    d.array()?;
    let root_params = d.decode()?;
    let root_hard_fork = d.decode()?;
    let root_cc = d.decode()?;
    let root_constitution = d.decode()?;

    let proposals: Vec<ProposalState> = d.decode()?;

    // Constitutional committee
    let cc_state = d.decode()?;

    // Constitution
    let constitution = d.decode()?;

    // Current Protocol Params
    let pparams = if let Some(dir) = protocol_parameters_dir {
        decode_seggregated_parameters(dir, d.decode()?)?
    } else {
        d.decode()?
    };
    let protocol_parameters = import_protocol_parameters(db, pparams)?;

    // Previous Protocol Params
    d.skip()?;

    // Future Protocol Params
    d.skip()?;

    // DRep Pulsing State
    d.array()?;

    d.array()?; // Pulsing Snapshot

    let votes = d.decode()?;

    d.skip()?; // DRep distr
    d.skip()?; // DRep state
    d.skip()?; // Pool distr

    d.array()?; // Ratify State

    d.skip()?; // Enact State

    let enacted: Vec<GovActionState> = d.decode()?;
    assert!(
        enacted.is_empty(),
        "unimplemented import scenario: snapshot contains expired governance action: {enacted:?}"
    );

    d.tag()?;
    let expired: Vec<ProposalId> = d.decode()?;
    assert!(
        expired.is_empty(),
        "unimplemented import scenario: snapshot contains expired governance action: {expired:?}"
    );

    let delayed: bool = d.decode()?;
    assert!(
        !delayed,
        "unimplemented import scenario: snapshot contains a ratified delaying governance action"
    );

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
            era_history,
            &protocol_parameters,
            accounts,
            &mut rewards,
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

    import_block_issuers(db, era_history, &protocol_parameters, block_issuers)?;

    import_stake_pools(
        db,
        point,
        era_history,
        &protocol_parameters,
        epoch,
        pools,
        pools_updates,
        pools_retirements,
    )?;

    import_constitution(db, constitution)?;

    import_proposals_roots(db, root_params, root_hard_fork, root_cc, root_constitution)?;

    import_constitutional_committee(
        db,
        point,
        era_history,
        &protocol_parameters,
        cc_state,
        cc_members,
    )?;

    import_dreps(db, point, era_history, &protocol_parameters, epoch, dreps)?;

    import_proposals(db, point, era_history, &protocol_parameters, proposals)?;

    import_votes(db, point, era_history, &protocol_parameters, votes)?;

    import_utxo(
        db,
        &with_progress,
        point,
        era_history,
        &protocol_parameters,
        utxo,
    )?;

    save_point(
        db,
        point,
        era_history,
        &protocol_parameters,
        governance_activity,
    )?;

    Ok(epoch)
}

fn save_point(
    db: &impl Store,
    point: &Point,
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
    mut governance_activity: GovernanceActivity,
) -> Result<(), Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();

    transaction.save(
        era_history,
        protocol_parameters,
        &mut governance_activity,
        point,
        None,
        Default::default(),
        Default::default(),
        iter::empty(),
    )?;

    transaction.set_governance_activity(&governance_activity)?;

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
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
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
                protocol_parameters,
                &mut default_governance_activity(),
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
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
    mut utxo: Vec<(TransactionInput, MemoizedTransactionOutput)>,
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
            era_history,
            protocol_parameters,
            &mut default_governance_activity(),
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
        )?;

        progress.tick(n);
    }

    transaction.commit()?;
    progress.clear();

    Ok(())
}

fn import_dreps<S: Store>(
    db: &S,
    point: &Point,
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
    epoch: Epoch,
    dreps: BTreeMap<StakeCredential, DRepState>,
) -> Result<(), impl std::error::Error + use<S>> {
    let mut known_dreps = BTreeMap::new();

    let era_first_epoch = era_history
        .era_first_epoch(epoch)
        .map_err(|e| StoreError::Internal(Box::new(e)))?;

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
        &mut default_governance_activity(),
        point,
        None,
        store::Columns {
            utxo: iter::empty(),
            pools: iter::empty(),
            accounts: iter::empty(),
            dreps: dreps.into_iter().map(|(credential, state)| {
                let registered_at =
                    known_dreps
                        .remove(&credential)
                        .unwrap_or_else(|| CertificatePointer {
                            transaction: TransactionPointer {
                                slot: point.slot_or_default(),
                                ..TransactionPointer::default()
                            },
                            ..CertificatePointer::default()
                        });

                let registration = DRepRegistration {
                    deposit: state.deposit,
                    valid_until: state.expiry,
                    registered_at,
                };

                (
                    credential,
                    (
                        Resettable::from(Option::from(state.anchor)),
                        Some(registration),
                    ),
                )
            }),
            cc_members: iter::empty(),
            proposals: iter::empty(),
            votes: iter::empty(),
        },
        Default::default(),
        iter::empty(),
    )?;

    transaction.commit()
}

fn import_proposals(
    db: &impl Store,
    point: &Point,
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
    proposals: Vec<ProposalState>,
) -> Result<(), Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();
    transaction.with_proposals(|iterator| {
        for (_, mut handle) in iterator {
            *handle.borrow_mut() = None;
        }
    })?;

    info!(size = proposals.len(), "proposals");

    transaction.save(
        era_history,
        protocol_parameters,
        &mut default_governance_activity(),
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
                        ComparableProposalId::from(proposal.id),
                        proposals::Value {
                            proposed_in: ProposalPointer {
                                transaction: TransactionPointer {
                                    slot: era_history.epoch_bounds(proposal.proposed_in)?.start,
                                    transaction_index: 0,
                                },
                                proposal_index,
                            },
                            valid_until: proposal.proposed_in
                                + protocol_parameters.gov_action_lifetime,
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
    )?;
    transaction.commit()?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn import_stake_pools<S: Store>(
    db: &S,
    point: &Point,
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
    epoch: Epoch,
    pools: BTreeMap<PoolId, PoolParams>,
    updates: BTreeMap<PoolId, PoolParams>,
    retirements: BTreeMap<PoolId, Epoch>,
) -> Result<(), impl std::error::Error + use<S>> {
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
        era_history,
        protocol_parameters,
        &mut default_governance_activity(),
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
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
    accounts: BTreeMap<StakeCredential, Account>,
    rewards_updates: &mut BTreeMap<StakeCredential, Set<Reward>>,
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
            era_history,
            protocol_parameters,
            &mut default_governance_activity(),
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

    let roots = ProposalsRootsRc {
        protocol_parameters: Option::from(protocol_parameters).map(Rc::new),
        hard_fork: Option::from(hard_fork).map(Rc::new),
        constitutional_committee: Option::from(constitutional_committee).map(Rc::new),
        constitution: Option::from(constitution).map(Rc::new),
    };

    info!(
        protocol_parameters = ?roots.protocol_parameters,
        hard_fork = ?roots.hard_fork,
        constitutional_committee = ?roots.constitutional_committee,
        constitution = ?roots.constitution,
        "proposal roots"
    );

    transaction.set_proposals_roots(&roots)?;
    transaction.commit()?;

    Ok(())
}

fn import_constitution(
    db: &impl Store,
    constitution: Constitution,
) -> Result<(), Box<dyn std::error::Error>> {
    let transaction = db.create_transaction();

    info!(
        anchor = constitution.anchor.url,
        guardrails = Option::from(constitution.guardrail_script.clone())
            .map(|s: ScriptHash| s.to_string().chars().take(8).collect())
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
    mut hot_cold_delegations: BTreeMap<StakeCredential, ConstitutionalCommitteeAuthorization>,
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
        &mut default_governance_activity(),
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
                    Some(ConstitutionalCommitteeAuthorization::DelegatedToHotCredential(
                        hot_cred,
                    )) => Resettable::Set(hot_cred),
                    None | Some(ConstitutionalCommitteeAuthorization::Resigned(..)) => {
                        Resettable::Reset
                    }
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
    actions: Vec<GovActionState>,
) -> Result<(), Box<dyn std::error::Error>> {
    let votes = actions
        .into_iter()
        .flat_map(|st| {
            let new_ballot_id = |voter| BallotId {
                proposal: ComparableProposalId::from(st.id.clone()),
                voter,
            };

            let mut votes = Vec::new();

            for (committee, vote) in st.committee_votes.into_iter() {
                let voter = match committee {
                    StakeCredential::AddrKeyhash(hash) => Voter::ConstitutionalCommitteeKey(hash),
                    StakeCredential::ScriptHash(hash) => Voter::ConstitutionalCommitteeScript(hash),
                };

                let ballot = Ballot { vote, anchor: None };

                votes.push((new_ballot_id(voter), ballot));
            }

            for (drep, vote) in st.dreps_votes.into_iter() {
                let voter = match drep {
                    StakeCredential::AddrKeyhash(hash) => Voter::DRepKey(hash),
                    StakeCredential::ScriptHash(hash) => Voter::DRepScript(hash),
                };

                let ballot = Ballot { vote, anchor: None };

                votes.push((new_ballot_id(voter), ballot));
            }

            for (pool_id, vote) in st.pools_votes.into_iter() {
                let voter = Voter::StakePoolKey(pool_id);

                let ballot = Ballot { vote, anchor: None };

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
        &mut default_governance_activity(),
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

// TODO: Move to Pallas
#[derive(Debug)]
#[allow(dead_code)]
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
        heterogeneous_array(d, |d, assert_len| {
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

// TODO: Move to Pallas
#[derive(Debug)]
enum ConstitutionalCommitteeAuthorization {
    DelegatedToHotCredential(StakeCredential),
    Resigned(#[allow(dead_code)] StrictMaybe<Anchor>),
}

impl<'d, C> cbor::decode::Decode<'d, C> for ConstitutionalCommitteeAuthorization {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        heterogeneous_array(d, |d, assert_len| match d.u8()? {
            0 => {
                assert_len(2)?;
                Ok(Self::DelegatedToHotCredential(d.decode_with(ctx)?))
            }
            1 => {
                assert_len(2)?;
                Ok(Self::Resigned(d.decode_with(ctx)?))
            }
            t => Err(cbor::decode::Error::message(format!(
                "unexpected ConstitutionalCommitteeAuthorization kind: {t}; expected 0 or 1."
            ))),
        })
    }
}

// TODO: Move to Pallas
#[derive(Debug)]
struct ConstitutionalCommittee {
    members: BTreeMap<StakeCredential, Epoch>,
    threshold: UnitInterval,
}

impl<'d, C> cbor::decode::Decode<'d, C> for ConstitutionalCommittee {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            Ok(ConstitutionalCommittee {
                members: d.decode_with(ctx)?,
                threshold: d.decode_with(ctx)?,
            })
        })
    }
}

fn default_governance_activity() -> GovernanceActivity {
    GovernanceActivity {
        consecutive_dormant_epochs: 0,
    }
}
