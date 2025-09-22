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

use std::{
    collections::BTreeMap,
    fmt::{self},
};

use amaru_kernel::{
    AlonzoValue, MemoizedTransactionOutput, MintedTransactionOutput, TransactionInput,
    TransactionInputAdapter, Value, protocol_parameters::ProtocolParameters,
};
use thiserror::Error;

use crate::context::UtxoSlice;

/*
* CollateralBalance is used to track difference in collateral input vlaue and collateral return value.
* The value of everything should be zero in this struct, otherwise value is not conserved.
* We allow negative values here so that we are able to display them in an error message
*/
#[derive(Debug)]
pub struct CollateralBalance {
    pub coin: i64,
    pub multiasset: BTreeMap<Vec<u8>, i64>,
}

impl fmt::Display for CollateralBalance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "({}, [{}])",
            self.coin,
            self.multiasset
                .iter()
                .map(|(asset_id, value)| format!("{}: {}", hex::encode(asset_id), value))
                .collect::<Vec<String>>()
                .join(",")
        )
    }
}

impl CollateralBalance {
    fn empty() -> Self {
        Self {
            coin: 0,
            multiasset: BTreeMap::new(),
        }
    }

    fn sub(&mut self, other: Self) {
        self.coin -= other.coin;

        for (key, value) in other.multiasset {
            self.multiasset
                .entry(key)
                .and_modify(|v| *v -= value)
                .or_insert(-value);
        }

        self.multiasset.retain(|_, v| *v != 0);
    }

    fn add_output_value(&mut self, output: &MemoizedTransactionOutput) {
        let output_balance: CollateralBalance = (&output.value).into();

        self.coin += output_balance.coin;
        for (key, value) in output_balance.multiasset {
            self.multiasset
                .entry(key)
                .and_modify(|v| *v += value)
                .or_insert(value);
        }
    }

    /// This method returns `True` if the `CollateralBalance` is considered "valid".
    ///
    /// A `True` return value doesn't mean that other related checks (IncorrectTotalCollateral, InsufficientBalance) can be skipped.
    ///
    ///
    /// In order for `CollateralBalance` to be "valid" it must:
    ///    - have no multiassets and,
    ///    - have a nonnegative coin value.
    fn is_valid(&self) -> bool {
        !self.multiasset.is_empty() || self.coin > 0
    }
}

impl From<Option<&MintedTransactionOutput<'_>>> for CollateralBalance {
    fn from(value: Option<&MintedTransactionOutput<'_>>) -> Self {
        match value {
            Some(output) => match output {
                amaru_kernel::PseudoTransactionOutput::Legacy(output) => {
                    CollateralBalance::from(&output.amount)
                }

                amaru_kernel::PseudoTransactionOutput::PostAlonzo(output) => {
                    CollateralBalance::from(&output.value)
                }
            },
            None => CollateralBalance::empty(),
        }
    }
}

impl From<&Value> for CollateralBalance {
    fn from(value: &Value) -> Self {
        match value {
            Value::Multiasset(coin, multiasset) => {
                let map = multiasset
                    .iter()
                    .flat_map(|(policy, assets)| {
                        assets.iter().map(|(asset_name, quantity)| {
                            let key = [policy.as_ref(), asset_name.as_ref()].concat();

                            (key, u64::from(quantity) as i64)
                        })
                    })
                    .collect::<BTreeMap<_, _>>();

                Self {
                    coin: *coin as i64,
                    multiasset: map,
                }
            }
            Value::Coin(coin) => Self {
                coin: *coin as i64,
                multiasset: BTreeMap::new(),
            },
        }
    }
}

impl From<&AlonzoValue> for CollateralBalance {
    fn from(value: &AlonzoValue) -> Self {
        match value {
            AlonzoValue::Multiasset(coin, multiasset) => {
                let map = multiasset
                    .iter()
                    .flat_map(|(policy, assets)| {
                        assets.iter().map(|(asset_name, quantity)| {
                            let key = [policy.as_ref(), asset_name.as_ref()].concat();

                            (key, *quantity as i64)
                        })
                    })
                    .collect::<BTreeMap<_, _>>();

                Self {
                    coin: *coin as i64,
                    multiasset: map,
                }
            }
            AlonzoValue::Coin(coin) => Self {
                coin: *coin as i64,
                multiasset: BTreeMap::new(),
            },
        }
    }
}

#[derive(Debug, Error)]
pub enum InvalidCollateral {
    #[error("Unknown input: {0}")]
    UnknownInput(TransactionInputAdapter),
    #[error("too many collateral inputs: provided: {provided} allowed: {allowed}")]
    TooManyInputs { provided: usize, allowed: usize },
    #[error("a collateral input is locked at a script address: {0}")]
    LockedAtScriptAddress(TransactionInputAdapter),
    #[error("total collateral value is insufficient: provided: {provided} required: {required}")]
    InsufficientBalance { provided: u64, required: u64 },
    #[error(
        "total collateral field (expected) does not equal actual collateral (provided): provided: {provided} expected: {expected} "
    )]
    IncorrectTotalCollateral { provided: u64, expected: u64 },
    #[error("No collateral was provided, but collateral is required")]
    NoCollateral,
    #[error("collateral has non-zero delta: {0}")]
    ValueNotConserved(CollateralBalance),
    // TODO: This error shouldn't exist, it's a placeholder for better error handling in less straight forward cases
    #[error("uncategorized error: {0}")]
    UncategorizedError(String),
}

/*
 Collateral validation occurs during fee validation in the Haskell node. See the comments below for notes on collateral validation:
 https://github.com/IntersectMBO/cardano-ledger/blob/master/eras/babbage/impl/src/Cardano/Ledger/Babbage/Rules/Utxo.hs#L180-L195
*/
pub fn execute<C>(
    context: &mut C,
    collaterals: Option<&[TransactionInput]>,
    collateral_return: Option<&MintedTransactionOutput<'_>>,
    tx_collateral: Option<u64>,
    fee: u64,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), InvalidCollateral>
where
    C: UtxoSlice,
{
    let collaterals = collaterals
        .filter(|c| !c.is_empty())
        .ok_or(InvalidCollateral::NoCollateral)?;

    let mut balance = CollateralBalance::empty();

    let allowed = protocol_parameters.max_collateral_inputs as usize;
    let provided = collaterals.len();
    if provided > allowed {
        return Err(InvalidCollateral::TooManyInputs { provided, allowed });
    }

    for collateral in collaterals.iter() {
        let output = context
            .lookup(collateral)
            .ok_or_else(|| InvalidCollateral::UnknownInput(collateral.clone().into()))?;

        if output.address.has_script() {
            return Err(InvalidCollateral::LockedAtScriptAddress(
                collateral.clone().into(),
            ));
        }

        balance.add_output_value(output);
    }

    let collateral_return_balance = collateral_return.into();

    balance.sub(collateral_return_balance);

    if !balance.is_valid() {
        return Err(InvalidCollateral::ValueNotConserved(balance));
    }

    let required = fee * protocol_parameters.collateral_percentage as u64;
    if balance.coin as i128 * 100 < required as i128 {
        return Err(InvalidCollateral::InsufficientBalance {
            provided: balance.coin as u64,
            required,
        });
    }

    if let Some(expected_balance) = tx_collateral
        && expected_balance != balance.coin as u64
    {
        return Err(InvalidCollateral::IncorrectTotalCollateral {
            provided: balance.coin as u64,
            expected: expected_balance,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::InvalidCollateral;
    use crate::{context::assert::AssertValidationContext, rules::tests::fixture_context};
    use amaru_kernel::protocol_parameters::PREPROD_INITIAL_PROTOCOL_PARAMETERS;
    use amaru_kernel::{
        KeepRaw, MintedTransactionBody, include_cbor, include_json,
        protocol_parameters::ProtocolParameters,
    };
    use test_case::test_case;

    macro_rules! fixture {
        ($hash:literal) => {
            (
                fixture_context!($hash),
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                PREPROD_INITIAL_PROTOCOL_PARAMETERS.clone(),
            )
        };
        ($hash:literal, $variant:literal) => {
            (
                fixture_context!($hash, $variant),
                include_cbor!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/tx.cbor"
                )),
                PREPROD_INITIAL_PROTOCOL_PARAMETERS.clone(),
            )
        };
    }

    #[test_case(
        fixture!("3b13b5c319249407028632579ee584edc38eaeb062dac5156437a627d126fbb1", "no-collateral-return");
        "happy path - ada only collateral"
    )]
    #[test_case(
        fixture!("3b13b5c319249407028632579ee584edc38eaeb062dac5156437a627d126fbb1");
        "happy path - ada only collateral with return and total field"
    )]
    #[test_case(
        fixture!("fe78fd37a5c864cde5416461195b288ab18721f6e64be4ee93eaef0979b928f9");
        "happy path - assets in collateral with return"
    )]
    #[test_case(
        fixture!("3b13b5c319249407028632579ee584edc38eaeb062dac5156437a627d126fbb1", "max-collateral-inputs") =>
        matches Err(InvalidCollateral::TooManyInputs { .. });
        "max collateral inputs"
    )]
    #[test_case(
        fixture!("3b13b5c319249407028632579ee584edc38eaeb062dac5156437a627d126fbb1", "unknown-input") =>
        matches Err(InvalidCollateral::UnknownInput(..));
        "unknown input"
    )]
    #[test_case(
        fixture!("3b13b5c319249407028632579ee584edc38eaeb062dac5156437a627d126fbb1", "locked-at-script") =>
        matches Err(InvalidCollateral::LockedAtScriptAddress(..));
        "locked at script"
    )]
    #[test_case(
        fixture!("3b13b5c319249407028632579ee584edc38eaeb062dac5156437a627d126fbb1", "no-collateral") =>
        matches Err(InvalidCollateral::NoCollateral);
        "no collateral"
    )]
    #[test_case(
        fixture!("3b13b5c319249407028632579ee584edc38eaeb062dac5156437a627d126fbb1", "insufficient-balance") =>
        matches Err(InvalidCollateral::InsufficientBalance { .. });
        "insufficient balance"
    )]
    #[test_case(
        fixture!("3b13b5c319249407028632579ee584edc38eaeb062dac5156437a627d126fbb1", "incorrect-total-collateral") =>
        matches Err(InvalidCollateral::IncorrectTotalCollateral { .. });
        "incorrect total balance"
    )]
    #[test_case(
        fixture!("fe78fd37a5c864cde5416461195b288ab18721f6e64be4ee93eaef0979b928f9", "no-collateral-return") =>
        matches Err(InvalidCollateral::ValueNotConserved(..));
        "value not conserved - no collateral return"
    )]
    #[test_case(
        fixture!("fe78fd37a5c864cde5416461195b288ab18721f6e64be4ee93eaef0979b928f9", "value-not-conserved-inputs") =>
        matches Err(InvalidCollateral::ValueNotConserved(..));
        "value not conserved - inputs > outputs"
    )]
    #[test_case(
        fixture!("fe78fd37a5c864cde5416461195b288ab18721f6e64be4ee93eaef0979b928f9", "value-not-conserved-outputs") =>
        matches Err(InvalidCollateral::ValueNotConserved(..));
        "value not conserved - outputs > inputs"
    )]
    fn collateral(
        (mut ctx, tx, pp): (
            AssertValidationContext,
            KeepRaw<'_, MintedTransactionBody<'_>>,
            ProtocolParameters,
        ),
    ) -> Result<(), InvalidCollateral> {
        super::execute(
            &mut ctx,
            tx.collateral.as_deref().map(|vec| vec.as_slice()),
            tx.collateral_return.as_ref(),
            tx.total_collateral,
            tx.fee,
            &pp,
        )
    }
}
