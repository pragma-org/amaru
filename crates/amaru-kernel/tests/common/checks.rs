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
    Address, Anchor, AssetName, AuxiliaryData, Block, BlockHeight, BootstrapWitness, Bytes, Certificate, Constitution,
    CostModels, Credential, DRep, DRepVotingThresholds, Ed25519Signature, Epoch, ExUnitPrices, ExUnits,
    GovernanceAction, Hash, Header, HeaderBody, KesPeriod, Lovelace, MajorProtocolVersion, MaxString128, MemoizedDatum,
    MemoizedPlutusData, MemoizedScript, MemoizedTransactionOutput, Metadatum, Mint, Multiasset, NativeScript, Network,
    NonEmptyKeyValuePairs, NonEmptySet, NonEmptyUniqueVec, NonZeroInt, OperationalCert, PlutusData, PlutusScript,
    PlutusVersion, PoolId, PoolMetadata, PoolVotingThresholds, PositiveCoin, Proposal, ProposalId, ProtocolParamUpdate,
    ProtocolVersion, RationalNumber, Redeemer, Redeemers, Relay, RewardAccount, Slot, Transaction, TransactionBody,
    TransactionId, TransactionIndex, TransactionInput, UnitRationalNumber, Value, VerificationKey,
    VerificationKeyWitness, Vote, Voter, VotingProcedure, WitnessSet,
    cardano::{
        plutus_data::{BigInt, Constr},
        relay::{IPv4, IPv6},
    },
    cbor,
    hash::size,
};
use amaru_minicbor_extra::{from_cbor_no_leftovers, from_cbor_no_leftovers_with, to_cbor, to_cbor_with};

use crate::{TestConfiguration, TestResults, read_directory};

/// The Conway rules the corpus covers, mapped to their amaru counterparts. Mirrors
/// `conwayRuleChecks` in the upstream generator's `app/LedgerRules.hs`.
///
/// `block` is the bare five-element block array, not the hard-fork-wrapped `(EraName, Block)` pair
/// the on-chain fixtures use.
///
/// The corpus roots a rule at every CDDL production, helper rules included, so several entries decode
/// through the same amaru type: the CDDL distinguishes `hash28` from `script_hash` and `addr_keyhash`,
/// amaru does not, and a rule whose samples the ledger decodes with a plain integer lands on a primitive.
pub const RULES: &[(&str, RoundTrip)] = &[
    ("%constr<plutus_data>", round_trip::<Constr<PlutusData>>),
    ("%multiasset<positive_coin>", round_trip::<Multiasset<PositiveCoin>>),
    ("addr_keyhash", round_trip::<Hash<{ size::KEY }>>),
    ("address", round_trip_address),
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
    ("certificates", round_trip::<NonEmptyUniqueVec<Certificate>>),
    ("coin", round_trip::<Lovelace>),
    ("committee_cold_credential", round_trip::<Credential>),
    ("committee_hot_credential", round_trip::<Credential>),
    ("constitution", round_trip::<Constitution>),
    ("cost_models", round_trip::<CostModels>),
    ("credential", round_trip::<Credential>),
    ("data", round_trip_data),
    ("datum_option", round_trip::<MemoizedDatum>),
    ("dns_name", round_trip::<MaxString128>),
    ("drep", round_trip::<DRep>),
    ("drep_credential", round_trip::<Credential>),
    ("drep_voting_thresholds", round_trip::<DRepVotingThresholds>),
    ("epoch", round_trip::<Epoch>),
    ("epoch_interval", round_trip::<u32>),
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
    ("metadatum_label", round_trip::<u64>),
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
    ("port", round_trip::<u16>),
    ("positive_coin", round_trip::<PositiveCoin>),
    ("positive_int", round_trip::<PositiveCoin>),
    ("potential_languages", round_trip::<u8>),
    ("proposal_procedure", round_trip::<Proposal>),
    ("proposal_procedures", round_trip::<NonEmptyUniqueVec<Proposal>>),
    ("protocol_param_update", round_trip::<ProtocolParamUpdate>),
    ("protocol_version", round_trip::<ProtocolVersion>),
    ("redeemer", round_trip::<Redeemer>),
    ("redeemers", round_trip::<Redeemers>),
    ("relay", round_trip::<Relay>),
    ("required_signers", round_trip::<NonEmptySet<Hash<{ size::KEY }>>>),
    ("reward_account", round_trip::<RewardAccount>),
    ("script", round_trip::<MemoizedScript>),
    ("script_data_hash", round_trip::<Hash<32>>),
    ("script_hash", round_trip::<Hash<{ size::SCRIPT }>>),
    ("sequence_number", round_trip::<u64>),
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
    (
        "voting_procedures",
        round_trip::<NonEmptyKeyValuePairs<Voter, NonEmptyKeyValuePairs<ProposalId, VotingProcedure>>>,
    ),
    ("vrf_keyhash", round_trip::<Hash<{ size::VRF_KEY }>>),
    ("vrf_vkey", round_trip::<VerificationKey>),
    ("withdrawals", round_trip::<NonEmptyKeyValuePairs<RewardAccount, Lovelace>>),
];

/// Encoders that reproduce the reference bytes for rules whose ordinary encoder does not.
///
/// Where a type is written to the wire in a form the encoder is not free to choose — because its bytes are
/// hashed — the suite asks for the exact bytes back. The ordinary encoder may legitimately differ, writing the
/// form the CDDL prescribes rather than the one that arrived. Supplying an encoder here that reproduces the
/// input is the proof that decoding kept everything the bytes carried: it can only be written if the decoded
/// value still holds every choice the producer made.
pub const EXACT_ENCODERS: &[(&str, RoundTrip)] = &[
    ("transaction_witness_set", round_trip_exact_witness_set),
    ("transaction", round_trip_exact_transaction),
    ("block", round_trip_exact_block),
];

/// The encoder the suite compares against the reference for a rule, exact where one is supplied.
pub fn exact_encoder(rule: &str) -> Option<RoundTrip> {
    EXACT_ENCODERS.iter().find(|(name, _)| *name == rule).map(|(_, encode)| *encode)
}

/// Write a witness set as the bytes it was decoded from.
///
/// The set tag is optional on the wire below protocol version 12, and the ordinary encoder always writes it,
/// as the Conway CDDL prescribes. The bytes are covered by the block body hash, so the producer's choice is part
/// of the value; a witness set records which of its fields carried the tag, and that is the only thing it has to
/// remember beyond its fields for this encoder to exist.
fn encode_witness_set_exact<W: cbor::encode::Write>(
    e: &mut cbor::Encoder<W>,
    ctx: &mut ProtocolVersion,
    witnesses: &WitnessSet,
) -> Result<(), cbor::encode::Error<W::Error>> {
    fn field<T: cbor::Encode<ProtocolVersion>, W: cbor::encode::Write>(
        e: &mut cbor::Encoder<W>,
        ctx: &mut ProtocolVersion,
        key: u8,
        tagged: bool,
        items: &[T],
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.u8(key)?;
        if tagged {
            e.tag(cbor::TAG_SET_258)?;
        }
        e.encode_with(items, ctx)?;
        Ok(())
    }

    let tagged = |key: u8| witnesses.set_tags & (1 << key) != 0;
    let present = [
        witnesses.verification_key_witness.is_some(),
        witnesses.native_script.is_some(),
        witnesses.bootstrap_witness.is_some(),
        witnesses.plutus_v1_script.is_some(),
        witnesses.plutus_data.is_some(),
        witnesses.redeemer.is_some(),
        witnesses.plutus_v2_script.is_some(),
        witnesses.plutus_v3_script.is_some(),
    ];
    e.map(present.iter().filter(|is_present| **is_present).count() as u64)?;

    if let Some(f) = &witnesses.verification_key_witness {
        field(e, ctx, 0, tagged(0), f)?;
    }
    if let Some(f) = &witnesses.native_script {
        field(e, ctx, 1, tagged(1), f)?;
    }
    if let Some(f) = &witnesses.bootstrap_witness {
        field(e, ctx, 2, tagged(2), f)?;
    }
    if let Some(f) = &witnesses.plutus_v1_script {
        field(e, ctx, 3, tagged(3), f)?;
    }
    if let Some(f) = &witnesses.plutus_data {
        field(e, ctx, 4, tagged(4), f)?;
    }
    if let Some(f) = &witnesses.redeemer {
        e.u8(5)?.encode_with(f, ctx)?;
    }
    if let Some(f) = &witnesses.plutus_v2_script {
        field(e, ctx, 6, tagged(6), f)?;
    }
    if let Some(f) = &witnesses.plutus_v3_script {
        field(e, ctx, 7, tagged(7), f)?;
    }

    Ok(())
}

/// Run an exact encoder over a freshly decoded value, turning the write errors into decode ones.
fn exact<T, F>(bytes: &[u8], version: ProtocolVersion, write: F) -> Result<Vec<u8>, cbor::decode::Error>
where
    T: for<'b> cbor::Decode<'b, ProtocolVersion>,
    F: FnOnce(
        &mut cbor::Encoder<&mut Vec<u8>>,
        &mut ProtocolVersion,
        &T,
    ) -> Result<(), cbor::encode::Error<std::convert::Infallible>>,
{
    let mut version = version;
    let value: T = from_cbor_no_leftovers_with(bytes, &mut version)?;
    let mut out = Vec::new();
    let mut encoder = cbor::Encoder::new(&mut out);
    write(&mut encoder, &mut version, &value).map_err(|e| cbor::decode::Error::message(e.to_string()))?;
    Ok(out)
}

fn round_trip_exact_witness_set(bytes: &[u8], version: ProtocolVersion) -> Result<Vec<u8>, cbor::decode::Error> {
    // The closure is not redundant: `encode_witness_set_exact` is generic over the writer, and naming it here
    // leaves that parameter unresolved.
    #[expect(clippy::redundant_closure)]
    exact::<WitnessSet, _>(bytes, version, |e, ctx, witnesses| encode_witness_set_exact(e, ctx, witnesses))
}

fn round_trip_exact_transaction(bytes: &[u8], version: ProtocolVersion) -> Result<Vec<u8>, cbor::decode::Error> {
    exact::<Transaction, _>(bytes, version, |e, ctx, transaction| {
        e.array(4)?;
        e.encode_with(&transaction.body, ctx)?;
        encode_witness_set_exact(e, ctx, &transaction.witnesses)?;
        e.encode_with(transaction.is_expected_valid, ctx)?;
        match &transaction.auxiliary_data {
            Some(auxiliary_data) => e.encode_with(auxiliary_data, ctx)?,
            None => e.null()?,
        };
        Ok(())
    })
}

fn round_trip_exact_block(bytes: &[u8], version: ProtocolVersion) -> Result<Vec<u8>, cbor::decode::Error> {
    exact::<Block, _>(bytes, version, |e, ctx, block| {
        e.array(5)?;
        e.encode_with(&block.header, ctx)?;
        e.encode_with(&block.transaction_bodies, ctx)?;
        e.array(block.transaction_witnesses.len() as u64)?;
        for witnesses in &block.transaction_witnesses {
            encode_witness_set_exact(e, ctx, witnesses)?;
        }
        e.encode_with(&block.auxiliary_data, ctx)?;
        e.encode_with(&block.invalid_transactions, ctx)?;
        Ok(())
    })
}

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
    let round_trip = &exact_encoder(rule).unwrap_or(*round_trip);
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

/// Decode a sample and re-encode it, for a type whose codec takes no context.
pub fn round_trip_without_context<T>(bytes: &[u8], _version: ProtocolVersion) -> Result<Vec<u8>, cbor::decode::Error>
where
    T: for<'b> cbor::Decode<'b, ()> + cbor::Encode<()>,
{
    let value: T = from_cbor_no_leftovers(bytes)?;
    Ok(to_cbor(&value))
}

/// Decode and re-encode an `address`, which is a CBOR byte string whose payload amaru parses with
/// `Address::from_bytes` rather than with a CBOR decoder. The round trip therefore unwraps the byte
/// string, parses the payload, and wraps the rendered address back into a byte string.
pub fn round_trip_address(bytes: &[u8], version: ProtocolVersion) -> Result<Vec<u8>, cbor::decode::Error> {
    let mut version = version;
    let payload: Bytes = from_cbor_no_leftovers_with(bytes, &mut version)?;
    let address = Address::from_bytes(&payload).ok_or_else(|| cbor::decode::Error::message("invalid address"))?;
    Ok(to_cbor_with(&Bytes::from(address.to_vec()), &mut version))
}

/// Decode and re-encode a `data`, which is `#6.24(bytes .cbor plutus_data)`: a tagged byte string whose
/// payload is itself the CBOR of a plutus datum. amaru has no standalone type for it, since it only ever
/// appears inline inside a `datum_option`, so the round trip unwraps the tag and the byte string, round-trips
/// the payload, and wraps the result back.
pub fn round_trip_data(bytes: &[u8], version: ProtocolVersion) -> Result<Vec<u8>, cbor::decode::Error> {
    let mut version = version;
    let mut d = cbor::Decoder::new(bytes);
    if d.tag()? != cbor::IanaTag::Cbor.tag() {
        return Err(cbor::decode::Error::message("unknown tag for a cbor-in-cbor payload"));
    }
    let payload = cbor::decode_bytes_v12(&mut d, &version)?;
    if d.position() != bytes.len() {
        return Err(cbor::decode::Error::message("leftover bytes"));
    }
    let data: MemoizedPlutusData = from_cbor_no_leftovers_with(&payload, &mut version)?;
    let re_encoded = to_cbor_with(&data, &mut version);

    let mut out = Vec::new();
    let mut e = cbor::Encoder::new(&mut out);
    e.tag(cbor::IanaTag::Cbor.tag())
        .and_then(|e| e.bytes(&re_encoded))
        .map_err(|e| cbor::decode::Error::message(e.to_string()))?;
    Ok(out)
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
