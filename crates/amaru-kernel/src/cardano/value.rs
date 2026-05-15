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
    collections::{BTreeMap, btree_map::Entry},
    ops::{AddAssign, SubAssign},
};

pub use pallas_primitives::conway::{AssetName, Multiasset, PolicyId, PositiveCoin, Value};
use serde::Deserialize;

/// A signed representation of a value, including a multiasset and lovelace.
///
/// Unlike [`Value`], entries here may be negative; allowing it to be used in value comparisons.
/// Multiasset entires cannot be zero.
#[derive(Default, Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Balance {
    coin: i64,
    multiasset: BTreeMap<(PolicyId, AssetName), i64>,
}

impl Balance {
    pub fn is_zero(&self) -> bool {
        self.coin == 0 && self.multiasset.is_empty()
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
    fn apply_delta(&mut self, key: (PolicyId, AssetName), delta: i64) {
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
            self.apply_delta(key.clone(), *qty);
        }
    }
}

/// Subtract another `Balance`. Multi-asset entries that net to zero are removed from the map.
impl SubAssign<&Balance> for Balance {
    fn sub_assign(&mut self, other: &Balance) {
        self.coin = self.coin.checked_sub(other.coin).unwrap_or_else(|| unreachable!("Lovelace accumulator underflow"));
        for (key, qty) in &other.multiasset {
            let neg = qty.checked_neg().unwrap_or_else(|| unreachable!("cannot negate i64::MIN"));
            self.apply_delta(key.clone(), neg);
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
                    self.apply_delta((*policy, name.clone()), positive_to_i64(qty));
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
                        positive_to_i64(qty).checked_neg().unwrap_or_else(|| unreachable!("cannot negate i64::MIN"));
                    self.apply_delta((*policy, name.clone()), neg);
                }
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

fn positive_to_i64(qty: &PositiveCoin) -> i64 {
    let raw: u64 = qty.into();
    i64::try_from(raw).unwrap_or_else(|_| unreachable!("PositiveCoin exceeds i64::MAX: {raw}"))
}
