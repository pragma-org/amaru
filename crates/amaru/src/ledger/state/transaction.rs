use super::{
    diff_bind::DiffBind, diff_epoch_reg::DiffEpochReg, diff_set::DiffSet,
    volatile_db::VolatileState,
};
use crate::ledger::kernel::{
    output_lovelace, Certificate, Hash, Lovelace, MintedTransactionBody, PoolId, PoolParams, Set,
    StakeCredential, TransactionInput, TransactionOutput, STAKE_CREDENTIAL_DEPOSIT,
};
use std::collections::{BTreeMap, BTreeSet};
use tracing::{debug, info_span, Span};

const TRANSACTION_EVENT_TARGET: &str = "amaru::ledger::state::transaction";

pub fn apply<T>(
    state: &mut VolatileState<T>,
    parent: &Span,
    is_failed: bool,
    transaction_id: Hash<32>,
    mut transaction_body: MintedTransactionBody<'_>,
    resolved_inputs: Vec<TransactionOutput>,
) {
    let span = info_span!(
        target: TRANSACTION_EVENT_TARGET,
        parent: parent,
        "apply.transaction",
        transaction.id = %transaction_id,
        transaction.inputs = tracing::field::Empty,
        transaction.outputs = tracing::field::Empty,
        transaction.certificates = tracing::field::Empty,
    )
    .entered();

    let (inputs, outputs, fees) = if is_failed {
        InputsOutputs::<true>::explode(&mut transaction_body, resolved_inputs)
    } else {
        InputsOutputs::<false>::explode(&mut transaction_body, ())
    };

    // TODO: There should really be an Iterator instance in Pallas
    // on those certificates...
    let certificates = transaction_body
        .certificates
        .map(|xs| xs.to_vec())
        .unwrap_or_default();

    span.record("transaction.inputs", inputs.len());
    span.record("transaction.outputs", outputs.len());
    span.record("transaction.certificates", certificates.len());

    apply_transaction_inputs_outputs(
        &mut state.utxo,
        transaction_id,
        inputs.into_iter().collect(),
        outputs,
    );

    state.fees += fees;

    apply_transaction_certificates(&span, &mut state.pools, &mut state.accounts, certificates);

    span.exit();
}

/// A trait to extract inputs, outputs and fees from a transaction; based on whether it is a failed
/// transaction or not. It is meant to provide a unified interface that makes the parallel between
/// the two cases.
trait InputsOutputs<const IS_FAILED: bool> {
    type ResolvedInputs;

    fn explode(
        body: &mut Self,
        resolved_inputs: Self::ResolvedInputs,
    ) -> (Vec<TransactionInput>, Vec<TransactionOutput>, Lovelace);
}

/// On successful transaction
///   - inputs are consumed;
///   - outputs are produced;
///   - fees are collected;
impl InputsOutputs<false> for MintedTransactionBody<'_> {
    type ResolvedInputs = ();

    fn explode(
        body: &mut Self,
        _resolved_inputs: Self::ResolvedInputs,
    ) -> (Vec<TransactionInput>, Vec<TransactionOutput>, Lovelace) {
        let inputs = core::mem::replace(&mut body.inputs, Set::from(vec![])).to_vec();
        let outputs = core::mem::take(&mut body.outputs)
            .into_iter()
            .map(|x| x.into())
            .collect::<Vec<_>>();

        (inputs, outputs, body.fee)
    }
}

/// On failed transactions:
///   - collateral inputs are consumed;
///   - collateral outputs produced (if any);
///   - the difference between collateral inputs and outputs is collected as fees.
impl InputsOutputs<true> for MintedTransactionBody<'_> {
    type ResolvedInputs = Vec<TransactionOutput>;

    fn explode(
        body: &mut Self,
        resolved_inputs: Self::ResolvedInputs,
    ) -> (Vec<TransactionInput>, Vec<TransactionOutput>, Lovelace) {
        let inputs = core::mem::take(&mut body.collateral)
            .map(|x| x.to_vec())
            .unwrap_or_default();

        let (outputs, collateral_return) = match core::mem::take(&mut body.collateral_return) {
            Some(output) => {
                let output = output.into();
                let collateral_return = output_lovelace(&output);
                (vec![output], collateral_return)
            }
            None => (vec![], 0),
        };

        let collateral_value = resolved_inputs
            .iter()
            .fold(0, |total, output| total + output_lovelace(output));

        let fees = collateral_value - collateral_return;

        (inputs, outputs, fees)
    }
}

fn apply_transaction_inputs_outputs(
    utxo: &mut DiffSet<TransactionInput, TransactionOutput>,
    transaction_id: Hash<32>,
    inputs: BTreeSet<TransactionInput>,
    outputs: Vec<TransactionOutput>,
) {
    let consumed: BTreeSet<_> = inputs;

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

    utxo.merge(DiffSet { consumed, produced });
}

fn apply_transaction_certificates(
    parent: &Span,
    pools: &mut DiffEpochReg<PoolId, PoolParams>,
    accounts: &mut DiffBind<StakeCredential, PoolId, Lovelace>,
    certificates: Vec<Certificate>,
) {
    for certificate in certificates {
        match certificate {
                Certificate::StakeRegistration(credential) | Certificate::Reg(credential, ..) | Certificate::VoteRegDeleg(credential, ..) => {
                    debug!(name: "certificate.stake.registration", target: TRANSACTION_EVENT_TARGET, parent: parent, credential = ?credential);
                        accounts
                        .register(credential, STAKE_CREDENTIAL_DEPOSIT as Lovelace, None);
                }
                Certificate::StakeDelegation(credential, pool)
                // FIXME: register DRep delegation
                | Certificate::StakeVoteDeleg(credential, pool, ..) => {
                    debug!(name: "certificate.stake.delegation", target: TRANSACTION_EVENT_TARGET, parent: parent, credential = ?credential, pool = %pool);
                    accounts.bind(credential, Some(pool));
                }
                Certificate::StakeRegDeleg(credential, pool, ..)
                // FIXME: register DRep delegation
                | Certificate::StakeVoteRegDeleg(credential, pool, ..) => {
                    debug!(name: "certificate.stake.registration", target: TRANSACTION_EVENT_TARGET, parent: parent, credential = ?credential);
                    debug!(name: "certificate.stake.delegation", target: TRANSACTION_EVENT_TARGET, parent: parent, credential = ?credential, pool = %pool);
                    accounts.register(
                        credential,
                        STAKE_CREDENTIAL_DEPOSIT as Lovelace,
                        Some(pool),
                    );
                }
                Certificate::StakeDeregistration(credential)
                | Certificate::UnReg(credential, ..) => {
                    debug!(name: "certificate.stake.deregistration", target: TRANSACTION_EVENT_TARGET, parent: parent, credential = ?credential);
                    accounts.unregister(credential);
                }
                Certificate::PoolRetirement(id, epoch) => {
                    debug!(name: "certificate.pool.retirement", target: TRANSACTION_EVENT_TARGET, parent: parent, pool = %id, epoch = %epoch);
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
                    debug!(
                        name: "certificate.pool.registration",
                        target: TRANSACTION_EVENT_TARGET,
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
