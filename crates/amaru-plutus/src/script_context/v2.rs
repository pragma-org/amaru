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

use std::collections::BTreeMap;

use amaru_kernel::{Address, MemoizedTransactionOutput, PlutusData};

use crate::{
    PlutusDataError, ToPlutusData, constr_v2,
    script_context::{OutputReference, PlutusDatums, PlutusWithdrawals, ScriptContext, TxInfo},
};

impl ToPlutusData<2> for ScriptContext<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v2!(0, [self.tx_info, self.script_purpose])
    }
}

impl ToPlutusData<2> for TxInfo<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        let fee = amaru_kernel::Value::Coin(self.fee);
        constr_v2!(
            0,
            [
                self.inputs,
                self.reference_inputs,
                self.outputs,
                fee,
                self.mint,
                self.certificates,
                self.withdrawals,
                self.valid_range,
                self.signatories,
                self.redeemers,
                self.data,
                constr_v2!(0, [self.id])?
            ]
        )
    }
}

impl ToPlutusData<2> for OutputReference<'_> {
    /// Serialize an `OutputReference` as PlutusData for PlutusV2.
    ///
    /// # Errors
    /// If the UTxO is locked at a bootstrap address, this will return a `PlutusDataError`.
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        if let Address::Byron(_) = self.output.address {
            return Err(PlutusDataError::unsupported_version("byron address included in OutputReference", 2));
        }

        constr_v2!(0, [self.input, self.output])
    }
}

impl ToPlutusData<2> for MemoizedTransactionOutput {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v2!(0, [self.address, self.value, self.datum, self.script])
    }
}

impl ToPlutusData<2> for PlutusDatums<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <BTreeMap<_, _> as ToPlutusData<2>>::to_plutus_data(&self.0)
    }
}

impl ToPlutusData<2> for PlutusWithdrawals {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        let map = self.iter().collect::<BTreeMap<_, _>>();
        <BTreeMap<_, _> as ToPlutusData<2>>::to_plutus_data(&map)
    }
}

impl ToPlutusData<2> for amaru_kernel::RewardAccount {
    /// In PlutusV1 and PlutusV2:
    /// Anywhere a `Credential` is used, it is actually an enum with variants `Pointer` and `Credential`
    ///
    /// It is actually not possible (by the ledger serialization) logic to construct a StakeAdress with a `Pointer`, so this can be hardcoded
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v2!(0, [self.credential()])
    }
}
