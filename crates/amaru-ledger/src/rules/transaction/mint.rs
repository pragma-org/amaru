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

use crate::context::{UtxoSlice, WitnessSlice};
use amaru_kernel::{MemoizedDatum, Multiasset, NonZeroInt, RequiredScript, ScriptPurpose};

pub fn execute<C>(context: &mut C, mint: Option<&Multiasset<NonZeroInt>>)
where
    C: UtxoSlice + WitnessSlice,
{
    if let Some(mint) = mint {
        let mut indices: Vec<usize> = (0..mint.len()).collect();
        indices.sort_by(|&a, &b| mint[a].0.cmp(&mint[b].0));

        for (mint_index, original_index) in indices.iter().enumerate() {
            let (policy, _) = mint[*original_index];
            context.require_script_witness(RequiredScript {
                hash: policy,
                index: mint_index as u32,
                purpose: ScriptPurpose::Mint,
                datum: MemoizedDatum::None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{context::assert::AssertValidationContext, rules::tests::fixture_context};
    use amaru_kernel::{include_cbor, include_json, json, MintedTransactionBody};
    use test_case::test_case;
    use tracing_json::assert_trace;

    macro_rules! fixture {
        ($hash:literal) => {
            (
                fixture_context!($hash),
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                include_json!(concat!("transactions/preprod/", $hash, "/expected.traces")),
            )
        };
        ($hash:literal, $variant:literal) => {
            (
                fixture_context!($hash, $variant),
                include_cbor!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/tx.cbor"
                )),
                include_json!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/expected.traces"
                )),
            )
        };
    }

    #[test_case(fixture!("99cd1c8159255cf384ece25f5516fa54daaee6c5efb3f006ecf9780a0775b1dc"))]
    fn test_mint(
        (mut ctx, tx, expected_traces): (
            AssertValidationContext,
            MintedTransactionBody<'_>,
            Vec<json::Value>,
        ),
    ) {
        assert_trace(
            || super::execute(&mut ctx, tx.mint.as_ref()),
            expected_traces,
        )
    }
}
