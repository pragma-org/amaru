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
    AssetName, Hash, MemoizedDatum, NonEmptyKeyValuePairs, NonZeroInt, RedeemerTag, RequiredScript, size::SCRIPT,
};

use crate::context::{BalanceSlice, UtxoSlice, WitnessSlice};

pub fn execute<C>(
    context: &mut C,
    mint: Option<&NonEmptyKeyValuePairs<Hash<SCRIPT>, NonEmptyKeyValuePairs<AssetName, NonZeroInt>>>,
) where
    C: UtxoSlice + WitnessSlice + BalanceSlice,
{
    if let Some(mint) = mint {
        let mut indices: Vec<usize> = (0..mint.len()).collect();
        indices.sort_by(|&a, &b| mint[a].0.cmp(&mint[b].0));

        for (mint_index, original_index) in indices.iter().enumerate() {
            let (policy, _) = mint[*original_index];
            context.require_script_witness(RequiredScript {
                hash: policy,
                index: mint_index as u32,
                purpose: RedeemerTag::Mint,
                datum: MemoizedDatum::None,
            })
        }

        context.add_mint(mint);
    }
}
