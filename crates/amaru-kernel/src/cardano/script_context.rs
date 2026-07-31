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

use crate::{ExUnits, MemoizedDatum, PlutusData, RedeemerEntry, RedeemerKey, RedeemerTag, ScriptPurpose, TxInfo};

/// One of the arguments passed to a Plutus validator.
///
///
/// It contains information about the transaction which is being validated, and the specific script which is being run.
///
/// A `ScriptContext` can only be constructed via the [`ScriptContext::new`](Self::new) function.
///
/// The serialized representation of `ScriptContext` may be different for each `PlutusVersion`,
/// so it is important to specify the correct `PlutusVersion` when serializing.
pub struct ScriptContext<'a> {
    pub tx_info: &'a TxInfo<'a>,
    pub redeemer_data: &'a PlutusData,
    pub redeemer_ex_units: ExUnits,
    pub datum: Option<&'a PlutusData>,
    pub script_purpose: &'a ScriptPurpose<'a>,
}

impl<'a> ScriptContext<'a> {
    /// Construct a new [`ScriptContext`] for a specific script execution (identified by its [`RedeemerKey`]).
    ///
    /// Returns `None` if no entry exists in `tx_info.redeemers` for the given key.
    pub fn new(tx_info: &'a TxInfo<'a>, redeemer_key: &RedeemerKey, redeemer_entry: &'a RedeemerEntry<'a>) -> Self {
        let datum = if redeemer_key.tag == RedeemerTag::Spend {
            tx_info.inputs.get(redeemer_key.index as usize).and_then(|output_ref| match &output_ref.output.datum {
                MemoizedDatum::None => None,
                MemoizedDatum::Hash(hash) => tx_info.data.0.get(hash).copied(),
                MemoizedDatum::Inline(data) => Some(data.as_ref().as_ref()),
            })
        } else {
            None
        };

        ScriptContext {
            tx_info,
            redeemer_data: redeemer_entry.data,
            redeemer_ex_units: redeemer_entry.ex_units,
            datum,
            script_purpose: &redeemer_entry.purpose,
        }
    }

    pub fn budget(&self) -> &ExUnits {
        &self.redeemer_ex_units
    }
}
