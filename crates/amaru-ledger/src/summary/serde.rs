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

//! This module contains a variety of helpers used to produce serialised values for the various
//! summaries.

use amaru_kernel::{DRep, Network, PoolId, StakeCredential, encode_bech32};
use std::collections::BTreeMap;

pub fn encode_pool_id(pool_id: &PoolId) -> String {
    encode_bech32("pool", pool_id.as_slice())
        .unwrap_or_else(|_| unreachable!("human-readable part 'pool' is okay"))
}

/// Serialize a (registerd) DRep to bech32, according to [CIP-0129](https://cips.cardano.org/cip/CIP-0129).
/// The always-Abstain and always-NoConfidence dreps are ignored (i.e. return `None`).
///
/// ```rust
/// use amaru_kernel::{DRep, Hash};
/// use amaru_ledger::summary::serde::encode_drep;
///
/// let key_drep = DRep::Key(Hash::from(
///   hex::decode("7a719c71d1bc67d2eb4af19f02fd48e7498843d33a22168111344a34")
///     .unwrap()
///     .as_slice()
/// ));
///
/// let script_drep = DRep::Script(Hash::from(
///   hex::decode("429b12461640cefd3a4a192f7c531d8f6c6d33610b727f481eb22d39")
///     .unwrap()
///     .as_slice()
/// ));
///
/// assert_eq!(
///   encode_drep(&DRep::Abstain).as_str(),
///   "abstain",
/// );
///
/// assert_eq!(
///   encode_drep(&DRep::NoConfidence).as_str(),
///   "no_confidence",
/// );
///
/// assert_eq!(
///   encode_drep(&key_drep).as_str(),
///   "drep1yfa8r8r36x7x05htftce7qhafrn5nzzr6vazy95pzy6y5dqac0ss7",
/// );
///
/// assert_eq!(
///   encode_drep(&script_drep).as_str(),
///   "drep1ydpfkyjxzeqvalf6fgvj7lznrk8kcmfnvy9hyl6gr6ez6wgsjaelx",
/// );
/// ```
pub fn encode_drep(drep: &DRep) -> String {
    match drep {
        DRep::Key(hash) => encode_bech32("drep", &[&[0x22], hash.as_slice()].concat()),
        DRep::Script(hash) => encode_bech32("drep", &[&[0x23], hash.as_slice()].concat()),
        DRep::Abstain => Ok("abstain".to_string()),
        DRep::NoConfidence => Ok("no_confidence".to_string()),
    }
    .unwrap_or_else(|_| unreachable!("human-readable part 'drep' is okay"))
}

pub fn encode_stake_credential(network: Network, credential: &StakeCredential) -> String {
    encode_bech32(
        "stake_test",
        &match credential {
            StakeCredential::AddrKeyhash(hash) => {
                [&[0xe0 | network.value()], hash.as_slice()].concat()
            }
            StakeCredential::ScriptHash(hash) => {
                [&[0xf0 | network.value()], hash.as_slice()].concat()
            }
        },
    )
    .unwrap_or_else(|_| unreachable!("human-readable part 'stake_test' is okay"))
}

pub fn serialize_map<K, V: serde::ser::Serialize, S: serde::ser::SerializeStruct>(
    field: &'static str,
    s: &mut S,
    m: &BTreeMap<K, V>,
    serialize_key: impl Fn(&K) -> String,
) -> Result<(), S::Error> {
    let mut elems = m
        .iter()
        .map(|(k, v)| (serialize_key(k), v))
        .collect::<Vec<_>>();
    elems.sort_by(|a, b| a.0.cmp(&b.0));
    s.serialize_field(field, &elems.into_iter().collect::<BTreeMap<String, &V>>())
}
