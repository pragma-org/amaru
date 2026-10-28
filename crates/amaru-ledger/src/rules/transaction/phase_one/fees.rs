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

use amaru_kernel::{
    ExUnitPrices, ExUnits, HasExUnits, Lovelace, MemoizedTransactionOutput, ProtocolParameters, RationalNumber,
    TransactionInput, WitnessSet,
    cardano::value::Balance,
    minimum_fee,
    rational_number::{SafeRatio, floor_to_lovelace, into_safe_ratio, safe_ratio},
};
use num::{BigUint, Zero};

use crate::{
    context::{BalanceSlice, PotsSliceMut},
    rules::transaction::phase_one::inputs::ResolvedCollateral,
};

#[derive(Debug, thiserror::Error)]
pub enum InvalidFees {
    #[error("declared fee {provided} below minimum {minimum}")]
    FeeTooSmall { provided: Lovelace, minimum: Lovelace },

    #[error("invalid collateral: {0}")]
    Collateral(#[from] InvalidCollateral),
}

#[derive(Debug, thiserror::Error)]
pub enum InvalidCollateral {
    #[error("a collateral input is locked at a script address: {0}")]
    LockedAtScriptAddress(TransactionInput),
    #[error("effective collateral value (={effective}) is insufficient; at least {required} is required")]
    InsufficientBalance { effective: Lovelace, required: u128 },
    #[error("declared collateral (={declared}) does not equal effective collateral (={effective})")]
    DeclaredCollateralMismatch { effective: Lovelace, declared: Lovelace },
    #[error("No collateral was provided, but collateral is required")]
    NoCollateral,
    #[error("collateral has non-zero delta: {0}")]
    ValueNotConserved(Balance),
}

/// Validates declared fee against the computed minimum and, when the transaction carries
/// redeemers, its collateral. Then collects the fee into the context pots.
///
/// `ref_scripts_size` is the total on-wire byte size of every script attached
/// to the transaction's reference-input UTxO entries; already accumulated by
/// [`crate::rules::transaction::phase_one::inputs::execute`].
///
/// Reference: <https://github.com/IntersectMBO/cardano-ledger/blob/0cfbf861cfb456660a7b73281c6fb714a53d40f9/eras/conway/impl/src/Cardano/Ledger/Conway/Tx.hs#L89>
pub(crate) fn when_valid<C>(context: &mut C, declared: DeclaredFees<'_>) -> Result<(), InvalidFees>
where
    C: PotsSliceMut + BalanceSlice,
{
    let fees = declared.fees;
    validate(context, declared)?;
    context.add_fees(fees);
    Ok(())
}

/// Validates declared fee and collateral like [`when_valid`], then collects the effective collateral into the
/// context pots instead of the fee.
pub(crate) fn when_invalid<C>(context: &mut C, declared: DeclaredFees<'_>) -> Result<(), InvalidFees>
where
    C: PotsSliceMut + BalanceSlice,
{
    let effective_collateral = validate(context, declared)?;
    context.add_fees(effective_collateral);
    Ok(())
}

pub(crate) struct DeclaredFees<'a> {
    pub fees: Lovelace,
    pub collateral: ResolvedCollateral,
    pub collateral_return: Option<&'a MemoizedTransactionOutput>,
    pub declared_collateral: Option<Lovelace>,
    pub tx_size: u64,
    pub witness_set: &'a WitnessSet,
    pub ref_scripts_size: u64,
    pub protocol_parameters: &'a ProtocolParameters,
}

/// Returns the effective collateral, zero when the transaction carries no redeemers.
fn validate<C>(context: &mut C, declared: DeclaredFees<'_>) -> Result<Lovelace, InvalidFees>
where
    C: BalanceSlice,
{
    let DeclaredFees {
        fees,
        collateral,
        collateral_return,
        declared_collateral,
        tx_size,
        witness_set,
        ref_scripts_size,
        protocol_parameters: pp,
    } = declared;

    let minimum = minimum_fee(tx_size, witness_set, ref_scripts_size, pp);

    if fees < minimum {
        return Err(InvalidFees::FeeTooSmall { provided: fees, minimum });
    }

    let effective_collateral = if witness_set.redeemer.is_some() {
        validate_collateral(collateral, collateral_return, declared_collateral, fees, pp)?
    } else {
        0
    };

    context.produce_lovelace(fees);

    Ok(effective_collateral)
}

/// Collateral validation occurs during fee validation in the Haskell node. See the comments below for notes on
/// collateral validation:
///
/// <https://github.com/IntersectMBO/cardano-ledger/blob/master/eras/babbage/impl/src/Cardano/Ledger/Babbage/Rules/Utxo.hs#L180-L195>
fn validate_collateral(
    collateral: ResolvedCollateral,
    collateral_return: Option<&MemoizedTransactionOutput>,
    declared_collateral: Option<Lovelace>,
    fee: Lovelace,
    pp: &ProtocolParameters,
) -> Result<Lovelace, InvalidCollateral> {
    if let Some(input) = collateral.locked_at_script {
        return Err(InvalidCollateral::LockedAtScriptAddress(input));
    }

    let mut effective_collateral = collateral.value;

    if let Some(collateral_return) = collateral_return {
        effective_collateral -= &collateral_return.value;
    }

    if collateral.provided == 0 {
        return Err(InvalidCollateral::NoCollateral);
    }

    // In order for a collateral balance to be valid it must:
    //    - have no multiassets and
    //    - have a nonnegative coin value
    if effective_collateral.coin() < 0 || effective_collateral.has_assets() {
        return Err(InvalidCollateral::ValueNotConserved(effective_collateral));
    }

    let required = u128::from(fee) * u128::from(pp.collateral_percentage);
    let effective = Lovelace::try_from(effective_collateral.coin())
        .unwrap_or_else(|_| unreachable!("collateral exceeds the total supply"));

    if u128::from(effective) * 100 < required {
        return Err(InvalidCollateral::InsufficientBalance { effective, required: required.div_ceil(100) });
    }

    if let Some(declared) = declared_collateral
        && declared != effective
    {
        return Err(InvalidCollateral::DeclaredCollateralMismatch { effective, declared });
    }

    Ok(effective)
}
