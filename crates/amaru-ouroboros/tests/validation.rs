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

use std::{collections::BTreeMap, fs::File, io::BufReader, sync::Arc};

use amaru_kernel::{
    ConsensusParameters, Epoch, EraHistory, Header, IsHeader, NetworkName, Nonce, PoolId, Slot, cbor, ed25519,
    hash::Hash,
};
use amaru_ouroboros::{kes, praos, praos::header::AssertHeaderError};
use amaru_ouroboros_traits::{PoolSummaries, has_stake_distribution::mock_ledger_state::MockLedgerState};
use ctor::ctor;
use num::CheckedSub;
use serde::{Deserialize, Deserializer};

#[ctor(unsafe)]
fn init() {
    // initialize tracing crate
    tracing_subscriber::fmt::init();
}

/// This test validates a number of headers against the header validation rules.
/// Each test case specifies:
///  - If the rules are supposed to succeed.
///  - Or if one of them is supposed to fail. In that case the error type is specified.
///
/// The fixtures in `tests/data/header-test-cases.json` have been generated from some
/// `ouroboros-consensus` Haskell code. They carry the generation context, the hex-encoded
/// header and the expected outcome.
#[test]
fn validation_conforms_to_header_test_cases() {
    let file = File::open("tests/data/header-test-cases.json").unwrap();
    let mut cases: Vec<HeaderTestCase> = serde_json::from_reader(BufReader::new(file)).expect("decode test cases");

    // NOTE: guards against a truncated or mis-parsed fixture silently passing the test
    assert_eq!(cases.len(), 107, "unexpected number of header test cases");

    for case in cases.iter_mut() {
        let context = &case.context;
        let block_header = case.header.get_header().expect("cannot extract header from bytes");

        let pool_id = block_header.pool_id();
        let era_history = NetworkName::Preprod.as_era_history().expect("era");
        let slot = block_header.slot();
        let target = era_history
            .slot_to_epoch_unchecked_horizon(slot)
            .ok()
            .and_then(|e| e.checked_sub(Epoch::TWO))
            .expect("test vector epoch should be >= 2");
        let summaries = pool_summaries(context, &case.ledger_state, pool_id, target);
        let result = validate_header(context, &block_header, &summaries, era_history, slot);

        match (&case.expected, result) {
            (Expected::Pass, Ok(())) => (),
            (Expected::Error(expected), Err(error)) => {
                let actual = format!("{:?}", error);
                assert!(
                    actual.contains(expected),
                    "[{}] {}\nexpected error to contain {:?}, got {:?}\ncontext: {:?}",
                    case.title,
                    case.description,
                    expected,
                    error,
                    context
                );
            }
            (Expected::Pass, Err(error)) => {
                panic!(
                    "[{}] {}\nexpected validation to succeed, failed with error {:?}\ncontext: {:?}",
                    case.title, case.description, error, context
                )
            }
            (Expected::Error(expected), Ok(())) => {
                panic!(
                    "[{}] {}\nexpected validation to fail with {:?}, but it succeeded\ncontext: {:?}",
                    case.title, case.description, expected, context
                )
            }
        }
    }
}

/// Error surfaced while validating a header in the same order as `ValidateHeader::check_header`:
/// the pool is resolved from the ledger first (which may fail or report an unknown pool), then the
/// praos assertions run.
#[derive(Debug)]
enum ValidationError {
    UnknownPool,
    #[expect(dead_code, reason = "surfaced through the Debug output in failure assertions")]
    Assert(Box<AssertHeaderError>),
}

/// Build the pool summaries the ledger exposes for a case, mirroring the three `ledgerState`
/// situations encoded in the fixtures:
///  - `missingPool`: the epoch is known but the pool is absent, so `get_pool` returns `Ok(None)`.
///  - `failing`: no stake distribution for the epoch, so `get_pool` returns an error.
///  - otherwise: the issuing pool is registered with the mocked stake and VRF key.
fn pool_summaries(
    context: &GeneratorContext,
    ledger_state: &Option<String>,
    pool: PoolId,
    target: Epoch,
) -> PoolSummaries {
    match ledger_state.as_deref() {
        Some("missingPool") => {
            let mut by_epoch = BTreeMap::new();
            by_epoch.insert(target, BTreeMap::new());
            PoolSummaries { by_epoch }
        }
        Some("failing") => PoolSummaries::default(),
        _ => mock_ledger_state(context).to_pool_summaries(pool, target),
    }
}

/// Run the same validation steps as `ValidateHeader::check_header`: resolve the pool from the
/// ledger, look up the latest opcert sequence number (here: from the generation context), then
/// run the praos assertions.
#[allow(clippy::expect_used)]
// The assertions returned by `assert_all` fail with `AssertHeaderError`, which is large by design;
// only its size inside `ValidationError` is under our control, and that one is boxed.
#[allow(clippy::result_large_err)]
fn validate_header(
    context: &GeneratorContext,
    header: &Header,
    summaries: &PoolSummaries,
    era_history: &EraHistory,
    slot: Slot,
) -> Result<(), ValidationError> {
    use rayon::prelude::*;

    let pool_id = header.pool_id();
    let last_opcert_sequence_number = context.operational_certificate_counters.get(&pool_id).copied();
    let consensus_parameters = Arc::new(consensus_parameters_from_context(context));

    let pool_summary = summaries
        .get_pool(slot, &pool_id, era_history)
        .map_err(|e| ValidationError::Assert(Box::new(AssertHeaderError::PoolError(e))))?
        .ok_or(ValidationError::UnknownPool)?;

    praos::header::assert_all(consensus_parameters, header, last_opcert_sequence_number, &pool_summary, &context.nonce)
        .and_then(|assertions| assertions.into_par_iter().try_for_each(|assert| assert()))
        .map_err(|e| ValidationError::Assert(Box::new(e)))
}

fn mock_ledger_state(context: &GeneratorContext) -> MockLedgerState {
    MockLedgerState { vrf_verification_key_hash: context.vrf_verification_key_hash, stake: 1, active_stake: 1 }
}

#[allow(clippy::expect_used)]
fn consensus_parameters_from_context(context: &GeneratorContext) -> ConsensusParameters {
    ConsensusParameters::create(
        0,
        context.praos_slots_per_kes_period,
        context.praos_max_kes_evolution,
        context.active_slot_coeff,
        NetworkName::Preprod.as_era_history().expect("missing network default EraHistory"),
    )
}

/// Test case for a given generated header
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeaderTestCase {
    title: String,
    #[serde(default)]
    description: String,
    context: GeneratorContext,
    #[serde(deserialize_with = "deserialize_header")]
    header: HeaderWrapper,
    #[serde(default)]
    ledger_state: Option<String>,
    expected: Expected,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Expected {
    Pass,
    Error(String),
}

/// Context from which a header has been generated.
///
/// The context provides extra information needed to validate a
/// header, like the nonce, the operational certificate counters, etc.
/// It also provides secret keys that were used to sign the header and
/// produce the VRF output, in order to help troubleshoot the
/// validation process in case of test failures.
///
/// TODO: The stake distribution should be added to the context, the
/// tester currently assumes the pool signing the header has 100% of the
/// stake.
#[derive(Deserialize)]
struct GeneratorContext {
    #[serde(rename = "praosSlotsPerKESPeriod")]
    praos_slots_per_kes_period: u64,
    #[serde(rename = "praosMaxKESEvo")]
    praos_max_kes_evolution: u64,
    #[serde(rename = "kesSignKey", deserialize_with = "deserialize_secret_kes_key")]
    kes_secret_key: KesKeyWrapper,
    #[serde(rename = "coldSignKey", deserialize_with = "deserialize_secret_ed25519_key")]
    cold_secret_key: ed25519::SecretKey,
    #[serde(rename = "vrfVKeyHash", deserialize_with = "deserialize_vrf_verification_key_hash")]
    vrf_verification_key_hash: Hash<32>,
    #[serde(deserialize_with = "deserialize_nonce")]
    nonce: Nonce,
    #[serde(rename = "ocertCounters")]
    operational_certificate_counters: BTreeMap<PoolId, u64>,
    #[serde(rename = "activeSlotCoeff")]
    active_slot_coeff: f64,
}

impl std::fmt::Debug for GeneratorContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratorContext")
            .field("praos_slots_per_kes_period", &self.praos_slots_per_kes_period)
            .field("praos_max_kes_evolution", &self.praos_max_kes_evolution)
            .field("kes_secret_key", &self.kes_secret_key)
            .field("cold_secret_key", &self.cold_secret_key)
            .field("vrf_verification_key_hash", &self.vrf_verification_key_hash)
            .field("nonce", &self.nonce)
            .field("operational_certificate_counters", &self.operational_certificate_counters)
            .field("active_slot_coeff", &self.active_slot_coeff)
            .finish()
    }
}

pub struct KesKeyWrapper {
    bytes: Vec<u8>,
}

impl std::fmt::Debug for KesKeyWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KesKeyWrapper").field("bytes", &hex::encode(&self.bytes)).finish()
    }
}

pub struct KesKeyWrapperError {
    pub reason: String,
}

impl KesKeyWrapper {
    pub fn get_kes_secret_key(&'_ mut self) -> Result<kes::SecretKey<'_>, KesKeyWrapperError> {
        kes::SecretKey::from_bytes(&mut self.bytes).map_err(|err| KesKeyWrapperError { reason: err.to_string() })
    }
}

#[derive(Debug)]
struct HeaderWrapper {
    bytes: Vec<u8>,
}

impl HeaderWrapper {
    fn get_header(&mut self) -> Result<Header, ()> {
        cbor::decode(self.bytes.as_slice()).map_err(|_| ())
    }
}

fn deserialize_header<'de, D>(deserializer: D) -> Result<HeaderWrapper, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = <String>::deserialize(deserializer)?;
    let bytes = hex::decode(buf).map_err(serde::de::Error::custom)?;
    Ok(HeaderWrapper { bytes })
}

fn deserialize_secret_kes_key<'de, D>(deserializer: D) -> Result<KesKeyWrapper, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = <String>::deserialize(deserializer)?;
    let bytes = hex::decode(buf).map_err(serde::de::Error::custom)?;
    Ok(KesKeyWrapper { bytes })
}

fn deserialize_secret_ed25519_key<'de, D>(deserializer: D) -> Result<ed25519::SecretKey, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = <String>::deserialize(deserializer)?;
    let decoded = hex::decode(buf).map_err(serde::de::Error::custom)?;
    let bytes: [u8; ed25519::SECRET_KEY_LENGTH] = decoded
        .try_into()
        .map_err(|e| serde::de::Error::custom(format!("cannot convert vector to secret key: {:?}", e)))?;
    Ok(bytes)
}

fn deserialize_vrf_verification_key_hash<'de, D>(deserializer: D) -> Result<Hash<32>, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = <String>::deserialize(deserializer)?;
    let decoded = hex::decode(buf).map_err(serde::de::Error::custom)?;
    let num_bytes = decoded.len();
    let bytes: [u8; 32] = decoded.try_into().map_err(|e| {
        serde::de::Error::custom(format!("cannot convert vector to secret vrf key hash (len = {}): {:?}", num_bytes, e))
    })?;
    Ok(Hash::new(bytes))
}

fn deserialize_nonce<'de, D>(deserializer: D) -> Result<Hash<32>, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = <String>::deserialize(deserializer)?;
    let decoded = hex::decode(buf).map_err(serde::de::Error::custom)?;
    let bytes =
        decoded.try_into().map_err(|e| serde::de::Error::custom(format!("cannot convert vector to nonce: {:?}", e)))?;
    Ok(Hash::new(bytes))
}
