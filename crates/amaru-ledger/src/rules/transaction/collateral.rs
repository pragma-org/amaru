use std::collections::BTreeMap;

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
 *
 * i64 has a lower maximum than u64 due to the data structure, but the value for coins must always be in the range:
 *  [-9223372036854775808, 18446744073709551615]
 * Which is within the bounds of an i64, so conversions are safe i64<->u64
 */
#[derive(Debug)]
pub struct CollateralBalance {
    pub coin: i64,
    pub multiasset: BTreeMap<Vec<u8>, i64>,
}

impl CollateralBalance {
    fn sub(&mut self, other: Self) {
        self.coin -= other.coin;
        for (key, value) in other.multiasset {
            match self.multiasset.get_mut(&key) {
                Some(v) => {
                    *v -= value;
                    if *v == 0 {
                        self.multiasset.remove(&key);
                    }
                }
                None => {
                    self.multiasset.insert(key, -value);
                }
            };
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
    // TODO: can we provide more context, such as the difference in values?
    #[error("Collateral input value not conserved")]
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

    let mut balance = CollateralBalance {
        coin: 0,
        multiasset: BTreeMap::new(),
    };

    let allowed = protocol_parameters.max_collateral_inputs as usize;
    let provided = collaterals.len();
    if provided > allowed {
        return Err(InvalidCollateral::TooManyInputs { provided, allowed });
    }

    for collateral in collaterals.iter() {
        let output = context
            .lookup(collateral)
            .ok_or_else(|| InvalidCollateral::UnknownInput(collateral.into()))?;

        if output.address.has_script() {
            return Err(InvalidCollateral::LockedAtScriptAddress(collateral.into()));
        }

        add_value_to_balance(&mut balance, output);
    }

    let collateral_return_balance = match collateral_return {
        Some(output) => match output {
            amaru_kernel::PseudoTransactionOutput::Legacy(output) => {
                CollateralBalance::from(&output.amount)
            }

            amaru_kernel::PseudoTransactionOutput::PostAlonzo(output) => {
                CollateralBalance::from(&output.value)
            }
        },
        None => CollateralBalance {
            coin: 0,
            multiasset: BTreeMap::new(),
        },
    };

    balance.sub(collateral_return_balance);

    if !balance.multiasset.is_empty() || balance.coin < 0 {
        return Err(InvalidCollateral::ValueNotConserved(balance));
    }

    // We're avoiding floating points and truncating values (exactly what the Haskell node does)
    // so we check that balance * 100 = fee * collPercentage
    // When we display to the user, we want to display minimum_collateral in lovelace, not in 100ths of a lovelace,
    // so we divide by 100 and round up
    let minimum_collateral = fee * protocol_parameters.collateral_percentage as u64;
    if balance.coin * 100 < minimum_collateral as i64 {
        return Err(InvalidCollateral::InsufficientBalance {
            provided: balance.coin as u64,
            required: minimum_collateral.div_ceil(100),
        });
    }

    if let Some(expected_balance) = tx_collateral {
        if expected_balance != balance.coin as u64 {
            return Err(InvalidCollateral::IncorrectTotalCollateral {
                provided: balance.coin as u64,
                expected: expected_balance,
            });
        }
    }

    Ok(())
}

fn add_value_to_balance(balance: &mut CollateralBalance, output: &MemoizedTransactionOutput) {
    let output_balance: CollateralBalance = (&output.value).into();

    balance.coin += output_balance.coin;
    for (key, value) in output_balance.multiasset {
        balance
            .multiasset
            .entry(key)
            .and_modify(|v| *v += value)
            .or_insert(value);
    }
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
