// Copyright 2026 PRAGMA
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

use std::{collections::BTreeSet, path::Path};

use amaru_kernel::{
    Address, Anchor, AssetName, AuxiliaryData, Block, BlockHeight, BootstrapWitness, Bytes, Certificate, Certificates,
    Constitution, CostModels, Credential, DRep, DRepVotingThresholds, Ed25519Signature, Epoch, ExUnitPrices, ExUnits,
    GovernanceAction, Hash, Header, HeaderBody, InlineData, KesPeriod, Lovelace, MajorProtocolVersion, MaxString128,
    MemoizedDatum, MemoizedPlutusData, MemoizedScript, MemoizedTransactionOutput, Metadatum, Mint, Multiasset,
    NativeScript, Network, NonZeroInt, OperationalCert, PlutusData, PlutusScript, PlutusVersion, PoolId, PoolMetadata,
    PoolVotingThresholds, PositiveCoin, Proposal, ProposalId, Proposals, ProtocolParamUpdate, ProtocolVersion,
    RationalNumber, Redeemer, Redeemers, Relay, RewardAccount, Slot, Transaction, TransactionBody, TransactionId,
    TransactionIndex, TransactionInput, UnitRationalNumber, Value, VerificationKey, VerificationKeyWitness, Vote,
    Voter, VotingProcedure, WitnessSet,
    cardano::{
        plutus_data::{BigInt, Constr},
        relay::{IPv4, IPv6},
        voting_procedures::VotingProcedures,
        withdrawals::Withdrawals,
    },
    hash::size,
};

use crate::{RoundTrip, TestConfiguration, TestResults, read_directory, round_trip, round_trip_without_context};

/// List of rules the corpus carries, and the round-trip function to use for each.
/// The round-trip function decodes a sample and re-encodes it, returning the re-encoded bytes.
/// The suite normalizes and compares those bytes against the expected bytes in the corpus.
pub const RULES: &[(&str, RoundTrip)] = &[
    ("%constr<plutus_data>", round_trip::<Constr<PlutusData>>),
    ("%multiasset<positive_coin>", round_trip::<Multiasset<PositiveCoin>>),
    ("addr_keyhash", round_trip::<Hash<{ size::KEY }>>),
    ("address", round_trip::<Address>),
    ("alonzo_transaction_output", round_trip::<MemoizedTransactionOutput>),
    ("anchor", round_trip::<Anchor>),
    ("asset_name", round_trip::<AssetName>),
    ("auxiliary_data", round_trip::<AuxiliaryData>),
    ("auxiliary_data_array", round_trip::<AuxiliaryData>),
    ("auxiliary_data_hash", round_trip::<Hash<{ AuxiliaryData::HASH_SIZE }>>),
    ("auxiliary_data_map", round_trip::<AuxiliaryData>),
    ("auxiliary_scripts", round_trip::<Vec<NativeScript>>),
    ("babbage_transaction_output", round_trip::<MemoizedTransactionOutput>),
    ("big_int", round_trip::<BigInt>),
    ("block", round_trip::<Block>),
    ("block_number", round_trip::<BlockHeight>),
    ("bootstrap_witness", round_trip::<BootstrapWitness>),
    ("bounded_bytes", round_trip::<Bytes>),
    ("certificate", round_trip::<Certificate>),
    ("certificates", round_trip::<Certificates>),
    ("coin", round_trip::<Lovelace>),
    ("committee_cold_credential", round_trip::<Credential>),
    ("committee_hot_credential", round_trip::<Credential>),
    ("constitution", round_trip::<Constitution>),
    ("cost_models", round_trip::<CostModels>),
    ("credential", round_trip::<Credential>),
    ("data", round_trip::<InlineData>),
    ("datum_option", round_trip::<MemoizedDatum>),
    ("dns_name", round_trip::<MaxString128>),
    ("drep", round_trip::<DRep>),
    ("drep_credential", round_trip::<Credential>),
    ("drep_voting_thresholds", round_trip::<DRepVotingThresholds>),
    ("epoch", round_trip::<Epoch>),
    ("ex_unit_prices", round_trip::<ExUnitPrices>),
    ("ex_units", round_trip::<ExUnits>),
    ("gov_action", round_trip::<GovernanceAction>),
    ("gov_action_id", round_trip::<ProposalId>),
    ("guardrails_script_hash", round_trip::<Hash<{ size::SCRIPT }>>),
    ("hash28", round_trip::<Hash<{ size::CREDENTIAL }>>),
    ("hash32", round_trip::<Hash<{ size::TRANSACTION_BODY }>>),
    ("header", round_trip::<Header>),
    ("header_body", round_trip::<HeaderBody>),
    ("ipv4", round_trip::<IPv4>),
    ("ipv6", round_trip::<IPv6>),
    ("kes_period", round_trip::<KesPeriod>),
    ("kes_vkey", round_trip::<VerificationKey>),
    ("language", round_trip::<PlutusVersion>),
    ("major_protocol_version", round_trip::<MajorProtocolVersion>),
    ("metadata", round_trip::<AuxiliaryData>),
    ("metadatum", round_trip::<Metadatum>),
    ("mint", round_trip::<Mint>),
    ("native_script", round_trip::<NativeScript>),
    ("network_id", round_trip::<Network>),
    ("nonnegative_interval", round_trip::<RationalNumber>),
    ("nonzero_int64", round_trip::<NonZeroInt>),
    ("operational_cert", round_trip::<OperationalCert>),
    ("plutus_data", round_trip::<MemoizedPlutusData>),
    ("plutus_v1_script", round_trip::<PlutusScript<1>>),
    ("plutus_v2_script", round_trip::<PlutusScript<2>>),
    ("plutus_v3_script", round_trip::<PlutusScript<3>>),
    ("policy_id", round_trip::<Hash<{ size::SCRIPT }>>),
    ("pool_keyhash", round_trip::<PoolId>),
    ("pool_metadata", round_trip::<PoolMetadata>),
    ("pool_voting_thresholds", round_trip::<PoolVotingThresholds>),
    ("positive_coin", round_trip::<PositiveCoin>),
    ("proposal_procedure", round_trip::<Proposal>),
    ("proposal_procedures", round_trip::<Proposals>),
    ("protocol_param_update", round_trip::<ProtocolParamUpdate>),
    ("protocol_version", round_trip::<ProtocolVersion>),
    ("redeemer", round_trip::<Redeemer>),
    ("redeemers", round_trip::<Redeemers>),
    ("relay", round_trip::<Relay>),
    ("reward_account", round_trip::<RewardAccount>),
    ("script", round_trip::<MemoizedScript>),
    ("script_data_hash", round_trip::<Hash<32>>),
    ("script_hash", round_trip::<Hash<{ size::SCRIPT }>>),
    ("signature", round_trip::<Ed25519Signature>),
    ("signkey_kes", round_trip::<Ed25519Signature>),
    ("slot", round_trip::<Slot>),
    ("stake_credential", round_trip::<Credential>),
    ("transaction", round_trip::<Transaction>),
    ("transaction_body", round_trip::<TransactionBody>),
    ("transaction_id", round_trip_without_context::<TransactionId>),
    ("transaction_index", round_trip::<TransactionIndex>),
    ("transaction_input", round_trip::<TransactionInput>),
    ("transaction_output", round_trip::<MemoizedTransactionOutput>),
    ("transaction_witness_set", round_trip::<WitnessSet>),
    ("unit_interval", round_trip::<UnitRationalNumber>),
    ("url", round_trip::<MaxString128>),
    ("value", round_trip::<Value>),
    ("vkey", round_trip::<VerificationKey>),
    ("vkeywitness", round_trip::<VerificationKeyWitness>),
    ("vote", round_trip::<Vote>),
    ("voter", round_trip::<Voter>),
    ("voting_procedure", round_trip::<VotingProcedure>),
    ("voting_procedures", round_trip::<VotingProcedures>),
    ("vrf_keyhash", round_trip::<Hash<{ size::VRF_KEY }>>),
    ("vrf_vkey", round_trip::<VerificationKey>),
    ("withdrawals", round_trip::<Withdrawals>),
];

/// Fail when the corpus carries a rule amaru does not yet check.
pub fn check_no_unknown_rules(root: &Path) -> anyhow::Result<()> {
    let known: BTreeSet<&str> = RULES.iter().map(|(name, _)| *name).collect();
    for entry in read_directory(root)? {
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !known.contains(name.as_str()) {
            anyhow::bail!("corpus contains the unknown rule `{name}`; add it to RULES");
        }
    }
    Ok(())
}

/// Check that the corpus samples for a rule round-trip through amaru as expected, returning the results.
pub fn check_rule(
    test_configuration: &TestConfiguration,
    rule: &str,
    round_trip: &RoundTrip,
) -> anyhow::Result<TestResults> {
    let tests = test_configuration.read_tests_for(rule)?;
    let mut test_results = TestResults::new();

    for test_key in tests {
        let to_decode = test_key.read_bytes()?;
        let expected_cbor = test_key.read_expected_cbor()?;
        let actual_cbor = round_trip(&to_decode, test_configuration.protocol_version());
        test_results.update(&test_key, &to_decode, actual_cbor, expected_cbor)?;
    }
    Ok(test_results)
}

/// Check that there are no unacknowledged failures in the test results.
/// Also check that there are no stale acknowledgements, i.e. acknowledgements for failures that did not occur in this run.
pub fn check_unacknowledged_failures(test_results: &TestResults) {
    let unacknowledged =
        test_results.unacknowledged.iter().map(|(rule, class)| format!("{}: {}", rule, class)).collect::<Vec<_>>();
    assert!(
        unacknowledged.is_empty(),
        "\n❌ {}/{} samples did not match their expectation but
   {}\n\n{}\n\n",
        test_results.failures.len(),
        test_results.total(),
        pluralize(
            unacknowledged.len(),
            "1 rule failure was not acknowledged",
            "{} rule failures were not acknowledged"
        ),
        unacknowledged.join("\n")
    );

    let stale =
        test_results.stale.iter().map(|failure| format!("{}: {}", failure.rule, failure.class)).collect::<Vec<_>>();
    assert!(
        stale.is_empty(),
        "\n❌ {}/{} samples did not match their expectation.
   no unacknowledged rule failures but {}\n\n{}\n\n",
        test_results.failures.len(),
        test_results.total(),
        pluralize(stale.len(), "there is 1 stale acknowledgement", "there are {} stale acknowledgements"),
        stale.join("\n")
    );
}

/// Return a pluralized string based on the count.
fn pluralize(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 { singular.to_string() } else { plural.replace("{}", &count.to_string()) }
}
