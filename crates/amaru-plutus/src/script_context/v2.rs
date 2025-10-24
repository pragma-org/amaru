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
    Constr, DEFAULT_TAG, MaybeIndefArray, ToConstrTag, ToPlutusData, constr_v2,
    script_context::{
        AddrKeyhash, Certificate, DatumHash, KeyValuePairs, Lovelace, OutputRef, PlutusData,
        Redeemer, TimeRange, TransactionId, TransactionOutput, Value, v1::ScriptPurpose,
    },
};

pub use crate::script_context::v1::ScriptContext;
use amaru_kernel::StakeAddress;

// Reference: https://github.com/IntersectMBO/plutus/blob/master/plutus-ledger-api/src/PlutusLedgerApi/V2/Contexts.hs#L82
pub struct TxInfo<'a> {
    inputs: Vec<OutputRef<'a>>,
    reference_inputs: Vec<OutputRef<'a>>,
    outputs: Vec<TransactionOutput<'a>>,
    fee: Value<'a>,
    mint: Value<'a>,
    certificates: Vec<Certificate>,
    withdrawals: KeyValuePairs<StakeAddress, Lovelace>,
    valid_range: TimeRange,
    signatories: Vec<AddrKeyhash>,
    redeemers: KeyValuePairs<ScriptPurpose<'a>, Redeemer>,
    data: KeyValuePairs<DatumHash, PlutusData>,
    id: TransactionId,
}

impl ToPlutusData<2> for TxInfo<'_> {
    fn to_plutus_data(&self) -> PlutusData {
        constr_v2!(
            0,
            [
                self.inputs,
                self.reference_inputs,
                self.outputs,
                self.fee,
                self.mint,
                self.certificates,
                self.withdrawals,
                self.valid_range,
                self.signatories,
                self.redeemers,
                self.data,
                constr_v2!(0, [self.id])
            ]
        )
    }
}

impl ToPlutusData<2> for OutputRef<'_> {
    fn to_plutus_data(&self) -> PlutusData {
        constr_v2!(0, [self.input, self.output])
    }
}

impl ToPlutusData<2> for TransactionOutput<'_> {
    fn to_plutus_data(&self) -> PlutusData {
        constr_v2!(0, [self.address, self.value, self.datum, self.script])
    }
}
