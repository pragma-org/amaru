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
    AddrType, Address, AddressError, HasOwnership, Hash, MemoizedTransactionOutput, ProtocolParameters,
    StakeCredential, TransactionInput, cardano::value::Balance, cbor, is_locked_by_script, transaction_input_to_string,
};
use thiserror::Error;

use crate::context::{UtxoSlice, WitnessSlice};

enum CollateralWitness {
    VKey(Hash<28>),
    Bootstrap(Hash<28>),
}

#[derive(Debug, Error)]
pub enum InvalidCollateral {
    #[error("Unknown input: {}", transaction_input_to_string(.0))]
    UnknownInput(TransactionInput),
    #[error("too many collateral inputs: provided: {provided} allowed: {allowed}")]
    TooManyInputs { provided: usize, allowed: usize },
    #[error("a collateral input is locked at a script address: {}", transaction_input_to_string(.0))]
    LockedAtScriptAddress(TransactionInput),
    #[error("total collateral value is insufficient: provided: {provided} required: {required}")]
    InsufficientBalance { provided: i64, required: u64 },
    #[error(
        "total collateral field (expected) does not equal actual collateral (provided): provided: {provided} expected: {expected} "
    )]
    IncorrectTotalCollateral { provided: i64, expected: u64 },
    #[error("No collateral was provided, but collateral is required")]
    NoCollateral,
    #[error("collateral has non-zero delta: {0}")]
    ValueNotConserved(Balance),
    #[error("invalid Byron address payload at collateral input {}: {error}", transaction_input_to_string(input))]
    InvalidByronAddressPayload { input: TransactionInput, error: Box<cbor::decode::Error> },
}

/*
 Collateral validation occurs during fee validation in the Haskell node. See the comments below for notes on collateral validation:
 https://github.com/IntersectMBO/cardano-ledger/blob/master/eras/babbage/impl/src/Cardano/Ledger/Babbage/Rules/Utxo.hs#L180-L195
*/
pub fn execute<C>(
    context: &mut C,
    collaterals: Option<&[TransactionInput]>,
    collateral_return: Option<&MemoizedTransactionOutput>,
    tx_collateral: Option<u64>,
    fee: u64,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), InvalidCollateral>
where
    C: UtxoSlice + WitnessSlice,
{
    let collaterals = collaterals.filter(|c| !c.is_empty()).ok_or(InvalidCollateral::NoCollateral)?;

    let mut balance = Balance::empty();

    let allowed = protocol_parameters.max_collateral_inputs as usize;
    let provided = collaterals.len();
    if provided > allowed {
        return Err(InvalidCollateral::TooManyInputs { provided, allowed });
    }

    for collateral in collaterals.iter() {
        let collateral_input =
            context.lookup(collateral).ok_or_else(|| InvalidCollateral::UnknownInput(collateral.clone()))?;

        if is_locked_by_script(&collateral_input.address) {
            return Err(InvalidCollateral::LockedAtScriptAddress(collateral.clone()));
        }

        let witness = match &collateral_input.address {
            Address::Shelley(addr) => match addr.owner() {
                StakeCredential::AddrKeyhash(hash) => CollateralWitness::VKey(hash),
                StakeCredential::ScriptHash(_) => unreachable!("already rejected by is_locked_by_script"),
            },
            Address::Byron(byron_address) => {
                let payload = byron_address.decode().map_err(|e| {
                    #[allow(clippy::wildcard_enum_match_arm)]
                    match e {
                        AddressError::InvalidByronCbor(error) => InvalidCollateral::InvalidByronAddressPayload {
                            input: collateral.clone(),
                            error: Box::new(error),
                        },
                        _ => unreachable!("byron_address.decode() only returns InvalidByronCbor"),
                    }
                })?;

                #[allow(clippy::wildcard_enum_match_arm)]
                match payload.addrtype {
                    AddrType::PubKey => CollateralWitness::Bootstrap(payload.root),
                    _ => unreachable!("non-PubKey Byron address in collateral input"),
                }
            }
            Address::Stake(_) => unreachable!("found a stake address in a TransactionOutput"),
        };

        balance += collateral_input.value.as_ref();

        match witness {
            CollateralWitness::VKey(hash) => context.require_vkey_witness(hash),
            CollateralWitness::Bootstrap(root) => context.require_bootstrap_witness(root),
        }
    }

    if let Some(collateral_return) = collateral_return {
        balance -= collateral_return.value.as_ref();
    }

    // In order for a collateral balance to be valid it must:
    //    - have no multiassets and
    //    - have a nonnegative coin value
    if !balance.coin() >= 0 && !balance.has_assets() {
        return Err(InvalidCollateral::ValueNotConserved(balance));
    }

    let required = fee * protocol_parameters.collateral_percentage as u64;
    if balance.coin() as i128 * 100 < required as i128 {
        return Err(InvalidCollateral::InsufficientBalance {
            provided: balance.coin(),
            required: required.div_ceil(100),
        });
    }

    if let Some(expected_balance) = tx_collateral
        && expected_balance != balance.coin() as u64
    {
        return Err(InvalidCollateral::IncorrectTotalCollateral {
            provided: balance.coin(),
            expected: expected_balance,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{PREPROD_DEFAULT_PROTOCOL_PARAMETERS, ProtocolParameters, TransactionBody, include_cbor};
    use test_case::test_case;

    use super::InvalidCollateral;
    use crate::{context::assert::AssertValidationContext, rules::tests::fixture_context};

    macro_rules! fixture {
        ($hash:literal) => {
            (
                fixture_context!($hash),
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone(),
            )
        };
        ($hash:literal, $variant:literal) => {
            (
                fixture_context!($hash, $variant),
                include_cbor!(concat!("transactions/preprod/", $hash, "/", $variant, "/tx.cbor")),
                PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone(),
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
    #[test_case(fixture!("3b13b5c319249407028632579ee584edc38eaeb062dac5156437a627d126fbb1", "delegation-script");
        "happy path - script hash delegation part"
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
    // This tx now fails to decode because conway txes cannot have empty collateral
    //#[test_case(
    //    fixture!("3b13b5c319249407028632579ee584edc38eaeb062dac5156437a627d126fbb1", "no-collateral") =>
    //    matches Err(InvalidCollateral::NoCollateral);
    //    "no collateral"
    //)]
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
        (mut ctx, tx, pp): (AssertValidationContext, TransactionBody, ProtocolParameters),
    ) -> Result<(), InvalidCollateral> {
        super::execute(
            &mut ctx,
            tx.collateral.as_deref(),
            tx.collateral_return.as_ref(),
            tx.total_collateral,
            tx.fee,
            &pp,
        )
    }
}
