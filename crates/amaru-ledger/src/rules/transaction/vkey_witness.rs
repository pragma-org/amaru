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

// TODO: uncomment and fix, these tests were written for an older version of this validation
// #[cfg(test)]
// mod tests {
//     use std::collections::BTreeMap;

//     use crate::{
//         context::assert::{AssertPreparationContext, AssertValidationContext},
//         rules::transaction::InvalidVKeyWitness,
//         test::{fake_input, fake_output},
//     };
//     use amaru_kernel::{
//         cbor, Bytes, Hash, KeepRaw, MintedTransactionBody, OriginalHash,
//         PostAlonzoTransactionOutput, TransactionInput, TransactionOutput, Value, WitnessSet,
//     };

//     #[test]
//     fn valid_vkey_witnesses() {
//         // The following transaction contains 19 stake key registrations + delegations in a single tx.
//         // Found on Preprod (90412100dcf9229b187c9064f0f05375268e96ccb25524d762e67e3cb0c0259c)
//         let ctx: AssertPreparationContext = AssertPreparationContext {
//             utxo: BTreeMap::from([(
//                 fake_input!(
//                     "D2BC2334497A827666E1F4AC61FF7992BF3177D6C8D512E74BEF22DE6039A55A",
//                     0
//                 ),
//                 fake_output!("00cd886de6de6312f4831d84e358888ab6415cc80b691f11a6baf32001adcbbee04ac19bd87b4f12b4f8d32df72fc1ef9adf152dd288a8041d"),
//             )]),
//         };

//         let tx_bytes = hex::decode("a60081825820d2bc2334497a827666e1f4ac61ff7992bf3177d6c8d512e74bef22de6039a55a00018182583900cd886de6de6312f4831d84e358888ab6415cc80b691f11a6baf32001adcbbee04ac19bd87b4f12b4f8d32df72fc1ef9adf152dd288a8041d1b00000002081f8b8f021a00075c81031a043d995904982682008200581c2ba45f0b60fc8be9384b5188656f9ac3048bd465f3de14dde7853fdc83028200581c2ba45f0b60fc8be9384b5188656f9ac3048bd465f3de14dde7853fdc581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581c3f1a67b95d4cb4ab439affb50466da7712e924077194415493de338583028200581c3f1a67b95d4cb4ab439affb50466da7712e924077194415493de3385581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581cded5d7ac31e7c03d770c38836d2996e74fd5a5bddcabe835e855d67083028200581cded5d7ac31e7c03d770c38836d2996e74fd5a5bddcabe835e855d670581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581cb3980ac6590aff8fd7e10ed83750521bb3919bd63fcc16a7e1fcea0083028200581cb3980ac6590aff8fd7e10ed83750521bb3919bd63fcc16a7e1fcea00581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581ccec4b1096dcf1f0b0d2c5ed5debf36d907f73a5ac5bedf4674b1571583028200581ccec4b1096dcf1f0b0d2c5ed5debf36d907f73a5ac5bedf4674b15715581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581c8c31be8d39395aabf155fb4174b07c2c3431cb98b7b21d51aa12326383028200581c8c31be8d39395aabf155fb4174b07c2c3431cb98b7b21d51aa123263581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581cbfd8a17549611b9659cb9bd53f972d8d9e0d6d32ea07fe7a641f9d1c83028200581cbfd8a17549611b9659cb9bd53f972d8d9e0d6d32ea07fe7a641f9d1c581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581c23b799d83ea8675b4fcfcdaef8981ad744f5b62ced6f01c0164fd74783028200581c23b799d83ea8675b4fcfcdaef8981ad744f5b62ced6f01c0164fd747581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581c25616a433105acdaf929eb2590e32648c7b02fedd2e4c07dfc29bbe383028200581c25616a433105acdaf929eb2590e32648c7b02fedd2e4c07dfc29bbe3581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581ccd86ed3327f523ad0817c63ba7da2946cd42fef1390b96a308e0043d83028200581ccd86ed3327f523ad0817c63ba7da2946cd42fef1390b96a308e0043d581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581c48198980f38c18b4ed266c9cef185f145c85ca51f4d4b41f26eb233183028200581c48198980f38c18b4ed266c9cef185f145c85ca51f4d4b41f26eb2331581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581c367d982913e809f9ad8b7f5224bb6a2866008e6bf48d6857e530379c83028200581c367d982913e809f9ad8b7f5224bb6a2866008e6bf48d6857e530379c581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581c4d722cd091ffe932e726473fcad026e816015574d03c30d18647c32083028200581c4d722cd091ffe932e726473fcad026e816015574d03c30d18647c320581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581c8c3c1931a5fef6a451bfb35e9620acc48441d0cc591fad3f7a7d7f4083028200581c8c3c1931a5fef6a451bfb35e9620acc48441d0cc591fad3f7a7d7f40581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581ccdb1515a1d4bc8c94eca1927b1fb5260d626df7ef17e8099ff70036a83028200581ccdb1515a1d4bc8c94eca1927b1fb5260d626df7ef17e8099ff70036a581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581cf547fcbd9eed6e4e069d8ea28c2400604f184f350740b1b657a2e12383028200581cf547fcbd9eed6e4e069d8ea28c2400604f184f350740b1b657a2e123581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581c644d2c5cc26891fff43cb0d841a76305eb9c58a1f20fdb97c32db0b183028200581c644d2c5cc26891fff43cb0d841a76305eb9c58a1f20fdb97c32db0b1581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581cd0b068f432e64836034fafc4a860a25f8ff33570bcd5169f1b88cbf883028200581cd0b068f432e64836034fafc4a860a25f8ff33570bcd5169f1b88cbf8581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea8882008200581cc3efda3ac26f6bda5d358825da28b961c70e3a4260e054b6840182a383028200581cc3efda3ac26f6bda5d358825da28b961c70e3a4260e054b6840182a3581c8ed5ab11e76094fa2a2ab29fc3a57498d07e68f6fde6f326162eea88081a043d8b49").expect("Failed to decode hex transaction body");
//         let transaction_body: KeepRaw<'_, MintedTransactionBody<'_>> =
//             cbor::decode(&tx_bytes).expect("Failed to cbor decode transaction body");
//         let witness_set_bytes = hex::decode("a10094825820721d7f3be6ee5b4f9ecf602588085699856a0e2ad9d54dbe5926734583c4fc5f584045fb0235925a0dfec423c2aa0d0969b1fbd6843b6238eb6b01c9821c28d66d48db6a6192a4a6660842d9b4c6bcfe1c0c4010c9f15f0f0db1ad7b7dc489734002825820d43ef2fdb2d81d8da3044396b0dee9418f525e865b51de4292c6aad2f608d25358400606225b58b68edc78240fdfcb890265c93b548a303c9ba2e3fe19082b489761f3dc4dbfcdc6919c21a358ffa0bafa32ef78552bea7c29751645f8a3495ceb038258202ba1f455b997999db5de6fc57a0fe73b1d2914e0305461a661a0774f86ddd579584005047074e8d6e4555d9d2ad28d58c1214b302e9c632ba3499f19621abb8104e09a9f9445721289eefe2752e1b82f54041f284f04d302f3b82a8e66ea289cac0a8258209daba133588901615cdb33f8dae0cafdc205ead437c93307eb64eba64ec57d82584015b74fdd94cd825ee251c8b12f3fe5ce218227cf9e29cb6c8ef83ad4cebab090544d2ddbe0876897ddb2bda13096a0cfc9a38a5403be7a26db6c9417615ccd0582582057082d30db321fbf91b4199563ccfc665e55965bfecc938aa7f597c7cfcaffcd58400292bdf4b74d63ef509c04766158d56adb6bc6cad55d4b8d5c76f60c90cc1256f4fc300bd966584d55e6facf38278c85880fa266318c89f27948f8a25e2175088258203618e8b3ea5c3149a5e9fdec4c48e99f9156681adaada2c4d9653010196a6945584023759868d9d644f4bc3f0044575c25a72f89e8d055f447d29f85719e31eb5dba40648aa002345d8cbc6ac45cb84a4f01b38deb8d274dfa8a7f2969f61575f400825820cf314ce079efc1afb916da19673a2085087cdc5bf817c658c6a2a81938e2e083584097a2a63e09c850b2dc221e3041b2746354b5f7723595d64a5bb7d6c006aafc8d244f4e30041b97daf1ac8702ad0b6fb77b1167e019d9ff080894d827ba45cd0f825820617ef8962672cd233c567c6443e1c0120be799b5e6fe5147e338735e31fe62bc58407496043a30e21c6e68c978a84c26bc9970eb0231540dd7c73c45aedf73fb7071ef20bb1c31f70f74bc6c5bb599f87089ca877469fce61a1712747ede4991a90382582033784e9e8716b417c334cd7c0d20bd262a6f6939ec3ee9a7b31eef9b011edb0058404bc307ce50fbe2b8104ce165c16a6f965505c6e1899e9df4dc48e2c46a75bdfdee1cd8009baf0111ffd4ff28a54ff1ac3cc3d822ca9494d5698dbea56e0c790082582040694db8e0d9ed216e22970aad24fc139e47490f5d6f2c25614346e434efab40584034bef212cfd6a9380191e0104bd4a528108f98598791c0c3a8fa0d9753692af32dc864ea11ac93d1ae0c4f88e8599b1b342b7707e23df63080d163bb51bfda098258201a816c7e415a1d331ca43a36c15fda6690aebfd3832bfae9f4c093b0f67a52cb58402ff7da6006629b208497272ad839cd85660f6545641296fa19de0730547fc4af49e9f90747a26c694dc965a4ab18d028ac90d3870cc3fb4d947bf9c470528d0682582003ac61161ce5a258cc1fdb3280fabbb7f883249c837c0d4a20d8b60bec682434584071ee1f5fcf4324bc0ccc6476f6068a0490b22a996040db5c57bff4a4c144a8672170ba4bc4a71b2ea21193e3ab2e03094e1f8b973780b64792c11e6f4ad52d01825820932c248659ce74fd3c1d41b441c29342c5ce21e70da003db983763634ff90c1f5840aa3791164559157870730de9f84b33ad0a507ea06814b0d784a98e03f363c957d6e3083943d973c428a9973cf35ca6c0e4c522af970d2fee9006f016ddac2d07825820cad34456e4a3821fd6f5e05f2ebe63617483bd3621eca93bfdf3e26b005fc6935840fff30867f1e8eba4fc6beb19a60bbc8ecf42ab3e3c38bc2f534f69cf7d9791ee754b5b8ea64a39130baad7ab8469b29a31c18afab2cf9b502a51a2af41780e09825820284a77ded6cc9143c0ed871d8ddb0d1bb70d0febc75a1e28bf6b3509af9546c4584034380f09934525713a8720f6f304f2808dc49efd5cfabcec21052ff2905de817b1dfc88764bfb606ff1821a4cd2efd6f67e9f15139c328c5e5ae7e4785549305825820f97b6758f86af29b3b4a70d3e641a32284189e0d45f693c14d329942931b3a025840f7ff13c545ab7e90881d5a4ea292a1a10079e7cf7e3750efa708faa5323b580977109ae70c5e64a190f781ecaeeae86392dd726162dbf2f047dd5edac6b78f07825820b696e9d8b8b237da26728c80f7dc8689e158160caf0d2789173aa2ded1dda340584092f9697385bd7fd503623da804bfa497c86f1ab48eca58c1ad29c8ef957994565e700e3c9c71e1a407fcfb7b146c43596d05b5a3d696c1606c1155ff1598c00f8258200b7a3819cb333701d21a082510e98cf7caa5682b84cbbe01bb5253802292b7a0584097467b191d2806e422e77dfe9d8de983cb0fe8f8e5b1f1759613595febd8d4fc675a7b3472033117e055f837363522f5101afb3382a3185288a7f22502e8600f8258202dfa32371baf663a433614731ba5c09d79e0349ea73fb4dcc043c2d2c02142a158407db4a1021d7a4a9a8279afc94b9fa3f138f594a251ef16fa064ab8521f94f8183eb59cafdf3c56f9025fa49967b453e8cd7d9877ef49709881ce859bcb13a50b82582064b78faa9ae632670e34a250aa2c7443ca0fbde03184e840d157acf00d571b345840e713172cc41ff713d4d445ca7af649440a97e47f9b1d7202562caaa2f4173e85b66b0976a4c58a9c1722926e7343c879f63d2723dc9e38bbdd13ba0501dbe105").expect("Failed to decode hex transaction witness set bytes");
//         let witness_set: WitnessSet =
//             cbor::decode(&witness_set_bytes).expect("Failed to cbor decode witness set");

//         assert!(matches!(
//             super::execute(
//                 &mut AssertValidationContext::from(ctx),
//                 transaction_body.original_hash(),
//                 witness_set.vkeywitness.as_deref(),
//             ),
//             Ok(())
//         ));
//     }

//     #[test]
//     fn missing_spending_vkey() {
//         // The following test relies on a real tranasction found on Preprod (44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca)
//         // The context is modified to require a different signer than what is in the witness (and what is actually required on Preprod)
//         let ctx: AssertPreparationContext = AssertPreparationContext {
//             utxo: BTreeMap::from([(
//                 fake_input!(
//                     "4dce7cbd967a6c6d2d6d47886200745ac82f356963bd8bae15d8cc1816fcb242",
//                     7
//                 ),
//                 fake_output!("6100000000000000000000000000000000000000000000000000000000"),
//             )]),
//         };

//         let tx_bytes = hex::decode("a500d90102818258204dce7cbd967a6c6d2d6d47886200745ac82f356963bd8bae15d8cc1816fcb242070182825839003dd93ebbcc68b9f97bf72a7e511861e66af16ef1281881b393157fb9fd663095e5adc264dc2782d25ed53fc8352f89b47ba337041b5cda581a001e848082583900e39f803ff75c492db47934bbd4a73d7f76cc4e1d2e682b47ee7d8745eb54ea5e1faa92afe966f769d1f6b4d11d68450ca5d1f00e93aec38e1a00777ef7021a00029309031b000000012a05f200081901f4").expect("Failed to decode hex transaction body");
//         let transaction_body: KeepRaw<'_, MintedTransactionBody<'_>> =
//             cbor::decode(&tx_bytes).expect("Failed to cbor decode transaction body");
//         let witness_set_bytes = hex::decode("a100d9010281825820119feb7e0216cd3e4d3f1d0b491ba94741e06f3ee3a186fe5479ca889e5f0324584006266c476581708eb949ca85d609d53b255f19a997f377ef3bda026689a80a116000f1901cdacfe531fd3a567dd18f99b147612a8a4e90ebb95c992c4daf3105").expect("Failed to decode hex transaction witness set bytes");
//         let witness_set: WitnessSet =
//             cbor::decode(&witness_set_bytes).expect("Failed to cbor decode witness set");

//         assert!(matches!(
//             super::execute(
//                 &mut AssertValidationContext::from(ctx),
//                 transaction_body.original_hash(),
//                 witness_set.vkeywitness.as_deref(),
//             ),
//             Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { .. })
//         ));
//     }

//     #[test]
//     fn missing_required_signer_vkey() {
//         // the following test relies on a handrolled transaction based off of a Preprod transaction (806aef9b20b9fcf2b3ee49b4aa20ebdfae6e0a32a2d8ce877aba8769e96c26bb).
//         // It's been modified to only include the bare minimum to meet this test requirements
//         let ctx: AssertPreparationContext = AssertPreparationContext {
//             utxo: BTreeMap::from([(
//                 fake_input!(
//                     "4dce7cbd967a6c6d2d6d47886200745ac82f356963bd8bae15d8cc1816fcb242",
//                     8
//                 ),
//                 fake_output!("00ef5cd7bb3021d124d1b1c4e06feef8d515bb70e94dd4fec0a9b382dbeb54ea5e1faa92afe966f769d1f6b4d11d68450ca5d1f00e93aec38e"),
//             )]),
//         };

//         let tx_bytes = hex::decode("A400D90102818258204DCE7CBD967A6C6D2D6D47886200745AC82F356963BD8BAE15D8CC1816FCB242080181825839001FEA90E269045C287ECD728C96C1C921A9AE8593FF44866E6B143B97EB54EA5E1FAA92AFE966F769D1F6B4D11D68450CA5D1F00E93AEC38E1A00915B71021A00073B0F0ED9010281581CEF5CD7BB3021D124D1B1C4E06FEEF8D515BB70E94DD4FEC0A9B382DB").expect("Failed to decode hex transaction body");
//         let transaction_body: KeepRaw<'_, MintedTransactionBody<'_>> =
//             cbor::decode(&tx_bytes).expect("Failed to cbor decode transaction body");
//         let witness_set_bytes = hex::decode("A100D90102818258204521C5D4BC35C1A58E1A105CEEF6D97E91EF886BB2193FC009EE7650D10631CE5840ADF57F70E2E4E38C3F6B45F19614AD7409E048B763821D67147867B534DD5FFB84FFA34B2A6C06FD3C97583F7004D12FDB40AD69FC3A7C60BC6947D535E4CF04").expect("Failed to decode hex transaction witness set bytes");
//         let witness_set: WitnessSet =
//             cbor::decode(&witness_set_bytes).expect("Failed to cbor decode witness set");

//         match super::execute(
//             &mut AssertValidationContext::from(ctx),
//             transaction_body.original_hash(),
//             witness_set.vkeywitness.as_deref(),
//         ) {
//             Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { missing_key_hashes }) => {
//                 assert_eq!(
//                     missing_key_hashes,
//                     vec![Hash::from(
//                         hex::decode("EF5CD7BB3021D124D1B1C4E06FEEF8D515BB70E94DD4FEC0A9B382DB")
//                             .expect("failed to decode")
//                             .as_slice(),
//                     )]
//                 )
//             }
//             Ok(_) => panic!("Expected Err, got Ok"),
//             Err(e) => panic!("Unexpected error variant: {:?}", e),
//         }
//     }

//     #[test]
//     fn missing_withdraw_vkey() {
//         // the following test relies on a handrolled transaction based off of a Preprod transaction (bd7aee1f39142e1064dd0f504e2b2d57268c3ea9521aca514592e0d831bd5aca).
//         // It's been modified to only include the bare minimum to meet this test requirements
//         let ctx: AssertPreparationContext = AssertPreparationContext {
//             utxo: BTreeMap::from([(
//                 fake_input!(
//                     "04DFDF00D9D363DD7CCA936F706682B805C8B399855789DB84EDD31792A86FCD",
//                     0
//                 ),
//                 fake_output!("00155340E4A02B5D2E7B39E7044101C8C3DA40A37AB4F17AFDF53DADCF61C083BA69CA5E6946E8DDFE34034CE84817C1B6A806B112706109DA"),
//             )]),
//         };

//         let tx_bytes = hex::decode("A5008182582004DFDF00D9D363DD7CCA936F706682B805C8B399855789DB84EDD31792A86FCD00018182583900155340E4A02B5D2E7B39E7044101C8C3DA40A37AB4F17AFDF53DADCF61C083BA69CA5E6946E8DDFE34034CE84817C1B6A806B112706109DA1B0000057ABC45295E021A0002D3D5031A0436D93805A1581DE061C083BA69CA5E6946E8DDFE34034CE84817C1B6A806B112706109DA1B000000012CDAEBA6").expect("Failed to decode hex transaction body");
//         let transaction_body: KeepRaw<'_, MintedTransactionBody<'_>> =
//             cbor::decode(&tx_bytes).expect("Failed to cbor decode transaction body");
//         let witness_set_bytes = hex::decode("A100818258206B1C60A45618DEBD80CE7EA0713A1313648A890E36A98574716112D8ABB0F60958407CD44DD19E1C3B15B4D9EF1058787885E0360CB47ED127F449F0B65EA4A7E3AB011CD43403FC49A0FD5F19C39726501502E287587D3F3EED1D0A0AF6164A960F").expect("Failed to decode hex transaction witness set bytes");
//         let witness_set: WitnessSet =
//             cbor::decode(&witness_set_bytes).expect("Failed to cbor decode witness set");

//         match super::execute(
//             &mut AssertValidationContext::from(ctx),
//             transaction_body.original_hash(),
//             witness_set.vkeywitness.as_deref(),
//         ) {
//             Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { missing_key_hashes }) => {
//                 assert_eq!(
//                     missing_key_hashes,
//                     vec![Hash::from(
//                         hex::decode("61C083BA69CA5E6946E8DDFE34034CE84817C1B6A806B112706109DA")
//                             .expect("failed to decode")
//                             .as_slice(),
//                     )]
//                 )
//             }
//             Ok(_) => panic!("Expected Err, got Ok"),
//             Err(e) => panic!("Unexpected error variant: {:?}", e),
//         }
//     }

//     // TODO: add tests for voting procedures

//     //TODO: expand certificate tests to handle more certificate types
//     #[test]
//     fn missing_certificate_vkey() {
//         // the following test relies on a handrolled transaction based off of a Preprod transaction (4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2)
//         // The witness set has been modified to exclude the witness assosciated with the certificate
//         let ctx: AssertPreparationContext = AssertPreparationContext {
//             utxo: BTreeMap::from([(
//                 fake_input!(
//                     "BA5CC479647E7DD872901E89347C0DC626E1B073E513B50E1E6E2E92F57FAD49",
//                     13
//                 ),
//                 fake_output!("6082e016828989cd9d809b50d6976d9efa9bc5b2c1a78d4b3bfa1bb83b"),
//             )]),
//         };

//         let tx_bytes = hex::decode("A400D9010281825820BA5CC479647E7DD872901E89347C0DC626E1B073E513B50E1E6E2E92F57FAD490D018182581D6082E016828989CD9D809B50D6976D9EFA9BC5B2C1A78D4B3BFA1BB83B1A00958940021A00030D4004D901028183028200581C112909208360FB65678272A1D6FF45CF5CCCBCBB52BCB0C59BB74862581CF3FF0CF44CF5A63FF15BF18523D15B9A1937FA8653008DB76D66DAFA").expect("Failed to decode hex transaction body");
//         let transaction_body: KeepRaw<'_, MintedTransactionBody<'_>> =
//             cbor::decode(&tx_bytes).expect("Failed to cbor decode transaction body");
//         let witness_set_bytes = hex::decode("A100D901028182582045A35A111726F809CF2C33980CA06E45D29DB1B06153C54E6EAAFB6E4ABFB2E958401616E2F8210DB9A7E16FF7FA004CFB0B828FDBA975D564F7275F4C9B7ED274C9DEC3831D1BAB34ED2114F563A966B676049A8C99371DABA04BB06C3CB07B7F0E").expect("Failed to decode hex transaction witness set bytes");
//         let witness_set: WitnessSet =
//             cbor::decode(&witness_set_bytes).expect("Failed to cbor decode witness set");

//         match super::execute(
//             &mut AssertValidationContext::from(ctx),
//             &transaction_body,
//             &witness_set.vkeywitness,
//         ) {
//             Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { missing_key_hashes }) => {
//                 assert_eq!(
//                     missing_key_hashes,
//                     vec![Hash::from(
//                         hex::decode("112909208360fb65678272a1d6ff45cf5cccbcbb52bcb0c59bb74862")
//                             .expect("failed to decode")
//                             .as_slice(),
//                     )]
//                 )
//             }
//             Ok(_) => panic!("Expected Err, got Ok"),
//             Err(e) => panic!("Unexpected error variant: {:?}", e),
//         }
//     }
// }
