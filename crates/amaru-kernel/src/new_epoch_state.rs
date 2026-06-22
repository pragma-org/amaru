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

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Account, Bytes, CertificatePointer, DRep, DRepRegistration, DRepState, Epoch, Hash, Lovelace, Network, NetworkName,
    Nullable, PoolId, PoolMetadata, PoolParams, RationalNumber, Relay, RewardAccount, Set, StakeCredential,
    StakePayload, StrictMaybe, cbor, new_stake_address, reward_account_to_stake_credential, size,
};

/// The set of registered pool ids from a decoded pool state. The read-path only
/// needs pool existence, not the parameters.
pub fn pool_ids(pools: BTreeMap<PoolId, PoolParams>) -> BTreeSet<PoolId> {
    pools.into_keys().collect()
}

// NOTE:  DRep registration pointer fabrication
//
// a NewEpochState records no DRep registration pointer, so callers stamp a
// synthesized `registered_at`. Any rule that orders against it,
// e.g. "vote delegation must follow DRep registration", can't be meaningfully
// checked on snapshot-seeded state; exercising that ordering needs an in-block
// registration instead.
pub fn drep_registration(state: DRepState, registered_at: CertificatePointer) -> DRepRegistration {
    DRepRegistration { deposit: state.deposit, registered_at, valid_until: state.expiry }
}

pub fn decode_node_pool_state(
    d: &mut cbor::Decoder<'_>,
    network: NetworkName,
) -> Result<(BTreeMap<PoolId, PoolParams>, BTreeMap<PoolId, PoolParams>, BTreeMap<PoolId, Epoch>), cbor::decode::Error>
{
    d.array()?;

    let mut node_network = network;
    let _pool_vrf_key_hashes: BTreeMap<Hash<{ size::VRF_KEY }>, u64> =
        d.decode().map_err(|err| contextualize_decode_error("node pool vrf key hashes", err))?;
    let pools = decode_node_pool_map(d, &mut node_network, "node pools", |d, network| {
        let params: NodePoolStateParams = d.decode_with(network)?;
        Ok(params)
    })?;
    let pools_updates = decode_node_pool_map(d, &mut node_network, "node pool updates", |d, network| {
        let params: NodePoolUpdateParams = d.decode_with(network)?;
        Ok(params)
    })?;
    let pools_retirements: BTreeMap<PoolId, Epoch> =
        d.decode().map_err(|err| contextualize_decode_error("node pool retirements", err))?;

    Ok((
        pools.into_iter().map(|(id, params)| (id, params.into_pool_params(id))).collect(),
        pools_updates.into_iter().map(|(id, params)| (id, params.into_pool_params(id))).collect(),
        pools_retirements,
    ))
}

fn decode_node_pool_map<T>(
    d: &mut cbor::Decoder<'_>,
    network: &mut NetworkName,
    field_name: &'static str,
    mut decode_value: impl FnMut(&mut cbor::Decoder<'_>, &mut NetworkName) -> Result<T, cbor::decode::Error>,
) -> Result<BTreeMap<PoolId, T>, cbor::decode::Error> {
    let len = d.map().map_err(|err| contextualize_decode_error(field_name, err))?;
    let mut entries = BTreeMap::new();
    let mut index = 0_u64;

    loop {
        match len {
            Some(total) if index == total => break,
            None if d.datatype()? == cbor::data::Type::Break => {
                d.skip()?;
                break;
            }
            _ => {}
        }

        let key_offset = d.position();
        let pool_id: PoolId = d.decode_with(network).map_err(|err| {
            contextualize_decode_error(format!("{field_name} key at entry {index} offset {key_offset}"), err)
        })?;
        let value_offset = d.position();
        let value = decode_value(d, network).map_err(|err| {
            contextualize_decode_error(format!("{field_name} value at entry {index} offset {value_offset}"), err)
        })?;
        entries.insert(pool_id, value);
        index += 1;
    }

    Ok(entries)
}

pub fn decode_node_accounts(
    d: &mut cbor::Decoder<'_>,
) -> Result<BTreeMap<StakeCredential, Account>, cbor::decode::Error> {
    d.array()?;
    let accounts: BTreeMap<StakeCredential, NodeAccount> = d.decode()?;
    let mut pointers: BTreeMap<StakeCredential, Set<(u64, u64, u64)>> = d.decode()?;
    d.skip()?; // dsFutureGenDelegs
    d.skip()?; // dsGenDelegs

    Ok(accounts
        .into_iter()
        .map(|(credential, account)| {
            let pointers = pointers.remove(&credential).unwrap_or_else(|| Vec::new().into());
            (credential, account.into_account(pointers))
        })
        .collect())
}

#[derive(Debug)]
struct NodePoolParams {
    vrf: Hash<{ size::VRF_KEY }>,
    pledge: Lovelace,
    cost: Lovelace,
    margin: RationalNumber,
    reward_account: RewardAccount,
    owners: Set<Hash<{ size::KEY }>>,
    relays: Vec<Relay>,
    metadata: StrictMaybe<PoolMetadata>,
}

impl NodePoolParams {
    fn into_pool_params(self, id: PoolId) -> PoolParams {
        PoolParams {
            id,
            vrf: self.vrf,
            pledge: self.pledge,
            cost: self.cost,
            margin: self.margin,
            reward_account: self.reward_account,
            owners: self.owners,
            relays: self.relays,
            metadata: match self.metadata {
                StrictMaybe::Nothing => Nullable::Null,
                StrictMaybe::Just(metadata) => Nullable::Some(metadata),
            },
        }
    }
}

#[derive(Debug)]
struct NodePoolUpdateParams(NodePoolParams);

#[derive(Debug)]
struct NodePoolStateParams(NodePoolParams);

impl NodePoolUpdateParams {
    fn into_pool_params(self, id: PoolId) -> PoolParams {
        self.0.into_pool_params(id)
    }
}

impl NodePoolStateParams {
    fn into_pool_params(self, id: PoolId) -> PoolParams {
        self.0.into_pool_params(id)
    }
}

fn decode_optional_node_pool_metadata(
    d: &mut cbor::Decoder<'_>,
    len: Option<u64>,
    fields_before_metadata: u64,
    decode_metadata: impl FnOnce(&mut cbor::Decoder<'_>) -> Result<StrictMaybe<PoolMetadata>, cbor::decode::Error>,
) -> Result<(StrictMaybe<PoolMetadata>, u64, bool), cbor::decode::Error> {
    match len {
        Some(total) if total <= fields_before_metadata => Ok((StrictMaybe::Nothing, fields_before_metadata, false)),
        None if d.datatype()? == cbor::data::Type::Break => {
            d.skip()?;
            Ok((StrictMaybe::Nothing, fields_before_metadata, true))
        }
        _ => Ok((decode_metadata(d)?, fields_before_metadata + 1, false)),
    }
}

fn skip_remaining_array_fields(
    d: &mut cbor::Decoder<'_>,
    len: Option<u64>,
    consumed: u64,
    break_consumed: bool,
) -> Result<(), cbor::decode::Error> {
    match len {
        Some(total) => {
            for _ in consumed..total {
                d.skip()?;
            }
        }
        None if break_consumed => {}
        None => {
            while d.datatype()? != cbor::data::Type::Break {
                d.skip()?;
            }
            d.skip()?;
        }
    }

    Ok(())
}

fn contextualize_decode_error(context: impl Into<String>, err: cbor::decode::Error) -> cbor::decode::Error {
    if err.is_end_of_input() { err } else { cbor::decode::Error::message(format!("{}: {err}", context.into())) }
}

fn skip_node_pool_delegators(d: &mut cbor::Decoder<'_>) -> Result<(), cbor::decode::Error> {
    if d.datatype()? == cbor::data::Type::Tag {
        let found_tag = d.tag().map_err(|err| contextualize_decode_error("node pool delegators tag", err))?;

        if found_tag != cbor::data::Tag::new(258) {
            return Err(cbor::decode::Error::message(format!("unexpected node pool delegators tag: {found_tag:?}")));
        }
    }

    match d.array().map_err(|err| contextualize_decode_error("node pool delegators collection", err))? {
        Some(total) => {
            for index in 0..total {
                d.skip()
                    .map_err(|err| contextualize_decode_error(format!("node pool delegators element {index}"), err))?;
            }
        }
        None => {
            let mut index = 0_u64;

            while d.datatype()? != cbor::data::Type::Break {
                d.skip()
                    .map_err(|err| contextualize_decode_error(format!("node pool delegators element {index}"), err))?;
                index += 1;
            }
            d.skip().map_err(|err| contextualize_decode_error("node pool delegators break", err))?;
        }
    }

    Ok(())
}

impl<'b> cbor::decode::Decode<'b, NetworkName> for NodePoolParams {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut NetworkName) -> Result<Self, cbor::decode::Error> {
        let len = d.array().map_err(|err| contextualize_decode_error("node pool entry", err))?;

        let vrf = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool vrf", err))?;
        let pledge = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool pledge", err))?;
        let cost = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool cost", err))?;
        let margin = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool margin", err))?;
        let reward_account = {
            let reward_account: NodeRewardAccount =
                d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool reward account", err))?;
            reward_account.0
        };
        let owners = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool owners", err))?;
        let relays = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool relays", err))?;
        let (metadata, consumed, break_consumed) = decode_optional_node_pool_metadata(d, len, 7, |d| {
            d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool metadata", err))
        })?;

        skip_remaining_array_fields(d, len, consumed, break_consumed)
            .map_err(|err| contextualize_decode_error("node pool trailing fields", err))?;

        Ok(NodePoolParams { vrf, pledge, cost, margin, reward_account, owners, relays, metadata })
    }
}

impl<'b> cbor::decode::Decode<'b, NetworkName> for NodePoolUpdateParams {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut NetworkName) -> Result<Self, cbor::decode::Error> {
        let len = d.array().map_err(|err| contextualize_decode_error("node pool update entry", err))?;

        let _operator: PoolId =
            d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update operator", err))?;

        let vrf = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update vrf", err))?;
        let pledge = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update pledge", err))?;
        let cost = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update cost", err))?;
        let margin = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update margin", err))?;
        let reward_account = {
            let reward_account: NodeRewardAccount =
                d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update reward account", err))?;
            reward_account.0
        };
        let owners = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update owners", err))?;
        let relays = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update relays", err))?;
        let (metadata, consumed, break_consumed) = decode_optional_node_pool_metadata(d, len, 8, |d| {
            let metadata: NodePoolUpdateMetadata =
                d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool update metadata", err))?;
            Ok(metadata.0)
        })?;

        skip_remaining_array_fields(d, len, consumed, break_consumed)
            .map_err(|err| contextualize_decode_error("node pool update trailing fields", err))?;

        Ok(NodePoolUpdateParams(NodePoolParams { vrf, pledge, cost, margin, reward_account, owners, relays, metadata }))
    }
}

impl<'b> cbor::decode::Decode<'b, NetworkName> for NodePoolStateParams {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut NetworkName) -> Result<Self, cbor::decode::Error> {
        let len = d.array().map_err(|err| contextualize_decode_error("node pool entry", err))?;

        let vrf = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool vrf", err))?;
        let pledge = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool pledge", err))?;
        let cost = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool cost", err))?;
        let margin = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool margin", err))?;
        let reward_account = {
            let reward_account: NodeRewardAccount =
                d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool reward account", err))?;
            reward_account.0
        };
        let owners = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool owners", err))?;
        let relays = d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool relays", err))?;
        let (metadata, consumed, _) = decode_optional_node_pool_metadata(d, len, 7, |d| {
            d.decode_with(ctx).map_err(|err| contextualize_decode_error("node pool metadata", err))
        })?;

        d.skip().map_err(|err| {
            contextualize_decode_error(format!("node pool deposit (len={len:?}, consumed={consumed})"), err)
        })?;

        let consumed = consumed + 1;
        let (consumed, break_consumed) = match len {
            Some(total) if total <= consumed => (consumed, false),
            None if d.datatype()? == cbor::data::Type::Break => {
                d.skip()?;
                (consumed, true)
            }
            _ => {
                skip_node_pool_delegators(d).map_err(|err| {
                    contextualize_decode_error(format!("node pool delegators (len={len:?}, consumed={consumed})"), err)
                })?;
                (consumed + 1, false)
            }
        };

        skip_remaining_array_fields(d, len, consumed, break_consumed)
            .map_err(|err| contextualize_decode_error("node pool trailing fields", err))?;

        Ok(NodePoolStateParams(NodePoolParams { vrf, pledge, cost, margin, reward_account, owners, relays, metadata }))
    }
}

struct NodePoolUpdateMetadata(StrictMaybe<PoolMetadata>);

impl<'b> cbor::decode::Decode<'b, NetworkName> for NodePoolUpdateMetadata {
    #[allow(clippy::wildcard_enum_match_arm)]
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut NetworkName) -> Result<Self, cbor::decode::Error> {
        match d.datatype()? {
            cbor::data::Type::Array | cbor::data::Type::ArrayIndef => {
                let mut probe = d.probe();
                let len = probe.array()?;
                if len == Some(0) {
                    d.array()?;
                    Ok(Self(StrictMaybe::Nothing))
                } else if matches!(probe.datatype()?, cbor::data::Type::String | cbor::data::Type::StringIndef) {
                    let metadata: PoolMetadata = d.decode_with(ctx)?;
                    Ok(Self(StrictMaybe::Just(metadata)))
                } else {
                    let metadata: StrictMaybe<PoolMetadata> = d.decode_with(ctx)?;
                    Ok(Self(metadata))
                }
            }
            other => Err(cbor::decode::Error::type_mismatch(other)),
        }
    }
}

#[derive(Debug)]
struct NodeAccount {
    rewards: Lovelace,
    deposit: Lovelace,
    pool: Nullable<PoolId>,
    drep: Nullable<DRep>,
}

impl NodeAccount {
    fn into_account(self, pointers: Set<(u64, u64, u64)>) -> Account {
        Account {
            rewards_and_deposit: if self.rewards == 0 && self.deposit == 0 {
                StrictMaybe::Nothing
            } else {
                StrictMaybe::Just((self.rewards, self.deposit))
            },
            pointers,
            pool: match self.pool {
                Nullable::Some(pool) => StrictMaybe::Just(pool),
                Nullable::Null | Nullable::Undefined => StrictMaybe::Nothing,
            },
            drep: match self.drep {
                Nullable::Some(drep) => StrictMaybe::Just(drep),
                Nullable::Null | Nullable::Undefined => StrictMaybe::Nothing,
            },
        }
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for NodeAccount {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;

        Ok(NodeAccount {
            rewards: d.decode_with(ctx)?,
            deposit: d.decode_with(ctx)?,
            pool: d.decode_with(ctx)?,
            drep: d.decode_with(ctx)?,
        })
    }
}

struct NodeRewardAccount(RewardAccount);

impl<'b> cbor::decode::Decode<'b, NetworkName> for NodeRewardAccount {
    #[allow(clippy::wildcard_enum_match_arm)]
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut NetworkName) -> Result<Self, cbor::decode::Error> {
        match d.datatype()? {
            cbor::data::Type::Bytes | cbor::data::Type::BytesIndef => {
                let reward_account: RewardAccount = d.decode_with(ctx)?;
                reward_account_to_stake_credential(&reward_account)
                    .ok_or_else(|| cbor::decode::Error::message("unexpected malformed node reward account bytes"))?;

                Ok(Self(reward_account))
            }
            cbor::data::Type::Array | cbor::data::Type::ArrayIndef => {
                let credential = d.decode_with(ctx)?;
                let network: Network = (*ctx).into();
                let payload = match credential {
                    StakeCredential::AddrKeyhash(hash) => StakePayload::Stake(hash),
                    StakeCredential::ScriptHash(hash) => StakePayload::Script(hash),
                };

                Ok(Self(Bytes::from(new_stake_address(network, payload).to_vec())))
            }
            other => Err(cbor::decode::Error::type_mismatch(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{NodeRewardAccount, decode_optional_node_pool_metadata, skip_remaining_array_fields};
    use crate::{Bytes, Hash, NetworkName, StakeCredential, StrictMaybe, cbor};

    #[test]
    fn missing_optional_metadata_in_definite_arrays_is_treated_as_nothing() {
        let bytes = [0x82, 0x01, 0x02];
        let mut decoder = cbor::Decoder::new(&bytes);
        let len = decoder.array().unwrap();

        assert_eq!(decoder.u8().unwrap(), 1);
        assert_eq!(decoder.u8().unwrap(), 2);

        let (metadata, consumed, break_consumed) =
            decode_optional_node_pool_metadata(&mut decoder, len, 2, |_| Ok(StrictMaybe::Nothing)).unwrap();

        assert!(matches!(metadata, StrictMaybe::Nothing));
        assert_eq!(consumed, 2);
        assert!(!break_consumed);

        skip_remaining_array_fields(&mut decoder, len, consumed, break_consumed).unwrap();
        assert!(decoder.datatype().is_err());
    }

    #[test]
    fn missing_optional_metadata_in_indefinite_arrays_consumes_break() {
        let bytes = [0x9f, 0x01, 0x02, 0xff];
        let mut decoder = cbor::Decoder::new(&bytes);
        let len = decoder.array().unwrap();

        assert_eq!(decoder.u8().unwrap(), 1);
        assert_eq!(decoder.u8().unwrap(), 2);

        let (metadata, consumed, break_consumed) =
            decode_optional_node_pool_metadata(&mut decoder, len, 2, |_| Ok(StrictMaybe::Nothing)).unwrap();

        assert!(matches!(metadata, StrictMaybe::Nothing));
        assert_eq!(consumed, 2);
        assert!(break_consumed);

        skip_remaining_array_fields(&mut decoder, len, consumed, break_consumed).unwrap();
        assert!(decoder.datatype().is_err());
    }

    #[test]
    fn node_reward_account_bytes_preserve_embedded_network() {
        let reward_account =
            Bytes::from(hex::decode("e0e3af434a5516854f20191807cc5ea85b57b4fd0f050f3eab28af19ee").unwrap());
        let bytes = cbor::to_vec(&reward_account).unwrap();
        let mut decoder = cbor::Decoder::new(bytes.as_slice());
        let mut network = NetworkName::Mainnet;

        let decoded: NodeRewardAccount = decoder.decode_with(&mut network).unwrap();

        assert_eq!(decoded.0, reward_account);
    }

    #[test]
    fn node_reward_account_credential_decodes_to_snapshot_network_reward_account() {
        let credential = StakeCredential::AddrKeyhash(Hash::new(
            hex::decode("e3af434a5516854f20191807cc5ea85b57b4fd0f050f3eab28af19ee").unwrap().try_into().unwrap(),
        ));
        let bytes = cbor::to_vec(&credential).unwrap();
        let mut decoder = cbor::Decoder::new(bytes.as_slice());
        let mut network = NetworkName::Mainnet;

        let decoded: NodeRewardAccount = decoder.decode_with(&mut network).unwrap();

        assert_eq!(
            decoded.0,
            Bytes::from(hex::decode("e1e3af434a5516854f20191807cc5ea85b57b4fd0f050f3eab28af19ee").unwrap())
        );
    }
}
