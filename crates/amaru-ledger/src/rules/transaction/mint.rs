use std::ops::Deref;

use amaru_kernel::{Multiasset, NonZeroInt, RequiredScript, ScriptPurpose};

use crate::context::{UtxoSlice, WitnessSlice};

pub fn execute<C>(context: &mut C, mint: Option<&Multiasset<NonZeroInt>>)
where
    C: UtxoSlice + WitnessSlice,
{
    if let Some(mint) = mint {
        let mut indices: Vec<usize> = (0..mint.len()).collect();
        indices.sort_by(|&a, &b| mint[a].0.cmp(&mint[b].0));

        let mint = mint.deref();
        for (mint_index, original_index) in indices.iter().enumerate() {
            let (policy, _) = mint[*original_index];
            context.require_script_witness(RequiredScript {
                hash: policy,
                index: mint_index as u32,
                purpose: ScriptPurpose::Mint,
                datum_option: None,
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
