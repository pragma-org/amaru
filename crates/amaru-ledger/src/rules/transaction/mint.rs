use amaru_kernel::{Multiasset, NonZeroInt, ScriptPurpose};

use crate::context::{UtxoSlice, WitnessSlice};

pub fn execute<C>(context: &mut C, mint: Option<&Multiasset<NonZeroInt>>)
where
    C: UtxoSlice + WitnessSlice,
{
    if let Some(mint) = mint {
        mint.iter().enumerate().for_each(|(index, (policy, _))| {
            context.require_script_witness(*policy, index as u32, ScriptPurpose::Mint)
        });
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
