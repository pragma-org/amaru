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
    Address, Credential, HasOwnership, Hash, Lovelace, MemoizedTransactionOutput, ProtocolParameters, TransactionInput,
    address::byron::AddressType, cardano::value::Balance,
};
use thiserror::Error;

use crate::context::{BalanceSlice, UtxoSlice, WitnessSlice};

enum CollateralWitness {
    VerificationKey(Hash<28>),
    Bootstrap(Hash<28>),
}

#[derive(Debug, Error)]
pub enum InvalidCollateral {
    #[error("Unknown input: {0}")]
    UnknownInput(TransactionInput),
    #[error("too many collateral inputs: provided: {provided} allowed: {allowed}")]
    TooManyInputs { provided: usize, allowed: usize },
    #[error("a collateral input is locked at a script address: {0}")]
    LockedAtScriptAddress(TransactionInput),
    #[error("effective collateral value (={effective}) is insufficient; at least {required} is required")]
    InsufficientBalance { effective: Lovelace, required: Lovelace },
    #[error("declared collateral (={declared}) does not equal effective collateral (={effective})")]
    DeclaredCollateralMismatch { effective: Lovelace, declared: Lovelace },
    #[error("No collateral was provided, but collateral is required")]
    NoCollateral,
    #[error("collateral has non-zero delta: {0}")]
    ValueNotConserved(Balance),
}

/*
 Collateral validation occurs during fee validation in the Haskell node. See the comments below for notes on collateral validation:
 https://github.com/IntersectMBO/cardano-ledger/blob/master/eras/babbage/impl/src/Cardano/Ledger/Babbage/Rules/Utxo.hs#L180-L195
*/
pub fn execute<C>(
    context: &mut C,
    collaterals: Option<&[TransactionInput]>,
    collateral_return: Option<&MemoizedTransactionOutput>,
    declared_collateral: Option<u64>,
    fee: u64,
    protocol_parameters: &ProtocolParameters,
    has_redeemers: bool,
) -> Result<Lovelace, InvalidCollateral>
where
    C: UtxoSlice + WitnessSlice + BalanceSlice,
{
    let collaterals = collaterals.unwrap_or(&[]);

    let allowed = protocol_parameters.max_collateral_inputs as usize;
    let provided = collaterals.len();
    if provided > allowed {
        return Err(InvalidCollateral::TooManyInputs { provided, allowed });
    }

    let mut effective_collateral = Balance::empty();

    for collateral in collaterals.iter() {
        let collateral_input = context.lookup(collateral).ok_or(InvalidCollateral::UnknownInput(*collateral))?;

        if !has_redeemers {
            continue;
        }

        if collateral_input.address.is_locked_by_script() {
            return Err(InvalidCollateral::LockedAtScriptAddress(*collateral));
        }

        let witness = match &collateral_input.address {
            Address::Shelley(addr) => match addr.owner() {
                Credential::KeyHash(hash) => Some(CollateralWitness::VerificationKey(hash)),
                Credential::ScriptHash(_) => None,
            },
            Address::Byron(byron_address) => {
                match byron_address.address_type {
                    AddressType::VerificationKey => Some(CollateralWitness::Bootstrap(byron_address.root)),
                    // FIXME: Not unreachable at all?
                    AddressType::RedemptionVoucher => {
                        unreachable!("non verification key Byron address in collateral input")
                    }
                }
            }
            // FIXME: Not unreachable at all?
            Address::Stake(_) => unreachable!("found a stake address in a TransactionOutput"),
        };

        effective_collateral += &collateral_input.value;

        match witness {
            Some(CollateralWitness::VerificationKey(hash)) => context.require_verification_key_witness(hash),
            Some(CollateralWitness::Bootstrap(root)) => context.require_bootstrap_witness(root),
            None => (),
        }
    }

    if let Some(collateral_return) = collateral_return {
        effective_collateral -= &collateral_return.value;
    }

    if has_redeemers {
        if provided == 0 {
            return Err(InvalidCollateral::NoCollateral);
        }

        // In order for a collateral balance to be valid it must:
        //    - have no multiassets and
        //    - have a nonnegative coin value
        if effective_collateral.coin() < 0 || effective_collateral.has_assets() {
            return Err(InvalidCollateral::ValueNotConserved(effective_collateral));
        }

        let required = fee * protocol_parameters.collateral_percentage as Lovelace;
        let effective = effective_collateral.coin() as Lovelace;

        if effective as i128 * 100 < required as i128 {
            return Err(InvalidCollateral::InsufficientBalance { effective, required: required.div_ceil(100) });
        }

        if let Some(declared) = declared_collateral
            && declared != effective
        {
            return Err(InvalidCollateral::DeclaredCollateralMismatch { effective, declared });
        }

        return Ok(effective);
    }

    Ok(0)
}
