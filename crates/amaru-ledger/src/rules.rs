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

mod block;
mod traits;
mod transaction;

use crate::{
    context::{PreparationContext, ValidationContext},
    rules::transaction::{
        bootstrap_witness, disjoint_ref_inputs,
        metadata::{self, InvalidTransactionMetadata},
        outputs, vkey_witness,
    },
    state::FailedTransactions,
};
use amaru_kernel::{
    cbor, protocol_parameters::ProtocolParameters, AuxiliaryData, ExUnits, Hash, KeepRaw, Lovelace,
    MintedBlock, MintedTransactionBody, MintedWitnessSet, OriginalHash, Redeemers,
    TransactionInput,
};
use block::{body_size::block_body_size_valid, ex_units::*, header_size::block_header_size_valid};
use std::{
    array::TryFromSliceError,
    fmt,
    fmt::{Debug, Display},
    ops::Deref,
};
use thiserror::Error;
use tracing::{instrument, Level};

#[derive(Debug, Error)]
pub enum BlockPreparationError {
    #[error("Deserialization error")]
    DeserializationError,
}

#[derive(Debug, Error)]
pub enum BlockValidationError {
    #[error("Deserialization error")]
    DeserializationError,
    #[error("Rule Violations: {0:?}")]
    RuleViolations(Vec<RuleViolation>),
    #[error("Cascading rule violations: root: {0:?}, resulting error(s): {1:?}")]
    Composite(RuleViolation, Box<BlockValidationError>),
    // TODO: This error shouldn't exist, it's a placeholder for better error handling in less straight forward cases
    #[error("Uncategorized error: {0}")]
    UncategorizedError(String),
}

#[derive(Debug, Error)]
pub enum RuleViolation {
    #[error("Block body size mismatch: supplied {supplied}, actual {actual}")]
    BlockBodySizeMismatch { supplied: usize, actual: usize },
    #[error("Block header size too big: supplied {supplied}, max {max}")]
    BlockHeaderSizeTooBig { supplied: usize, max: usize },
    #[error("Too many execution units in block: provided (mem: {}, steps: {}), max (mem: {}, steps: {})", provided.mem, provided.steps, max.mem, max.steps)]
    TooManyExUnitsBlock { provided: ExUnits, max: ExUnits },
    #[error(
        "Invalid transaction (hash: {transaction_hash}, index: {transaction_index}): {violation} "
    )]
    InvalidTransaction {
        transaction_hash: Hash<32>,
        transaction_index: u32,
        violation: TransactionRuleViolation,
    },
}

#[derive(Debug, Error)]
pub enum TransactionRuleViolation {
    #[error("Inputs included in both reference inputs and spent inputs: intersection [{}]", intersection.iter().map(|input| format!("{}#{}", input.transaction_id, input.transaction_id)).collect::<Vec<_>>().join(", "))]
    NonDisjointRefInputs { intersection: Vec<TransactionInput> },
    #[error("Invalid transaction outputs: [{}]", format_vec(invalid_outputs))]
    InvalidOutputs {
        invalid_outputs: Vec<WithPosition<InvalidOutput>>,
    },
    #[error("Invalid transaction metadata: {0}")]
    InvalidTransactionMetadata(#[from] InvalidTransactionMetadata),
    #[error(
        "Missing required signatures: pkhs [{}]",
        format_vec(missing_key_hashes)
    )]
    MissingRequiredVkeyWitnesses { missing_key_hashes: Vec<Hash<28>> },
    #[error(
        "Invalid verification key witnesses: [{}]",
        format_vec(invalid_witnesses)
    )]
    InvalidVKeyWitnesses {
        invalid_witnesses: Vec<WithPosition<InvalidVKeyWitness>>,
    },
    #[error(
        "Missing required signatures: bootstrap roots [{}]",
        format_vec(missing_bootstrap_roots)
    )]
    MissingRequiredBootstrapWitnesses {
        missing_bootstrap_roots: Vec<Hash<28>>,
    },
    #[error(
        "Invalid bootstrap witnesses: indices [{}]",
        format_vec(invalid_witnesses)
    )]
    InvalidBootstrapWitnesses {
        invalid_witnesses: Vec<WithPosition<InvalidVKeyWitness>>,
    },
    #[error("Unexpected bytes instead of reward account in {context:?} at position {position}")]
    MalformedRewardAccount {
        bytes: Vec<u8>,
        context: TransactionField,
        position: usize,
    },
    // TODO: This error shouldn't exist, it's a placeholder for better error handling in less straight forward cases
    #[error("Uncategorized error: {0}")]
    UncategorizedError(String),
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

#[derive(Debug, Error)]
pub enum InvalidOutput {
    #[error(
        "output doesn't contain enough Lovelace: minimum: {minimum_value}, given: {given_value}"
    )]
    OutputTooSmall {
        minimum_value: Lovelace,
        given_value: Lovelace,
    },
}

#[derive(Debug, Error)]
pub enum InvalidVKeyWitness {
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

#[derive(Debug)]
pub enum TransactionField {
    Withdrawals,
}

impl From<Vec<Option<RuleViolation>>> for BlockValidationError {
    fn from(violations: Vec<Option<RuleViolation>>) -> Self {
        BlockValidationError::RuleViolations(violations.into_iter().flatten().collect())
    }
}

#[instrument(level = Level::TRACE, skip_all)]
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

#[instrument(level = Level::TRACE, skip_all)]
pub fn validate_block(
    context: &mut impl ValidationContext,
    protocol_params: ProtocolParameters,
    block: &MintedBlock<'_>,
) -> Result<(), BlockValidationError> {
    block_header_size_valid(block.header.raw_cbor(), &protocol_params)
        .map_err(|err| BlockValidationError::RuleViolations(vec![err]))?;

    block_body_size_valid(&block.header.header_body, block)
        .map_err(|err| BlockValidationError::RuleViolations(vec![err]))?;

    // TODO: rewrite this to use iterators defined on `Redeemers` and `MaybeIndefArray`, ideally
    let ex_units = block
        .transaction_witness_sets
        .iter()
        .flat_map(|witness_set| {
            witness_set
                .redeemer
                .iter()
                .map(|redeemers| match redeemers.deref() {
                    Redeemers::List(list) => list.iter().map(|r| r.ex_units).collect::<Vec<_>>(),
                    Redeemers::Map(map) => map.iter().map(|(_, r)| r.ex_units).collect::<Vec<_>>(),
                })
        })
        .flatten()
        .collect::<Vec<_>>();

    block_ex_units_valid(ex_units, &protocol_params)
        .map_err(|err| BlockValidationError::RuleViolations(vec![err]))?;

    let transactions = block.transaction_bodies.deref().to_vec();

    let failed_transactions = FailedTransactions::from_block(block);

    let witness_sets = block.transaction_witness_sets.deref().to_vec();

    // using `zip` here instead of enumerate as it is safer to cast from u32 to usize than usize to u32
    // Realistically, we're never gonna hit the u32 limit with the number of transactions in a block (a boy can dream)
    for (i, transaction) in (0u32..).zip(transactions.iter()) {
        let witness_set =
            witness_sets
                .get(i as usize)
                .ok_or(BlockValidationError::UncategorizedError(format!(
                    "Missing witness set for transaction index {}",
                    i
                )))?;

        let auxiliary_data: Option<&AuxiliaryData> = block
            .auxiliary_data_set
            .iter()
            .find(|key_pair| key_pair.0 == i)
            .map(|key_pair| key_pair.1.deref());

        validate_transaction(
            context,
            !failed_transactions.has(i),
            transaction,
            witness_set,
            auxiliary_data,
            &protocol_params,
        )
        .map_err(|err| {
            BlockValidationError::RuleViolations(vec![RuleViolation::InvalidTransaction {
                transaction_hash: transaction.original_hash(),
                transaction_index: i,
                violation: err,
            }])
        })?;
    }

    Ok(())
}

pub fn validate_transaction(
    context: &mut impl ValidationContext,
    is_valid: bool,
    transaction_body: &KeepRaw<'_, MintedTransactionBody<'_>>,
    transaction_witness_set: &MintedWitnessSet<'_>,
    transaction_auxiliary_data: Option<&AuxiliaryData>,
    protocol_params: &ProtocolParameters,
) -> Result<(), TransactionRuleViolation> {
    let new_output_reference = |index: u64| -> TransactionInput {
        TransactionInput {
            transaction_id: transaction_body.original_hash(),
            index,
        }
    };

    metadata::execute(transaction_body, transaction_auxiliary_data)?;

    disjoint_ref_inputs::execute(transaction_body)?;

    outputs::execute(
        protocol_params,
        transaction_body.outputs.iter(),
        &mut |index, output| {
            if is_valid {
                context.produce(new_output_reference(index), output);
            }
        },
    )?;

    outputs::execute(
        protocol_params,
        transaction_body.collateral_return.iter(),
        &mut |index, output| {
            if !is_valid {
                // NOTE: Collateral outputs are indexed as if starting at the end of standard
                // outputs.
                let offset = transaction_body.outputs.len() as u64;
                context.produce(new_output_reference(index + offset), output);
            }
        },
    )?;

    vkey_witness::execute(
        context,
        transaction_body,
        &transaction_witness_set.vkeywitness,
    )?;

    bootstrap_witness::execute(
        context,
        transaction_body,
        &transaction_witness_set.bootstrap_witness,
    )?;

    Ok(())
}

#[instrument(level = Level::TRACE, skip_all, fields(block.size = bytes.len()))]
pub fn parse_block(bytes: &[u8]) -> Result<MintedBlock<'_>, BlockPreparationError> {
    let (_, block): (u16, MintedBlock<'_>) =
        cbor::decode(bytes).map_err(|_| BlockPreparationError::DeserializationError)?;
    Ok(block)
}

fn format_vec<T: Display>(items: &[T]) -> String {
    items
        .iter()
        .map(|item| item.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::assert::{AssertPreparationContext, AssertValidationContext};
    use amaru_kernel::{Bytes, PostAlonzoTransactionOutput, TransactionOutput, Value};
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

    fn fake_input(transaction_id: &str, index: u64) -> TransactionInput {
        TransactionInput {
            transaction_id: Hash::from(hex::decode(transaction_id).unwrap().as_slice()),
            index,
        }
    }

    fn fake_output(address: &str) -> TransactionOutput {
        TransactionOutput::PostAlonzo(PostAlonzoTransactionOutput {
            address: Bytes::from(hex::decode(address).unwrap()),
            value: Value::Coin(0),
            datum_option: None,
            script_ref: None,
        })
    }

    #[test]
    fn validate_block_success() {
        let mut ctx = (*CONWAY_BLOCK_CONTEXT).clone();

        let block = parse_block(&CONWAY_BLOCK).unwrap();

        prepare_block(&mut ctx, &block);

        let results = validate_block(
            &mut AssertValidationContext::from(ctx),
            ProtocolParameters::default(),
            &block,
        );

        assert!(results.is_ok())
    }

    #[test]
    fn validate_block_serialization_err() {
        assert!(parse_block(&MODIFIED_CONWAY_BLOCK)
            .is_err_and(|e| matches!(e, BlockPreparationError::DeserializationError)),);
    }

    #[test]
    #[allow(clippy::wildcard_enum_match_arm)]
    fn validate_block_vkey_witness_missing() {
        let mut ctx = (*CONWAY_BLOCK_CONTEXT).clone();
        ctx.utxo
            .entry(fake_input(
                "2e6b2226fd74ab0cadc53aaa18759752752bd9b616ea48c0e7b7be77d1af4bf4",
                0,
            ))
            .and_modify(|o| {
                *o = fake_output("6100000000000000000000000000000000000000000000000000000000")
            });

        let block = parse_block(&CONWAY_BLOCK).unwrap();

        prepare_block(&mut ctx, &block);

        assert!(validate_block(
            &mut AssertValidationContext::from(ctx),
            ProtocolParameters::default(),
            &block
        )
        .is_err_and(|e| match e {
            BlockValidationError::RuleViolations(violations) => {
                violations.iter().any(|rule_violation| {
                    matches!(
                        rule_violation,
                        RuleViolation::InvalidTransaction {
                            violation: TransactionRuleViolation::MissingRequiredVkeyWitnesses { .. },
                            ..
                        },
                    )
                })
            }
            _ => false,
        }));
    }

    #[test]
    #[allow(clippy::wildcard_enum_match_arm)]
    fn validate_block_header_size_too_big() {
        let pp = ProtocolParameters {
            max_header_size: 1,
            ..Default::default()
        };

        let mut ctx = (*CONWAY_BLOCK_CONTEXT).clone();

        let block = parse_block(&CONWAY_BLOCK).unwrap();

        prepare_block(&mut ctx, &block);

        assert!(
            validate_block(&mut AssertValidationContext::from(ctx), pp, &block).is_err_and(|e| {
                match e {
                    BlockValidationError::RuleViolations(violations) => {
                        violations.iter().any(|rule_violation| {
                            matches!(
                                rule_violation,
                                RuleViolation::BlockHeaderSizeTooBig {
                                    supplied: _,
                                    max: _
                                }
                            )
                        })
                    }
                    _ => false,
                }
            })
        )
    }
}
