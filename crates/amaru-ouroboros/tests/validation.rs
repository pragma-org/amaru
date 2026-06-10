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
    ConsensusParameters, Epoch, Header, NetworkName, Nonce, PoolId, Slot, cbor, size::VRF_KEY,
    utils::serde::hex_to_bytes,
};
use amaru_ouroboros::praos;
use amaru_ouroboros_traits::{
    HasStakeDistribution, PoolSummary,
    has_stake_distribution::{GetPoolError, mock_ledger_state::MockLedgerState},
};
use pallas_crypto::{hash::Hash, key::ed25519::SecretKey};
use pallas_primitives::babbage;
use serde::{Deserialize, Deserializer};

#[test]
fn header_test_cases() {
    let file = File::open("tests/data/header-test-cases.json").unwrap();
    let cases: Vec<HeaderTestCase> = serde_json::from_reader(BufReader::new(file)).expect("decode test cases");

    for case in &cases {
        let minted = case.header.minted().expect("decode minted header");
        let raw_body = minted.header_body.raw_cbor();
        let header = Header::from(minted);
        let params = Arc::new(consensus_parameters_from_context(&case.context));
        let ledger: Arc<dyn HasStakeDistribution> = match case.ledger_state {
            LedgerState::FromContext => Arc::new(mock_ledger_state(&case.context)),
            LedgerState::MissingPool => Arc::new(MissingPoolLedger),
            LedgerState::Failing => Arc::new(FailingLedger),
        };
        let result: Result<Vec<_>, _> =
            praos::header::assert_all(params, &header, raw_body, ledger, &case.context.nonce)
                .and_then(|assertions| assertions.into_iter().map(|a| a()).collect());

        match (&case.expected, result) {
            (Expected::Pass, Ok(_)) => (),
            (Expected::Error(expected), Err(err)) => {
                let actual = format!("{:?}", err);
                assert!(
                    actual.contains(expected),
                    "[{}] {}\nexpected error to contain {:?}, got {:?}\ncontext: {:?}",
                    case.title,
                    case.description,
                    expected,
                    err,
                    case.context
                );
            }
            (Expected::Pass, Err(e)) => panic!(
                "[{}] {}\nexpected pass, got error: {:?}\ncontext: {:?}",
                case.title, case.description, e, case.context
            ),
            (Expected::Error(expected), Ok(_)) => panic!(
                "[{}] {}\nexpected error containing {:?}, got pass\ncontext: {:?}",
                case.title, case.description, expected, case.context
            ),
        }
    }
}

/// Context from which a header has been generated.
///
/// The context provides extra information needed to validate a
/// header, like the nonce, the operational certificate counters, etc.
/// It also provides secret keys that were used to sign the header and
/// produce the VRF output, in order to help troubleshoot the
/// validation process in case of test failures.
///
/// The fixtures in `tests/data/header-test-cases.json` have initially been generated from some
/// the ouroboros-consensus Haskell code. They carry the generation context, the hex-encoded header
/// and the expected outcome.
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
    cold_secret_key: SecretKey,
    #[serde(rename = "vrfVKeyHash")]
    vrf_vkey_hash: Hash<VRF_KEY>,
    nonce: Nonce,
    #[serde(rename = "ocertCounters")]
    operational_certificate_counters: BTreeMap<PoolId, u64>,
    #[serde(rename = "activeSlotCoeff")]
    active_slot_coeff: f64,
}

struct KesKeyWrapper {
    bytes: Vec<u8>,
}

impl std::fmt::Debug for KesKeyWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KesKeyWrapper").field("bytes", &hex::encode(&self.bytes)).finish()
    }
}

impl std::fmt::Debug for GeneratorContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratorContext")
            .field("praos_slots_per_kes_period", &self.praos_slots_per_kes_period)
            .field("praos_max_kes_evolution", &self.praos_max_kes_evolution)
            .field("kes_secret_key", &self.kes_secret_key)
            .field("cold_secret_key", &self.cold_secret_key)
            .field("vrf_vkey_hash", &self.vrf_vkey_hash)
            .field("nonce", &self.nonce)
            .field("operational_certificate_counters", &self.operational_certificate_counters)
            .field("active_slot_coeff", &self.active_slot_coeff)
            .finish()
    }
}

fn deserialize_secret_kes_key<'de, D: Deserializer<'de>>(d: D) -> Result<KesKeyWrapper, D::Error> {
    let s = String::deserialize(d)?;
    let bytes = hex::decode(s).map_err(serde::de::Error::custom)?;
    Ok(KesKeyWrapper { bytes })
}

fn deserialize_secret_ed25519_key<'de, D: Deserializer<'de>>(d: D) -> Result<SecretKey, D::Error> {
    let s = String::deserialize(d)?;
    let decoded = hex::decode(s).map_err(serde::de::Error::custom)?;
    let bytes: [u8; SecretKey::SIZE] =
        decoded.try_into().map_err(|e| serde::de::Error::custom(format!("bad ed25519 secret key length: {e:?}")))?;
    Ok(bytes.into())
}

fn mock_ledger_state(context: &GeneratorContext) -> MockLedgerState {
    MockLedgerState {
        vrf_vkey_hash: context.vrf_vkey_hash,
        stake: 1,
        active_stake: 1,
        op_certs: context.operational_certificate_counters.clone(),
    }
}

fn consensus_parameters_from_context(context: &GeneratorContext) -> ConsensusParameters {
    ConsensusParameters::create(
        0,
        context.praos_slots_per_kes_period,
        context.praos_max_kes_evolution,
        context.active_slot_coeff,
        NetworkName::Preprod.into(),
        context.operational_certificate_counters.clone(),
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
    header: EncodedHeader,
    #[serde(default)]
    ledger_state: LedgerState,
    expected: Expected,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
enum LedgerState {
    #[default]
    FromContext,
    MissingPool,
    Failing,
}

struct MissingPoolLedger;

impl HasStakeDistribution for MissingPoolLedger {
    fn get_pool(&self, _slot: Slot, _pool: &PoolId) -> Result<Option<PoolSummary>, GetPoolError> {
        Ok(None)
    }
}

struct FailingLedger;

impl HasStakeDistribution for FailingLedger {
    fn get_pool(&self, slot: Slot, _pool: &PoolId) -> Result<Option<PoolSummary>, GetPoolError> {
        Err(GetPoolError::StakeDistributionNotAvailable(slot, Epoch::from(0)))
    }
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
struct EncodedHeader(#[serde(deserialize_with = "hex_to_bytes")] Vec<u8>);

impl EncodedHeader {
    fn minted(&self) -> Result<babbage::MintedHeader<'_>, cbor::decode::Error> {
        cbor::decode(&self.0)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Expected {
    Pass,
    Error(String),
}
