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
    output_lovelace, reward_account_to_stake_credential, Anchor, Certificate, Hash, Lovelace,
    MintedTransactionBody, NonEmptyKeyValuePairs, PoolId, PoolParams, Set, StakeCredential,
    TransactionInput, TransactionOutput, STAKE_CREDENTIAL_DEPOSIT,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    vec,
};
use tracing::{trace, trace_span, Span};

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
    apply_certificates(
        &span,
        &mut state.pools,
        &mut state.accounts,
        &mut state.dreps,
        certificates,
    );

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

/// Flatten complex certificates. A complex certificate is a certificate that is a combination of multiple certificates.
/// Simple certificate are returned as a vec containing only themselves.
fn flatten_certificate(certificate: Certificate) -> Vec<Certificate> {
    match certificate {
        Certificate::StakeVoteDeleg(stake_credential, hash, drep) => {
            vec![
                Certificate::VoteDeleg(stake_credential.clone(), drep),
                Certificate::StakeDelegation(stake_credential, hash),
            ]
        }
        Certificate::StakeRegDeleg(stake_credential, hash, coin) => {
            vec![
                Certificate::Reg(stake_credential.clone(), coin),
                Certificate::StakeDelegation(stake_credential, hash),
            ]
        }
        Certificate::StakeVoteRegDeleg(stake_credential, hash, drep, coin) => {
            vec![
                Certificate::Reg(stake_credential.clone(), coin),
                Certificate::StakeDelegation(stake_credential.clone(), hash),
                Certificate::VoteDeleg(stake_credential, drep),
            ]
        }
        Certificate::VoteRegDeleg(stake_credential, drep, coin) => {
            vec![
                Certificate::Reg(stake_credential.clone(), coin),
                Certificate::VoteDeleg(stake_credential, drep),
            ]
        }
        _ => vec![certificate],
    }
}

fn apply_certificates(
    parent: &Span,
    pools: &mut DiffEpochReg<PoolId, PoolParams>,
    accounts: &mut DiffBind<StakeCredential, PoolId, Lovelace>,
    dreps: &mut DiffBind<StakeCredential, Anchor, Lovelace>,
    certificates: Vec<Certificate>,
) {
    certificates
        .into_iter()
        .flat_map(flatten_certificate)
        .for_each(|certificate| {
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
                    id,vrf,
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
            Certificate::PoolRetirement(id, epoch) => {
                trace!(name: "certificate.pool.retirement", target: EVENT_TARGET, parent: parent, pool = %id, epoch = %epoch);
                pools.unregister(id, epoch)
            },
            Certificate::StakeRegistration(credential)
                | Certificate::Reg(credential, _) => {
                trace!(name: "certificate.stake.registration", target: EVENT_TARGET, parent: parent, credential = ?credential);
                accounts.register(credential, STAKE_CREDENTIAL_DEPOSIT as Lovelace, None);
            },
            Certificate::StakeDeregistration(credential)
                | Certificate::UnReg(credential, _) => {
                trace!(name: "certificate.stake.deregistration", target: EVENT_TARGET, parent: parent, credential = ?credential);
                accounts.unregister(credential);
            },
            Certificate::StakeDelegation(credential, pool) => {
                trace!(name: "certificate.stake.delegation", target: EVENT_TARGET, parent: parent, credential = ?credential, pool = %pool);
                accounts.bind(credential, Some(pool));
            },
            Certificate::RegDRepCert(credential, coin, anchor) => {
                trace!(name: "drep.registration", target: EVENT_TARGET, parent: parent, credential = ?credential, coin = ?coin, anchor = ?anchor);
                dreps.register(credential, coin, anchor.into());
            },
            Certificate::UnRegDRepCert(credential, coin) => {
                trace!(name: "drep.unregistration", target: EVENT_TARGET, parent: parent, credential = ?credential, coin = ?coin);
                dreps.unregister(credential);
            },
            Certificate::UpdateDRepCert(credential, anchor) => {
                trace!(name: "drep.update", target: EVENT_TARGET, parent: parent, credential = ?credential, anchor = ?anchor);
                dreps.bind(credential, anchor.into());
            },
            // Ignore complex type certificates as they have been made useless via `flatten_certificate` 
            Certificate::StakeVoteDeleg{..} | Certificate::StakeRegDeleg{..} | Certificate::StakeVoteRegDeleg{..} | Certificate::VoteRegDeleg{..} => {},
            // FIXME: Process other types of certificates
            _ => {}
        }
    });
}
