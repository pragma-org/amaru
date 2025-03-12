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
    output_lovelace, reward_account_to_stake_credential, Anchor, Certificate, CertificatePointer,
    DRep, Hash, Lovelace, MintedTransactionBody, NonEmptyKeyValuePairs, PoolId, PoolParams, Set,
    Slot, StakeCredential, TransactionInput, TransactionOutput, STAKE_CREDENTIAL_DEPOSIT,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    vec,
};
use tracing::{trace, trace_span, Span};

const EVENT_TARGET: &str = "amaru::ledger::state::transaction";

#[allow(clippy::too_many_arguments)]
pub fn apply(
    state: &mut VolatileState,
    parent: &Span,
    is_failed: bool,
    transaction_id: Hash<32>,
    absolute_slot: Slot,
    transaction_index: usize,
    mut transaction_body: MintedTransactionBody<'_>,
    resolved_collateral_inputs: Vec<TransactionOutput>,
) {
    let span = trace_span!(
        target: EVENT_TARGET,
        parent: parent,
        "apply.transaction",
        transaction.id = %transaction_id,
        transaction.inputs = tracing::field::Empty,
        transaction.outputs = tracing::field::Empty,
        transaction.certificates = tracing::field::Empty,
        transaction.withdrawals = tracing::field::Empty,
    )
    .entered();

    let (utxo, fees) = if is_failed {
        apply_io_failed(
            &span,
            transaction_id,
            &mut transaction_body,
            resolved_collateral_inputs,
        )
    } else {
        apply_io(&span, transaction_id, &mut transaction_body)
    };
    state.utxo.merge(utxo);

    state.fees += fees;

    // TODO: There should really be an Iterator instance in Pallas
    // on those certificates...
    let certificates = transaction_body
        .certificates
        .map(|xs| xs.to_vec())
        .unwrap_or_default();
    span.record("transaction.certificates", certificates.len());
    certificates
        .into_iter()
        .enumerate()
        .for_each(|(certificate_index, certificate)| {
            apply_certificate(
                &span,
                &mut state.pools,
                &mut state.accounts,
                &mut state.dreps,
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
    span.record("transaction.withdrawals", withdrawals.len());
    state
        .withdrawals
        .extend(withdrawals.iter().map(|(account, _)| {
            #[allow(clippy::panic)]
            reward_account_to_stake_credential(account).unwrap_or_else(|| {
                panic!("invalid reward account found in transaction ({transaction_id}): {account}")
            })
        }));

    span.exit();
}

/// On successful transaction
///   - inputs are consumed;
///   - outputs are produced;
///   - fees are collected;
pub fn apply_io(
    span: &Span,
    transaction_id: Hash<32>,
    body: &mut MintedTransactionBody<'_>,
) -> (DiffSet<TransactionInput, TransactionOutput>, Lovelace) {
    let consumed = core::mem::replace(&mut body.inputs, Set::from(vec![]))
        .to_vec()
        .into_iter()
        .collect::<BTreeSet<_>>();
    span.record("transaction.inputs", consumed.len());

    let outputs = core::mem::take(&mut body.outputs)
        .into_iter()
        .map(|x| x.into())
        .collect::<Vec<_>>();
    span.record("transaction.outputs", outputs.len());

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

    (DiffSet { consumed, produced }, body.fee)
}

/// On failed transactions:
///   - collateral inputs are consumed;
///   - collateral outputs produced (if any);
///   - the difference between collateral inputs and outputs is collected as fees.
fn apply_io_failed(
    span: &Span,
    transaction_id: Hash<32>,
    body: &mut MintedTransactionBody<'_>,
    resolved_inputs: Vec<TransactionOutput>,
) -> (DiffSet<TransactionInput, TransactionOutput>, Lovelace) {
    let consumed = core::mem::take(&mut body.collateral)
        .map(|x| x.to_vec())
        .unwrap_or_default()
        .into_iter()
        .collect::<BTreeSet<_>>();
    span.record("transaction.inputs", consumed.len());

    let total_collateral = resolved_inputs
        .iter()
        .fold(0, |total, output| total + output_lovelace(output));

    match core::mem::take(&mut body.collateral_return) {
        Some(output) => {
            span.record("transaction.outputs", 1);
            let output = output.into();

            let collateral_return = output_lovelace(&output);

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
            span.record("transaction.outputs", 0);
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
    parent: &Span,
    pools: &mut DiffEpochReg<PoolId, PoolParams>,
    accounts: &mut DiffBind<StakeCredential, PoolId, (DRep, CertificatePointer), Lovelace>,
    dreps: &mut DiffBind<StakeCredential, Anchor, Empty, (Lovelace, CertificatePointer)>,
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
            trace!(target: EVENT_TARGET, parent: parent, pool = %id, params = ?params, "certificate.pool.registration");
            pools.register(id, params)
        }
        Certificate::PoolRetirement(id, epoch) => {
            trace!(target: EVENT_TARGET, parent: parent, pool = %id, epoch = %epoch, "certificate.pool.retirement");
            pools.unregister(id, epoch)
        }
        Certificate::StakeRegistration(credential) => {
            trace!(target: EVENT_TARGET, parent: parent, credential = ?credential, "certificate.stake.registration");
            accounts
                .register(credential, STAKE_CREDENTIAL_DEPOSIT as Lovelace, None, None)
                .unwrap();
        }
        Certificate::Reg(credential, coin) => {
            trace!(target: EVENT_TARGET, parent: parent, credential = ?credential, "certificate.stake.registration");
            accounts.register(credential, coin, None, None).unwrap();
        }
        Certificate::StakeDeregistration(credential) | Certificate::UnReg(credential, _) => {
            trace!(target: EVENT_TARGET, parent: parent, credential = ?credential, "certificate.stake.deregistration");
            accounts.unregister(credential);
        }
        Certificate::StakeDelegation(credential, pool) => {
            trace!(target: EVENT_TARGET, parent: parent, credential = ?credential, pool = %pool, "certificate.stake.delegation");
            accounts.bind_left(credential, Some(pool)).unwrap();
        }
        Certificate::StakeVoteDeleg(credential, pool, drep) => {
            let drep_deleg = Certificate::VoteDeleg(credential.clone(), drep);
            apply_certificate(parent, pools, accounts, dreps, drep_deleg, pointer);
            let pool_deleg = Certificate::StakeDelegation(credential, pool);
            apply_certificate(parent, pools, accounts, dreps, pool_deleg, pointer);
        }
        Certificate::StakeRegDeleg(credential, pool, coin) => {
            let reg = Certificate::Reg(credential.clone(), coin);
            apply_certificate(parent, pools, accounts, dreps, reg, pointer);
            let pool_deleg = Certificate::StakeDelegation(credential, pool);
            apply_certificate(parent, pools, accounts, dreps, pool_deleg, pointer);
        }
        Certificate::StakeVoteRegDeleg(credential, pool, drep, coin) => {
            let reg = Certificate::Reg(credential.clone(), coin);
            apply_certificate(parent, pools, accounts, dreps, reg, pointer);
            let pool_deleg = Certificate::StakeDelegation(credential.clone(), pool);
            apply_certificate(parent, pools, accounts, dreps, pool_deleg, pointer);
            let drep_deleg = Certificate::VoteDeleg(credential, drep);
            apply_certificate(parent, pools, accounts, dreps, drep_deleg, pointer);
        }
        Certificate::VoteRegDeleg(credential, drep, coin) => {
            let reg = Certificate::Reg(credential.clone(), coin);
            apply_certificate(parent, pools, accounts, dreps, reg, pointer);
            let drep_deleg = Certificate::VoteDeleg(credential, drep);
            apply_certificate(parent, pools, accounts, dreps, drep_deleg, pointer);
        }
        Certificate::RegDRepCert(credential, coin, anchor) => {
            trace!(target: EVENT_TARGET, parent: parent, credential = ?credential, coin = ?coin, anchor = ?anchor, "drep.registration");
            dreps
                .register(credential, (coin, pointer), Option::from(anchor), None)
                .unwrap();
        }
        Certificate::UnRegDRepCert(credential, coin) => {
            trace!(target: EVENT_TARGET, parent: parent, credential = ?credential, coin = ?coin, "drep.unregistration");
            dreps.unregister(credential);
        }
        Certificate::UpdateDRepCert(credential, anchor) => {
            trace!(target: EVENT_TARGET, parent: parent, credential = ?credential, anchor = ?anchor, "drep.update");
            dreps.bind_left(credential, Option::from(anchor)).unwrap();
        }
        Certificate::VoteDeleg(credential, drep) => {
            trace!(target: EVENT_TARGET, parent: parent, credential = ?credential, "vote.delegation");
            accounts
                .bind_right(credential, Some((drep, pointer)))
                .unwrap();
        }
        // FIXME: Process other types of certificates
        Certificate::AuthCommitteeHot { .. } | Certificate::ResignCommitteeCold { .. } => {}
    }
}
