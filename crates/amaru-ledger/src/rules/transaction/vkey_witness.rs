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
    rules::{
        format_vec, verify_ed25519_signature, InvalidEd25519Signature, TransactionField,
        WithPosition,
    },
};
use amaru_kernel::{Hash, Hasher, TransactionId, VKeyWitness};
use std::collections::BTreeSet;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InvalidVKeyWitness {
    #[error(
        "missing required signatures: pkhs [{}]",
        format_vec(missing_key_hashes)
    )]
    MissingRequiredVkeyWitnesses { missing_key_hashes: Vec<Hash<28>> },

    #[error(
        "invalid verification key witnesses: [{}]",
        format_vec(invalid_witnesses)
    )]
    InvalidSignatures {
        invalid_witnesses: Vec<WithPosition<InvalidEd25519Signature>>,
    },

    #[error("unexpected bytes instead of reward account in {context:?} at position {position}")]
    MalformedRewardAccount {
        bytes: Vec<u8>,
        context: TransactionField,
        position: usize,
    },
}

pub fn execute(
    context: &mut impl WitnessSlice,
    transaction_id: TransactionId,
    vkey_witnesses: Option<&Vec<VKeyWitness>>,
) -> Result<(), InvalidVKeyWitness> {
    let empty_vec = vec![];
    let vkey_witnesses = vkey_witnesses.unwrap_or(&empty_vec);
    let mut provided_vkey_hashes = BTreeSet::new();
    vkey_witnesses.iter().for_each(|witness| {
        provided_vkey_hashes.insert(Hasher::<224>::hash(&witness.vkey));
    });

    let missing_key_hashes = context
        .required_signers()
        .difference(&provided_vkey_hashes)
        .copied()
        .collect::<Vec<_>>();

    if !missing_key_hashes.is_empty() {
        return Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { missing_key_hashes });
    }

    let mut invalid_witnesses = vec![];
    vkey_witnesses
        .iter()
        .enumerate()
        .for_each(|(position, witness)| {
            verify_ed25519_signature(&witness.vkey, &witness.signature, transaction_id.as_slice())
                .unwrap_or_else(|element| {
                    invalid_witnesses.push(WithPosition { position, element })
                })
        });

    if !invalid_witnesses.is_empty() {
        return Err(InvalidVKeyWitness::InvalidSignatures { invalid_witnesses });
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use std::collections::BTreeMap;

    use crate::{
        context::assert::{AssertPreparationContext, AssertValidationContext},
        rules::transaction::InvalidVKeyWitness,
        test::{fake_input, fake_output},
    };
    use amaru_kernel::{
        cbor, Bytes, Hash, KeepRaw, MintedTransactionBody, PostAlonzoTransactionOutput,
        TransactionInput, TransactionOutput, Value, WitnessSet,
    };

    #[test]
    fn missing_spending_vkey() {
        // The following test relies on a real tranasction found on Preprod (44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca)
        // The context is modified to require a different signer than what is in the witness (and what is actually required on Preprod)
        let ctx: AssertPreparationContext = AssertPreparationContext {
            utxo: BTreeMap::from([(
                fake_input!(
                    "4dce7cbd967a6c6d2d6d47886200745ac82f356963bd8bae15d8cc1816fcb242",
                    7
                ),
                fake_output!("6100000000000000000000000000000000000000000000000000000000"),
            )]),
        };

        let tx_bytes = hex::decode("a500d90102818258204dce7cbd967a6c6d2d6d47886200745ac82f356963bd8bae15d8cc1816fcb242070182825839003dd93ebbcc68b9f97bf72a7e511861e66af16ef1281881b393157fb9fd663095e5adc264dc2782d25ed53fc8352f89b47ba337041b5cda581a001e848082583900e39f803ff75c492db47934bbd4a73d7f76cc4e1d2e682b47ee7d8745eb54ea5e1faa92afe966f769d1f6b4d11d68450ca5d1f00e93aec38e1a00777ef7021a00029309031b000000012a05f200081901f4").expect("Failed to decode hex transaction body");
        let transaction_body: KeepRaw<'_, MintedTransactionBody<'_>> =
            cbor::decode(&tx_bytes).expect("Failed to cbor decode transaction body");
        let witness_set_bytes = hex::decode("a100d9010281825820119feb7e0216cd3e4d3f1d0b491ba94741e06f3ee3a186fe5479ca889e5f0324584006266c476581708eb949ca85d609d53b255f19a997f377ef3bda026689a80a116000f1901cdacfe531fd3a567dd18f99b147612a8a4e90ebb95c992c4daf3105").expect("Failed to decode hex transaction witness set bytes");
        let witness_set: WitnessSet =
            cbor::decode(&witness_set_bytes).expect("Failed to cbor decode witness set");

        assert!(matches!(
            super::execute(
                &mut AssertValidationContext::from(ctx),
                &transaction_body,
                &witness_set.vkeywitness,
            ),
            Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { .. })
        ));
    }

    #[test]
    fn missing_required_signer_vkey() {
        // the following test relies on a handrolled transaction based off of a Preprod transaction (806aef9b20b9fcf2b3ee49b4aa20ebdfae6e0a32a2d8ce877aba8769e96c26bb).
        // It's been modified to only include the bare minimum to meet this test requirements
        let ctx: AssertPreparationContext = AssertPreparationContext {
            utxo: BTreeMap::from([(
                fake_input!(
                    "4dce7cbd967a6c6d2d6d47886200745ac82f356963bd8bae15d8cc1816fcb242",
                    8
                ),
                fake_output!("00ef5cd7bb3021d124d1b1c4e06feef8d515bb70e94dd4fec0a9b382dbeb54ea5e1faa92afe966f769d1f6b4d11d68450ca5d1f00e93aec38e"),
            )]),
        };

        let tx_bytes = hex::decode("A400D90102818258204DCE7CBD967A6C6D2D6D47886200745AC82F356963BD8BAE15D8CC1816FCB242080181825839001FEA90E269045C287ECD728C96C1C921A9AE8593FF44866E6B143B97EB54EA5E1FAA92AFE966F769D1F6B4D11D68450CA5D1F00E93AEC38E1A00915B71021A00073B0F0ED9010281581CEF5CD7BB3021D124D1B1C4E06FEEF8D515BB70E94DD4FEC0A9B382DB").expect("Failed to decode hex transaction body");
        let transaction_body: KeepRaw<'_, MintedTransactionBody<'_>> =
            cbor::decode(&tx_bytes).expect("Failed to cbor decode transaction body");
        let witness_set_bytes = hex::decode("A100D90102818258204521C5D4BC35C1A58E1A105CEEF6D97E91EF886BB2193FC009EE7650D10631CE5840ADF57F70E2E4E38C3F6B45F19614AD7409E048B763821D67147867B534DD5FFB84FFA34B2A6C06FD3C97583F7004D12FDB40AD69FC3A7C60BC6947D535E4CF04").expect("Failed to decode hex transaction witness set bytes");
        let witness_set: WitnessSet =
            cbor::decode(&witness_set_bytes).expect("Failed to cbor decode witness set");

        match super::execute(
            &mut AssertValidationContext::from(ctx),
            &transaction_body,
            &witness_set.vkeywitness,
        ) {
            Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { missing_key_hashes }) => {
                assert_eq!(
                    missing_key_hashes,
                    vec![Hash::from(
                        hex::decode("EF5CD7BB3021D124D1B1C4E06FEEF8D515BB70E94DD4FEC0A9B382DB")
                            .expect("failed to decode")
                            .as_slice(),
                    )]
                )
            }
            Ok(_) => panic!("Expected Err, got Ok"),
            Err(e) => panic!("Unexpected error variant: {:?}", e),
        }
    }

    #[test]
    fn missing_withdraw_vkey() {
        // the following test relies on a handrolled transaction based off of a Preprod transaction (bd7aee1f39142e1064dd0f504e2b2d57268c3ea9521aca514592e0d831bd5aca).
        // It's been modified to only include the bare minimum to meet this test requirements
        let ctx: AssertPreparationContext = AssertPreparationContext {
            utxo: BTreeMap::from([(
                fake_input!(
                    "04DFDF00D9D363DD7CCA936F706682B805C8B399855789DB84EDD31792A86FCD",
                    0
                ),
                fake_output!("00155340E4A02B5D2E7B39E7044101C8C3DA40A37AB4F17AFDF53DADCF61C083BA69CA5E6946E8DDFE34034CE84817C1B6A806B112706109DA"),
            )]),
        };

        let tx_bytes = hex::decode("A5008182582004DFDF00D9D363DD7CCA936F706682B805C8B399855789DB84EDD31792A86FCD00018182583900155340E4A02B5D2E7B39E7044101C8C3DA40A37AB4F17AFDF53DADCF61C083BA69CA5E6946E8DDFE34034CE84817C1B6A806B112706109DA1B0000057ABC45295E021A0002D3D5031A0436D93805A1581DE061C083BA69CA5E6946E8DDFE34034CE84817C1B6A806B112706109DA1B000000012CDAEBA6").expect("Failed to decode hex transaction body");
        let transaction_body: KeepRaw<'_, MintedTransactionBody<'_>> =
            cbor::decode(&tx_bytes).expect("Failed to cbor decode transaction body");
        let witness_set_bytes = hex::decode("A100818258206B1C60A45618DEBD80CE7EA0713A1313648A890E36A98574716112D8ABB0F60958407CD44DD19E1C3B15B4D9EF1058787885E0360CB47ED127F449F0B65EA4A7E3AB011CD43403FC49A0FD5F19C39726501502E287587D3F3EED1D0A0AF6164A960F").expect("Failed to decode hex transaction witness set bytes");
        let witness_set: WitnessSet =
            cbor::decode(&witness_set_bytes).expect("Failed to cbor decode witness set");

        match super::execute(
            &mut AssertValidationContext::from(ctx),
            &transaction_body,
            &witness_set.vkeywitness,
        ) {
            Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { missing_key_hashes }) => {
                assert_eq!(
                    missing_key_hashes,
                    vec![Hash::from(
                        hex::decode("61C083BA69CA5E6946E8DDFE34034CE84817C1B6A806B112706109DA")
                            .expect("failed to decode")
                            .as_slice(),
                    )]
                )
            }
            Ok(_) => panic!("Expected Err, got Ok"),
            Err(e) => panic!("Unexpected error variant: {:?}", e),
        }
    }

    // TODO: add tests for voting procedures

    //TODO: expand certificate tests to handle more certificate types
    #[test]
    fn missing_certificate_vkey() {
        // the following test relies on a handrolled transaction based off of a Preprod transaction (4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2)
        // The witness set has been modified to exclude the witness assosciated with the certificate
        let ctx: AssertPreparationContext = AssertPreparationContext {
            utxo: BTreeMap::from([(
                fake_input!(
                    "BA5CC479647E7DD872901E89347C0DC626E1B073E513B50E1E6E2E92F57FAD49",
                    13
                ),
                fake_output!("6082e016828989cd9d809b50d6976d9efa9bc5b2c1a78d4b3bfa1bb83b"),
            )]),
        };

        let tx_bytes = hex::decode("A400D9010281825820BA5CC479647E7DD872901E89347C0DC626E1B073E513B50E1E6E2E92F57FAD490D018182581D6082E016828989CD9D809B50D6976D9EFA9BC5B2C1A78D4B3BFA1BB83B1A00958940021A00030D4004D901028183028200581C112909208360FB65678272A1D6FF45CF5CCCBCBB52BCB0C59BB74862581CF3FF0CF44CF5A63FF15BF18523D15B9A1937FA8653008DB76D66DAFA").expect("Failed to decode hex transaction body");
        let transaction_body: KeepRaw<'_, MintedTransactionBody<'_>> =
            cbor::decode(&tx_bytes).expect("Failed to cbor decode transaction body");
        let witness_set_bytes = hex::decode("A100D901028182582045A35A111726F809CF2C33980CA06E45D29DB1B06153C54E6EAAFB6E4ABFB2E958401616E2F8210DB9A7E16FF7FA004CFB0B828FDBA975D564F7275F4C9B7ED274C9DEC3831D1BAB34ED2114F563A966B676049A8C99371DABA04BB06C3CB07B7F0E").expect("Failed to decode hex transaction witness set bytes");
        let witness_set: WitnessSet =
            cbor::decode(&witness_set_bytes).expect("Failed to cbor decode witness set");

        match super::execute(
            &mut AssertValidationContext::from(ctx),
            &transaction_body,
            &witness_set.vkeywitness,
        ) {
            Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { missing_key_hashes }) => {
                assert_eq!(
                    missing_key_hashes,
                    vec![Hash::from(
                        hex::decode("112909208360fb65678272a1d6ff45cf5cccbcbb52bcb0c59bb74862")
                            .expect("failed to decode")
                            .as_slice(),
                    )]
                )
            }
            Ok(_) => panic!("Expected Err, got Ok"),
            Err(e) => panic!("Unexpected error variant: {:?}", e),
        }
    }
}
