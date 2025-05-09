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

/*
This module acts as an anti-corruption layer between the amaru codebase and pallas. Said
differently, it mostly re-exports primitives and types from pallas, in a way that prevent pallas
imports from spreading across the rest of the ledger sub-component.

It's also the right place to put rather general functions or types that ought to be in pallas.
While elements are being contributed upstream, they might transiently live in this module.
*/

use network::PREPROD_SHELLEY_TRANSITION_EPOCH;
use num::{rational::Ratio, BigUint};
use pallas_addresses::{
    byron::{AddrAttrProperty, AddressPayload},
    Error, *,
};
use pallas_codec::minicbor::{decode, encode, Decode, Decoder, Encode, Encoder};
use pallas_primitives::conway::{
    MintedPostAlonzoTransactionOutput, Redeemer, RedeemersKey, RedeemersValue,
};
use sha3::{Digest as _, Sha3_256};
use std::{
    array::TryFromSliceError,
    cmp::Ordering,
    convert::Infallible,
    fmt::{self, Display, Formatter},
    ops::Deref,
    sync::LazyLock,
};

pub use pallas_addresses::{byron::AddrType, Address, Network, StakeAddress, StakePayload};
pub use pallas_codec::{
    minicbor as cbor,
    utils::{Bytes, KeyValuePairs, NonEmptyKeyValuePairs, Nullable, Set},
};
pub use pallas_crypto::{
    hash::{Hash, Hasher},
    key::ed25519,
};
pub use pallas_primitives::{
    // TODO: Shouldn't re-export alonzo, but prefer exporting unqualified identifiers directly.
    // Investigate.
    alonzo,
    babbage::{Header, MintedHeader},
    conway::{
        AddrKeyhash, Anchor, AuxiliaryData, Block, BootstrapWitness, Certificate, Coin,
        Constitution, CostModel, CostModels, DRep, DRepVotingThresholds, Epoch, ExUnitPrices,
        ExUnits, GovAction, GovActionId as ProposalId, HeaderBody, KeepRaw, MintedBlock,
        MintedTransactionBody, MintedTransactionOutput, MintedTx, MintedWitnessSet, NonEmptySet,
        PoolMetadata, PoolVotingThresholds, PostAlonzoTransactionOutput,
        ProposalProcedure as Proposal, ProtocolParamUpdate, ProtocolVersion,
        PseudoTransactionOutput, RationalNumber, Redeemers, Relay, RewardAccount, ScriptHash,
        StakeCredential, TransactionBody, TransactionInput, TransactionOutput, Tx, UnitInterval,
        VKeyWitness, Value, Voter, VotingProcedure, VotingProcedures, VrfKeyhash, WitnessSet,
    },
};
pub use pallas_traverse::{ComputeHash, OriginalHash};
pub use serde_json as json;
pub use sha3;
pub use slot_arithmetic::{Bound, EraHistory, EraParams, Slot, Summary};

pub mod macros;
pub mod network;
pub mod protocol_parameters;
pub mod serde_utils;

// Constants
// ----------------------------------------------------------------------------

pub const PROTOCOL_VERSION_9: ProtocolVersion = (9, 0);

pub const PROTOCOL_VERSION_10: ProtocolVersion = (10, 0);

/// Maximum supply of Ada, in lovelace (1 Ada = 1,000,000 Lovelace)
pub const MAX_LOVELACE_SUPPLY: u64 = 45000000000000000;

/// The maximum depth of a rollback, also known as the security parameter 'k'.
/// This translates down to the length of our volatile storage, containing states of the ledger
/// which aren't yet considered final.
///
// FIXME: import from genesis configuration
pub const CONSENSUS_SECURITY_PARAM: usize = 2160;

/// Multiplier applied to the CONSENSUS_SECURITY_PARAM to determine Shelley's epoch length.
pub const SHELLEY_EPOCH_LENGTH_SCALE_FACTOR: usize = 10;

/// Inverse of the active slot coefficient (i.e. 1/f);
pub const ACTIVE_SLOT_COEFF_INVERSE: usize = 20;

/// Number of slots in a Shelley epoch
pub const SHELLEY_EPOCH_LENGTH: usize =
    ACTIVE_SLOT_COEFF_INVERSE * SHELLEY_EPOCH_LENGTH_SCALE_FACTOR * CONSENSUS_SECURITY_PARAM;

/// Relative slot from which data of the previous epoch can be considered stable.
pub const STABILITY_WINDOW: usize = ACTIVE_SLOT_COEFF_INVERSE * CONSENSUS_SECURITY_PARAM * 2;

/// Multiplier applied to the CONSENSUS_SECURITY_PARAM to determine Byron's epoch length.
pub const BYRON_EPOCH_LENGTH_SCALE_FACTOR: usize = 10;

/// Number of blocks in a Byron epoch
pub const BYRON_EPOCH_LENGTH: usize = BYRON_EPOCH_LENGTH_SCALE_FACTOR * CONSENSUS_SECURITY_PARAM;

/// Number of slots in the Byron era, for PreProd
pub const BYRON_TOTAL_SLOTS: usize = BYRON_EPOCH_LENGTH * PREPROD_SHELLEY_TRANSITION_EPOCH;

/// Value, in Lovelace, that one must deposit when registering a new stake pool
pub const STAKE_POOL_DEPOSIT: usize = 500000000;

/// Value, in Lovelace, that one must deposit when registering a new stake credential
pub const STAKE_CREDENTIAL_DEPOSIT: usize = 2000000;

/// Number of slots for a single KES validity period.
pub const SLOTS_PER_KES_PERIOD: u64 = 129600;

/// Maximum number of KES key evolution. Combined with SLOTS_PER_KES_PERIOD, these values
/// indicates the validity period of a KES key before a new one is required.
pub const MAX_KES_EVOLUTION: u8 = 62;

/// Number of slots at the end of each epoch which do NOT contribute randomness to the candidate
/// nonce of the following epoch.
pub const RANDOMNESS_STABILIZATION_WINDOW: u64 =
    4 * (CONSENSUS_SECURITY_PARAM as u64) * (ACTIVE_SLOT_COEFF_INVERSE as u64);

// The monetary expansion value, a.k.a ρ
pub static MONETARY_EXPANSION: LazyLock<Ratio<BigUint>> =
    LazyLock::new(|| Ratio::new_raw(BigUint::from(3_u64), BigUint::from(1000_u64)));

/// Treasury tax, a.k.a τ
pub static TREASURY_TAX: LazyLock<Ratio<BigUint>> =
    LazyLock::new(|| Ratio::new_raw(BigUint::from(20_u64), BigUint::from(100_u64)));

/// Pledge influence parameter, a.k.a a0
pub static PLEDGE_INFLUENCE: LazyLock<Ratio<BigUint>> =
    LazyLock::new(|| Ratio::new_raw(BigUint::from(3_u64), BigUint::from(10_u64)));

/// The optimal number of stake pools target for the incentives, a.k.a k
pub const OPTIMAL_STAKE_POOLS_COUNT: usize = 500;

/// Epoch duration after which inactive Proposals are considered expired.
pub const GOV_ACTION_LIFETIME: u64 = 6;

/// Epoch duration after which inactive DReps are considered expired.
pub const DREP_EXPIRY: u64 = 20;

// Re-exports & extra aliases
// ----------------------------------------------------------------------------

pub type Lovelace = u64;

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub enum Point {
    Origin,
    Specific(u64, Vec<u8>),
}

impl Point {
    pub fn slot_or_default(&self) -> Slot {
        match self {
            Point::Origin => From::from(0),
            Point::Specific(slot, _) => From::from(*slot),
        }
    }
}

impl Display for Point {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Point::Origin => write!(
                f,
                "0.0000000000000000000000000000000000000000000000000000000000000000"
            ),
            Point::Specific(slot, vec) => write!(f, "{}.{}", slot, hex::encode(vec)),
        }
    }
}

impl From<&Point> for Hash<32> {
    fn from(point: &Point) -> Self {
        match point {
            // By convention, the hash of `Genesis` is all 0s.
            Point::Origin => Hash::from([0; 32]),
            Point::Specific(_, header_hash) => Hash::from(header_hash.as_slice()),
        }
    }
}

impl Encode<()> for Point {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), encode::Error<W::Error>> {
        match self {
            Point::Origin => e.array(0)?,
            Point::Specific(slot, hash) => e.array(2)?.u64(*slot)?.bytes(hash)?,
        };

        Ok(())
    }
}

impl<'b> Decode<'b, ()> for Point {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut ()) -> Result<Self, decode::Error> {
        let size = d.array()?;

        match size {
            Some(0) => Ok(Point::Origin),
            Some(2) => {
                let slot = d.u64()?;
                let hash = d.bytes()?;
                Ok(Point::Specific(slot, Vec::from(hash)))
            }
            _ => Err(decode::Error::message(
                "can't decode Point from array of size",
            )),
        }
    }
}

pub type TransactionId = Hash<32>;

pub type PoolId = Hash<28>;

pub type Nonce = Hash<32>;

pub type Withdrawal = (StakeAddress, Lovelace);

pub struct ExUnitsIter<'a> {
    source: ExUnitsIterSource<'a>,
}

type ExUnitsMapIter<'a> = std::iter::Map<
    std::slice::Iter<'a, (RedeemersKey, RedeemersValue)>,
    fn(&(RedeemersKey, RedeemersValue)) -> ExUnits,
>;

enum ExUnitsIterSource<'a> {
    List(std::iter::Map<std::slice::Iter<'a, Redeemer>, fn(&Redeemer) -> ExUnits>),
    Map(ExUnitsMapIter<'a>),
}

impl Iterator for ExUnitsIter<'_> {
    type Item = ExUnits;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.source {
            ExUnitsIterSource::List(iter) => iter.next(),
            ExUnitsIterSource::Map(iter) => iter.next(),
        }
    }
}

pub trait RedeemersExt {
    fn ex_units_iter(&self) -> ExUnitsIter<'_>;
}

impl RedeemersExt for Redeemers {
    fn ex_units_iter(&self) -> ExUnitsIter<'_> {
        match self {
            Redeemers::List(list) => ExUnitsIter {
                source: ExUnitsIterSource::List(list.iter().map(|r| r.ex_units)),
            },
            Redeemers::Map(map) => ExUnitsIter {
                source: ExUnitsIterSource::Map(map.iter().map(|(_, r)| r.ex_units)),
            },
        }
    }
}

// CBOR conversions
// ----------------------------------------------------------------------------

#[allow(clippy::unwrap_used)]
pub fn to_cbor<T: cbor::Encode<()>>(value: &T) -> Vec<u8> {
    let mut buffer = Vec::new();
    let result: Result<(), cbor::encode::Error<Infallible>> = cbor::encode(value, &mut buffer);
    result.unwrap(); // Infallible
    buffer
}

pub fn from_cbor<T: for<'d> cbor::Decode<'d, ()>>(bytes: &[u8]) -> Option<T> {
    cbor::decode(bytes).ok()
}

// PoolParams
// ----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolParams {
    pub id: PoolId,
    pub vrf: VrfKeyhash,
    pub pledge: Coin,
    pub cost: Coin,
    pub margin: UnitInterval,
    pub reward_account: RewardAccount,
    pub owners: Set<AddrKeyhash>,
    pub relays: Vec<Relay>,
    pub metadata: Nullable<PoolMetadata>,
}

impl<C> cbor::encode::Encode<C> for PoolParams {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(9)?;
        e.encode_with(self.id, ctx)?;
        e.encode_with(self.vrf, ctx)?;
        e.encode_with(self.pledge, ctx)?;
        e.encode_with(self.cost, ctx)?;
        e.encode_with(&self.margin, ctx)?;
        e.encode_with(&self.reward_account, ctx)?;
        e.encode_with(&self.owners, ctx)?;
        e.encode_with(&self.relays, ctx)?;
        e.encode_with(&self.metadata, ctx)?;
        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for PoolParams {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let _len = d.array()?;
        Ok(PoolParams {
            id: d.decode_with(ctx)?,
            vrf: d.decode_with(ctx)?,
            pledge: d.decode_with(ctx)?,
            cost: d.decode_with(ctx)?,
            margin: d.decode_with(ctx)?,
            reward_account: d.decode_with(ctx)?,
            owners: d.decode_with(ctx)?,
            relays: d.decode_with(ctx)?,
            metadata: d.decode_with(ctx)?,
        })
    }
}

impl serde::Serialize for PoolParams {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use pallas_addresses::Address;
        use serde::ser::SerializeStruct;
        use std::collections::BTreeMap;

        fn as_lovelace_map(n: u64) -> BTreeMap<String, BTreeMap<String, u64>> {
            let mut lovelace = BTreeMap::new();
            lovelace.insert("lovelace".to_string(), n);
            let mut ada = BTreeMap::new();
            ada.insert("ada".to_string(), lovelace);
            ada
        }

        fn as_string_ratio(r: &UnitInterval) -> String {
            format!("{}/{}", r.numerator, r.denominator)
        }

        fn as_bech32_addr(bytes: &[u8]) -> Result<String, Error> {
            Address::from_bytes(bytes).and_then(|addr| addr.to_bech32())
        }

        struct WrapRelay<'a>(&'a Relay);

        impl serde::Serialize for WrapRelay<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                match self.0 {
                    Relay::SingleHostAddr(port, ipv4, ipv6) => {
                        let mut s = serializer.serialize_struct("Relay::SingleHostAddr", 4)?;
                        s.serialize_field("type", "ipAddress")?;
                        if let Nullable::Some(ipv4) = ipv4 {
                            s.serialize_field(
                                "ipv4",
                                &format!("{}.{}.{}.{}", ipv4[0], ipv4[1], ipv4[2], ipv4[3]),
                            )?;
                        }
                        if let Nullable::Some(ipv6) = ipv6 {
                            s.serialize_field("ipv6", ipv6)?;
                        }
                        if let Nullable::Some(port) = port {
                            s.serialize_field("port", port)?;
                        }
                        s.end()
                    }
                    Relay::SingleHostName(port, hostname) => {
                        let mut s = serializer.serialize_struct("Relay::SingleHostName", 3)?;
                        s.serialize_field("type", "hostname")?;
                        s.serialize_field("hostname", hostname)?;
                        if let Nullable::Some(port) = port {
                            s.serialize_field("port", port)?;
                        }
                        s.end()
                    }
                    Relay::MultiHostName(hostname) => {
                        let mut s = serializer.serialize_struct("Relay::MultiHostName", 2)?;
                        s.serialize_field("type", "hostname")?;
                        s.serialize_field("hostname", hostname)?;
                        s.end()
                    }
                }
            }
        }

        let mut s = serializer.serialize_struct("PoolParams", 9)?;
        s.serialize_field("id", &hex::encode(self.id))?;
        s.serialize_field("vrfVerificationKeyHash", &hex::encode(self.vrf))?;
        s.serialize_field("pledge", &as_lovelace_map(self.pledge))?;
        s.serialize_field("cost", &as_lovelace_map(self.cost))?;
        s.serialize_field("margin", &as_string_ratio(&self.margin))?;
        s.serialize_field(
            "rewardAccount",
            &as_bech32_addr(&self.reward_account).map_err(serde::ser::Error::custom)?,
        )?;
        s.serialize_field(
            "owners",
            &self.owners.iter().map(hex::encode).collect::<Vec<String>>(),
        )?;
        s.serialize_field(
            "relays",
            &self
                .relays
                .iter()
                .map(WrapRelay)
                .collect::<Vec<WrapRelay<'_>>>(),
        )?;
        if let Nullable::Some(metadata) = &self.metadata {
            s.serialize_field("metadata", metadata)?;
        }
        s.end()
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, PartialOrd, Ord)]
pub struct TransactionPointer {
    pub slot: Slot,
    pub transaction_index: usize,
}

impl<C> cbor::encode::Encode<C> for TransactionPointer {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode_with(self.slot, ctx)?;
        e.encode_with(self.transaction_index, ctx)?;
        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for TransactionPointer {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let _len = d.array()?;
        Ok(TransactionPointer {
            slot: d.decode_with(ctx)?,
            transaction_index: d.decode_with(ctx)?,
        })
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, PartialOrd)]
pub struct CertificatePointer {
    pub transaction: TransactionPointer,
    pub certificate_index: usize,
}

impl CertificatePointer {
    pub fn slot(&self) -> Slot {
        self.transaction.slot
    }
}

impl<C> cbor::encode::Encode<C> for CertificatePointer {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode_with(self.transaction, ctx)?;
        e.encode_with(self.certificate_index, ctx)?;
        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for CertificatePointer {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let _len = d.array()?;
        Ok(CertificatePointer {
            transaction: d.decode_with(ctx)?,
            certificate_index: d.decode_with(ctx)?,
        })
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProposalPointer {
    pub transaction: TransactionPointer,
    pub proposal_index: usize,
}

impl ProposalPointer {
    pub fn slot(&self) -> Slot {
        self.transaction.slot
    }
}

impl<C> cbor::encode::Encode<C> for ProposalPointer {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode_with(self.transaction, ctx)?;
        e.encode_with(self.proposal_index, ctx)?;
        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for ProposalPointer {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let _len = d.array()?;
        Ok(ProposalPointer {
            transaction: d.decode_with(ctx)?,
            proposal_index: d.decode_with(ctx)?,
        })
    }
}

// Helpers
// ----------------------------------------------------------------------------

/// Turn any Bytes-like structure into a sized slice. Useful for crypto operation requiring
/// operands with specific bytes sizes. For example:
///
/// # ```
/// # let public_key: [u8; ed25519::PublicKey::SIZE] = into_sized_array(vkey, |error, expected| {
/// #     InvalidVKeyWitness::InvalidKeySize { error, expected }
/// # })?;
/// # ```
pub fn into_sized_array<const SIZE: usize, E, T>(
    bytes: T,
    into_error: impl Fn(TryFromSliceError, usize) -> E,
) -> Result<[u8; SIZE], E>
where
    T: Deref<Target = Bytes>,
{
    bytes
        .deref()
        .as_slice()
        .try_into()
        .map_err(|e| into_error(e, SIZE))
}

pub fn encode_bech32(hrp: &str, payload: &[u8]) -> Result<String, Box<dyn std::error::Error>> {
    let hrp = bech32::Hrp::parse(hrp)?;
    Ok(bech32::encode::<bech32::Bech32>(hrp, payload)?)
}

/// TODO: Ideally, we should either:
///
/// - Have pallas_traverse or a similar API works directly from base objects instead of KeepRaw
///   objects (i.e. MintedTransactionOutput)
/// - Ensure that our database iterator yields MintedTransactionOutput and not TransactionOutput, so
///   we can use pallas_traverse out of the box.
///
/// Doing the latter properly is a lifetime hell I am not willing to explore right now.
pub trait HasLovelace {
    fn lovelace(&self) -> Lovelace;
}

impl HasLovelace for Value {
    fn lovelace(&self) -> Lovelace {
        match self {
            Value::Coin(lovelace) => *lovelace,
            Value::Multiasset(lovelace, _) => *lovelace,
        }
    }
}

impl HasLovelace for alonzo::Value {
    fn lovelace(&self) -> Lovelace {
        match self {
            alonzo::Value::Coin(lovelace) => *lovelace,
            alonzo::Value::Multiasset(lovelace, _) => *lovelace,
        }
    }
}

impl HasLovelace for TransactionOutput {
    fn lovelace(&self) -> Lovelace {
        match self {
            TransactionOutput::Legacy(legacy) => legacy.amount.lovelace(),
            TransactionOutput::PostAlonzo(modern) => modern.value.lovelace(),
        }
    }
}

impl HasLovelace for MintedTransactionOutput<'_> {
    fn lovelace(&self) -> Lovelace {
        match self {
            PseudoTransactionOutput::Legacy(legacy) => legacy.amount.lovelace(),
            PseudoTransactionOutput::PostAlonzo(modern) => modern.value.lovelace(),
        }
    }
}

/// TODO: See 'output_lovelace', same remark applies.
pub fn output_stake_credential(
    output: &TransactionOutput,
) -> Result<Option<StakeCredential>, Error> {
    let address = Address::from_bytes(match output {
        TransactionOutput::Legacy(legacy) => &legacy.address[..],
        TransactionOutput::PostAlonzo(modern) => &modern.address[..],
    })?;
    //"unable to deserialise address from output: {output:#?}"

    Ok(match address {
        Address::Shelley(shelley) => match shelley.delegation() {
            ShelleyDelegationPart::Key(key) => Some(StakeCredential::AddrKeyhash(*key)),
            ShelleyDelegationPart::Script(script) => Some(StakeCredential::ScriptHash(*script)),
            ShelleyDelegationPart::Pointer(..) | ShelleyDelegationPart::Null => None,
        },
        Address::Byron(..) => None,
        Address::Stake(..) => unreachable!("stake address inside output?"),
    })
}

// StakeAddress
// ----------------------------------------------------------------------------

// TODO: Required because Pallas doesn't export any contructors for StakeAddress directly. Should
// be fixed there.
#[allow(clippy::expect_used)]
pub fn new_stake_address(network: Network, payload: StakePayload) -> StakeAddress {
    let fake_payment_part = ShelleyPaymentPart::Key(Hash::new([
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ]));
    let delegation_part = match payload {
        StakePayload::Stake(hash) => ShelleyDelegationPart::Key(hash),
        StakePayload::Script(hash) => ShelleyDelegationPart::Script(hash),
    };
    StakeAddress::try_from(ShelleyAddress::new(
        network,
        fake_payment_part,
        delegation_part,
    ))
    .expect("has non-empty delegation part")
}

// ProposalId
// ----------------------------------------------------------------------------

#[derive(Debug, Eq, PartialEq)]
// TODO: This type shouldn't exist, and `Ord` / `PartialOrd` should be derived in Pallas on
// 'GovActionId' already.
pub struct ComparableProposalId {
    pub inner: ProposalId,
}

impl From<ProposalId> for ComparableProposalId {
    fn from(inner: ProposalId) -> Self {
        Self { inner }
    }
}

impl From<ComparableProposalId> for ProposalId {
    fn from(comparable: ComparableProposalId) -> ProposalId {
        comparable.inner
    }
}

impl PartialOrd for ComparableProposalId {
    fn partial_cmp(&self, rhs: &Self) -> Option<Ordering> {
        Some(self.cmp(rhs))
    }
}

impl Ord for ComparableProposalId {
    fn cmp(&self, rhs: &Self) -> Ordering {
        match self.inner.transaction_id.cmp(&rhs.inner.transaction_id) {
            Ordering::Equal => self.inner.action_index.cmp(&rhs.inner.action_index),
            ordering @ Ordering::Less | ordering @ Ordering::Greater => ordering,
        }
    }
}

// StakeCredential
// ----------------------------------------------------------------------------

#[derive(Debug)]
pub enum StakeCredentialType {
    VerificationKey,
    Script,
}

impl std::fmt::Display for StakeCredentialType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            StakeCredentialType::VerificationKey => "verification_key",
            StakeCredentialType::Script => "script",
        })
    }
}

pub fn stake_credential_type(credential: &StakeCredential) -> StakeCredentialType {
    match credential {
        StakeCredential::AddrKeyhash(..) => StakeCredentialType::VerificationKey,
        StakeCredential::ScriptHash(..) => StakeCredentialType::Script,
    }
}

pub fn stake_credential_hash(credential: &StakeCredential) -> Hash<28> {
    match credential {
        StakeCredential::AddrKeyhash(hash) => *hash,
        StakeCredential::ScriptHash(hash) => *hash,
    }
}

// This function shouldn't exist and pallas should provide a RewardAccount = (Network,
// StakeCredential) out of the box instead of row bytes.
pub fn reward_account_to_stake_credential(account: &RewardAccount) -> Option<StakeCredential> {
    if let Ok(Address::Stake(stake_addr)) = Address::from_bytes(&account[..]) {
        match stake_addr.payload() {
            StakePayload::Stake(key) => Some(StakeCredential::AddrKeyhash(*key)),
            StakePayload::Script(script) => Some(StakeCredential::ScriptHash(*script)),
        }
    } else {
        None
    }
}

/// An 'unsafe' version of `reward_account_to_stake_credential` that panics when the given
/// RewardAccount isn't a `StakeCredential`.
#[allow(clippy::panic)]
pub fn expect_stake_credential(account: &RewardAccount) -> StakeCredential {
    reward_account_to_stake_credential(account)
        .unwrap_or_else(|| panic!("unexpected malformed reward account: {:?}", account))
}

pub trait HasExUnits {
    fn ex_units(&self) -> Vec<ExUnits>;
}

impl HasExUnits for MintedBlock<'_> {
    fn ex_units(&self) -> Vec<ExUnits> {
        self.transaction_witness_sets
            .iter()
            .flat_map(|witness_set| &witness_set.redeemer)
            .flat_map(|redeemers| redeemers.ex_units_iter())
            .collect()
    }
}

// Calculate the total ex units in a witness set
pub fn to_ex_units(witness_set: WitnessSet) -> ExUnits {
    match witness_set.redeemer {
        Some(redeemers) => match redeemers {
            Redeemers::List(redeemers) => redeemers
                .iter()
                .fold(ExUnits { mem: 0, steps: 0 }, |acc, redeemer| {
                    sum_ex_units(acc, redeemer.ex_units)
                }),
            Redeemers::Map(redeemers_map) => redeemers_map
                .into_iter()
                .fold(ExUnits { mem: 0, steps: 0 }, |acc, (_, redeemer)| {
                    sum_ex_units(acc, redeemer.ex_units)
                }),
        },
        None => ExUnits { mem: 0, steps: 0 },
    }
}

pub trait HasAddress {
    fn address(&self) -> Result<Address, pallas_addresses::Error>;
}

impl HasAddress for TransactionOutput {
    fn address(&self) -> Result<Address, pallas_addresses::Error> {
        match self {
            PseudoTransactionOutput::Legacy(transaction_output) => {
                Address::from_bytes(&transaction_output.address)
            }
            PseudoTransactionOutput::PostAlonzo(modern) => Address::from_bytes(&modern.address),
        }
    }
}

impl<'b> HasAddress for PseudoTransactionOutput<MintedPostAlonzoTransactionOutput<'b>> {
    fn address(&self) -> Result<Address, pallas_addresses::Error> {
        match self {
            PseudoTransactionOutput::Legacy(transaction_output) => {
                Address::from_bytes(&transaction_output.address)
            }
            PseudoTransactionOutput::PostAlonzo(modern) => Address::from_bytes(&modern.address),
        }
    }
}

pub fn to_network_id(network: &Network) -> u8 {
    match network {
        Network::Testnet => 0,
        Network::Mainnet => 1,
        Network::Other(id) => id.clone(),
    }
}

pub trait HasNetwork {
    /// Returns the Network of a given entity
    fn has_network(&self) -> Option<Network>;
}

impl HasNetwork for Address {
    fn has_network(&self) -> Option<Network> {
        match self {
            Address::Byron(address) => address.has_network(),
            Address::Shelley(_) | Address::Stake(_) => self.network(),
        }
    }
}

impl HasNetwork for ByronAddress {
    /// Based off of the CDDL specification for ByronAddress
    /// https://raw.githubusercontent.com/cardano-foundation/CIPs/master/CIP-0019/CIP-0019-byron-addresses.cddl
    fn has_network(&self) -> Option<Network> {
        let x: AddressPayload = from_cbor(&self.payload.0)?;
        for attribute in x.attributes.iter() {
            if let AddrAttrProperty::NetworkTag(network) = attribute {
                if let Some(bytes) = network.deref()[4..4].try_into().ok() {
                    return Some(Network::from(u8::from_be_bytes(bytes)));
                }
            }
        }

        None
    }
}
pub trait HasOwnership {
    /// Returns ownership credential of a given entity, if any.
    ///
    /// TODO: The return type is slightly misleading; we refer to a 'StakeCredential', whereas the
    /// underlying method mainly targets payment credentials in addresses. The reason for this side
    /// step is that there's no 'Credential' type in Pallas unforunately, and so we just borrow the
    /// structure of 'StakeCredential'.
    fn credential(&self) -> Option<StakeCredential>;
}

impl HasOwnership for Address {
    fn credential(&self) -> Option<StakeCredential> {
        match self {
            Address::Byron(_) => None,
            Address::Shelley(shelley_address) => Some(match shelley_address.payment() {
                ShelleyPaymentPart::Key(hash) => StakeCredential::AddrKeyhash(*hash),
                ShelleyPaymentPart::Script(hash) => StakeCredential::ScriptHash(*hash),
            }),
            Address::Stake(stake_address) => Some(match stake_address.payload() {
                StakePayload::Stake(hash) => StakeCredential::AddrKeyhash(*hash),
                StakePayload::Script(hash) => StakeCredential::ScriptHash(*hash),
            }),
        }
    }
}

impl HasOwnership for Voter {
    fn credential(&self) -> Option<StakeCredential> {
        Some(match self {
            Voter::ConstitutionalCommitteeKey(hash)
            | Voter::DRepKey(hash)
            | Voter::StakePoolKey(hash) => StakeCredential::AddrKeyhash(*hash),
            Voter::ConstitutionalCommitteeScript(hash) | Voter::DRepScript(hash) => {
                StakeCredential::ScriptHash(*hash)
            }
        })
    }
}

/// Construct the bootstrap root from a bootstrap witness
pub fn to_root(witness: &BootstrapWitness) -> Hash<28> {
    // CBOR header for data that will be encoded
    let prefix: &[u8] = &[131, 0, 130, 0, 88, 64];

    let mut sha_hasher = Sha3_256::new();
    sha_hasher.update(prefix);
    sha_hasher.update(witness.public_key.deref());
    sha_hasher.update(witness.chain_code.deref());
    sha_hasher.update(witness.attributes.deref());

    let sha_digest = sha_hasher.finalize();
    Hasher::<224>::hash(&sha_digest)
}

/// Create a new `ExUnits` that is the sum of two `ExUnits`
pub fn sum_ex_units(left: ExUnits, right: ExUnits) -> ExUnits {
    ExUnits {
        mem: left.mem + right.mem,
        steps: left.steps + right.steps,
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use test_case::test_case;

    #[test_case((42, 0, 0), (42, 0, 0) => with |(left, right)| assert_eq!(left, right); "reflexivity")]
    #[test_case((42, 0, 0), (43, 0, 0) => with |(left, right)| assert!(left < right); "across slots")]
    #[test_case((42, 0, 0), (42, 1, 0) => with |(left, right)| assert!(left < right); "across transactions")]
    #[test_case((42, 0, 0), (42, 0, 1) => with |(left, right)| assert!(left < right); "across certificates")]
    #[test_case((42, 0, 5), (42, 1, 0) => with |(left, right)| assert!(left < right); "across transactions and certs")]
    fn test_pointers(
        left: (u64, usize, usize),
        right: (u64, usize, usize),
    ) -> (CertificatePointer, CertificatePointer) {
        let new_pointer = |args: (Slot, usize, usize)| CertificatePointer {
            transaction: TransactionPointer {
                slot: args.0,
                transaction_index: args.1,
            },
            certificate_index: args.2,
        };

        (
            new_pointer((From::from(left.0), left.1, left.2)),
            new_pointer((From::from(right.0), right.1, right.2)),
        )
    }

    macro_rules! fixture {
        ($hash:literal) => {
            (
                include_cbor!(concat!("bootstrap_witnesses/", $hash, ".cbor")),
                hash!($hash),
            )
        };
    }

    #[test_case(fixture!("232b6238656c07529e08b152f669507e58e2cb7491d0b586d9dbe425"))]
    #[test_case(fixture!("323ea3dd5b510b1bd5380b413477179df9a6de89027fd817207f32c6"))]
    #[test_case(fixture!("59f44fd32ee319bcea9a51e7b84d7c4cb86f7b9b12f337f6ca9e9c85"))]
    #[test_case(fixture!("65b1fe57f0ed455254aacf1486c448d7f34038c4c445fa905de33d8e"))]
    #[test_case(fixture!("a5a8b29a838ce9525ce6c329c99dc89a31a7d8ae36a844eef55d7eb9"))]
    fn to_root_key_hash((bootstrap_witness, root): (BootstrapWitness, Hash<28>)) {
        assert_eq!(to_root(&bootstrap_witness).as_slice(), root.as_slice())
    }
}
