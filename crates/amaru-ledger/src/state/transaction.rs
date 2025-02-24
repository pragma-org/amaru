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
    diff_bind::DiffBind, diff_epoch_reg::DiffEpochReg, diff_set::DiffSet,
    volatile_db::VolatileState,
};
use amaru_kernel::{
    Certificate, Hash, Lovelace, MintedTransactionBody, NonEmptyKeyValuePairs, PoolId, PoolParams,
    STAKE_CREDENTIAL_DEPOSIT, Set, StakeCredential, TransactionInput, TransactionOutput,
    output_lovelace, reward_account_to_stake_credential,
};
use std::collections::{BTreeMap, BTreeSet};
use tracing::{Span, trace, trace_span};

const EVENT_TARGET: &str = "amaru::ledger::state::transaction";

pub fn apply(
    state: &mut VolatileState,
    parent: &Span,
    is_failed: bool,
    transaction_id: Hash<32>,
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
        InputsOutputs::<true>::apply(
            &span,
            transaction_id,
            &mut transaction_body,
            resolved_collateral_inputs,
        )
    } else {
        InputsOutputs::<false>::apply(&span, transaction_id, &mut transaction_body, ())
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
    apply_certificates(&span, &mut state.pools, &mut state.accounts, certificates);

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

/// A trait to extract inputs, outputs and fees from a transaction; based on whether it is a failed
/// transaction or not. It is meant to provide a unified interface that makes the parallel between
/// the two cases.
trait InputsOutputs<const IS_FAILED: bool> {
    type ResolvedInputs;

    fn apply(
        span: &Span,
        transaction_id: Hash<32>,
        body: &mut Self,
        resolved_inputs: Self::ResolvedInputs,
    ) -> (DiffSet<TransactionInput, TransactionOutput>, Lovelace);
}

/// On successful transaction
///   - inputs are consumed;
///   - outputs are produced;
///   - fees are collected;
impl InputsOutputs<false> for MintedTransactionBody<'_> {
    type ResolvedInputs = ();

    fn apply(
        span: &Span,
        transaction_id: Hash<32>,
        body: &mut Self,
        _resolved_inputs: Self::ResolvedInputs,
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
}

/// On failed transactions:
///   - collateral inputs are consumed;
///   - collateral outputs produced (if any);
///   - the difference between collateral inputs and outputs is collected as fees.
impl InputsOutputs<true> for MintedTransactionBody<'_> {
    type ResolvedInputs = Vec<TransactionOutput>;

    fn apply(
        span: &Span,
        transaction_id: Hash<32>,
        body: &mut Self,
        resolved_inputs: Self::ResolvedInputs,
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
}

fn apply_certificates(
    parent: &Span,
    pools: &mut DiffEpochReg<PoolId, PoolParams>,
    accounts: &mut DiffBind<StakeCredential, PoolId, Lovelace>,
    certificates: Vec<Certificate>,
) {
    for certificate in certificates {
        match certificate {
                Certificate::StakeRegistration(credential) | Certificate::Reg(credential, ..) | Certificate::VoteRegDeleg(credential, ..) => {
                    trace!(name: "certificate.stake.registration", target: EVENT_TARGET, parent: parent, credential = ?credential);
                    accounts.register(credential, STAKE_CREDENTIAL_DEPOSIT as Lovelace, None);
                }
                Certificate::StakeDelegation(credential, pool)
                // FIXME: register DRep delegation
                | Certificate::StakeVoteDeleg(credential, pool, ..) => {
                    trace!(name: "certificate.stake.delegation", target: EVENT_TARGET, parent: parent, credential = ?credential, pool = %pool);
                    accounts.bind(credential, Some(pool));
                }
                Certificate::StakeRegDeleg(credential, pool, ..)
                // FIXME: register DRep delegation
                | Certificate::StakeVoteRegDeleg(credential, pool, ..) => {
                    trace!(name: "certificate.stake.registration", target: EVENT_TARGET, parent: parent, credential = ?credential);
                    trace!(name: "certificate.stake.delegation", target: EVENT_TARGET, parent: parent, credential = ?credential, pool = %pool);
                    accounts.register(
                        credential,
                        STAKE_CREDENTIAL_DEPOSIT as Lovelace,
                        Some(pool),
                    );
                }
                Certificate::StakeDeregistration(credential)
                | Certificate::UnReg(credential, ..) => {
                    trace!(name: "certificate.stake.deregistration", target: EVENT_TARGET, parent: parent, credential = ?credential);
                    accounts.unregister(credential);
                }
                Certificate::PoolRetirement(id, epoch) => {
                    trace!(name: "certificate.pool.retirement", target: EVENT_TARGET, parent: parent, pool = %id, epoch = %epoch);
                    pools.unregister(id, epoch)
                }
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
                    trace!(
                        name: "certificate.pool.registration",
                        target: EVENT_TARGET,
                        parent: parent,
                        pool = %id,
                        params = ?params,
                    );

                    pools.register(id, params)
                }
                // FIXME: Process other types of certificates
                _ => {}
            }
    }
}
