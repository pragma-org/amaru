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

use amaru_kernel::{MemoizedDatum, Multiasset, NonZeroInt, RedeemerTag, RequiredScript};

use crate::context::{BalanceSlice, UtxoSlice, WitnessSlice};

pub fn execute<C>(context: &mut C, mint: Option<&Multiasset<NonZeroInt>>)
where
    C: UtxoSlice + WitnessSlice + BalanceSlice,
{
    if let Some(mint) = mint {
        for (index, hash) in mint.keys().enumerate() {
            context.require_script_witness(RequiredScript {
                hash: *hash,
                index: index as u32,
                purpose: RedeemerTag::Mint,
                datum: MemoizedDatum::None,
            })
        }

        context.add_mint(mint);
    }
}
