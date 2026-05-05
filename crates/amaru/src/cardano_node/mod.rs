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

use std::{collections::BTreeMap, time::Duration};

use amaru_kernel::{
    Account, Bytes, DRep, EraBound, EraHistory, EraName, EraParams, EraSummary, Hash, HeaderHash, Lovelace, Network,
    NetworkName, Nonce, Nullable, Point, PoolId, PoolMetadata, PoolParams, RationalNumber, Relay, RewardAccount, Set,
    StakeCredential, StakePayload, StrictMaybe, cbor, new_stake_address, reward_account_to_stake_credential, size,
};
use minicbor::Decoder;

use crate::bootstrap::InitialNonces;

pub(crate) mod mempack;
pub mod tvar;
pub struct ParsedStateSnapshot {
    pub slot: u64,
    pub hash: HeaderHash,
    pub era_history: EraHistory,
    pub ledger_data_begin: usize,
    pub ledger_data_end: usize,
}

pub fn parse_state_snapshot(
    d: &mut Decoder<'_>,
    network: &NetworkName,
) -> Result<ParsedStateSnapshot, Box<dyn std::error::Error>> {
    d.array()?;

    // version
    // https://github.com/abailly/ouroboros-consensus/blob/1508638f832772d21874e18e48b908fcb791cd49/ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Util/Versioned.hs#L95
    d.skip()?;

    // ext ledger state
    // https://github.com/abailly/ouroboros-consensus/blob/1508638f832772d21874e18e48b908fcb791cd49/ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Ledger/Extended.hs#L232
    d.array()?;

    // ledger state
    d.array()?;

    let mut eras: Vec<EraSummary> = decode_eras(d, network)?;

    d.array()?;
    let start: EraBound = d.decode()?;
    eras.push(EraSummary {
        start,
        end: None,
        params: EraParams {
            epoch_size_slots: network.default_epoch_size_in_slots(),
            slot_length: Duration::from_secs(1),
            era_name: EraName::Conway,
        },
    });

    let era_history = EraHistory::new(&eras, network.default_stability_window());

    // ledger state
    // https://github.com/abailly/ouroboros-consensus/blob/1508638f832772d21874e18e48b908fcb791cd49/ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Ledger.hs#L736
    d.array()?;

    // encoding version (2)
    d.skip()?;

    d.array()?;
    // tip
    // https://github.com/abailly/ouroboros-consensus/blob/1508638f832772d21874e18e48b908fcb791cd49/ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Ledger.hs#L694
    // the Tip is wrapped in a WithOrigin type hence the double array
    d.array()?;
    d.array()?;
    let slot = d.u64()?;
    let _height = d.u64()?;
    let hash: HeaderHash = d.decode()?;

    let begin = d.position();
    d.skip()?;
    let end = d.position();

    Ok(ParsedStateSnapshot { slot, hash, era_history, ledger_data_begin: begin, ledger_data_end: end })
}

fn extract_snapshot_nonces_after_prefix(
    d: &mut Decoder<'_>,
    parsed_snapshot: &ParsedStateSnapshot,
    tail: HeaderHash,
) -> Result<InitialNonces, Box<dyn std::error::Error>> {
    let at = Point::Specific(parsed_snapshot.slot.into(), parsed_snapshot.hash);

    d.skip().map_err(|err| format!("skip shelley transition: {err}"))?;
    d.skip().map_err(|err| format!("skip latest peras cert round: {err}"))?;

    // header state
    d.array().map_err(|err| format!("decode header state: {err}"))?;
    d.skip().map_err(|err| format!("skip header state tip: {err}"))?;

    // ChainDepState for Praos
    d.array().map_err(|err| format!("decode chain dep state: {err}"))?;
    d.skip().map_err(|err| format!("skip hfc state 1: {err}"))?;
    d.skip().map_err(|err| format!("skip hfc state 2: {err}"))?;
    d.skip().map_err(|err| format!("skip hfc state 3: {err}"))?;
    d.skip().map_err(|err| format!("skip hfc state 4: {err}"))?;
    d.skip().map_err(|err| format!("skip hfc state 5: {err}"))?;
    d.skip().map_err(|err| format!("skip hfc state 6: {err}"))?;

    // the actual PraosState
    d.array().map_err(|err| format!("decode praos state: {err}"))?;
    d.skip().map_err(|err| format!("skip praos era bounds: {err}"))?;

    // versioned TickedChainDepState
    d.array().map_err(|err| format!("decode ticked chain dep state: {err}"))?;
    d.skip().map_err(|err| format!("skip ticked chain dep state version: {err}"))?;
    d.array().map_err(|err| format!("decode praos payload: {err}"))?;

    // last slot
    d.array().map_err(|err| format!("decode last slot wrapper: {err}"))?;
    d.skip().map_err(|err| format!("skip last slot tag: {err}"))?;
    d.u64().map_err(|err| format!("decode last slot: {err}"))?;
    d.skip().map_err(|err| format!("skip ocert counters: {err}"))?;

    d.array().map_err(|err| format!("decode evolving nonce wrapper: {err}"))?;
    d.skip().map_err(|err| format!("skip evolving nonce tag: {err}"))?;
    let evolving: Nonce = d.decode().map_err(|err| format!("decode evolving nonce: {err}"))?;

    d.array().map_err(|err| format!("decode candidate nonce wrapper: {err}"))?;
    d.skip().map_err(|err| format!("skip candidate nonce tag: {err}"))?;
    let candidate: Nonce = d.decode().map_err(|err| format!("decode candidate nonce: {err}"))?;

    d.array().map_err(|err| format!("decode active nonce wrapper: {err}"))?;
    d.skip().map_err(|err| format!("skip active nonce tag: {err}"))?;
    let active: Nonce = d.decode().map_err(|err| format!("decode active nonce: {err}"))?;

    d.skip().map_err(|err| format!("skip lab nonce: {err}"))?;
    d.skip().map_err(|err| format!("skip last epoch nonce: {err}"))?;

    Ok(InitialNonces { at, active, evolving, candidate, tail })
}

pub fn parse_state_snapshot_with_nonces(
    mut d: Decoder<'_>,
    network: &NetworkName,
    tail: HeaderHash,
) -> Result<(ParsedStateSnapshot, InitialNonces), Box<dyn std::error::Error>> {
    let parsed_snapshot =
        parse_state_snapshot(&mut d, network).map_err(|err| format!("parse state snapshot prefix: {err}"))?;
    let initial_nonces = extract_snapshot_nonces_after_prefix(&mut d, &parsed_snapshot, tail)?;

    Ok((parsed_snapshot, initial_nonces))
}

/// This is the number of past eras before the current era in the "standard" Cardano history, e.g
/// from Byron to Babbage. Bump this number when a hard fork happens.
pub const PAST_ERAS_NUMBER: u8 = 6;

fn decode_eras(
    d: &mut minicbor::Decoder<'_>,
    network: &NetworkName,
) -> Result<Vec<EraSummary>, Box<dyn std::error::Error>> {
    let mut eras = Vec::new();

    for era_tag in 1..=PAST_ERAS_NUMBER {
        d.array()?;
        let start: EraBound = d.decode()?;
        let end: EraBound = d.decode()?;
        let params = if end.slot == 0.into() {
            #[expect(clippy::expect_used)]
            EraParams {
                epoch_size_slots: network.default_epoch_size_in_slots(),
                slot_length: Duration::from_secs(0),
                era_name: EraName::try_from(era_tag).expect("iteration over known era tags"),
            }
        } else {
            let end_slot = u64::from(end.slot);
            let start_slot = u64::from(start.slot);
            let end_epoch = u64::from(end.epoch);
            let start_epoch = u64::from(start.epoch);
            let end_ms = end.time.as_millis() as u64;
            let start_ms = start.time.as_millis() as u64;

            if end_slot <= start_slot || end_epoch <= start_epoch {
                return Err("Invalid era bounds (non-increasing)".into());
            }
            let slots_elapsed = end_slot - start_slot;
            let epochs_elapsed = end_epoch - start_epoch;
            let time_ms_elapsed = end_ms.saturating_sub(start_ms);

            // end_slot > start_slot => slots_elapsed > 0
            let slot_length = Duration::from_millis(time_ms_elapsed / slots_elapsed);

            #[expect(clippy::expect_used)]
            EraParams {
                epoch_size_slots: slots_elapsed / epochs_elapsed,
                slot_length,
                era_name: EraName::try_from(era_tag).expect("iteration over known era tags"),
            }
        };
        let summary = EraSummary { start, end: Some(end), params };
        eras.push(summary);
    }
    Ok(eras)
}

pub(crate) fn decode_node_pool_state(
    d: &mut cbor::Decoder<'_>,
    network: NetworkName,
) -> Result<
    (BTreeMap<PoolId, PoolParams>, BTreeMap<PoolId, PoolParams>, BTreeMap<PoolId, amaru_kernel::Epoch>),
    cbor::decode::Error,
> {
    d.array()?;

    let mut node_network = network;
    let _pool_deposits: BTreeMap<PoolId, Lovelace> =
        d.decode().map_err(|err| cbor::decode::Error::message(format!("node pool deposits: {err}")))?;
    let pools: BTreeMap<PoolId, NodePoolParams> =
        d.decode_with(&mut node_network).map_err(|err| cbor::decode::Error::message(format!("node pools: {err}")))?;
    let pools_updates: BTreeMap<PoolId, NodePoolUpdateParams> = d
        .decode_with(&mut node_network)
        .map_err(|err| cbor::decode::Error::message(format!("node pool updates: {err}")))?;
    let pools_retirements: BTreeMap<PoolId, amaru_kernel::Epoch> =
        d.decode().map_err(|err| cbor::decode::Error::message(format!("node pool retirements: {err}")))?;

    Ok((
        pools.into_iter().map(|(id, params)| (id, params.into_pool_params(id))).collect(),
        pools_updates.into_iter().map(|(id, params)| (id, params.into_pool_params(id))).collect(),
        pools_retirements,
    ))
}

pub(crate) fn decode_node_accounts(
    d: &mut cbor::Decoder<'_>,
) -> Result<BTreeMap<StakeCredential, Account>, cbor::decode::Error> {
    let accounts: BTreeMap<StakeCredential, NodeAccount> = d.decode()?;
    let mut pointers: BTreeMap<StakeCredential, Set<(u64, u64, u64)>> = d.decode()?;

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

impl NodePoolUpdateParams {
    fn into_pool_params(self, id: PoolId) -> PoolParams {
        self.0.into_pool_params(id)
    }
}

impl<'b> cbor::decode::Decode<'b, NetworkName> for NodePoolParams {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut NetworkName) -> Result<Self, cbor::decode::Error> {
        let len = d.array()?;

        let vrf = d.decode_with(ctx).map_err(|err| cbor::decode::Error::message(format!("node pool vrf: {err}")))?;
        let pledge =
            d.decode_with(ctx).map_err(|err| cbor::decode::Error::message(format!("node pool pledge: {err}")))?;
        let cost = d.decode_with(ctx).map_err(|err| cbor::decode::Error::message(format!("node pool cost: {err}")))?;
        let margin =
            d.decode_with(ctx).map_err(|err| cbor::decode::Error::message(format!("node pool margin: {err}")))?;
        let reward_account = {
            let reward_account: NodeRewardAccount = d
                .decode_with(ctx)
                .map_err(|err| cbor::decode::Error::message(format!("node pool reward account: {err}")))?;
            reward_account.0
        };
        let owners =
            d.decode_with(ctx).map_err(|err| cbor::decode::Error::message(format!("node pool owners: {err}")))?;
        let relays =
            d.decode_with(ctx).map_err(|err| cbor::decode::Error::message(format!("node pool relays: {err}")))?;
        let metadata =
            d.decode_with(ctx).map_err(|err| cbor::decode::Error::message(format!("node pool metadata: {err}")))?;

        match len {
            Some(total) => {
                for _ in 8..total {
                    d.skip()?;
                }
            }
            None => {
                while d.datatype()? != cbor::data::Type::Break {
                    d.skip()?;
                }
                d.skip()?;
            }
        }

        Ok(NodePoolParams { vrf, pledge, cost, margin, reward_account, owners, relays, metadata })
    }
}

impl<'b> cbor::decode::Decode<'b, NetworkName> for NodePoolUpdateParams {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut NetworkName) -> Result<Self, cbor::decode::Error> {
        let len = d.array()?;

        let _operator: PoolId = d
            .decode_with(ctx)
            .map_err(|err| cbor::decode::Error::message(format!("node pool update operator: {err}")))?;

        let vrf =
            d.decode_with(ctx).map_err(|err| cbor::decode::Error::message(format!("node pool update vrf: {err}")))?;
        let pledge = d
            .decode_with(ctx)
            .map_err(|err| cbor::decode::Error::message(format!("node pool update pledge: {err}")))?;
        let cost =
            d.decode_with(ctx).map_err(|err| cbor::decode::Error::message(format!("node pool update cost: {err}")))?;
        let margin = d
            .decode_with(ctx)
            .map_err(|err| cbor::decode::Error::message(format!("node pool update margin: {err}")))?;
        let reward_account = {
            let reward_account: NodeRewardAccount = d
                .decode_with(ctx)
                .map_err(|err| cbor::decode::Error::message(format!("node pool update reward account: {err}")))?;
            reward_account.0
        };
        let owners = d
            .decode_with(ctx)
            .map_err(|err| cbor::decode::Error::message(format!("node pool update owners: {err}")))?;
        let relays = d
            .decode_with(ctx)
            .map_err(|err| cbor::decode::Error::message(format!("node pool update relays: {err}")))?;
        let metadata = {
            let metadata: NodePoolUpdateMetadata = d
                .decode_with(ctx)
                .map_err(|err| cbor::decode::Error::message(format!("node pool update metadata: {err}")))?;
            metadata.0
        };

        match len {
            Some(total) => {
                for _ in 9..total {
                    d.skip()?;
                }
            }
            None => {
                while d.datatype()? != cbor::data::Type::Break {
                    d.skip()?;
                }
                d.skip()?;
            }
        }

        Ok(NodePoolUpdateParams(NodePoolParams { vrf, pledge, cost, margin, reward_account, owners, relays, metadata }))
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
        let credential = match d.datatype()? {
            cbor::data::Type::Bytes | cbor::data::Type::BytesIndef => {
                let reward_account: RewardAccount = d.decode_with(ctx)?;
                reward_account_to_stake_credential(&reward_account)
                    .ok_or_else(|| cbor::decode::Error::message("unexpected malformed node reward account bytes"))?
            }
            cbor::data::Type::Array | cbor::data::Type::ArrayIndef => d.decode_with(ctx)?,
            other => return Err(cbor::decode::Error::type_mismatch(other)),
        };

        let payload = match credential {
            StakeCredential::AddrKeyhash(hash) => StakePayload::Stake(hash),
            StakeCredential::ScriptHash(hash) => StakePayload::Script(hash),
        };
        let network: Network = (*ctx).into();

        Ok(Self(Bytes::from(new_stake_address(network, payload).to_vec())))
    }
}
