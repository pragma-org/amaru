// Copyright 2025 PRAGMA
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

use std::{fmt, mem, ops::Deref};

use amaru_kernel::{
    AuxiliaryData, EraHistory, HasTransactionId, Network, NetworkId, NetworkName, ProtocolParameters, TransactionBody,
    TransactionInput, TransactionPointer, WitnessSet, cardano::value::Balance,
};
use amaru_observability::debug_span;
use amaru_plutus::arena_pool::ArenaPool;
use thiserror::Error;

use crate::{
    context::ValidationContext, epoch_transition::GovernanceActivity,
    rules::transaction::phase_one::outputs::SupplementalDatumPolicy,
};

pub mod certificates;
pub use certificates::InvalidCertificates;

pub mod fees;
pub use fees::InvalidFees;

pub mod inputs;
pub use inputs::InvalidInputs;

pub mod collateral;
pub use collateral::InvalidCollateral;

pub mod metadata;
pub use metadata::InvalidTransactionMetadata;

pub mod outputs;
pub use outputs::InvalidOutputs;

pub mod proposals;

pub mod vkey_witness;
pub use vkey_witness::InvalidVKeyWitness;

pub mod voting_procedures;

pub mod withdrawals;
pub use withdrawals::InvalidWithdrawals;

pub mod scripts;
pub use scripts::InvalidScripts;

pub mod native_scripts;

pub mod validity_interval;
pub use validity_interval::InvalidValidityInterval;

pub mod mint;

#[cfg(test)]
mod fixture;

#[derive(Debug, Error)]
pub enum PhaseOneError {
    #[error("invalid inputs: {0}")]
    Inputs(#[from] InvalidInputs),

    #[error("invalid outputs: {0}")]
    Outputs(#[from] InvalidOutputs),

    #[error("invalid certificates: {0}")]
    Certificates(#[from] InvalidCertificates),

    #[error("invalid fees: {0}")]
    Fees(#[from] InvalidFees),

    #[error("invalid withdrawals: {0}")]
    Withdrawals(#[from] InvalidWithdrawals),

    #[error("invalid transaction verification key witness: {0}")]
    VKeyWitness(#[from] InvalidVKeyWitness),

    #[error("invalid transaction scripts: {0}")]
    Scripts(#[from] InvalidScripts),

    #[error("invalid collateral: {0}")]
    Collateral(#[from] InvalidCollateral),

    #[error("invalid proposals: {0}")]
    Proposals(#[from] proposals::InvalidProposals),

    #[error("invalid transaction metadata: {0}")]
    Metadata(#[from] InvalidTransactionMetadata),

    #[error("invalid network ID in transaction body: expected {expected:?} provided {provided:?}")]
    InvalidNetworkID { expected: Network, provided: Network },

    #[error("transaction too large: provided {provided} bytes, maximum {maximum} bytes")]
    TooLarge { provided: u64, maximum: u64 },

    #[error("invalid transaction validity interval: {0}")]
    ValidityInterval(#[from] InvalidValidityInterval),

    #[error("value not preserved: balance = {0}")]
    ValueNotPreserved(Balance),
}

#[expect(clippy::too_many_arguments)]
pub fn execute<C>(
    context: &mut C,
    arena_pool: &ArenaPool,
    network_name: NetworkName,
    protocol_parameters: &ProtocolParameters,
    era_history: &EraHistory,
    governance_activity: GovernanceActivity,
    pointer: TransactionPointer,
    is_valid: bool,
    mut transaction_body: TransactionBody,
    transaction_witness_set: &WitnessSet,
    transaction_auxiliary_data: Option<&AuxiliaryData>,
    tx_size: u64,
) -> Result<Vec<TransactionInput>, PhaseOneError>
where
    C: ValidationContext + fmt::Debug,
{
    let transaction_id = transaction_body.tx_id();

    let network: Network = network_name.into();

    fail_on_network_mismatch(transaction_body.network_id, network)?;

    fail_on_tx_size_too_large(tx_size, protocol_parameters)?;

    debug_span!(ledger::rules::phase_one::VALIDITY_INTERVAL).in_scope(|| {
        validity_interval::execute(
            transaction_body.validity_interval(),
            transaction_witness_set.redeemer.is_some(),
            era_history,
            pointer.slot,
        )
    })?;

    debug_span!(ledger::rules::phase_one::METADATA).in_scope(|| {
        metadata::execute(&transaction_body, transaction_auxiliary_data, protocol_parameters.protocol_version)
    })?;

    let ref_scripts_size = debug_span!(ledger::rules::phase_one::INPUTS).in_scope(|| {
        inputs::execute(
            context,
            transaction_body.inputs.deref(),
            transaction_body.reference_inputs.as_deref(),
            protocol_parameters,
        )
    })?;

    let fees = debug_span!(ledger::rules::phase_one::FEES).in_scope(|| {
        fees::execute(
            context,
            transaction_body.fee,
            tx_size,
            transaction_witness_set,
            ref_scripts_size,
            protocol_parameters,
        )
    })?;

    // TODO: The 'collateral' rule group shouldn't exist
    //
    // This is a mix of witness and fees; and instead of duplicating the collateral traversing
    // logic in both, we should augment fees and witness handling to also account for
    // collaterals.
    let collateral = debug_span!(ledger::rules::phase_one::COLLATERAL).in_scope(|| {
        collateral::execute(
            context,
            transaction_body.collateral.as_deref(),
            transaction_body.collateral_return.as_ref(),
            transaction_body.total_collateral,
            transaction_body.fee,
            protocol_parameters,
            transaction_witness_set.redeemer.is_some(),
        )
    })?;

    context.add_fees(if is_valid { fees } else { collateral });

    debug_span!(ledger::rules::phase_one::MINT).in_scope(|| mint::execute(context, transaction_body.mint.as_ref()));

    debug_span!(ledger::rules::phase_one::OUTPUTS).in_scope(|| {
        outputs::execute(
            context,
            protocol_parameters,
            network,
            mem::take(&mut transaction_body.collateral_return).map(|x| vec![x]).unwrap_or_default(),
            SupplementalDatumPolicy::Disallow,
            |_context, _index, _value| {
                if is_valid {
                    return None;
                }

                // NOTE(1): Collateral outputs are indexed based off the number of normal outputs.
                //
                // NOTE(2): We must process collateral before processing normal outputs, or, store
                // the output length elsewhere since after having consumed the outputs, the .len()
                // will always return zero.
                let offset = transaction_body.outputs.len() as u64;
                Some(TransactionInput { transaction_id: *transaction_id.as_ref(), index: offset })
            },
        )?;

        outputs::execute(
            context,
            protocol_parameters,
            network,
            mem::take(&mut transaction_body.outputs),
            SupplementalDatumPolicy::Allow,
            |context, index, value| {
                context.produce_value(value);

                if !is_valid {
                    return None;
                }

                Some(TransactionInput { transaction_id: *transaction_id.as_ref(), index })
            },
        )?;

        Ok::<_, PhaseOneError>(())
    })?;

    debug_span!(ledger::rules::phase_one::WITHDRAWALS).in_scope(|| {
        withdrawals::execute(
            context,
            mem::take(&mut transaction_body.withdrawals).map(|xs| xs.to_vec()),
            network,
            is_valid,
        )
    })?;

    // NOTE: Following validations (and state changes) are entirely skipped on invalid transactions
    //
    // For invalid transactions, we only count the deposits and refunds necessary for value
    // preservation which happens also in the case of invalid transactions.
    if is_valid {
        debug_span!(ledger::rules::phase_one::CERTIFICATES).in_scope(|| {
            certificates::execute(
                context,
                network,
                protocol_parameters,
                era_history,
                governance_activity,
                pointer,
                mem::take(&mut transaction_body.certificates),
            )
        })?;

        debug_span!(ledger::rules::phase_one::PROPOSALS).in_scope(|| {
            proposals::execute(
                context,
                network,
                protocol_parameters,
                era_history,
                (transaction_id, pointer),
                mem::take(&mut transaction_body.proposals).map(|xs| xs.to_vec()),
            )
        })?;

        debug_span!(ledger::rules::phase_one::VOTES)
            .in_scope(|| voting_procedures::execute(context, mem::take(&mut transaction_body.votes)));
    } else {
        debug_span!(ledger::rules::phase_one::CERTIFICATES).in_scope(|| {
            certificates::count_lovelace(context, protocol_parameters, mem::take(&mut transaction_body.certificates))
        });
        debug_span!(ledger::rules::phase_one::PROPOSALS).in_scope(|| {
            proposals::count_lovelace(
                context,
                protocol_parameters,
                mem::take(&mut transaction_body.proposals).map(|xs| xs.to_vec()),
            )
        });
    }

    if let Some(donation) = transaction_body.donation.map(u64::from) {
        debug_span!(ledger::rules::phase_one::DONATION).in_scope(|| {
            context.produce_lovelace(donation);
            context.add_donation(donation);
        });
    }

    debug_span!(ledger::rules::phase_one::SIGNATURES).in_scope(|| {
        for vk_hash in transaction_body.required_signers.as_deref().unwrap_or(&[]) {
            context.require_vkey_witness(*vk_hash);
        }

        vkey_witness::execute(
            context,
            transaction_id,
            transaction_witness_set.bootstrap_witness.as_deref(),
            transaction_witness_set.vkeywitness.as_deref(),
        )
    })?;

    debug_span!(ledger::rules::phase_one::SCRIPTS).in_scope(|| {
        scripts::execute(
            context,
            arena_pool,
            transaction_witness_set,
            transaction_body.validity_interval(),
            protocol_parameters,
            transaction_body.script_data_hash,
        )
    })?;

    let transaction_balance = context.balance();
    if !transaction_balance.is_zero() {
        return Err(PhaseOneError::ValueNotPreserved(transaction_balance));
    }

    // At last, consume inputs
    Ok(if is_valid {
        transaction_body.inputs.to_vec()
    } else {
        transaction_body.collateral.map(|x| x.to_vec()).unwrap_or_default()
    })
}

fn fail_on_tx_size_too_large(provided: u64, protocol_parameters: &ProtocolParameters) -> Result<(), PhaseOneError> {
    let maximum = protocol_parameters.max_transaction_size;
    if provided > maximum {
        return Err(PhaseOneError::TooLarge { provided, maximum });
    }
    Ok(())
}

fn fail_on_network_mismatch(provided: Option<NetworkId>, network: Network) -> Result<(), PhaseOneError> {
    if let Some(network_id) = provided {
        let provided: Network = u8::from(network_id).into();
        if network != provided {
            return Err(PhaseOneError::InvalidNetworkID { expected: network, provided });
        }
    }

    Ok(())
}
#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use amaru_kernel::{EraHistory, ProtocolParameters, Transaction, cbor, json, utils::serde::FilesystemRefResolver};
    use amaru_plutus::arena_pool::ArenaPool;

    use super::fixture::{Expected, Fixture, Predicate};
    use crate::context::DefaultValidationContext;

    // One test case per fixture under tests/data/phase-one/{pass,fail}, generated by build.rs
    include!(concat!(env!("OUT_DIR"), "/phase_one_test_cases.rs"));

    fn run_conformance(fixture_path: &str) {
        let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/phase-one");

        let raw = fs::read_to_string(fixtures_dir.join(format!("{fixture_path}.json")))
            .unwrap_or_else(|e| panic!("cannot read fixture {fixture_path}: {e}"));
        let fixture: Fixture =
            json::from_str(&raw).unwrap_or_else(|e| panic!("invalid json fixture {fixture_path}: {e}"));

        // Fixtures encode a standalone conway transaction (a 4-element array including the is_valid byte)
        // but the ledger expects a transaction to be the 3-element array (without the is_valid byte), so we subtract one byte to
        // match the size used for fee calculation. See the matching note in evaluate_ledger_states.rs.
        let tx_size = (fixture.transaction.len() - 1) as u64;

        let decoded = cbor::decode::<Transaction>(&fixture.transaction);

        if matches!(fixture.expected, Expected::DecodingFailure) {
            assert!(
                decoded.is_err(),
                "expected the transaction to fail to be decoded, but it was successfully decoded"
            );
            return;
        }

        let tx: Transaction = decoded.expect("decode tx");

        let resolver = FilesystemRefResolver::new(fixtures_dir);
        let era_history: EraHistory = fixture.era_history.resolve_into(&resolver).expect("resolve eraHistory");
        let protocol_parameters: ProtocolParameters =
            fixture.protocol_parameters.resolve(&resolver).expect("resolve protocolParameters");

        let mut ctx = DefaultValidationContext::new(
            fixture.initial_state.utxo,
            fixture.initial_state.pools,
            fixture.initial_state.accounts,
            fixture.initial_state.dreps,
            Default::default(),
            Default::default(),
            Default::default(),
        );

        let arena_pool = ArenaPool::new(1, 1024);

        let result = super::execute(
            &mut ctx,
            &arena_pool,
            fixture.network,
            &protocol_parameters,
            &era_history,
            fixture.initial_state.governance_activity,
            fixture.point,
            tx.is_expected_valid,
            tx.body,
            &tx.witnesses,
            tx.auxiliary_data.as_ref(),
            tx_size,
        )
        .map_err(Predicate::from);

        match (fixture.expected, result) {
            (Expected::Pass, Ok(_)) => (),
            (Expected::Pass, Err(actual)) => panic!("expected pass, got error: {actual:?}"),
            (Expected::Fail(expected), Err(actual)) => {
                assert_eq!(actual, expected, "expected {expected:?}, got {actual:?}");
            }
            (Expected::Fail(expected), Ok(_)) => panic!("expected fail ({expected:?}), got pass"),
            (Expected::DecodingFailure, _) => unreachable!("handled before decoding the transaction"),
        }
    }

    #[test]
    #[ignore]
    #[expect(clippy::print_stdout)]
    fn generate_fail_treasury_withdrawal_return_accounts_do_not_exist_0() {
        use amaru_kernel::{Anchor, GovernanceAction, Hash, KeyValuePairs, Nullable, Proposal, to_cbor};

        use super::fixture::tx_builder;

        let deposit: u64 = 100_000_000_000;
        let fee: u64 = 200_000;
        let output_lovelace: u64 = 4_800_000;
        let input_lovelace: u64 = output_lovelace + fee + deposit;

        let address = tx_builder::test_enterprise_address();
        let prev_tx_hash = Hash::<32>::new([0xCC; 32]);
        let input = amaru_kernel::TransactionInput { transaction_id: prev_tx_hash, index: 0 };
        let utxo_output = amaru_kernel::MemoizedTransactionOutput::new(
            false,
            address.clone(),
            amaru_kernel::MemoizedValue::new(amaru_kernel::Value::Coin(input_lovelace)).unwrap(),
            amaru_kernel::MemoizedDatum::None,
            None,
        );
        let tx_output = amaru_kernel::MemoizedTransactionOutput::new(
            false,
            address,
            amaru_kernel::MemoizedValue::new(amaru_kernel::Value::Coin(output_lovelace)).unwrap(),
            amaru_kernel::MemoizedDatum::None,
            None,
        );

        let return_account = tx_builder::test_reward_account(tx_builder::test_key_hash());
        let unregistered_account = tx_builder::test_reward_account(Hash::<28>::new([0xBB; 28]));

        let proposal = Proposal {
            deposit,
            reward_account: return_account,
            gov_action: GovernanceAction::TreasuryWithdrawals(
                KeyValuePairs::try_from(vec![(unregistered_account, 1_000_000u64)]).unwrap().as_pallas(),
                Nullable::Null,
            ),
            anchor: Anchor { url: "https://example.com".to_string(), content_hash: Hash::<32>::new([0; 32]) },
        };

        let proposals = amaru_kernel::NonEmptySet::try_from(vec![proposal]).unwrap();
        let tx_bytes =
            tx_builder::generate_signed_tx_with_proposals(vec![input.clone()], vec![tx_output], fee, Some(proposals));

        let input_hex = hex::encode(to_cbor(&input));
        let output_hex = hex::encode(to_cbor(&utxo_output));
        let credential_hex = hex::encode(to_cbor(&tx_builder::test_credential()));
        let tx_hex = hex::encode(&tx_bytes);

        println!("=== fail/TreasuryWithdrawalReturnAccountsDoNotExist/0.json ===");
        println!("input: {input_hex}");
        println!("output: {output_hex}");
        println!("credential: {credential_hex}");
        println!("transaction: {tx_hex}");
    }

    #[test]
    #[ignore]
    #[expect(clippy::print_stdout)]
    fn generate_fail_treasury_withdrawal_return_accounts_do_not_exist_1() {
        use amaru_kernel::{Anchor, GovernanceAction, Hash, KeyValuePairs, Nullable, Proposal, to_cbor};

        use super::fixture::tx_builder;

        let deposit: u64 = 100_000_000_000;
        let fee: u64 = 200_000;
        let output_lovelace: u64 = 4_800_000;
        let input_lovelace: u64 = output_lovelace + fee + deposit;

        let address = tx_builder::test_enterprise_address();
        let prev_tx_hash = Hash::<32>::new([0xCC; 32]);
        let input = amaru_kernel::TransactionInput { transaction_id: prev_tx_hash, index: 0 };
        let utxo_output = amaru_kernel::MemoizedTransactionOutput::new(
            false,
            address.clone(),
            amaru_kernel::MemoizedValue::new(amaru_kernel::Value::Coin(input_lovelace)).unwrap(),
            amaru_kernel::MemoizedDatum::None,
            None,
        );
        let tx_output = amaru_kernel::MemoizedTransactionOutput::new(
            false,
            address,
            amaru_kernel::MemoizedValue::new(amaru_kernel::Value::Coin(output_lovelace)).unwrap(),
            amaru_kernel::MemoizedDatum::None,
            None,
        );

        let return_account = tx_builder::test_reward_account(tx_builder::test_key_hash());
        let unregistered_account_1 = tx_builder::test_reward_account(Hash::<28>::new([0xBB; 28]));
        let unregistered_account_2 = tx_builder::test_reward_account(Hash::<28>::new([0xAA; 28]));

        let proposal = Proposal {
            deposit,
            reward_account: return_account,
            gov_action: GovernanceAction::TreasuryWithdrawals(
                KeyValuePairs::try_from(vec![
                    (unregistered_account_1, 1_000_000u64),
                    (unregistered_account_2, 2_000_000u64),
                ])
                .unwrap()
                .as_pallas(),
                Nullable::Null,
            ),
            anchor: Anchor { url: "https://example.com".to_string(), content_hash: Hash::<32>::new([0; 32]) },
        };

        let proposals = amaru_kernel::NonEmptySet::try_from(vec![proposal]).unwrap();
        let tx_bytes =
            tx_builder::generate_signed_tx_with_proposals(vec![input.clone()], vec![tx_output], fee, Some(proposals));

        let input_hex = hex::encode(to_cbor(&input));
        let output_hex = hex::encode(to_cbor(&utxo_output));
        let tx_hex = hex::encode(&tx_bytes);

        println!("=== fail/TreasuryWithdrawalReturnAccountsDoNotExist/1.json ===");
        println!("input: {input_hex}");
        println!("output: {output_hex}");
        println!("transaction: {tx_hex}");
    }

    #[test]
    #[ignore]
    #[expect(clippy::print_stdout)]
    fn generate_fail_treasury_withdrawal_return_accounts_do_not_exist_2() {
        use amaru_kernel::{Anchor, GovernanceAction, Hash, KeyValuePairs, Nullable, Proposal, to_cbor};

        use super::fixture::tx_builder;

        let deposit: u64 = 100_000_000_000;
        let fee: u64 = 200_000;
        let output_lovelace: u64 = 4_800_000;
        let input_lovelace: u64 = output_lovelace + fee + deposit;

        let address = tx_builder::test_enterprise_address();
        let prev_tx_hash = Hash::<32>::new([0xCC; 32]);
        let input = amaru_kernel::TransactionInput { transaction_id: prev_tx_hash, index: 0 };
        let utxo_output = amaru_kernel::MemoizedTransactionOutput::new(
            false,
            address.clone(),
            amaru_kernel::MemoizedValue::new(amaru_kernel::Value::Coin(input_lovelace)).unwrap(),
            amaru_kernel::MemoizedDatum::None,
            None,
        );
        let tx_output = amaru_kernel::MemoizedTransactionOutput::new(
            false,
            address,
            amaru_kernel::MemoizedValue::new(amaru_kernel::Value::Coin(output_lovelace)).unwrap(),
            amaru_kernel::MemoizedDatum::None,
            None,
        );

        let return_account = tx_builder::test_reward_account(tx_builder::test_key_hash());
        let registered_account = tx_builder::test_reward_account(Hash::<28>::new([0xDD; 28]));
        let unregistered_account = tx_builder::test_reward_account(Hash::<28>::new([0xBB; 28]));

        let proposal = Proposal {
            deposit,
            reward_account: return_account,
            gov_action: GovernanceAction::TreasuryWithdrawals(
                KeyValuePairs::try_from(vec![
                    (registered_account.clone(), 1_000_000u64),
                    (unregistered_account, 2_000_000u64),
                ])
                .unwrap()
                .as_pallas(),
                Nullable::Null,
            ),
            anchor: Anchor { url: "https://example.com".to_string(), content_hash: Hash::<32>::new([0; 32]) },
        };

        let proposals = amaru_kernel::NonEmptySet::try_from(vec![proposal]).unwrap();
        let tx_bytes =
            tx_builder::generate_signed_tx_with_proposals(vec![input.clone()], vec![tx_output], fee, Some(proposals));

        let input_hex = hex::encode(to_cbor(&input));
        let output_hex = hex::encode(to_cbor(&utxo_output));
        let registered_credential_hex =
            hex::encode(to_cbor(&amaru_kernel::StakeCredential::AddrKeyhash(Hash::<28>::new([0xDD; 28]))));
        let registered_account_hex = hex::encode(&registered_account[..]);
        let tx_hex = hex::encode(&tx_bytes);

        println!("=== fail/TreasuryWithdrawalReturnAccountsDoNotExist/2.json ===");
        println!("input: {input_hex}");
        println!("output: {output_hex}");
        println!("registered_credential: {registered_credential_hex}");
        println!("registered_account: {registered_account_hex}");
        println!("transaction: {tx_hex}");
    }

    #[test]
    #[ignore]
    #[expect(clippy::print_stdout)]
    fn generate_stake_delegation_fixture() {
        use amaru_kernel::{Certificate, Hash};

        use super::fixture::tx_builder;

        let pool = Hash::<28>::new([0xDD; 28]);
        let cred = tx_builder::test_credential();

        let (input_hex, output_hex, credential_hex, tx_hex) = tx_builder::generate_fixture_data(
            5_000_000,
            4_800_000,
            200_000,
            vec![Certificate::StakeDelegation(cred, pool)],
        );

        println!("=== pass/stake-delegation.json ===");
        println!("input: {input_hex}");
        println!("output: {output_hex}");
        println!("credential: {credential_hex}");
        println!("transaction: {tx_hex}");
    }

    #[test]
    #[ignore]
    #[expect(clippy::print_stdout)]
    fn generate_stake_dereg_rereg_deleg_fixture() {
        use amaru_kernel::{Certificate, Hash};

        use super::fixture::tx_builder;

        let pool = Hash::<28>::new([0xDD; 28]);
        let cred = tx_builder::test_credential();

        let (input_hex, output_hex, credential_hex, tx_hex) = tx_builder::generate_fixture_data(
            5_000_000,
            4_800_000,
            200_000,
            vec![
                Certificate::StakeDeregistration(cred.clone()),
                Certificate::Reg(cred.clone(), 2_000_000),
                Certificate::StakeDelegation(cred, pool),
            ],
        );

        println!("=== pass/stake-dereg-rereg-deleg.json ===");
        println!("input: {input_hex}");
        println!("output: {output_hex}");
        println!("credential: {credential_hex}");
        println!("transaction: {tx_hex}");
    }

    #[test]
    #[ignore]
    #[expect(clippy::print_stdout)]
    fn generate_stake_vote_deleg_fixture() {
        use amaru_kernel::{Certificate, DRep, Hash};

        use super::fixture::tx_builder;

        let pool = Hash::<28>::new([0xDD; 28]);
        let cred = tx_builder::test_credential();

        let (input_hex, output_hex, credential_hex, tx_hex) = tx_builder::generate_fixture_data(
            5_000_000,
            4_800_000,
            200_000,
            vec![Certificate::StakeVoteDeleg(cred, pool, DRep::Abstain)],
        );

        println!("=== pass/stake-vote-deleg.json ===");
        println!("input: {input_hex}");
        println!("output: {output_hex}");
        println!("credential: {credential_hex}");
        println!("transaction: {tx_hex}");
    }

    #[test]
    #[ignore]
    #[expect(clippy::print_stdout)]
    fn generate_stake_reg_deleg_fixture() {
        use amaru_kernel::{Certificate, Hash};

        use super::fixture::tx_builder;

        let pool = Hash::<28>::new([0xDD; 28]);
        let cred = tx_builder::test_credential();

        let (input_hex, output_hex, credential_hex, tx_hex) = tx_builder::generate_fixture_data(
            5_000_000,
            2_800_000,
            200_000,
            vec![Certificate::StakeRegDeleg(cred, pool, 2_000_000)],
        );

        println!("=== pass/stake-reg-deleg.json ===");
        println!("input: {input_hex}");
        println!("output: {output_hex}");
        println!("credential: {credential_hex}");
        println!("transaction: {tx_hex}");
    }

    #[test]
    #[ignore]
    #[expect(clippy::print_stdout)]
    fn generate_stake_vote_reg_deleg_fixture() {
        use amaru_kernel::{Certificate, DRep, Hash};

        use super::fixture::tx_builder;

        let pool = Hash::<28>::new([0xDD; 28]);
        let cred = tx_builder::test_credential();

        let (input_hex, output_hex, credential_hex, tx_hex) = tx_builder::generate_fixture_data(
            5_000_000,
            2_800_000,
            200_000,
            vec![Certificate::StakeVoteRegDeleg(cred, pool, DRep::Abstain, 2_000_000)],
        );

        println!("=== pass/stake-vote-reg-deleg.json ===");
        println!("input: {input_hex}");
        println!("output: {output_hex}");
        println!("credential: {credential_hex}");
        println!("transaction: {tx_hex}");
    }

    #[test]
    #[ignore]
    #[expect(clippy::print_stdout)]
    fn generate_fail_stake_key_registered_deleg_0() {
        use amaru_kernel::Certificate;

        use super::fixture::tx_builder;

        let cred = tx_builder::test_credential();

        let (input_hex, output_hex, credential_hex, tx_hex) =
            tx_builder::generate_fixture_data(5_000_000, 2_800_000, 200_000, vec![Certificate::Reg(cred, 2_000_000)]);

        println!("=== fail/StakeKeyRegisteredDELEG/0.json ===");
        println!("input: {input_hex}");
        println!("output: {output_hex}");
        println!("credential: {credential_hex}");
        println!("transaction: {tx_hex}");
    }

    #[test]
    #[ignore]
    #[expect(clippy::print_stdout)]
    fn generate_fail_stake_key_registered_deleg_1() {
        use amaru_kernel::{Certificate, Hash};

        use super::fixture::tx_builder;

        let pool = Hash::<28>::new([0xDD; 28]);
        let cred = tx_builder::test_credential();

        let (input_hex, output_hex, credential_hex, tx_hex) = tx_builder::generate_fixture_data(
            5_000_000,
            2_800_000,
            200_000,
            vec![Certificate::StakeRegDeleg(cred, pool, 2_000_000)],
        );

        println!("=== fail/StakeKeyRegisteredDELEG/1.json ===");
        println!("input: {input_hex}");
        println!("output: {output_hex}");
        println!("credential: {credential_hex}");
        println!("transaction: {tx_hex}");
    }

    #[test]
    #[ignore]
    #[expect(clippy::print_stdout)]
    fn generate_fail_stake_key_registered_deleg_2() {
        use amaru_kernel::{Certificate, DRep};

        use super::fixture::tx_builder;

        let cred = tx_builder::test_credential();

        let (input_hex, output_hex, credential_hex, tx_hex) = tx_builder::generate_fixture_data(
            5_000_000,
            2_800_000,
            200_000,
            vec![Certificate::VoteRegDeleg(cred, DRep::Abstain, 2_000_000)],
        );

        println!("=== fail/StakeKeyRegisteredDELEG/2.json ===");
        println!("input: {input_hex}");
        println!("output: {output_hex}");
        println!("credential: {credential_hex}");
        println!("transaction: {tx_hex}");
    }

    #[test]
    #[ignore]
    #[expect(clippy::print_stdout)]
    fn generate_fail_stake_key_registered_deleg_3() {
        use amaru_kernel::{Certificate, DRep, Hash};

        use super::fixture::tx_builder;

        let pool = Hash::<28>::new([0xDD; 28]);
        let cred = tx_builder::test_credential();

        let (input_hex, output_hex, credential_hex, tx_hex) = tx_builder::generate_fixture_data(
            5_000_000,
            2_800_000,
            200_000,
            vec![Certificate::StakeVoteRegDeleg(cred, pool, DRep::Abstain, 2_000_000)],
        );

        println!("=== fail/StakeKeyRegisteredDELEG/3.json ===");
        println!("input: {input_hex}");
        println!("output: {output_hex}");
        println!("credential: {credential_hex}");
        println!("transaction: {tx_hex}");
    }

    #[test]
    #[ignore]
    #[expect(clippy::print_stdout)]
    fn generate_fail_delegatee_pool_not_registered_0() {
        use amaru_kernel::{Certificate, Hash};

        use super::fixture::tx_builder;

        let pool = Hash::<28>::new(*b"\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef");
        let cred = tx_builder::test_credential();

        let (input_hex, output_hex, credential_hex, tx_hex) = tx_builder::generate_fixture_data(
            5_000_000,
            4_800_000,
            200_000,
            vec![Certificate::StakeDelegation(cred, pool)],
        );

        println!("=== fail/DelegateeStakePoolNotRegistered/0.json ===");
        println!("input: {input_hex}");
        println!("output: {output_hex}");
        println!("credential: {credential_hex}");
        println!("transaction: {tx_hex}");
    }

    #[test]
    #[ignore]
    #[expect(clippy::print_stdout)]
    fn generate_fail_delegatee_pool_not_registered_1() {
        use amaru_kernel::{Certificate, DRep, Hash};

        use super::fixture::tx_builder;

        let pool = Hash::<28>::new(*b"\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef");
        let cred = tx_builder::test_credential();

        let (input_hex, output_hex, credential_hex, tx_hex) = tx_builder::generate_fixture_data(
            5_000_000,
            4_800_000,
            200_000,
            vec![Certificate::StakeVoteDeleg(cred, pool, DRep::Abstain)],
        );

        println!("=== fail/DelegateeStakePoolNotRegistered/1.json ===");
        println!("input: {input_hex}");
        println!("output: {output_hex}");
        println!("credential: {credential_hex}");
        println!("transaction: {tx_hex}");
    }

    #[test]
    #[ignore]
    #[expect(clippy::print_stdout)]
    fn generate_fail_delegatee_pool_not_registered_2() {
        use amaru_kernel::{Certificate, Hash};

        use super::fixture::tx_builder;

        let pool = Hash::<28>::new(*b"\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef");
        let cred = tx_builder::test_credential();

        let (input_hex, output_hex, credential_hex, tx_hex) = tx_builder::generate_fixture_data(
            5_000_000,
            2_800_000,
            200_000,
            vec![Certificate::StakeRegDeleg(cred, pool, 2_000_000)],
        );

        println!("=== fail/DelegateeStakePoolNotRegistered/2.json ===");
        println!("input: {input_hex}");
        println!("output: {output_hex}");
        println!("credential: {credential_hex}");
        println!("transaction: {tx_hex}");
    }

    #[test]
    #[ignore]
    #[expect(clippy::print_stdout)]
    fn generate_fail_delegatee_pool_not_registered_3() {
        use amaru_kernel::{Certificate, DRep, Hash};

        use super::fixture::tx_builder;

        let pool = Hash::<28>::new(*b"\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef");
        let cred = tx_builder::test_credential();

        let (input_hex, output_hex, credential_hex, tx_hex) = tx_builder::generate_fixture_data(
            5_000_000,
            2_800_000,
            200_000,
            vec![Certificate::StakeVoteRegDeleg(cred, pool, DRep::Abstain, 2_000_000)],
        );

        println!("=== fail/DelegateeStakePoolNotRegistered/3.json ===");
        println!("input: {input_hex}");
        println!("output: {output_hex}");
        println!("credential: {credential_hex}");
        println!("transaction: {tx_hex}");
    }
}