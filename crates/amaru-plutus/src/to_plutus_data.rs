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

use std::{borrow::Cow, collections::BTreeMap, ops::Deref, time::SystemTime};

use amaru_kernel::{
    Address, AssetName, BorrowedScript, Bytes, Credential, CurrencySymbol, EMPTY_ASSET_NAME, Epoch, HasScriptHash,
    Hash, Int, KeyValuePairs, MemoizedDatum, MemoizedScript, NonEmptyKeyValuePairs, NonZeroInt, PlutusData, PoolId,
    ProtocolVersion, Set, StakeReference, TimeRange, TransactionId, Value, plutus_data::BigInt,
};
use thiserror::Error;

use crate::{constr, script_context::IsPrePlutusVersion3};

/// Something a Plutus language version cannot represent in its script context.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UnsupportedFeature {
    #[error("voting procedures unsupported")]
    VotingProcedures,

    #[error("proposal procedures unsupported")]
    ProposalProcedures,

    #[error("current treasury value unsupported")]
    CurrentTreasuryValue,

    #[error("treasury donation unsupported")]
    TreasuryDonation,

    #[error("inline datums unsupported")]
    InlineDatums,

    #[error("voting purpose unsupported")]
    VotingPurpose,

    #[error("proposing purpose unsupported")]
    ProposingPurpose,

    #[error("byron address in script context")]
    ByronAddressInContext,

    /// The certificate, rendered with its `Debug` output, has no Plutus V1 or V2 representation.
    #[error("illegal certificate type: {0}")]
    IllegalCertificate(String),
}

/// Represents an error that occured during serialization to `PlutusData`.
#[derive(Debug, Error)]
pub enum PlutusDataError {
    #[error("unsupported for Plutus V{version}: {feature}")]
    UnsupportedVersion { feature: UnsupportedFeature, version: u8 },

    #[error("{0}")]
    Custom(String),
}

impl PlutusDataError {
    pub fn unsupported_version(feature: UnsupportedFeature, version: u8) -> Self {
        Self::UnsupportedVersion { feature, version }
    }
}

/// Serializing a type to PlutusData, which can then be serialised to CBOR.
pub trait ToPlutusData<const VERSION: u8> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError>;
}

#[doc(hidden)]
pub struct PlutusVersion<const V: u8>;

/// A trait to restrict generic parameter `V` on `ToPlutusData` instances
/// to versions we know about.
pub trait IsKnownPlutusVersion {}
impl IsKnownPlutusVersion for PlutusVersion<1> {}
impl IsKnownPlutusVersion for PlutusVersion<2> {}
impl IsKnownPlutusVersion for PlutusVersion<3> {}

/// Plutus Version 1
pub const PLUTUS_V1: PlutusVersion<1> = PlutusVersion;
/// Plutus Version 2
pub const PLUTUS_V2: PlutusVersion<2> = PlutusVersion;
/// Plutus Version 3
pub const PLUTUS_V3: PlutusVersion<3> = PlutusVersion;

impl<const V: u8> ToPlutusData<V> for CurrencySymbol
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            Self::Lovelace => <Vec<u8> as ToPlutusData<V>>::to_plutus_data(&vec![]),
            Self::Native(policy_id) => policy_id.to_plutus_data(),
        }
    }
}

/// Reshape a ledger [`Value`] into the canonical Plutus value map: a [`BTreeMap`] keyed by
/// [`CurrencySymbol`] (lovelace first, then policies ascending) whose inner maps are keyed by asset
/// name ascending. Collecting through `BTreeMap`s re-imposes this canonical order regardless of the
/// order assets happen to appear on the wire.
///
/// `include_zero_lovelace` selects the version-specific lovelace rule: PlutusV1 and PlutusV2 always
/// carry a lovelace entry (`true`), whereas PlutusV3 omits it when zero (`false`).
fn canonical_value_map(
    value: &Value,
    include_zero_lovelace: bool,
) -> BTreeMap<CurrencySymbol, BTreeMap<Cow<'_, AssetName>, u64>> {
    let (coin, multiasset) = match value {
        Value::Coin(coin) => (*coin, None),
        Value::Multiasset(coin, multiasset) => (*coin, Some(multiasset)),
    };

    let mut map = BTreeMap::new();

    if include_zero_lovelace || coin > 0 {
        map.insert(CurrencySymbol::Lovelace, BTreeMap::from([(Cow::Owned(EMPTY_ASSET_NAME), coin)]));
    }

    if let Some(multiasset) = multiasset {
        for (policy_id, assets) in multiasset.iter() {
            map.insert(
                CurrencySymbol::Native(*policy_id),
                assets.iter().map(|(name, amount)| (Cow::Borrowed(name), amount.into())).collect(),
            );
        }
    }

    map
}

impl<const V: u8> ToPlutusData<V> for Value
where
    PlutusVersion<V>: IsKnownPlutusVersion + IsPrePlutusVersion3,
{
    /// In PlutusV1 and PlutusV2 the lovelace entry is always present, even when its quantity is zero.
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <BTreeMap<_, _> as ToPlutusData<V>>::to_plutus_data(&canonical_value_map(self, true))
    }
}

impl ToPlutusData<3> for Value {
    /// In PlutusV3 the lovelace entry is omitted when its quantity is zero.
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <BTreeMap<_, _> as ToPlutusData<3>>::to_plutus_data(&canonical_value_map(self, false))
    }
}

impl<const V: u8> ToPlutusData<V> for MemoizedDatum
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            MemoizedDatum::None => constr!(0),
            MemoizedDatum::Hash(hash) => constr!(1, [hash]),
            MemoizedDatum::Inline(data) => constr!(2, [data.as_ref().data()]),
        }
    }
}

impl<const V: u8> ToPlutusData<V> for Address
where
    PlutusVersion<V>: IsKnownPlutusVersion,
    amaru_kernel::RewardAccount: ToPlutusData<V>,
{
    /// Byron addresses have no representation in any Plutus version, so translating one is an error.
    ///
    /// Alonzo filtered Byron inputs out of the PlutusV1 context. In Conway, `transTxOutV1` and `transTxOutV2`
    /// (`Cardano.Ledger.Conway.TxInfo` and `Cardano.Ledger.Babbage.TxInfo`) fail with `ByronTxOutInContext` for
    /// every Plutus version, whether the address belongs to an input, a reference input or an output.
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            Address::Shelley(shelley_address) => {
                let payment_part = shelley_address.payment();
                let stake_part = shelley_address.delegation();

                let payment_part_plutus_data = <Credential as ToPlutusData<V>>::to_plutus_data(payment_part)?;

                let stake_part_plutus_data = match stake_part {
                    Some(StakeReference::Credential(credential)) => Some(constr!(0, [credential])?).to_plutus_data(),
                    Some(StakeReference::Pointer(pointer)) => {
                        Some(constr!(1, [pointer.slot, pointer.transaction, pointer.certificate])?).to_plutus_data()
                    }
                    None => None::<Credential>.to_plutus_data(),
                }?;

                constr!(0, [payment_part_plutus_data, stake_part_plutus_data])
            }
            Address::Byron(_) => {
                Err(PlutusDataError::unsupported_version(UnsupportedFeature::ByronAddressInContext, V))
            }
        }
    }
}

#[allow(clippy::expect_used)]
fn system_time_to_posix_millis(st: SystemTime) -> u64 {
    st.duration_since(SystemTime::UNIX_EPOCH).expect("system time before Unix epoch?").as_millis() as u64
}

impl<const V: u8> ToPlutusData<V> for TimeRange
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        let lower = match self.lower_bound {
            Some(x) => constr!(0, [constr!(1, [system_time_to_posix_millis(x)])?, true]),
            None => constr!(0, [constr!(0)?, true]),
        };
        let upper = match self.upper_bound {
            Some(x) => constr!(0, [constr!(1, [system_time_to_posix_millis(x)])?, false]),
            None => constr!(0, [constr!(2)?, true]),
        };

        constr!(0, [lower?, upper?])
    }
}

impl<const V: u8> ToPlutusData<V> for Credential
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            Credential::KeyHash(hash) => {
                constr!(0, [hash])
            }
            Credential::ScriptHash(hash) => {
                constr!(1, [hash])
            }
        }
    }
}

impl<A, const V: u8> ToPlutusData<V> for Box<A>
where
    PlutusVersion<V>: IsKnownPlutusVersion,
    A: ToPlutusData<V>,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        self.as_ref().to_plutus_data()
    }
}

impl<A, const V: u8> ToPlutusData<V> for Option<A>
where
    PlutusVersion<V>: IsKnownPlutusVersion,
    A: ToPlutusData<V>,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            None => constr!(1),
            Some(data) => constr!(0, [data]),
        }
    }
}

impl<const BYTES: usize, const V: u8> ToPlutusData<V> for Hash<BYTES>
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::bytes(self.to_vec()))
    }
}

impl<const V: u8> ToPlutusData<V> for Bytes
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::bytes(self.to_vec()))
    }
}

impl<const V: u8> ToPlutusData<V> for AssetName
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::bytes(self.to_vec()))
    }
}

impl<const V: u8> ToPlutusData<V> for BorrowedScript<'_>
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        self.script_hash().to_plutus_data()
    }
}

impl<const V: u8> ToPlutusData<V> for MemoizedScript
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    /// A script appears in a transaction output only as its hash; the encoding is delegated to
    /// the borrowed [`BorrowedScript`] view to keep that hashing logic in a single place.
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <BorrowedScript<'_> as ToPlutusData<V>>::to_plutus_data(&BorrowedScript::from(self))
    }
}

impl<const V: u8> ToPlutusData<V> for bool
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            false => constr!(0),
            true => constr!(1),
        }
    }
}

impl<const V: u8> ToPlutusData<V> for i32
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::int(BigInt::Int(Int::from(*self as i64))))
    }
}

impl<const V: u8> ToPlutusData<V> for i64
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::int(BigInt::Int(Int::from(*self))))
    }
}

impl<const V: u8> ToPlutusData<V> for u16
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::int(BigInt::Int(Int::from(*self as i64))))
    }
}

impl<const V: u8> ToPlutusData<V> for u32
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::int(BigInt::Int(Int::from(*self as i64))))
    }
}

impl<const V: u8> ToPlutusData<V> for u64
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    #[allow(clippy::unwrap_used)]
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        // Unwrap is safe here, u64 cannot possible be too big for the `Int` structure
        Ok(PlutusData::int(BigInt::Int(Int::try_from(*self as i128).unwrap())))
    }
}

impl<const V: u8> ToPlutusData<V> for Epoch
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    #[allow(clippy::unwrap_used)]
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        u64::from(*self).to_plutus_data()
    }
}

impl<const V: u8> ToPlutusData<V> for ProtocolVersion
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    #[allow(clippy::unwrap_used)]
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        (self.major(), self.minor()).to_plutus_data()
    }
}

impl<const V: u8> ToPlutusData<V> for usize
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    #[allow(clippy::unwrap_used)]
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        // Unwrap is safe here, usize cannot possible be too big for the `Int` structure
        Ok(PlutusData::int(BigInt::Int(Int::try_from(*self as i128).unwrap())))
    }
}

impl<const V: u8, T> ToPlutusData<V> for Vec<T>
where
    PlutusVersion<V>: IsKnownPlutusVersion,
    T: ToPlutusData<V>,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::array(self.iter().map(|a| a.to_plutus_data()).collect::<Result<Vec<_>, _>>()?))
    }
}

impl<const V: u8, T> ToPlutusData<V> for Set<T>
where
    PlutusVersion<V>: IsKnownPlutusVersion,
    T: Ord + ToPlutusData<V>,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::array(self.iter().map(|a| a.to_plutus_data()).collect::<Result<Vec<_>, _>>()?))
    }
}

impl<const V: u8> ToPlutusData<V> for Vec<u8>
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::bytes(self.clone()))
    }
}

impl<const VER: u8, K, V> ToPlutusData<VER> for BTreeMap<K, V>
where
    PlutusVersion<VER>: IsKnownPlutusVersion,
    K: ToPlutusData<VER> + Ord,
    V: ToPlutusData<VER>,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::map(
            self.iter().map(|(k, v)| Ok((k.to_plutus_data()?, v.to_plutus_data()?))).collect::<Result<Vec<_>, _>>()?,
        ))
    }
}

impl<const VER: u8, K, V> ToPlutusData<VER> for KeyValuePairs<K, V>
where
    PlutusVersion<VER>: IsKnownPlutusVersion,
    K: ToPlutusData<VER> + Eq + Clone,
    V: ToPlutusData<VER> + Clone,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::map(
            self.iter()
                .map(|(key, value)| Ok((key.to_plutus_data()?, value.to_plutus_data()?)))
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }
}

impl<const VER: u8, K, V> ToPlutusData<VER> for NonEmptyKeyValuePairs<K, V>
where
    PlutusVersion<VER>: IsKnownPlutusVersion,
    K: ToPlutusData<VER> + Eq + Clone,
    V: ToPlutusData<VER> + Clone,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::map(
            self.iter()
                .map(|(key, value): &(K, V)| Ok((key.to_plutus_data()?, value.to_plutus_data()?)))
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }
}

impl<const V: u8> ToPlutusData<V> for NonZeroInt
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        i64::from(self).to_plutus_data()
    }
}

impl<const VER: u8, K, V> ToPlutusData<VER> for (K, V)
where
    PlutusVersion<VER>: IsKnownPlutusVersion,
    K: ToPlutusData<VER>,
    V: ToPlutusData<VER>,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr!(0, [self.0, self.1])
    }
}

impl<const V: u8> ToPlutusData<V> for PlutusData
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(self.clone())
    }
}

impl<const V: u8, T> ToPlutusData<V> for &T
where
    PlutusVersion<V>: IsKnownPlutusVersion,
    T: ToPlutusData<V>,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        T::to_plutus_data(*self)
    }
}

impl<const V: u8, T> ToPlutusData<V> for Cow<'_, T>
where
    PlutusVersion<V>: IsKnownPlutusVersion,
    T: ToPlutusData<V> + ToOwned,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        self.as_ref().to_plutus_data()
    }
}

impl<const V: u8> ToPlutusData<V> for PoolId
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        self.deref().to_plutus_data()
    }
}

impl<const V: u8> ToPlutusData<V> for TransactionId
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        self.deref().to_plutus_data()
    }
}
