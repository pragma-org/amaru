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

use crate::{
    constr,
    script_context::{
        CurrencySymbol, DatumOption, RequiredSigners, Script, StakeAddress, TimeRange,
    },
};
use amaru_kernel::{
    Address, BigInt, Bytes, ComputeHash, Hash, Int, KeyValuePairs, MaybeIndefArray, MemoizedDatum,
    NonEmptyKeyValuePairs, NonZeroInt, Nullable, PlutusData, Redeemer, ShelleyDelegationPart,
    ShelleyPaymentPart, StakeCredential, StakePayload,
};
use thiserror::Error;

use std::{borrow::Cow, collections::BTreeMap};

#[derive(Debug, Error)]
pub enum PlutusDataError {
    #[error("unsupported for Plutus V{version}: {message}")]
    UnsupportedVersion { message: String, version: u8 },

    #[error("{0}")]
    Custom(String),
}

impl PlutusDataError {
    pub fn unsupported_version(message: impl Into<String>, version: u8) -> Self {
        Self::UnsupportedVersion {
            message: message.into(),
            version,
        }
    }
}

/// Serializing a type to PlutusData, which can then be serialised to CBOR.
pub trait ToPlutusData<const VERSION: u8> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError>;
}

pub struct PlutusVersion<const V: u8>;

/// A trait to restrict generic parameter `V` on `ToPlutusData` instances, to versions we know
/// about.
pub trait IsKnownPlutusVersion {}
impl IsKnownPlutusVersion for PlutusVersion<1> {}
impl IsKnownPlutusVersion for PlutusVersion<2> {}
impl IsKnownPlutusVersion for PlutusVersion<3> {}

pub const PLUTUS_V1: PlutusVersion<1> = PlutusVersion;
pub const PLUTUS_V2: PlutusVersion<2> = PlutusVersion;
pub const PLUTUS_V3: PlutusVersion<3> = PlutusVersion;

impl<const V: u8> ToPlutusData<V> for CurrencySymbol
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            Self::Ada => <Vec<u8> as ToPlutusData<V>>::to_plutus_data(&vec![]),
            Self::Native(policy_id) => policy_id.to_plutus_data(),
        }
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
            MemoizedDatum::Inline(data) => constr!(2, [data.as_ref()]),
        }
    }
}

impl<const V: u8> ToPlutusData<V> for DatumOption<'_>
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            DatumOption::None => constr!(0),
            DatumOption::Hash(hash) => constr!(1, [hash]),
            DatumOption::Inline(data) => constr!(2, [data]),
        }
    }
}

impl<const V: u8> ToPlutusData<V> for RequiredSigners
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        self.0.iter().collect::<Vec<_>>().to_plutus_data()
    }
}

impl<const V: u8> ToPlutusData<V> for Address
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    /// In both Plutus v1 and v2 encodings, Byron addresses are not possible encodings.
    ///
    /// In [PlutusV1](https://github.com/IntersectMBO/cardano-ledger/blob/59b52bb31c76a4a805e18860f68f549ec9022b14/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/Plutus/TxInfo.hs#L111-L112), outputs containing Byron addresses are filtered out.
    ///
    /// In [PlutusV2](https://github.com/IntersectMBO/cardano-ledger/blob/232511b0fa01cd848cd7a569d1acc322124cf9b8/eras/conway/impl/src/Cardano/Ledger/Conway/TxInfo.hs#L306), Byron addresses are completely disallowed, throwing an error instead
    // FIXME: make byron addresses impossible at the type level, so that this is not an issue, an error is thrown
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            Address::Shelley(shelley_address) => {
                let payment_part = shelley_address.payment();
                let stake_part = shelley_address.delegation();

                let payment_part_plutus_data = match payment_part {
                    ShelleyPaymentPart::Key(payment_keyhash) => {
                        constr!(0, [payment_keyhash])
                    }
                    ShelleyPaymentPart::Script(script_hash) => {
                        constr!(1, [script_hash])
                    }
                }?;

                let stake_part_plutus_data = match stake_part {
                    ShelleyDelegationPart::Key(stake_keyhash) => {
                        Some(constr!(0, [StakeCredential::AddrKeyhash(*stake_keyhash)])?)
                            .to_plutus_data()
                    }
                    ShelleyDelegationPart::Script(script_hash) => {
                        Some(constr!(0, [StakeCredential::ScriptHash(*script_hash)])?)
                            .to_plutus_data()
                    }
                    ShelleyDelegationPart::Pointer(pointer) => Some(constr!(
                        1,
                        [pointer.slot(), pointer.tx_idx(), pointer.cert_idx()]
                    )?)
                    .to_plutus_data(),
                    ShelleyDelegationPart::Null => None::<StakeCredential>.to_plutus_data(),
                }?;

                constr!(0, [payment_part_plutus_data, stake_part_plutus_data])
            }
            Address::Stake(stake_address) => stake_address.to_plutus_data(),
            Address::Byron(_) => unreachable!("unable to encode Byron address in PlutusData"),
        }
    }
}

impl<const V: u8> ToPlutusData<V> for TimeRange
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        let lower = match self.lower_bound {
            Some(x) => constr!(0, [constr!(1, [u64::from(x)])?, true]),
            None => constr!(0, [constr!(0)?, true]),
        };
        let upper = match self.upper_bound {
            Some(x) => constr!(0, [constr!(1, [u64::from(x)])?, false]),
            None => constr!(0, [constr!(2)?, true]),
        };

        constr!(0, [lower?, upper?])
    }
}

impl<const V: u8> ToPlutusData<V> for amaru_kernel::StakeAddress
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self.payload() {
            StakePayload::Stake(keyhash) => constr!(0, [keyhash]),
            StakePayload::Script(script_hash) => constr!(1, [script_hash]),
        }
    }
}

impl<const V: u8> ToPlutusData<V> for StakeAddress
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        amaru_kernel::StakeAddress::from(self.clone()).to_plutus_data()
    }
}

impl<const V: u8> ToPlutusData<V> for StakeCredential
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            StakeCredential::AddrKeyhash(hash) => {
                constr!(0, [hash])
            }
            StakeCredential::ScriptHash(hash) => {
                constr!(1, [hash])
            }
        }
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
        Ok(PlutusData::BoundedBytes(self.to_vec().into()))
    }
}

impl<const V: u8> ToPlutusData<V> for Bytes
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::BoundedBytes(self.to_vec().into()))
    }
}

impl<const V: u8> ToPlutusData<V> for Script<'_>
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            Script::Native(native) => native.compute_hash().to_plutus_data(),
            Script::PlutusV1(plutus) => plutus.compute_hash().to_plutus_data(),
            Script::PlutusV2(plutus) => plutus.compute_hash().to_plutus_data(),
            Script::PlutusV3(plutus) => plutus.compute_hash().to_plutus_data(),
        }
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
        Ok(PlutusData::BigInt(BigInt::Int(Int::from(*self as i64))))
    }
}

impl<const V: u8> ToPlutusData<V> for i64
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::BigInt(BigInt::Int(Int::from(*self))))
    }
}

impl<const V: u8> ToPlutusData<V> for u32
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::BigInt(BigInt::Int(Int::from(*self as i64))))
    }
}

impl<const V: u8> ToPlutusData<V> for u64
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    #[allow(clippy::unwrap_used)]
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        // Unwrap is safe here, u64 cannot possible be too big for the `Int` structure
        Ok(PlutusData::BigInt(BigInt::Int(
            Int::try_from(*self as i128).unwrap(),
        )))
    }
}

impl<const V: u8> ToPlutusData<V> for usize
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    #[allow(clippy::unwrap_used)]
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        // Unwrap is safe here, usize cannot possible be too big for the `Int` structure
        Ok(PlutusData::BigInt(BigInt::Int(
            Int::try_from(*self as i128).unwrap(),
        )))
    }
}

impl<const V: u8, T> ToPlutusData<V> for Vec<T>
where
    PlutusVersion<V>: IsKnownPlutusVersion,
    T: ToPlutusData<V>,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        if self.is_empty() {
            Ok(PlutusData::Array(MaybeIndefArray::Def(vec![])))
        } else {
            Ok(PlutusData::Array(MaybeIndefArray::Indef(
                self.iter()
                    .map(|a| a.to_plutus_data())
                    .collect::<Result<_, _>>()?,
            )))
        }
    }
}

impl<const V: u8> ToPlutusData<V> for Vec<u8>
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::BoundedBytes(self.clone().into()))
    }
}

impl<const VER: u8, K, V> ToPlutusData<VER> for BTreeMap<K, V>
where
    PlutusVersion<VER>: IsKnownPlutusVersion,
    K: ToPlutusData<VER> + Ord,
    V: ToPlutusData<VER>,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::Map(
            self.iter()
                .map(|(k, v)| Ok((k.to_plutus_data()?, v.to_plutus_data()?)))
                .collect::<Result<_, _>>()?,
        ))
    }
}

impl<const VER: u8, K, V> ToPlutusData<VER> for KeyValuePairs<K, V>
where
    PlutusVersion<VER>: IsKnownPlutusVersion,
    K: ToPlutusData<VER> + Clone,
    V: ToPlutusData<VER> + Clone,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::Map(KeyValuePairs::Def(
            self.iter()
                .map(|(key, value)| Ok((key.to_plutus_data()?, value.to_plutus_data()?)))
                .collect::<Result<Vec<_>, _>>()?,
        )))
    }
}

impl<const VER: u8, K, V> ToPlutusData<VER> for NonEmptyKeyValuePairs<K, V>
where
    PlutusVersion<VER>: IsKnownPlutusVersion,
    K: ToPlutusData<VER> + Clone,
    V: ToPlutusData<VER> + Clone,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(PlutusData::Map(KeyValuePairs::Def(
            self.iter()
                .map(|(key, value)| Ok((key.to_plutus_data()?, value.to_plutus_data()?)))
                .collect::<Result<Vec<_>, _>>()?,
        )))
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

impl<const V: u8> ToPlutusData<V> for Redeemer
where
    PlutusVersion<V>: IsKnownPlutusVersion,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        Ok(self.data.clone())
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

impl<const V: u8, T> ToPlutusData<V> for Nullable<T>
where
    PlutusVersion<V>: IsKnownPlutusVersion,
    T: ToPlutusData<V> + Clone,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            Nullable::Some(t) => constr!(0, [t]),
            Nullable::Null | Nullable::Undefined => constr!(1),
        }
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
