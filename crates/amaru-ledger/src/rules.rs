mod block;
mod transaction;

use std::ops::Deref;

use amaru_kernel::{
    cbor, protocol_parameters::ProtocolParameters, AuxiliaryData, ExUnits, Hash, Hasher,
    MintedBlock, MintedTransactionBody, MintedWitnessSet, OriginalHash, Redeemers,
};
use amaru_kernel::{TransactionInput, TransactionOutput};
use block::{body_size::block_body_size_valid, ex_units::*, header_size::block_header_size_valid};
use thiserror::Error;
use tracing::{instrument, Level};
use transaction::{disjoint_ref_inputs::disjoint_ref_inputs, metadata::validate_metadata};
use transaction::{metadata::InvalidTransactionMetadata, output_size::validate_output_size};

#[derive(Debug, Error)]
pub enum BlockValidationError {
    #[error("Serialization error")]
    SerializationError,
    #[error("Rule Violations: {0:?}")]
    RuleViolations(Vec<RuleViolation>),
    #[error("Cascading rule violations: root: {0:?}, resulting error(s): {1:?}")]
    Composite(RuleViolation, Box<BlockValidationError>),
}

#[derive(Debug, Error)]
pub enum RuleViolation {
    #[error("Block body size mismatch: supplied {supplied}, actual {actual}")]
    BlockBodySizeMismatch { supplied: usize, actual: usize },
    #[error("Block header size too big: supplied {supplied}, max {max}")]
    BlockHeaderSizeTooBig { supplied: usize, max: usize },
    #[error("Too many execution units in block: provided {provided:?}, max {max:?}")]
    TooManyExUnitsBlock { provided: ExUnits, max: ExUnits },
    #[error("Invalid transaction (hash: {transaction_hash:?}, index: {transaction_index}): {violation} ")]
    InvalidTransaction {
        transaction_hash: Hash<32>,
        transaction_index: u32,
        violation: TransactionRuleViolation,
    },
}

#[derive(Debug, Error)]
pub enum TransactionRuleViolation {
    #[error(
        "Inputs included in both reference inputs and spent inputs: intersection {intersection:?}"
    )]
    NonDisjointRefInputs { intersection: Vec<TransactionInput> },
    #[error("Outputs too small: outputs {outputs_too_small:?}")]
    OutputTooSmall {
        outputs_too_small: Vec<TransactionOutput>,
    },
    #[error("Invalid transaction metadata: {0}")]
    InvalidTransactionMetadata(#[from] InvalidTransactionMetadata),
}

impl From<Vec<Option<RuleViolation>>> for BlockValidationError {
    fn from(violations: Vec<Option<RuleViolation>>) -> Self {
        BlockValidationError::RuleViolations(violations.into_iter().flatten().collect())
    }
}

pub fn validate_block(
    raw_block: &[u8],
    protocol_params: ProtocolParameters,
) -> Result<(Hash<32>, MintedBlock<'_>), BlockValidationError> {
    let (block_header_hash, block) = parse_block(raw_block)?;

    block_header_size_valid(block.header.raw_cbor(), &protocol_params)
        .map_err(|err| BlockValidationError::RuleViolations(vec![err.into()]))?;
    block_body_size_valid(&block.header.header_body, &block)
        .map_err(|err| BlockValidationError::RuleViolations(vec![err.into()]))?;

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
        .map_err(|err| BlockValidationError::RuleViolations(vec![err.into()]))?;

    let transactions = block.transaction_bodies.deref().to_vec();

    let witness_sets = block.transaction_witness_sets.deref().to_vec();

    let empty_vec = vec![];
    let invalid_transactions = block.invalid_transactions.as_deref().unwrap_or(&empty_vec);

    // using `zip` here instead of enumerate as it is safer to cast from u32 to usize than usize to u32
    // Realistically, we're never gonna hit the u32 limit with the number of transactions in a block (a boy can dream)
    for (i, transaction) in (0u32..).zip(transactions.iter()) {
        // TODO handle `as` correctly
        let witness_set = match witness_sets.get(i as usize) {
            Some(witness_set) => witness_set,
            None => continue,
        };

        let is_valid = !invalid_transactions.contains(&i);

        let auxiliary_data: Option<&AuxiliaryData> = block
            .auxiliary_data_set
            .iter()
            .find(|key_pair| key_pair.0 == i)
            .map(|key_pair| key_pair.1.deref());

        validate_transaction(
            transaction,
            witness_set,
            auxiliary_data,
            is_valid,
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

    Ok((block_header_hash, block))
}

pub fn validate_transaction(
    transaction_body: &MintedTransactionBody<'_>,
    _witness_set: &MintedWitnessSet<'_>,
    auxiliary_data: Option<&AuxiliaryData>,
    _is_valid: bool,
    protocol_params: &ProtocolParameters,
) -> Result<(), TransactionRuleViolation> {
    validate_metadata(transaction_body, auxiliary_data)?;
    disjoint_ref_inputs(transaction_body)?;
    validate_output_size(transaction_body, protocol_params)?;

    Ok(())
}

#[instrument(level = Level::TRACE, skip(bytes), fields(block.size = bytes.len()))]
fn parse_block(bytes: &[u8]) -> Result<(Hash<32>, MintedBlock<'_>), BlockValidationError> {
    let (_, block): (u16, MintedBlock<'_>) =
        cbor::decode(bytes).map_err(|_| BlockValidationError::SerializationError)?;

    Ok((Hasher::<256>::hash(block.header.raw_cbor()), block))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_block_success() {
        // These bytes are Conway3.block from Pallas https://github.com/txpipe/pallas/blob/main/test_data/conway3.block
        let bytes = hex::decode("820785828a1a00153df41a01aa8a0458201bbf3961f179735b68d8f85bcff85b1eaaa6ec3fa6218e4b6f4be7c6129e37ba5820472a53a312467a3b66ede974399b40d1ea428017bc83cf9647d421b21d1cb74358206ee6456894a5931829207e497e0be77898d090d0ac0477a276712dee34e51e05825840d35e871ff75c9a243b02c648bccc5edf2860edba0cc2014c264bbbdb51b2df50eff2db2da1803aa55c9797e0cc25bdb4486a4059c4687364ad66ed15b4ec199f58508af7f535948fac488dc74123d19c205ea2b02cbbf91104bbad140d4ba4bb4d75f7fdb762586802f116bdba3ecaa0840614a2b96d619006c3274b590bcd2599e39a17951cbc3db6348fa2688158384f081901965820d8038b5679ffc770b060578bcd7b33045f2c3aa5acc7bd8cde8b705cfe673d7584582030449be32ae7b8363fde830fc9624945862b281e481ec7f5997c75d1f2316c560018ca5840f5d96ce2055a67709c8e6809c882f71ebd7fc6350018d36d803a55b9230ec6c4cbcd41a09255db45214e278f89b39005ac0f213473acbf455165cdcaa9558e0c8209005901c02ba5dda40daa84b3f9c524016c21d7ce13f585062e35298aa31ea590fee809e75ae999dff9b3ee188e01cfcecc384faba50ca673af2388c3cf7407206019920e99e195bc8e6d1a42ef2b7fb549a8da0591180da17db7a24334b098bfef839334761ec51c2bd8a044fd1785b4e216f811dbdcba63eb853a477d3ea87a3b2d61ccfeae74765c51ec1313ffb121573bae4fc3a742825168760f615a0b2b6ef8a42084f9465501774310772de17a574d8d6bef6b14f4277c8b792b4f60f6408262e7aee5e95b8539df07f953d16b209b6d8fa598a6c51ab90659523720c98ffd254bf305106c0b9c6938c33323e191b5afbad8939270c76a82dc2124525aab11396b9de746be6d7fae2c1592c6546474cebe07d1f48c05f36f762d218d9d2ca3e67c27f0a3d82cdd1bab4afa7f3f5d3ecb10c6449300c01b55e5d83f6cefc6a12382577fc7f3de09146b5f9d78f48113622ee923c3484e53bff74df65895ec0ddd43bc9f00bf330681811d5d20d0e30eed4e0d4cc2c75d1499e05572b13fb4e7b0dabf6e36d1988b47fbdecffc01316885f802cd6c60e044bf50a15418530d628cffd506d4eb0db6155be94ce84fbf6529ee06ec78e9c3009c0f5504978dd150926281a400d90102828258202e6b2226fd74ab0cadc53aaa18759752752bd9b616ea48c0e7b7be77d1af4bf400825820d5dc99581e5f479d006aca0cd836c2bb7ddcd4a243f8e9485d3c969df66462cb00018182583900bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965411b0000000ba4332169021a0002c71d14d9010281841b0000000ba43b7400581de0061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965418400f6a2001bffffffffffffffff09d81e821bfffffffffffffffe1bfffffffffffffffff68275687474703a2f2f636f73746d646c732e74657374735820931f1d8cdfdc82050bd2baadfe384df8bf99b00e36cb12bfb8795beab3ac7fe581a100d9010281825820794ff60d3c35b97f55896d1b2a455fe5e89b77fb8094d27063ff1f260d21a67358403894a10bf9fca0592391cdeabd39891fc2f960fae5a2743c73391c495dfdf4ba4f1cb5ede761bebd7996eba6bbe4c126bcd1849afb9504f4ae7fb4544a93ff0ea080").expect("Failed to decode Conway3.block hex");

        assert!(validate_block(bytes.as_slice(), ProtocolParameters::default()).is_ok())
    }

    #[test]
    fn validate_block_serialization_err() {
        // These bytes are modified to be invalid CBOR, originally from Conway3.block from Pallas https://github.com/txpipe/pallas/blob/main/test_data/conway3.block
        let bytes = hex::decode("830785828a1a00153df41a01aa8a0458201bbf3961f179735b68d8f85bcff85b1eaaa6ec3fa6218e4b6f4be7c6129e37ba5820472a53a312467a3b66ede974399b40d1ea428017bc83cf9647d421b21d1cb74358206ee6456894a5931829207e497e0be77898d090d0ac0477a276712dee34e51e05825840d35e871ff75c9a243b02c648bccc5edf2860edba0cc2014c264bbbdb51b2df50eff2db2da1803aa55c9797e0cc25bdb4486a4059c4687364ad66ed15b4ec199f58508af7f535948fac488dc74123d19c205ea2b02cbbf91104bbad140d4ba4bb4d75f7fdb762586802f116bdba3ecaa0840614a2b96d619006c3274b590bcd2599e39a17951cbc3db6348fa2688158384f081901965820d8038b5679ffc770b060578bcd7b33045f2c3aa5acc7bd8cde8b705cfe673d7584582030449be32ae7b8363fde830fc9624945862b281e481ec7f5997c75d1f2316c560018ca5840f5d96ce2055a67709c8e6809c882f71ebd7fc6350018d36d803a55b9230ec6c4cbcd41a09255db45214e278f89b39005ac0f213473acbf455165cdcaa9558e0c8209005901c02ba5dda40daa84b3f9c524016c21d7ce13f585062e35298aa31ea590fee809e75ae999dff9b3ee188e01cfcecc384faba50ca673af2388c3cf7407206019920e99e195bc8e6d1a42ef2b7fb549a8da0591180da17db7a24334b098bfef839334761ec51c2bd8a044fd1785b4e216f811dbdcba63eb853a477d3ea87a3b2d61ccfeae74765c51ec1313ffb121573bae4fc3a742825168760f615a0b2b6ef8a42084f9465501774310772de17a574d8d6bef6b14f4277c8b792b4f60f6408262e7aee5e95b8539df07f953d16b209b6d8fa598a6c51ab90659523720c98ffd254bf305106c0b9c6938c33323e191b5afbad8939270c76a82dc2124525aab11396b9de746be6d7fae2c1592c6546474cebe07d1f48c05f36f762d218d9d2ca3e67c27f0a3d82cdd1bab4afa7f3f5d3ecb10c6449300c01b55e5d83f6cefc6a12382577fc7f3de09146b5f9d78f48113622ee923c3484e53bff74df65895ec0ddd43bc9f00bf330681811d5d20d0e30eed4e0d4cc2c75d1499e05572b13fb4e7b0dabf6e36d1988b47fbdecffc01316885f802cd6c60e044bf50a15418530d628cffd506d4eb0db6155be94ce84fbf6529ee06ec78e9c3009c0f5504978dd150926281a400d90102828258202e6b2226fd74ab0cadc53aaa18759752752bd9b616ea48c0e7b7be77d1af4bf400825820d5dc99581e5f479d006aca0cd836c2bb7ddcd4a243f8e9485d3c969df66462cb00018182583900bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965411b0000000ba4332169021a0002c71d14d9010281841b0000000ba43b7400581de0061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965418400f6a2001bffffffffffffffff09d81e821bfffffffffffffffe1bfffffffffffffffff68275687474703a2f2f636f73746d646c732e74657374735820931f1d8cdfdc82050bd2baadfe384df8bf99b00e36cb12bfb8795beab3ac7fe581a100d9010281825820794ff60d3c35b97f55896d1b2a455fe5e89b77fb8094d27063ff1f260d21a67358403894a10bf9fca0592391cdeabd39891fc2f960fae5a2743c73391c495dfdf4ba4f1cb5ede761bebd7996eba6bbe4c126bcd1849afb9504f4ae7fb4544a93ff0ea080").expect("Failed to decode Conway3.block hex");

        let pp = ProtocolParameters::default();

        assert!(validate_block(bytes.as_slice(), pp)
            .is_err_and(|e| matches!(e, BlockValidationError::SerializationError)))
    }

    #[test]
    #[allow(clippy::wildcard_enum_match_arm)]
    fn validate_block_header_size_too_big() {
        // These bytes are Conway3.block from Pallas https://github.com/txpipe/pallas/blob/main/test_data/conway3.block
        let bytes = hex::decode("820785828a1a00153df41a01aa8a0458201bbf3961f179735b68d8f85bcff85b1eaaa6ec3fa6218e4b6f4be7c6129e37ba5820472a53a312467a3b66ede974399b40d1ea428017bc83cf9647d421b21d1cb74358206ee6456894a5931829207e497e0be77898d090d0ac0477a276712dee34e51e05825840d35e871ff75c9a243b02c648bccc5edf2860edba0cc2014c264bbbdb51b2df50eff2db2da1803aa55c9797e0cc25bdb4486a4059c4687364ad66ed15b4ec199f58508af7f535948fac488dc74123d19c205ea2b02cbbf91104bbad140d4ba4bb4d75f7fdb762586802f116bdba3ecaa0840614a2b96d619006c3274b590bcd2599e39a17951cbc3db6348fa2688158384f081901965820d8038b5679ffc770b060578bcd7b33045f2c3aa5acc7bd8cde8b705cfe673d7584582030449be32ae7b8363fde830fc9624945862b281e481ec7f5997c75d1f2316c560018ca5840f5d96ce2055a67709c8e6809c882f71ebd7fc6350018d36d803a55b9230ec6c4cbcd41a09255db45214e278f89b39005ac0f213473acbf455165cdcaa9558e0c8209005901c02ba5dda40daa84b3f9c524016c21d7ce13f585062e35298aa31ea590fee809e75ae999dff9b3ee188e01cfcecc384faba50ca673af2388c3cf7407206019920e99e195bc8e6d1a42ef2b7fb549a8da0591180da17db7a24334b098bfef839334761ec51c2bd8a044fd1785b4e216f811dbdcba63eb853a477d3ea87a3b2d61ccfeae74765c51ec1313ffb121573bae4fc3a742825168760f615a0b2b6ef8a42084f9465501774310772de17a574d8d6bef6b14f4277c8b792b4f60f6408262e7aee5e95b8539df07f953d16b209b6d8fa598a6c51ab90659523720c98ffd254bf305106c0b9c6938c33323e191b5afbad8939270c76a82dc2124525aab11396b9de746be6d7fae2c1592c6546474cebe07d1f48c05f36f762d218d9d2ca3e67c27f0a3d82cdd1bab4afa7f3f5d3ecb10c6449300c01b55e5d83f6cefc6a12382577fc7f3de09146b5f9d78f48113622ee923c3484e53bff74df65895ec0ddd43bc9f00bf330681811d5d20d0e30eed4e0d4cc2c75d1499e05572b13fb4e7b0dabf6e36d1988b47fbdecffc01316885f802cd6c60e044bf50a15418530d628cffd506d4eb0db6155be94ce84fbf6529ee06ec78e9c3009c0f5504978dd150926281a400d90102828258202e6b2226fd74ab0cadc53aaa18759752752bd9b616ea48c0e7b7be77d1af4bf400825820d5dc99581e5f479d006aca0cd836c2bb7ddcd4a243f8e9485d3c969df66462cb00018182583900bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965411b0000000ba4332169021a0002c71d14d9010281841b0000000ba43b7400581de0061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965418400f6a2001bffffffffffffffff09d81e821bfffffffffffffffe1bfffffffffffffffff68275687474703a2f2f636f73746d646c732e74657374735820931f1d8cdfdc82050bd2baadfe384df8bf99b00e36cb12bfb8795beab3ac7fe581a100d9010281825820794ff60d3c35b97f55896d1b2a455fe5e89b77fb8094d27063ff1f260d21a67358403894a10bf9fca0592391cdeabd39891fc2f960fae5a2743c73391c495dfdf4ba4f1cb5ede761bebd7996eba6bbe4c126bcd1849afb9504f4ae7fb4544a93ff0ea080").expect("Failed to decode Conway3.block hex");

        let pp = ProtocolParameters {
            max_header_size: 1,
            ..Default::default()
        };

        assert!(
            validate_block(bytes.as_slice(), pp).is_err_and(|e| match e {
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
            })
        )
    }
}
