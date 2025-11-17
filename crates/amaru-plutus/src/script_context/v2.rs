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

use std::{borrow::Cow, collections::BTreeMap};

use amaru_kernel::{Address, KeyValuePairs};

use crate::{
    PlutusDataError, ToPlutusData, constr_v2,
    script_context::{
        Datums, OutputRef, PlutusData, Redeemers, ScriptPurpose, TransactionOutput, TxInfo, Value,
        Withdrawals,
    },
};

impl ToPlutusData<2> for TxInfo<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        let fee: Value<'_> = self.fee.into();
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

impl ToPlutusData<2> for OutputRef<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        // In PlutusV2, Byron addresses are not allowed
        if let Address::Byron(_) = *self.output.address {
            return Err(PlutusDataError::unsupported_version(
                "byron address included in OutputRef",
                2,
            ));
        }

        constr_v2!(0, [self.input, self.output])
    }
}

impl ToPlutusData<2> for TransactionOutput<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v2!(0, [self.address, self.value, self.datum, self.script])
    }
}

impl ToPlutusData<2> for Datums<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <BTreeMap<_, _> as ToPlutusData<2>>::to_plutus_data(&self.0)
    }
}

impl ToPlutusData<2> for Withdrawals {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <BTreeMap<_, _> as ToPlutusData<2>>::to_plutus_data(&self.0)
    }
}

impl ToPlutusData<2> for Redeemers<'_, ScriptPurpose<'_>> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        let converted: Result<Vec<_>, _> = self
            .0
            .iter()
            .map(|(purpose, data)| {
                Ok((
                    <ScriptPurpose<'_> as ToPlutusData<2>>::to_plutus_data(purpose)?,
                    <Cow<'_, _> as ToPlutusData<2>>::to_plutus_data(data)?,
                ))
            })
            .collect();

        Ok(PlutusData::Map(KeyValuePairs::Def(converted?)))
    }
}
