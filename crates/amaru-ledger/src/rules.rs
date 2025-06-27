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

use crate::context::PreparationContext;
use amaru_kernel::{cbor, ed25519, into_sized_array, Bytes, MintedBlock};
use std::{array::TryFromSliceError, fmt, fmt::Display};
use thiserror::Error;
use tracing::{instrument, Level};

pub use block::execute as validate_block;

pub mod block;
pub mod transaction;

#[derive(Debug)]
pub enum TransactionField {
    Withdrawals,
}

#[derive(Debug)]
pub struct WithPosition<T: Display> {
    pub position: usize,
    pub element: T,
}

impl<T: Display> Display for WithPosition<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "#{}: {}", self.position, self.element)
    }
}

#[instrument(level = Level::TRACE, skip_all, name="ledger.prepare_block")]
pub fn prepare_block<'block>(
    context: &mut impl PreparationContext<'block>,
    block: &'block MintedBlock<'_>,
) {
    block.transaction_bodies.iter().for_each(|transaction| {
        let inputs = transaction.inputs.iter();

        let collaterals = transaction
            .collateral
            .as_deref()
            .map(|xs| xs.as_slice())
            .unwrap_or(&[])
            .iter();

        let reference_inputs = transaction
            .reference_inputs
            .as_deref()
            .map(|xs| xs.as_slice())
            .unwrap_or(&[])
            .iter();

        inputs
            .chain(reference_inputs)
            .chain(collaterals)
            .for_each(|input| context.require_input(input));
    });
}

#[instrument(level = Level::TRACE, skip_all, name = "ledger.parse_block", fields(block.size = bytes.len()))]
pub fn parse_block(bytes: &[u8]) -> Result<MintedBlock<'_>, cbor::decode::Error> {
    let (_, block): (u16, MintedBlock<'_>) = cbor::decode(bytes)?;
    Ok(block)
}

pub(crate) fn format_vec<T: Display>(items: &[T]) -> String {
    items
        .iter()
        .map(|item| item.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Error)]
pub enum InvalidEd25519Signature {
    #[error("invalid signature size: {error:?}")]
    InvalidSignatureSize {
        error: TryFromSliceError,
        expected: usize,
    },
    #[error("invalid verification key size: {error:?}")]
    InvalidKeySize {
        error: TryFromSliceError,
        expected: usize,
    },
    #[error("invalid signature for given key")]
    InvalidSignature,
}

pub(crate) fn verify_ed25519_signature(
    vkey: &Bytes,
    signature: &Bytes,
    message: &[u8],
) -> Result<(), InvalidEd25519Signature> {
    // TODO: vkey should come as sized bytes out of the serialization.
    // To be fixed upstream in Pallas.
    let public_key = ed25519::PublicKey::from(into_sized_array(vkey, |error, expected| {
        InvalidEd25519Signature::InvalidKeySize { error, expected }
    })?);

    // TODO: signature should come as sized bytes out of the serialization.
    // To be fixed upstream in Pallas.
    let signature = ed25519::Signature::from(into_sized_array(signature, |error, expected| {
        InvalidEd25519Signature::InvalidSignatureSize { error, expected }
    })?);

    if !public_key.verify(message, &signature) {
        Err(InvalidEd25519Signature::InvalidSignature)
    } else {
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        context::assert::{AssertPreparationContext, AssertValidationContext},
        rules::{
            self,
            block::{BlockValidation, InvalidBlockDetails},
        },
        tests::{fake_input, fake_output},
    };
    use amaru_kernel::protocol_parameters::ProtocolParameters;
    use std::{collections::BTreeMap, sync::LazyLock};

    static CONWAY_BLOCK: LazyLock<Vec<u8>> = LazyLock::new(|| {
        // These bytes are Conway3.block from Pallas https://github.com/txpipe/pallas/blob/main/test_data/conway3.block
        hex::decode("820785828a1a00153df41a01aa8a0458201bbf3961f179735b68d8f85bcff85b1eaaa6ec3fa6218e4b6f4be7c6129e37ba5820472a53a312467a3b66ede974399b40d1ea428017bc83cf9647d421b21d1cb74358206ee6456894a5931829207e497e0be77898d090d0ac0477a276712dee34e51e05825840d35e871ff75c9a243b02c648bccc5edf2860edba0cc2014c264bbbdb51b2df50eff2db2da1803aa55c9797e0cc25bdb4486a4059c4687364ad66ed15b4ec199f58508af7f535948fac488dc74123d19c205ea2b02cbbf91104bbad140d4ba4bb4d75f7fdb762586802f116bdba3ecaa0840614a2b96d619006c3274b590bcd2599e39a17951cbc3db6348fa2688158384f081901965820d8038b5679ffc770b060578bcd7b33045f2c3aa5acc7bd8cde8b705cfe673d7584582030449be32ae7b8363fde830fc9624945862b281e481ec7f5997c75d1f2316c560018ca5840f5d96ce2055a67709c8e6809c882f71ebd7fc6350018d36d803a55b9230ec6c4cbcd41a09255db45214e278f89b39005ac0f213473acbf455165cdcaa9558e0c8209005901c02ba5dda40daa84b3f9c524016c21d7ce13f585062e35298aa31ea590fee809e75ae999dff9b3ee188e01cfcecc384faba50ca673af2388c3cf7407206019920e99e195bc8e6d1a42ef2b7fb549a8da0591180da17db7a24334b098bfef839334761ec51c2bd8a044fd1785b4e216f811dbdcba63eb853a477d3ea87a3b2d61ccfeae74765c51ec1313ffb121573bae4fc3a742825168760f615a0b2b6ef8a42084f9465501774310772de17a574d8d6bef6b14f4277c8b792b4f60f6408262e7aee5e95b8539df07f953d16b209b6d8fa598a6c51ab90659523720c98ffd254bf305106c0b9c6938c33323e191b5afbad8939270c76a82dc2124525aab11396b9de746be6d7fae2c1592c6546474cebe07d1f48c05f36f762d218d9d2ca3e67c27f0a3d82cdd1bab4afa7f3f5d3ecb10c6449300c01b55e5d83f6cefc6a12382577fc7f3de09146b5f9d78f48113622ee923c3484e53bff74df65895ec0ddd43bc9f00bf330681811d5d20d0e30eed4e0d4cc2c75d1499e05572b13fb4e7b0dabf6e36d1988b47fbdecffc01316885f802cd6c60e044bf50a15418530d628cffd506d4eb0db6155be94ce84fbf6529ee06ec78e9c3009c0f5504978dd150926281a400d90102828258202e6b2226fd74ab0cadc53aaa18759752752bd9b616ea48c0e7b7be77d1af4bf400825820d5dc99581e5f479d006aca0cd836c2bb7ddcd4a243f8e9485d3c969df66462cb00018182583900bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965411b0000000ba4332169021a0002c71d14d9010281841b0000000ba43b7400581de0061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965418400f6a2001bffffffffffffffff09d81e821bfffffffffffffffe1bfffffffffffffffff68275687474703a2f2f636f73746d646c732e74657374735820931f1d8cdfdc82050bd2baadfe384df8bf99b00e36cb12bfb8795beab3ac7fe581a100d9010281825820794ff60d3c35b97f55896d1b2a455fe5e89b77fb8094d27063ff1f260d21a67358403894a10bf9fca0592391cdeabd39891fc2f960fae5a2743c73391c495dfdf4ba4f1cb5ede761bebd7996eba6bbe4c126bcd1849afb9504f4ae7fb4544a93ff0ea080").expect("Failed to decode Conway3.block hex")
    });

    static MODIFIED_CONWAY_BLOCK: LazyLock<Vec<u8>> = LazyLock::new(|| {
        // These bytes are modified to be invalid CBOR, originally from Conway3.block from Pallas https://github.com/txpipe/pallas/blob/main/test_data/conway3.block
        hex::decode("830785828a1a00153df41a01aa8a0458201bbf3961f179735b68d8f85bcff85b1eaaa6ec3fa6218e4b6f4be7c6129e37ba5820472a53a312467a3b66ede974399b40d1ea428017bc83cf9647d421b21d1cb74358206ee6456894a5931829207e497e0be77898d090d0ac0477a276712dee34e51e05825840d35e871ff75c9a243b02c648bccc5edf2860edba0cc2014c264bbbdb51b2df50eff2db2da1803aa55c9797e0cc25bdb4486a4059c4687364ad66ed15b4ec199f58508af7f535948fac488dc74123d19c205ea2b02cbbf91104bbad140d4ba4bb4d75f7fdb762586802f116bdba3ecaa0840614a2b96d619006c3274b590bcd2599e39a17951cbc3db6348fa2688158384f081901965820d8038b5679ffc770b060578bcd7b33045f2c3aa5acc7bd8cde8b705cfe673d7584582030449be32ae7b8363fde830fc9624945862b281e481ec7f5997c75d1f2316c560018ca5840f5d96ce2055a67709c8e6809c882f71ebd7fc6350018d36d803a55b9230ec6c4cbcd41a09255db45214e278f89b39005ac0f213473acbf455165cdcaa9558e0c8209005901c02ba5dda40daa84b3f9c524016c21d7ce13f585062e35298aa31ea590fee809e75ae999dff9b3ee188e01cfcecc384faba50ca673af2388c3cf7407206019920e99e195bc8e6d1a42ef2b7fb549a8da0591180da17db7a24334b098bfef839334761ec51c2bd8a044fd1785b4e216f811dbdcba63eb853a477d3ea87a3b2d61ccfeae74765c51ec1313ffb121573bae4fc3a742825168760f615a0b2b6ef8a42084f9465501774310772de17a574d8d6bef6b14f4277c8b792b4f60f6408262e7aee5e95b8539df07f953d16b209b6d8fa598a6c51ab90659523720c98ffd254bf305106c0b9c6938c33323e191b5afbad8939270c76a82dc2124525aab11396b9de746be6d7fae2c1592c6546474cebe07d1f48c05f36f762d218d9d2ca3e67c27f0a3d82cdd1bab4afa7f3f5d3ecb10c6449300c01b55e5d83f6cefc6a12382577fc7f3de09146b5f9d78f48113622ee923c3484e53bff74df65895ec0ddd43bc9f00bf330681811d5d20d0e30eed4e0d4cc2c75d1499e05572b13fb4e7b0dabf6e36d1988b47fbdecffc01316885f802cd6c60e044bf50a15418530d628cffd506d4eb0db6155be94ce84fbf6529ee06ec78e9c3009c0f5504978dd150926281a400d90102828258202e6b2226fd74ab0cadc53aaa18759752752bd9b616ea48c0e7b7be77d1af4bf400825820d5dc99581e5f479d006aca0cd836c2bb7ddcd4a243f8e9485d3c969df66462cb00018182583900bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965411b0000000ba4332169021a0002c71d14d9010281841b0000000ba43b7400581de0061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965418400f6a2001bffffffffffffffff09d81e821bfffffffffffffffe1bfffffffffffffffff68275687474703a2f2f636f73746d646c732e74657374735820931f1d8cdfdc82050bd2baadfe384df8bf99b00e36cb12bfb8795beab3ac7fe581a100d9010281825820794ff60d3c35b97f55896d1b2a455fe5e89b77fb8094d27063ff1f260d21a67358403894a10bf9fca0592391cdeabd39891fc2f960fae5a2743c73391c495dfdf4ba4f1cb5ede761bebd7996eba6bbe4c126bcd1849afb9504f4ae7fb4544a93ff0ea080").expect("Failed to decode Conway3.block hex")
    });

    static CONWAY_BLOCK_CONTEXT: LazyLock<AssertPreparationContext> =
        LazyLock::new(|| AssertPreparationContext {
            utxo: BTreeMap::from([
                (
                    fake_input(
                        "2e6b2226fd74ab0cadc53aaa18759752752bd9b616ea48c0e7b7be77d1af4bf4",
                        0,
                    ),
                    fake_output("61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335"),
                ),
                (
                    fake_input(
                        "d5dc99581e5f479d006aca0cd836c2bb7ddcd4a243f8e9485d3c969df66462cb",
                        0,
                    ),
                    fake_output("61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335"),
                ),
            ]),
        });

    #[test]
    fn validate_block_success() {
        let mut ctx = (*CONWAY_BLOCK_CONTEXT).clone();

        let block = parse_block(&CONWAY_BLOCK).unwrap();

        prepare_block(&mut ctx, &block);

        let results = rules::block::execute(
            &mut AssertValidationContext::from(ctx),
            &ProtocolParameters::default(),
            &block,
        );

        assert!(matches!(results, BlockValidation::Valid(())));
    }

    #[test]
    fn validate_block_serialization_err() {
        assert!(parse_block(&MODIFIED_CONWAY_BLOCK).is_err())
    }

    #[test]
    fn validate_block_header_size_too_big() {
        let pp = ProtocolParameters {
            max_header_size: 1,
            ..Default::default()
        };

        let mut ctx = (*CONWAY_BLOCK_CONTEXT).clone();

        let block = parse_block(&CONWAY_BLOCK).unwrap();

        prepare_block(&mut ctx, &block);

        let results = rules::block::execute(&mut AssertValidationContext::from(ctx), &pp, &block);

        assert!(matches!(
            results,
            BlockValidation::Invalid(InvalidBlockDetails::HeaderSizeTooBig { .. })
        ))
    }

    macro_rules! fixture_context {
        ($hash:literal) => {
            include_json!(concat!("transactions/preprod/", $hash, "/context.json"))
        };
        ($hash:literal, $variant:literal) => {
            include_json!(concat!(
                "transactions/preprod/",
                $hash,
                "/",
                $variant,
                "/context.json"
            ))
        };
    }
    pub(crate) use fixture_context;
}
