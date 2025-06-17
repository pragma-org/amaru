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

use pallas_addresses::{
    byron::{AddrAttrProperty, AddressPayload},
    Error, *,
};
use pallas_codec::{
    minicbor::{decode, encode, Decode, Decoder, Encode, Encoder},
    utils::CborWrap,
};
use pallas_primitives::alonzo::Value as AlonzoValue;
use pallas_primitives::{
    conway::{
        MintedPostAlonzoTransactionOutput, NativeScript, PseudoDatumOption, Redeemer, RedeemerTag,
        RedeemersKey, RedeemersValue,
    },
    DatumHash, PlutusData, PlutusScript,
};
use sha3::{Digest as _, Sha3_256};
use std::{
    array::TryFromSliceError,
    cmp::Ordering,
    collections::BTreeSet,
    convert::Infallible,
    fmt::{self, Display, Formatter},
    ops::Deref,
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
    babbage::{Header, MintedHeader},
    conway::{
        AddrKeyhash, Anchor, AuxiliaryData, Block, BootstrapWitness, Certificate, Coin,
        Constitution, CostModel, CostModels, DRep, DRepVotingThresholds, DatumOption, ExUnitPrices,
        ExUnits, GovAction, GovActionId as ProposalId, HeaderBody, KeepRaw, MintedBlock,
        MintedScriptRef, MintedTransactionBody, MintedTransactionOutput, MintedTx,
        MintedWitnessSet, Multiasset, NonEmptySet, NonZeroInt, PoolMetadata, PoolVotingThresholds,
        PostAlonzoTransactionOutput, ProposalProcedure as Proposal, ProtocolParamUpdate,
        ProtocolVersion, PseudoScript, PseudoTransactionOutput, RationalNumber, Redeemers, Relay,
        RewardAccount, ScriptHash, ScriptRef, StakeCredential, TransactionBody, TransactionInput,
        TransactionOutput, Tx, UnitInterval, VKeyWitness, Value, Voter, VotingProcedure,
        VotingProcedures, VrfKeyhash, WitnessSet,
    },
};
pub use pallas_traverse::{ComputeHash, OriginalHash};
pub use serde_json as json;
pub use sha3;
pub use slot_arithmetic::{Bound, EraHistory, EraParams, Slot, Summary};

use crate::network::NetworkName;

pub mod block;
pub mod macros;
pub mod network;
pub mod protocol_parameters;
pub mod serde_utils;

// Constants
// ----------------------------------------------------------------------------

pub const PROTOCOL_VERSION_9: ProtocolVersion = (9, 0);

pub const PROTOCOL_VERSION_10: ProtocolVersion = (10, 0);

// Re-exports & extra aliases
// ----------------------------------------------------------------------------

pub type Lovelace = u64;

pub type EpochInterval = u32;

pub type ScriptPurpose = RedeemerTag;

#[derive(Clone, Eq, PartialEq, Debug, serde::Deserialize)]
pub struct RequiredScript {
    pub hash: ScriptHash,
    pub index: u32,
    pub purpose: ScriptPurpose,
    #[serde(default, deserialize_with = "serde_utils::deserialize_option_proxy")]
    pub datum_option: Option<DatumOption>,
}

impl PartialOrd for RequiredScript {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl From<&RequiredScript> for ScriptHash {
    fn from(value: &RequiredScript) -> Self {
        value.hash
    }
}

impl Ord for RequiredScript {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.hash.cmp(&other.hash) {
            Ordering::Equal => match self.purpose.as_index().cmp(&other.purpose.as_index()) {
                Ordering::Equal => self.index.cmp(&other.index),
                by_purpose @ Ordering::Less | by_purpose @ Ordering::Greater => by_purpose,
            },
            by_hash @ Ordering::Less | by_hash @ Ordering::Greater => by_hash,
        }
    }
}

impl From<&RequiredScript> for RedeemersKey {
    fn from(value: &RequiredScript) -> Self {
        RedeemersKey {
            tag: value.purpose,
            index: value.index,
        }
    }
}

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub enum Point {
    Origin,
    Specific(u64, Vec<u8>),
}

impl Point {
    pub fn slot_or_default(&self) -> Slot {
        match self {
            Point::Origin => Slot::from(0),
            Point::Specific(slot, _) => Slot::from(*slot),
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

/// Convenient type alias to any kind of block
pub type RawBlock = Vec<u8>;

pub const EMPTY_BLOCK: Vec<u8> = vec![];

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

// This allows us to avoid cloning, but it's a pretty awful API.
// Ideally, this is something that Pallas would own and cleanup.
#[derive(Debug, PartialEq, Eq)]
pub enum BorrowedScript<'a> {
    NativeScript(&'a NativeScript),
    PlutusV1Script(&'a PlutusScript<1>),
    PlutusV2Script(&'a PlutusScript<2>),
    PlutusV3Script(&'a PlutusScript<3>),
}

impl<'a> From<&'a PseudoScript<NativeScript>> for BorrowedScript<'a> {
    fn from(value: &'a PseudoScript<NativeScript>) -> Self {
        match value {
            PseudoScript::NativeScript(script) => BorrowedScript::NativeScript(script),
            PseudoScript::PlutusV1Script(script) => BorrowedScript::PlutusV1Script(script),
            PseudoScript::PlutusV2Script(script) => BorrowedScript::PlutusV2Script(script),
            PseudoScript::PlutusV3Script(script) => BorrowedScript::PlutusV3Script(script),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum BorrowedDatumOption<'a> {
    Hash(&'a DatumHash),
    Data(&'a CborWrap<PlutusData>),
}

impl<'a> From<&'a DatumOption> for BorrowedDatumOption<'a> {
    fn from(value: &'a DatumOption) -> Self {
        match value {
            PseudoDatumOption::Hash(hash) => Self::Hash(hash),
            PseudoDatumOption::Data(cbor_wrap) => Self::Data(cbor_wrap),
        }
    }
}

// FIXME: we are cloning here. Can we avoid that?
impl From<BorrowedDatumOption<'_>> for DatumOption {
    fn from(value: BorrowedDatumOption<'_>) -> Self {
        match value {
            BorrowedDatumOption::Hash(hash) => Self::Hash(*hash),
            BorrowedDatumOption::Data(cbor_wrap) => {
                Self::Data(CborWrap(cbor_wrap.to_owned().unwrap()))
            }
        }
    }
}

#[derive(Eq, PartialEq)]
pub struct ScriptRefWithHash<'a> {
    pub hash: ScriptHash,
    pub script: BorrowedScript<'a>,
}

impl PartialOrd for ScriptRefWithHash<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScriptRefWithHash<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.hash.cmp(&other.hash)
    }
}

impl From<&ScriptRefWithHash<'_>> for ScriptHash {
    fn from(value: &ScriptRefWithHash<'_>) -> Self {
        value.hash
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

impl HasLovelace for AlonzoValue {
    fn lovelace(&self) -> Lovelace {
        match self {
            AlonzoValue::Coin(lovelace) => *lovelace,
            AlonzoValue::Multiasset(lovelace, _) => *lovelace,
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

/// Calculate the total ex units in a witness set
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

/// Collect provided scripts and compute each ScriptHash in a witness set
pub fn get_provided_scripts<'a>(
    witness_set: &'a MintedWitnessSet<'_>,
) -> BTreeSet<ScriptRefWithHash<'a>> {
    let mut provided_scripts: BTreeSet<ScriptRefWithHash<'a>> = BTreeSet::new();

    if let Some(native_scripts) = witness_set.native_script.as_ref() {
        provided_scripts.extend(
            native_scripts
                .iter()
                .map(|native_script| ScriptRefWithHash {
                    hash: native_script.script_hash(),
                    script: BorrowedScript::NativeScript(native_script.deref()),
                }),
        )
    };

    fn collect_plutus_scripts<'a, const VERSION: usize, A>(
        accum: &mut A,
        scripts: Option<&'a NonEmptySet<PlutusScript<VERSION>>>,
        lift: impl Fn(&'a PlutusScript<VERSION>) -> BorrowedScript<'a>,
    ) where
        A: Extend<ScriptRefWithHash<'a>>,
    {
        if let Some(plutus_scripts) = scripts {
            accum.extend(plutus_scripts.iter().map(|script| ScriptRefWithHash {
                hash: script.script_hash(),
                script: lift(script),
            }))
        }
    }

    collect_plutus_scripts(
        &mut provided_scripts,
        witness_set.plutus_v1_script.as_ref(),
        BorrowedScript::PlutusV1Script,
    );
    collect_plutus_scripts(
        &mut provided_scripts,
        witness_set.plutus_v2_script.as_ref(),
        BorrowedScript::PlutusV2Script,
    );
    collect_plutus_scripts(
        &mut provided_scripts,
        witness_set.plutus_v3_script.as_ref(),
        BorrowedScript::PlutusV3Script,
    );

    provided_scripts
}

pub fn display_collection<T>(collection: impl IntoIterator<Item = T>) -> String
where
    T: std::fmt::Display,
{
    collection
        .into_iter()
        .map(|item| item.to_string())
        .collect::<Vec<_>>()
        .join(", ")
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

pub trait HasDatum {
    fn datum(&self) -> Option<BorrowedDatumOption<'_>>;
}

impl HasDatum for TransactionOutput {
    fn datum(&self) -> Option<BorrowedDatumOption<'_>> {
        match self {
            PseudoTransactionOutput::Legacy(transaction_output) => transaction_output
                .datum_hash
                .as_ref()
                .map(BorrowedDatumOption::Hash),
            PseudoTransactionOutput::PostAlonzo(transaction_output) => transaction_output
                .datum_option
                .as_ref()
                .map(BorrowedDatumOption::from),
        }
    }
}

pub trait HasScriptRef {
    fn has_script_ref(&self) -> Option<&ScriptRef>;
}

impl HasScriptRef for TransactionOutput {
    fn has_script_ref(&self) -> Option<&ScriptRef> {
        match self {
            TransactionOutput::PostAlonzo(transaction_output) => {
                transaction_output.script_ref.as_deref()
            }
            TransactionOutput::Legacy(_) => None,
        }
    }
}

pub fn to_network_id(network: &Network) -> u8 {
    match network {
        Network::Testnet => 0,
        Network::Mainnet => 1,
        Network::Other(id) => *id,
    }
}

pub trait HasNetwork {
    /// Returns the Network of a given entity
    fn has_network(&self) -> Network;
}

impl HasNetwork for Address {
    #[allow(clippy::unwrap_used)]
    fn has_network(&self) -> Network {
        match self {
            Address::Byron(address) => address.has_network(),
            // Safe to unwrap here, as there will always be a network value for self.network as long as it is not Address::Byron (which is handled above)
            Address::Shelley(_) | Address::Stake(_) => self.network().unwrap(),
        }
    }
}

impl HasNetwork for ByronAddress {
    /*
        According to the Byron address specification (https://raw.githubusercontent.com/cardano-foundation/CIPs/master/CIP-0019/CIP-0019-byron-addresses.cddl),
        the attributes can optionally contain a u32 network discriminant, identifying a specific testnet network.

        When decoding Byron address attributes (https://github.com/IntersectMBO/cardano-ledger/blob/2d1e94cf96d00ba0da53883c388fa0aba6d74624/eras/byron/ledger/impl/src/Cardano/Chain/Common/AddrAttributes.hs#L122-L144),
        the Haskell node defaults NetworkMagic to NetworkMainOrStage, unless otherwise specified. The discriminant can be any `NetworkMagic` (sometimes referred to as `ProtocolMagic`), identifying a specific testnet.
        If present, it is Testnet(discriminant).

        It does not, notabtly, validate this discriminant, as evidenced by this conflicting Byron address on Preprod: 2cWKMJemoBaiqkR9D1YZ2xQ2BhVxzauukrsxm8ttZUrto1f7kr5J1tD9uhtEtTc9U4PuF (found in tx 9738801cc4f7e46bb3561a138a403fa8470e8a4faf2df5009023e7bbcdf09cb4).
        This address encodes a `NetworkMagic` of 1097911063. The `NetworkMagic` of Preprod is 1 (https://book.world.dev.cardano.org/environments/preprod/byron-genesis.json).


        As a result, since we are only checking the network of a Byron address for validation, we will mirror the Haskell node logic and disregard the discriminant when fetching the network from an address.
        (https://github.com/IntersectMBO/cardano-ledger/blob/2d1e94cf96d00ba0da53883c388fa0aba6d74624/libs/cardano-ledger-core/src/Cardano/Ledger/Address.hs#L152)
    */
    #[allow(clippy::unwrap_used)]
    fn has_network(&self) -> Network {
        // Unwrap is safe, we know that there is a valid address payload if it is a Byron address.
        let x: AddressPayload = from_cbor(&self.payload.0).unwrap();
        for attribute in x.attributes.iter() {
            if let AddrAttrProperty::NetworkTag(_) = attribute {
                // We are ignoring the network discriminant here, as the Haskell node does
                return Network::Testnet;
            }
        }

        Network::Mainnet
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

pub trait HasScriptHash {
    /*
        To compute a script hash, one must prepend a tag to the bytes of the script before hashing.

        The tag (u8) is determined by the language, as such:

          0 for native scripts
          1 for Plutus V1 scripts
          2 for Plutus V2 scripts
          3 for Plutus V3 scripts
    */
    fn script_hash(&self) -> ScriptHash;
}

impl HasScriptHash for MintedScriptRef<'_> {
    fn script_hash(&self) -> ScriptHash {
        match self {
            PseudoScript::NativeScript(native_script) => {
                let mut buffer: Vec<u8> = vec![0];
                buffer.extend_from_slice(native_script.raw_cbor());
                Hasher::<224>::hash(&buffer)
            }
            PseudoScript::PlutusV1Script(plutus_script) => plutus_script.script_hash(),
            PseudoScript::PlutusV2Script(plutus_script) => plutus_script.script_hash(),
            PseudoScript::PlutusV3Script(plutus_script) => plutus_script.script_hash(),
        }
    }
}

impl HasScriptHash for ScriptRef {
    fn script_hash(&self) -> ScriptHash {
        match self {
            ScriptRef::NativeScript(native_script) => {
                let mut buffer: Vec<u8>;
                buffer = vec![0];
                // FIXME: don't reserialize the native script here.
                //
                // This happens because scripts may be found in reference inputs, which have been
                // stripped from their 'KeepRaw' structure already and thus; have lost their
                // 'original' bytes.
                //
                // While native scripts are simple in essence, they don't have any canonical form.
                // For example, an array of signatures (all-of) may be serialised as definite or
                // indefinite. Which will change serialisation and hash.
                //
                // Rather than reserialising them when storing them in db, we shall keep their
                // original bytes and possibly only deserialise on-demand (we only need to
                // deserialise them if their execution is required).
                let native_script = to_cbor(&native_script);
                buffer.extend_from_slice(native_script.as_slice());
                Hasher::<224>::hash(&buffer)
            }
            PseudoScript::PlutusV1Script(plutus_script) => plutus_script.script_hash(),
            PseudoScript::PlutusV2Script(plutus_script) => plutus_script.script_hash(),
            PseudoScript::PlutusV3Script(plutus_script) => plutus_script.script_hash(),
        }
    }
}

impl HasScriptHash for KeepRaw<'_, NativeScript> {
    fn script_hash(&self) -> ScriptHash {
        let mut buffer: Vec<u8> = vec![0];
        buffer.extend_from_slice(self.raw_cbor());
        Hasher::<224>::hash(&buffer)
    }
}

impl<const VERSION: usize> HasScriptHash for PlutusScript<VERSION> {
    fn script_hash(&self) -> ScriptHash {
        let mut buffer: Vec<u8> = vec![VERSION as u8];
        buffer.extend_from_slice(self.as_ref());
        Hasher::<224>::hash(&buffer)
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

pub trait HasIndex {
    fn as_index(&self) -> u32;
}

impl HasIndex for ScriptPurpose {
    fn as_index(&self) -> u32 {
        match self {
            RedeemerTag::Spend => 0,
            RedeemerTag::Mint => 1,
            RedeemerTag::Cert => 2,
            RedeemerTag::Reward => 3,
            RedeemerTag::Vote => 4,
            RedeemerTag::Propose => 5,
        }
    }
}

/// Create a new `ExUnits` that is the sum of two `ExUnits`
pub fn sum_ex_units(left: ExUnits, right: ExUnits) -> ExUnits {
    ExUnits {
        mem: left.mem + right.mem,
        steps: left.steps + right.steps,
    }
}

pub fn default_ledger_dir(network: NetworkName) -> String {
    format!("./ledger.{}.db", network.to_string().to_lowercase())
}

pub fn default_chain_dir(network: NetworkName) -> String {
    format!("./chain.{}.db", network.to_string().to_lowercase())
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
            new_pointer((Slot::from(left.0), left.1, left.2)),
            new_pointer((Slot::from(right.0), right.1, right.2)),
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
