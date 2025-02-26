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

use amaru_ouroboros_traits::mock::MockLedgerState;
use pallas_codec::minicbor;
use pallas_primitives::babbage;
use std::{collections::HashMap, fs::File, io::BufReader};

use amaru_ouroboros::{kes, praos};
use ctor::ctor;
use pallas_crypto::{hash::Hash, key::ed25519::SecretKey};
use pallas_math::math::FixedDecimal;
use serde::{Deserialize, Deserializer, Serialize};

/// Context from which a header has been generated.
///
/// The context provides extra information needed to validate a
/// header, like the nonce, the operational certificate counters, etc.
/// It also provides secret keys that were used to sign the header and
/// produce the VRF output, in order to help troubleshoot the
/// validation process in case of test failures.
///
/// This context and the associated `MutatedHeader` are generated from
/// Haskell code and validated with [ouroboros-consensus]() code. There
/// is [PR]() in the making to integrate the generator into the consensus
/// codebase and provide a standalone executable to generate and validate
/// arbitrary headers.
///
/// TODO: The stake distribution should be added to the context, the
/// tester currently assumes the pool signing the header as 100% of the
/// stake.
#[derive(Deserialize)]
struct GeneratorContext {
    #[serde(rename = "praosSlotsPerKESPeriod")]
    praos_slots_per_kes_period: u64,
    #[serde(rename = "praosMaxKESEvo")]
    praos_max_kes_evolution: u64,
    #[serde(rename = "kesSignKey", deserialize_with = "deserialize_secret_kes_key")]
    kes_secret_key: KesKeyWrapper,
    #[serde(
        rename = "coldSignKey",
        deserialize_with = "deserialize_secret_ed25519_key"
    )]
    cold_secret_key: SecretKey,
    #[serde(rename = "vrfVKeyHash", deserialize_with = "deserialize_vrf_vkey_hash")]
    vrf_vkey_hash: Hash<32>,
    #[serde(deserialize_with = "deserialize_nonce")]
    nonce: Hash<32>,
    #[serde(rename = "ocertCounters")]
    operational_certificate_counters: HashMap<Hash<28>, u64>,
    #[serde(rename = "activeSlotCoeff")]
    active_slot_coeff: f64,
}

impl GeneratorContext {
    fn active_slot_coeff_fraction(&self) -> pallas_math::math_dashu::Decimal {
        FixedDecimal::from((self.active_slot_coeff * 100.0) as u64) / FixedDecimal::from(100u64)
    }
}

impl std::fmt::Debug for GeneratorContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratorContext")
            .field(
                "praos_slots_per_kes_period",
                &self.praos_slots_per_kes_period,
            )
            .field("praos_max_kes_evolution", &self.praos_max_kes_evolution)
            .field("kes_secret_key", &self.kes_secret_key)
            .field("cold_secret_key", &self.cold_secret_key)
            .field("nonce", &self.nonce)
            .field(
                "operational_certificate_counters",
                &self.operational_certificate_counters,
            )
            .field("active_slot_coeff", &self.active_slot_coeff)
            .finish()
    }
}

#[derive(Debug)]
pub struct KesKeyWrapper {
    bytes: Vec<u8>,
}

pub struct KesKeyWrapperError {
    pub reason: String,
}

impl KesKeyWrapper {
    pub fn get_kes_secret_key(&'_ mut self) -> Result<kes::SecretKey<'_>, KesKeyWrapperError> {
        kes::SecretKey::from_bytes(&mut self.bytes).map_err(|err| KesKeyWrapperError {
            reason: err.to_string(),
        })
    }
}

fn deserialize_secret_kes_key<'de, D>(deserializer: D) -> Result<KesKeyWrapper, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = <String>::deserialize(deserializer)?;
    let bytes = hex::decode(buf).map_err(serde::de::Error::custom)?;
    Ok(KesKeyWrapper { bytes })
}

fn deserialize_secret_ed25519_key<'de, D>(deserializer: D) -> Result<SecretKey, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = <String>::deserialize(deserializer)?;
    let decoded = hex::decode(buf).map_err(serde::de::Error::custom)?;
    let bytes: [u8; SecretKey::SIZE] = decoded.try_into().map_err(|e| {
        serde::de::Error::custom(format!("cannot convert vector to secret key: {:?}", e))
    })?;
    Ok(bytes.into())
}

fn deserialize_vrf_vkey_hash<'de, D>(deserializer: D) -> Result<Hash<32>, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = <String>::deserialize(deserializer)?;
    let decoded = hex::decode(buf).map_err(serde::de::Error::custom)?;
    let num_bytes = decoded.len();
    let bytes: [u8; 32] = decoded.try_into().map_err(|e| {
        serde::de::Error::custom(format!(
            "cannot convert vector to secret vrf key hash (len = {}): {:?}",
            num_bytes, e
        ))
    })?;
    Ok(Hash::new(bytes))
}

fn deserialize_nonce<'de, D>(deserializer: D) -> Result<Hash<32>, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = <String>::deserialize(deserializer)?;
    let decoded = hex::decode(buf).map_err(serde::de::Error::custom)?;
    let bytes = decoded.try_into().map_err(|e| {
        serde::de::Error::custom(format!("cannot convert vector to nonce: {:?}", e))
    })?;
    Ok(Hash::new(bytes))
}

#[derive(Debug, Deserialize)]
struct MutatedHeader {
    #[serde(deserialize_with = "deserialize_header")]
    header: HeaderWrapper,
    mutation: Mutation,
}

#[derive(Debug)]
struct HeaderWrapper {
    bytes: Vec<u8>,
}

impl HeaderWrapper {
    fn get_header(&mut self) -> Result<babbage::MintedHeader<'_>, ()> {
        minicbor::decode(self.bytes.as_slice()).map_err(|_| ())
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

#[derive(Debug, Serialize, Deserialize)]
enum Mutation {
    #[allow(clippy::enum_variant_names)]
    NoMutation,
    MutateKESKey,
    MutateColdKey,
    MutateKESPeriod,
    MutateKESPeriodBefore,
    MutateCounterOver1,
    MutateCounterUnder,
}

fn mock_ledger_state(context: &GeneratorContext) -> MockLedgerState {
    MockLedgerState {
        vrf_vkey_hash: context.vrf_vkey_hash,
        stake: 1,
        active_stake: 1,
        op_certs: context.operational_certificate_counters.clone(),
        slots_per_kes_period: context.praos_slots_per_kes_period,
        max_kes_evolutions: context.praos_max_kes_evolution,
    }
}

#[ctor]
fn init() {
    // initialize tracing crate
    tracing_subscriber::fmt::init();
}

const EXPECTED_SLOT_NUMBER: u64 = 9169164218553922239u64;

#[test]
fn can_read_and_write_json_test_vectors() {
    let file = File::open("tests/data/test-vector.json").unwrap();
    let result: Result<Vec<(GeneratorContext, MutatedHeader)>, serde_json::Error> =
        serde_json::from_reader(BufReader::new(file));
    assert!(result.is_ok());
    let mut vec = result.unwrap();
    let header = vec[0].1.header.get_header().expect("cannot create header");
    // NOTE: this magic number ensures that we read an up-to-date test vector
    assert_eq!(header.header_body.slot, EXPECTED_SLOT_NUMBER);
}

#[test]
fn validation_conforms_to_test_vectors() {
    use rayon::prelude::*;

    let file = File::open("tests/data/test-vector.json").unwrap();
    let result: Result<Vec<(GeneratorContext, MutatedHeader)>, serde_json::Error> =
        serde_json::from_reader(BufReader::new(file));
    result
        .expect("cannot deserialize test vectors")
        .iter_mut()
        .enumerate()
        .for_each(|(header_index, test)| {
            let context = &test.0;
            test.1
                .header
                .get_header()
                .map(|header| {
                    let expected = &test.1.mutation;
                    let ledger_state = mock_ledger_state(context);
                    let epoch_nonce = context.nonce;
                    let active_slot_coeff = context.active_slot_coeff_fraction();
                    let assertions = praos::header::assert_all(
                        &header,
                        &ledger_state,
                        &epoch_nonce,
                        &active_slot_coeff,
                    )
                        .unwrap()
                        .into_par_iter()
                        .map(|assert| assert())
                        .collect::<Result<Vec<_>, _>>();

                    match (expected, assertions) {
                        (Mutation::NoMutation, Ok(_)) => (),
                        (Mutation::NoMutation, Err(e)) => {
                            panic!(
                                "[{}] expected validation to succeed, failed with error {:?}\n header: {:?}\n context: {:?}",
                                header_index, e, header, context
                            )
                        }
                        (_, Ok(_)) => {
                            panic!(
                                "[{}] expected validation to fail ({:?}), but it succeeded\n header: {:?}\n context: {:?}",
                                header_index, expected, header, context
                            )
                        }
                        (_, Err(_)) => (),
                    }
                })
                .expect("cannot extract header from bytes");
        });
}
