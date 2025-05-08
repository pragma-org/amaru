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
    context::WitnessSlice,
    rules::{format_vec, verify_ed25519_signature, InvalidEd25519Signature, WithPosition},
};
use amaru_kernel::{to_root, BootstrapWitness, Hash, TransactionId};
use std::collections::BTreeSet;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InvalidBootstrapWitnesses {
    #[error(
        "missing required signatures: bootstrap roots [{}]",
        format_vec(missing_bootstrap_roots)
    )]
    MissingRequiredBootstrapWitnesses {
        missing_bootstrap_roots: Vec<Hash<28>>,
    },

    #[error(
        "invalid bootstrap witnesses: indices [{}]",
        format_vec(invalid_witnesses)
    )]
    InvalidSignatures {
        invalid_witnesses: Vec<WithPosition<InvalidEd25519Signature>>,
    },
}

pub fn execute(
    context: &mut impl WitnessSlice,
    transaction_id: TransactionId,
    bootstrap_witnesses: Option<&Vec<BootstrapWitness>>,
) -> Result<(), InvalidBootstrapWitnesses> {
    let empty_vec = vec![];
    let bootstrap_witnesses = bootstrap_witnesses.unwrap_or(&empty_vec);

    let mut provided_bootstrap_roots = BTreeSet::new();
    bootstrap_witnesses.iter().for_each(|witness| {
        provided_bootstrap_roots.insert(to_root(witness));
    });

    let missing_bootstrap_roots = context
        .required_bootstrap_signers()
        .difference(&provided_bootstrap_roots)
        .copied()
        .collect::<Vec<_>>();

    if !missing_bootstrap_roots.is_empty() {
        return Err(
            InvalidBootstrapWitnesses::MissingRequiredBootstrapWitnesses {
                missing_bootstrap_roots,
            },
        );
    }

    let mut invalid_witnesses = vec![];
    bootstrap_witnesses
        .iter()
        .enumerate()
        .for_each(|(position, witness)| {
            verify_ed25519_signature(
                &witness.public_key,
                &witness.signature,
                transaction_id.as_slice(),
            )
            .unwrap_or_else(|element| invalid_witnesses.push(WithPosition { position, element }))
        });

    if !invalid_witnesses.is_empty() {
        return Err(InvalidBootstrapWitnesses::InvalidSignatures { invalid_witnesses });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        context::assert::AssertValidationContext,
        rules::{tests::fixture_context, InvalidEd25519Signature, WithPosition},
    };
    use amaru_kernel::{
        include_cbor, include_json, json, KeepRaw, OriginalHash, TransactionBody, WitnessSet,
    };
    use test_case::test_case;
    use tracing_json::assert_trace;

    use super::InvalidBootstrapWitnesses;

    macro_rules! fixture {
        ($hash:literal) => {
            (
                fixture_context!($hash),
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                include_cbor!(concat!("transactions/preprod/", $hash, "/witness.cbor")),
                include_json!(concat!("transactions/preprod/", $hash, "/expected.traces")),
            )
        };
        ($hash:literal, $variant:literal) => {
            (
                fixture_context!($hash, $variant),
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                include_cbor!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/witness.cbor"
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

    #[test_case(fixture!("49e6100c24938acb075f3415ddd989c7e91a5c52b8eb848364c660577e11594a"); "happy path")]
    #[test_case(fixture!("49e6100c24938acb075f3415ddd989c7e91a5c52b8eb848364c660577e11594a", "missing-required-witness") =>
        matches Err(InvalidBootstrapWitnesses::MissingRequiredBootstrapWitnesses { missing_bootstrap_roots })
        if missing_bootstrap_roots.len() == 1 && hex::encode(missing_bootstrap_roots[0]) == "65b1fe57f0ed455254aacf1486c448d7f34038c4c445fa905de33d8f";
        "Missing Required Witness")]
    #[test_case(fixture!("49e6100c24938acb075f3415ddd989c7e91a5c52b8eb848364c660577e11594a", "invalid-signature") =>
        matches Err(InvalidBootstrapWitnesses::InvalidSignatures { invalid_witnesses })
        if invalid_witnesses.len() == 1 && matches!(invalid_witnesses[0], WithPosition {
            position: 0,
            element: InvalidEd25519Signature::InvalidSignature});
        "Invalid Signatures: Invalid Signature")]
    #[test_case(fixture!("49e6100c24938acb075f3415ddd989c7e91a5c52b8eb848364c660577e11594a", "invalid-signature-size") =>
        matches Err(InvalidBootstrapWitnesses::InvalidSignatures { invalid_witnesses })
        if invalid_witnesses.len() == 1 && matches!(invalid_witnesses[0], WithPosition {
            position: 0,
            element: InvalidEd25519Signature::InvalidSignatureSize { ..}});
        "Invalid Signatures: Invalid Signature Size")]
    // InvalidKeySize is enforced by the validation context type (Hash<28>).
    // If the key in the signature is the wrong length, the execute function will fail with MissingRequiredWitness
    fn bootstrap_witness(
        (mut ctx, tx, witness_set, expected_traces): (
            AssertValidationContext<'_>,
            KeepRaw<'_, TransactionBody<'_>>,
            KeepRaw<'_, WitnessSet<'_>>,
            Vec<json::Value>,
        ),
    ) -> Result<(), InvalidBootstrapWitnesses> {
        assert_trace(
            || {
                let transaction_id = tx.original_hash();
                super::execute(
                    &mut ctx,
                    transaction_id,
                    witness_set.bootstrap_witness.as_deref(),
                )
            },
            expected_traces,
        )
    }
}
