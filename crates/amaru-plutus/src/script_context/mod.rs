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

use amaru_kernel::Slot;
use amaru_kernel::{
    AddrKeyhash, Certificate, DatumHash, EraHistory, KeyValuePairs, Lovelace,
    MemoizedTransactionOutput as TransactionOutput, PlutusData, PolicyId, Redeemer, StakeAddress,
    TransactionId, TransactionInput, Value, Voter, Withdrawal,
};

use amaru_slot_arithmetic::{EraHistoryError, TimeMs};

pub mod v1;
pub mod v2;
pub mod v3;

pub trait IsPrePlutusVersion3 {}
impl IsPrePlutusVersion3 for PlutusVersion<1> {}
impl IsPrePlutusVersion3 for PlutusVersion<2> {}

pub use v1::ScriptContext as ScriptContextV1;
pub use v1::TxInfo as TxInfoV1;
pub use v2::ScriptContext as ScriptContextV2;
pub use v2::TxInfo as TxInfoV2;
pub use v3::TxInfo as TxInfoV3;

use crate::PlutusVersion;

pub struct OutputRef {
    pub input: TransactionInput,
    pub output: TransactionOutput,
}

pub struct TimeRange {
    pub lower_bound: Option<TimeMs>,
    pub upper_bound: Option<TimeMs>,
}

impl TimeRange {
    pub fn new(
        valid_from_slot: Option<Slot>,
        valid_to_slot: Option<Slot>,
        tip: &Slot,
        era_history: &EraHistory,
    ) -> Result<Self, EraHistoryError> {
        let lower_bound = valid_from_slot
            .map(|slot| era_history.slot_to_relative_time(slot, *tip))
            .transpose()?;
        let upper_bound = valid_to_slot
            .map(|slot| era_history.slot_to_relative_time(slot, *tip))
            .transpose()?;

        Ok(Self {
            lower_bound,
            upper_bound,
        })
    }
}
