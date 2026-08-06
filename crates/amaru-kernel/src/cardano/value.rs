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

use std::{
    borrow::Cow,
    collections::{BTreeMap, btree_map::Entry},
    fmt::{self, Display, Formatter},
    ops::{AddAssign, SubAssign},
};

use crate::{
    AssetName, Hash, Lovelace, Multiasset, NonZeroInt, PositiveCoin, cbor,
    size::{CREDENTIAL, SCRIPT},
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Value {
    Coin(Lovelace),
    Multiasset(Lovelace, Multiasset<PositiveCoin>),
}

impl<C> cbor::encode::Encode<C> for Value {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Value::Coin(coin) => {
                e.encode_with(coin, ctx)?;
            }
            Value::Multiasset(coin, other) => {
                e.array(2)?;
                e.encode_with(coin, ctx)?;
                e.encode_with(other, ctx)?;
            }
        };

        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for Value {
    #[expect(clippy::wildcard_enum_match_arm)]
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        match d.datatype()? {
            cbor::data::Type::U8 | cbor::data::Type::U16 | cbor::data::Type::U32 | cbor::data::Type::U64 => {
                Ok(Value::Coin(d.decode_with(ctx)?))
            }
            cbor::data::Type::Array | cbor::data::Type::ArrayIndef => cbor::heterogeneous_array(d, |d, assert_len| {
                assert_len(2)?;
                let coin = d.decode_with(ctx)?;
                let multiasset = d.decode_with(ctx)?;
                Ok(Value::Multiasset(coin, multiasset))
            }),
            _ => Err(cbor::decode::Error::message("unknown cbor data type for Value enum")),
        }
    }
}

/// An identifier for a currency in a [`Value`].
///
///
/// This identifier is specifically used to enforce canonical ordering in a PlutusData representation of [`Value`].
/// Lovelace is encoded as the empty bytestring and, always sorts ahead of native assets.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CurrencySymbol {
    Lovelace,
    Native(Hash<CREDENTIAL>),
}

/// The assets minted and burned by a transaction.
///
/// A map from minting-policy [`struct@Hash`] to that policy's assets, each carrying a signed
/// quantity: positive mints, negative burns. Unlike [`Value`], amounts are signed and
/// there is no ada entry; only native assets can be minted or burned.
#[derive(Debug, Default)]
pub struct PlutusMint<'a>(pub BTreeMap<Hash<CREDENTIAL>, BTreeMap<Cow<'a, AssetName>, i64>>);

/// Signed multi-asset bundle as it appears on `TransactionBody.mint`
pub type Mint = Multiasset<NonZeroInt>;

impl<'a> From<&'a Mint> for PlutusMint<'a> {
    fn from(value: &'a Mint) -> Self {
        let mints = value
            .iter()
            .map(|(policy, multiasset)| {
                (
                    *policy,
                    multiasset
                        .iter()
                        .map(|(asset_name, amount)| (Cow::Borrowed(asset_name), (*amount).into()))
                        .collect(),
                )
            })
            .collect();

        Self(mints)
    }
}

/// A signed representation of a value, including a multiasset and lovelace.
///
/// Unlike [`Value`], entries here may be negative; allowing it to be used in value comparisons.
/// Multiasset entires cannot be zero.
#[derive(Default, Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Balance {
    coin: i64,
    multiasset: BTreeMap<(Hash<{ SCRIPT }>, AssetName), i128>,
}

impl Display for Balance {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "({}, [{}])",
            self.coin,
            self.multiasset
                .iter()
                .map(|((policy, asset_name), value)| format!(
                    "{}: {}",
                    hex::encode([policy.as_ref(), asset_name.as_ref()].concat()),
                    value
                ))
                .collect::<Vec<String>>()
                .join(", ")
        )
    }
}

impl Balance {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn coin(&self) -> i64 {
        self.coin
    }

    pub fn has_assets(&self) -> bool {
        !self.multiasset.is_empty()
    }
    pub fn is_zero(&self) -> bool {
        self.coin == 0 && !self.has_assets()
    }

    fn add_coin(&mut self, amount: u64) {
        self.coin = self
            .coin
            .checked_add(lovelace_to_i64(amount))
            .unwrap_or_else(|| unreachable!("Lovelace accumulator overflow"));
    }

    fn sub_coin(&mut self, amount: u64) {
        self.coin = self
            .coin
            .checked_sub(lovelace_to_i64(amount))
            .unwrap_or_else(|| unreachable!("Lovelace accumulator underflow"));
    }

    /// Apply a signed `delta` to the multi-asset entry at `key`. A delta of zero is a no-op; an
    /// entry that nets to zero is removed.
    fn apply_delta(&mut self, key: (Hash<{ SCRIPT }>, AssetName), delta: i128) {
        if delta == 0 {
            return;
        }
        match self.multiasset.entry(key) {
            Entry::Vacant(v) => {
                v.insert(delta);
            }
            Entry::Occupied(mut o) => {
                let new = o.get().checked_add(delta).unwrap_or_else(|| unreachable!("multi-asset quantity overflow"));
                if new == 0 {
                    o.remove();
                } else {
                    *o.get_mut() = new;
                }
            }
        }
    }
}

/// Accumulate another `Balance`. Multi-asset entries that net to zero are removed from the map.
impl AddAssign<&Balance> for Balance {
    fn add_assign(&mut self, other: &Balance) {
        self.coin = self.coin.checked_add(other.coin).unwrap_or_else(|| unreachable!("Lovelace accumulator overflow"));
        for (key, qty) in &other.multiasset {
            self.apply_delta(*key, *qty);
        }
    }
}

/// Subtract another `Balance`. Multi-asset entries that net to zero are removed from the map.
impl SubAssign<&Balance> for Balance {
    fn sub_assign(&mut self, other: &Balance) {
        self.coin = self.coin.checked_sub(other.coin).unwrap_or_else(|| unreachable!("Lovelace accumulator underflow"));
        for (key, qty) in &other.multiasset {
            let neg = qty.checked_neg().unwrap_or_else(|| unreachable!("cannot negate i128::MIN"));
            self.apply_delta(*key, neg);
        }
    }
}

/// Accumulate a `Value` into the balance. Multi-asset entries that net to zero are removed from the map.
impl AddAssign<&Value> for Balance {
    fn add_assign(&mut self, value: &Value) {
        let (coin, multiasset) = split_value(value);
        self.add_coin(coin);
        if let Some(ma) = multiasset {
            for (policy, assets) in ma.iter() {
                for (name, qty) in assets.iter() {
                    self.apply_delta((*policy, *name), positive_to_i128(qty));
                }
            }
        }
    }
}

/// Subtract a `Value` from the balance. Multi-asset entries that net to zero are removed from the map.
impl SubAssign<&Value> for Balance {
    fn sub_assign(&mut self, value: &Value) {
        let (coin, multiasset) = split_value(value);
        self.sub_coin(coin);
        if let Some(ma) = multiasset {
            for (policy, assets) in ma.iter() {
                for (name, qty) in assets.iter() {
                    let neg =
                        positive_to_i128(qty).checked_neg().unwrap_or_else(|| unreachable!("cannot negate i128::MIN"));
                    self.apply_delta((*policy, *name), neg);
                }
            }
        }
    }
}

/// Apply a signed `Mint`: each `(policy, asset, signed_qty)` entry is added to the balance with
/// its sign preserved. Positive mints require outputs, negative mints require inputs.
impl AddAssign<&Mint> for Balance {
    fn add_assign(&mut self, mint: &Mint) {
        for (policy, assets) in mint.iter() {
            for (name, qty) in assets.iter() {
                self.apply_delta((*policy, *name), i128::from(i64::from(qty)));
            }
        }
    }
}

impl From<&Value> for Balance {
    fn from(value: &Value) -> Self {
        let mut balance = Balance::default();
        balance += value;
        balance
    }
}

fn split_value(value: &Value) -> (u64, Option<&Multiasset<PositiveCoin>>) {
    match value {
        Value::Coin(c) => (*c, None),
        Value::Multiasset(c, ma) => (*c, Some(ma)),
    }
}

fn lovelace_to_i64(amount: u64) -> i64 {
    i64::try_from(amount).unwrap_or_else(|_| unreachable!("Lovelace exceeds i64::MAX: {amount}"))
}

fn positive_to_i128(qty: &PositiveCoin) -> i128 {
    let raw: u64 = qty.into();
    i128::from(raw)
}
