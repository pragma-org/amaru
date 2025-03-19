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

use super::{
    diff_bind::{DiffBind, Empty},
    diff_epoch_reg::DiffEpochReg,
    diff_set::DiffSet,
    volatile_db::VolatileState,
};
use amaru_kernel::{
    reward_account_to_stake_credential, Anchor, Certificate, CertificatePointer, DRep, Epoch,
    HasLovelace, Hash, Lovelace, MintedTransactionBody, NonEmptyKeyValuePairs, PoolId, PoolParams,
    Set, Slot, StakeCredential, TransactionInput, TransactionOutput, STAKE_CREDENTIAL_DEPOSIT,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    vec,
};
use tracing::{instrument, trace, Level, Span};

#[allow(clippy::too_many_arguments)]
#[instrument(level = Level::TRACE, name = "transaction", skip_all, fields(transaction.id= %transaction_id, transaction.inputs, transaction.outputs, transaction.certificates))]
pub fn apply(
    state: &mut VolatileState,
    is_failed: bool,
    transaction_id: Hash<32>,
    absolute_slot: Slot,
    transaction_index: usize,
    mut transaction_body: MintedTransactionBody<'_>,
    resolved_collateral_inputs: Vec<TransactionOutput>,
) {
    let (utxo, fees) = if is_failed {
        let consumed = core::mem::take(&mut transaction_body.collateral)
            .map(|x| x.to_vec())
            .unwrap_or_default()
            .into_iter()
            .collect::<BTreeSet<_>>();
        apply_io_failed(
            transaction_id,
            &mut transaction_body,
            resolved_collateral_inputs,
            consumed,
        )
    } else {
        let consumed = core::mem::replace(&mut transaction_body.inputs, Set::from(vec![]))
            .to_vec()
            .into_iter()
            .collect::<BTreeSet<_>>();

        let outputs = core::mem::take(&mut transaction_body.outputs)
            .into_iter()
            .map(|x| x.into())
            .collect::<Vec<TransactionOutput>>();
        apply_io(transaction_id, &mut transaction_body, consumed, outputs)
    };
    state.utxo.merge(utxo);

    state.fees += fees;

    // TODO: There should really be an Iterator instance in Pallas
    // on those certificates...
    let certificates = transaction_body
        .certificates
        .map(|xs| xs.to_vec())
        .unwrap_or_default();

    Span::current().record("transaction.certificates", certificates.len());

    certificates
        .into_iter()
        .enumerate()
        .for_each(|(certificate_index, certificate)| {
            apply_certificate(
                &mut state.pools,
                &mut state.accounts,
                &mut state.dreps,
                &mut state.committee,
                certificate,
                CertificatePointer {
                    slot: absolute_slot,
                    transaction_index,
                    certificate_index,
                },
            )
        });

    let withdrawals = transaction_body
        .withdrawals
        .unwrap_or_else(|| NonEmptyKeyValuePairs::Def(vec![]));
    state
        .withdrawals
        .extend(withdrawals.iter().map(|(account, _)| {
            #[allow(clippy::panic)]
            reward_account_to_stake_credential(account).unwrap_or_else(|| {
                panic!("invalid reward account found in transaction ({transaction_id}): {account}")
            })
        }));

    Span::current().record("transaction.withdrawals", withdrawals.len());
}

/// On successful transaction
///   - inputs are consumed;
///   - outputs are produced;
///   - fees are collected;
pub fn apply_io(
    transaction_id: Hash<32>,
    body: &mut MintedTransactionBody<'_>,
    consumed: BTreeSet<TransactionInput>,
    outputs: Vec<TransactionOutput>,
) -> (DiffSet<TransactionInput, TransactionOutput>, Lovelace) {
    let produced = outputs
        .into_iter()
        .enumerate()
        .map(|(index, output)| {
            (
                TransactionInput {
                    transaction_id,
                    index: index as u64,
                },
                output,
            )
        })
        .collect::<BTreeMap<_, _>>();

    Span::current().record("transaction.inputs", consumed.len());
    Span::current().record("transaction.outputs", produced.len());

    (DiffSet { consumed, produced }, body.fee)
}

/// On failed transactions:
///   - collateral inputs are consumed;
///   - collateral outputs produced (if any);
///   - the difference between collateral inputs and outputs is collected as fees.
fn apply_io_failed(
    transaction_id: Hash<32>,
    body: &mut MintedTransactionBody<'_>,
    resolved_inputs: Vec<TransactionOutput>,
    consumed: BTreeSet<TransactionInput>,
) -> (DiffSet<TransactionInput, TransactionOutput>, Lovelace) {
    Span::current().record("transaction.inputs", consumed.len());

    let total_collateral = resolved_inputs
        .iter()
        .fold(0, |total, output| total + output.lovelace());

    match core::mem::take(&mut body.collateral_return) {
        Some(output) => {
            Span::current().record("transaction.outputs", 1);
            let output = TransactionOutput::from(output);

            let collateral_return = output.lovelace();

            let fees = total_collateral - collateral_return;

            let mut produced = BTreeMap::new();
            produced.insert(
                TransactionInput {
                    transaction_id,
                    // NOTE: Yes, you read that right. The index associated to collateral
                    // outputs is the length of non-collateral outputs. So if a transaction has
                    // two outputs, its (only) collateral output is accessible at index `1`,
                    // and there's no collateral output at index `0` whatsoever.
                    index: body.outputs.len() as u64,
                },
                output,
            );

            (DiffSet { consumed, produced }, fees)
        }
        None => {
            Span::current().record("transaction.outputs", 0);
            (
                DiffSet {
                    consumed,
                    produced: BTreeMap::default(),
                },
                total_collateral,
            )
        }
    }
}

#[allow(clippy::unwrap_used)]
fn apply_certificate(
    pools: &mut DiffEpochReg<PoolId, PoolParams>,
    accounts: &mut DiffBind<StakeCredential, PoolId, (DRep, CertificatePointer), Lovelace>,
    dreps: &mut DiffBind<StakeCredential, Anchor, Empty, (Lovelace, CertificatePointer)>,
    committee: &mut DiffBind<StakeCredential, StakeCredential, Empty, Epoch>,
    certificate: Certificate,
    pointer: CertificatePointer,
) {
    match certificate {
        Certificate::PoolRegistration {
            operator: id,
            vrf_keyhash: vrf,
            pledge,
            cost,
            margin,
            reward_account,
            pool_owners: owners,
            relays,
            pool_metadata: metadata,
        } => {
            let params = PoolParams {
                id,
                vrf,
                pledge,
                cost,
                margin,
                reward_account,
                owners,
                relays,
                metadata,
            };
            trace!(pool = %id, params = ?params, "certificate.pool.registration");
            pools.register(id, params)
        }
        Certificate::PoolRetirement(id, epoch) => {
            trace!(pool = %id, epoch = %epoch, "certificate.pool.retirement");
            pools.unregister(id, epoch)
        }
        Certificate::StakeRegistration(credential) => {
            trace!(credential = ?credential, "certificate.stake.registration");
            accounts
                .register(credential, STAKE_CREDENTIAL_DEPOSIT as Lovelace, None, None)
                .unwrap();
        }
        Certificate::Reg(credential, coin) => {
            trace!(credential = ?credential, "certificate.stake.registration");
            accounts.register(credential, coin, None, None).unwrap();
        }
        Certificate::StakeDeregistration(credential) | Certificate::UnReg(credential, _) => {
            trace!(credential = ?credential, "certificate.stake.deregistration");
            accounts.unregister(credential);
        }
        Certificate::StakeDelegation(credential, pool) => {
            trace!(credential = ?credential, pool = %pool, "certificate.stake.delegation");
            accounts.bind_left(credential, Some(pool)).unwrap();
        }
        Certificate::StakeVoteDeleg(credential, pool, drep) => {
            let drep_deleg = Certificate::VoteDeleg(credential.clone(), drep);
            apply_certificate(
                pools, accounts, dreps, committee, drep_deleg, pointer,
            );
            let pool_deleg = Certificate::StakeDelegation(credential, pool);
            apply_certificate(
                pools, accounts, dreps, committee, pool_deleg, pointer,
            );
        }
        Certificate::StakeRegDeleg(credential, pool, coin) => {
            let reg = Certificate::Reg(credential.clone(), coin);
            apply_certificate(pools, accounts, dreps, committee, reg, pointer);
            let pool_deleg = Certificate::StakeDelegation(credential, pool);
            apply_certificate(
                pools, accounts, dreps, committee, pool_deleg, pointer,
            );
        }
        Certificate::StakeVoteRegDeleg(credential, pool, drep, coin) => {
            let reg = Certificate::Reg(credential.clone(), coin);
            apply_certificate(pools, accounts, dreps, committee, reg, pointer);
            let pool_deleg = Certificate::StakeDelegation(credential.clone(), pool);
            apply_certificate(
                pools, accounts, dreps, committee, pool_deleg, pointer,
            );
            let drep_deleg = Certificate::VoteDeleg(credential, drep);
            apply_certificate(
                pools, accounts, dreps, committee, drep_deleg, pointer,
            );
        }
        Certificate::VoteRegDeleg(credential, drep, coin) => {
            let reg = Certificate::Reg(credential.clone(), coin);
            apply_certificate(pools, accounts, dreps, committee, reg, pointer);
            let drep_deleg = Certificate::VoteDeleg(credential, drep);
            apply_certificate(
                pools, accounts, dreps, committee, drep_deleg, pointer,
            );
        }
        Certificate::RegDRepCert(drep, deposit, anchor) => {
            trace!(drep = ?drep, deposit = ?deposit, anchor = ?anchor, "certificate.drep.registration");
            dreps
                .register(drep, (deposit, pointer), Option::from(anchor), None)
                .unwrap();
        }
        Certificate::UnRegDRepCert(drep, refund) => {
            trace!(drep = ?drep, refund = ?refund, "certificate.drep.retirement");
            dreps.unregister(drep);
        }
        Certificate::UpdateDRepCert(drep, anchor) => {
            trace!(drep = ?drep, anchor = ?anchor, "certificate.drep.update");
            dreps.bind_left(drep, Option::from(anchor)).unwrap();
        }
        Certificate::VoteDeleg(credential, drep) => {
            trace!(credential = ?credential, drep = ?drep, "certificate.vote.delegation");
            accounts
                .bind_right(credential, Some((drep, pointer)))
                .unwrap();
        }
        Certificate::AuthCommitteeHot(cold_credential, hot_credential) => {
            trace!(name: "committee.hot_key", target: EVENT_TARGET, parent: parent, cold_credential = ?cold_credential, hot_credential = ?hot_credential);
            committee
                .bind_left(cold_credential, Some(hot_credential))
                .unwrap();
        }
        Certificate::ResignCommitteeCold(cold_credential, anchor) => {
            trace!(name: "committee.resign_cold_key", target: EVENT_TARGET, parent: parent, cold_credential = ?cold_credential, anchor = ?anchor);
            committee.bind_left(cold_credential, None).unwrap();
        }
    }
}
