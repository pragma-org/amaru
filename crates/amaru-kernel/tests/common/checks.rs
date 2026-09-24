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
    AuxiliaryData, Block, Certificate, CostModels, Credential, DRep, GovernanceAction, Header, HeaderBody,
    MemoizedDatum, MemoizedPlutusData, MemoizedScript, MemoizedTransactionOutput, NativeScript, Proposal,
    ProtocolParamUpdate, ProtocolVersion, Redeemer, Redeemers, Relay, Transaction, TransactionBody, TransactionInput,
    Value, VotingProcedure, WitnessSet, cbor,
};
use amaru_minicbor_extra::{from_cbor_no_leftovers_with, to_cbor_with};

use crate::{TestConfiguration, TestResults, read_directory};

/// The Conway rules the corpus covers, mapped to their amaru counterparts. Mirrors
/// `conwayRuleChecks` in the upstream generator's `app/LedgerRules.hs`.
///
/// `block` is the bare five-element block array, not the hard-fork-wrapped `(EraName, Block)` pair
/// the on-chain fixtures use.
pub const RULES: &[(&str, RoundTrip)] = &[
    ("auxiliary_data", round_trip::<AuxiliaryData>),
    ("block", round_trip::<Block>),
    ("certificate", round_trip::<Certificate>),
    ("cost_models", round_trip::<CostModels>),
    ("credential", round_trip::<Credential>),
    ("datum_option", round_trip::<MemoizedDatum>),
    ("drep", round_trip::<DRep>),
    ("gov_action", round_trip::<GovernanceAction>),
    ("header", round_trip::<Header>),
    ("header_body", round_trip::<HeaderBody>),
    ("native_script", round_trip::<NativeScript>),
    ("plutus_data", round_trip::<MemoizedPlutusData>),
    ("proposal_procedure", round_trip::<Proposal>),
    ("protocol_param_update", round_trip::<ProtocolParamUpdate>),
    ("redeemer", round_trip::<Redeemer>),
    ("redeemers", round_trip::<Redeemers>),
    ("relay", round_trip::<Relay>),
    ("script", round_trip::<MemoizedScript>),
    ("transaction", round_trip::<Transaction>),
    ("transaction_body", round_trip::<TransactionBody>),
    ("transaction_input", round_trip::<TransactionInput>),
    ("transaction_output", round_trip::<MemoizedTransactionOutput>),
    ("transaction_witness_set", round_trip::<WitnessSet>),
    ("value", round_trip::<Value>),
    ("voting_procedure", round_trip::<VotingProcedure>),
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
        test_results.update(&test_key, actual_cbor, expected_cbor)?;
    }
    Ok(test_results)
}

/// Decode a sample and re-encode it, yielding the re-encoded bytes.
pub type RoundTrip = fn(&[u8], ProtocolVersion) -> Result<Vec<u8>, cbor::decode::Error>;

pub fn round_trip<T>(bytes: &[u8], version: ProtocolVersion) -> Result<Vec<u8>, cbor::decode::Error>
where
    T: for<'b> cbor::Decode<'b, ProtocolVersion> + cbor::Encode<ProtocolVersion>,
{
    let mut version = version;
    let value: T = from_cbor_no_leftovers_with(bytes, &mut version)?;
    Ok(to_cbor_with(&value, &mut version))
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
