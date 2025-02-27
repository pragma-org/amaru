mod block;
mod transaction;

use amaru_kernel::{
    alonzo::MaybeIndefArray, cbor, protocol_parameters::ProtocolParameters, Block, Hash, Hasher,
    MintedBlock, Redeemers, TransactionBody, WitnessSet,
};

use block::{
    body_size::{block_body_size_valid, BlockBodySizeTooBig},
    ex_units::*,
    header_size::{block_header_size_valid, BlockHeaderSizeTooBig},
};
use tracing::{instrument, Level};
use transaction::disjoint_ref_inputs::{disjoint_ref_inputs, NonDisjointRefInputs};
use transaction::output_size::{validate_output_size, OutputTooSmall};

pub enum BlockValidationError {
    SerializationError,
    RuleViolations(Vec<RuleViolation>),
    Composite(RuleViolation, Box<BlockValidationError>),
}

pub enum RuleViolation {
    BlockBodySizeTooBig(BlockBodySizeTooBig),
    BlockHeaderSizeTooBig(BlockHeaderSizeTooBig),
    TooManyExUnitsBlock(TooManyExUnits),
    InvalidTransaction {
        transaction_hash: Hash<32>,
        transaction_index: u32,
        violation: TransactionRuleViolation,
    },
}

pub enum TransactionRuleViolation {
    NonDisjointRefInputs(NonDisjointRefInputs),
    OutputTooSmall(OutputTooSmall),
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
    let (block_header_hash, minted_block) = parse_block(raw_block)?;
    let block: Block = minted_block.clone().into();

    block_header_size_valid(minted_block.header.raw_cbor(), &protocol_params)
        .map_err(|err| BlockValidationError::RuleViolations(vec![err.into()]))?;
    block_body_size_valid(&block.header.header_body, &protocol_params)
        .map_err(|err| BlockValidationError::RuleViolations(vec![err.into()]))?;

    // TODO: rewrite this to use iterators defined on `Redeemers` and `MaybeIndefArray`, ideally
    let ex_units = match block.transaction_witness_sets.clone() {
        MaybeIndefArray::Def(vec) => vec,
        MaybeIndefArray::Indef(vec) => vec,
    }
    .into_iter()
    .flat_map(|witness_set| {
        witness_set
            .redeemer
            .into_iter()
            .map(|redeemers| match redeemers {
                Redeemers::List(list) => list.iter().map(|r| r.ex_units).collect::<Vec<_>>(),
                Redeemers::Map(map) => map.iter().map(|(_, r)| r.ex_units).collect::<Vec<_>>(),
            })
    })
    .flatten()
    .collect::<Vec<_>>();

    block_ex_units_valid(ex_units, &protocol_params)
        .map_err(|err| BlockValidationError::RuleViolations(vec![err.into()]))?;

    let transactions = match block.transaction_bodies {
        MaybeIndefArray::Def(vec) => vec,
        MaybeIndefArray::Indef(vec) => vec,
    };

    let witness_sets = match block.transaction_witness_sets {
        MaybeIndefArray::Def(vec) => vec,
        MaybeIndefArray::Indef(vec) => vec,
    };

    let invalid_transactions = match block.invalid_transactions {
        Some(invalid_transactions) => match invalid_transactions {
            MaybeIndefArray::Def(vec) => vec,
            MaybeIndefArray::Indef(vec) => vec,
        },
        None => Vec::new(),
    };

    for (i, transaction) in (0u32..).zip(transactions) {
        // TODO handle `as` correctly
        let witness_set = match witness_sets.get(i as usize) {
            Some(witness_set) => witness_set,
            None => continue,
        };

        let is_valid = !invalid_transactions.contains(&i);

        validate_transaction(&transaction, witness_set, is_valid, &protocol_params).map_err(
            |err| {
                BlockValidationError::RuleViolations(vec![RuleViolation::InvalidTransaction {
                    // TODO: get the actual transaction hash
                    transaction_hash: Hash::new([0; 32]),
                    transaction_index: i,
                    violation: err,
                }])
            },
        )?;
    }

    Ok((block_header_hash, minted_block))
}

pub fn validate_transaction(
    transaction_body: &TransactionBody,
    _witness_set: &WitnessSet,
    _is_valid: bool,
    protocol_params: &ProtocolParameters,
) -> Result<(), TransactionRuleViolation> {
    disjoint_ref_inputs(transaction_body).map_err(|err| err.into())?;
    validate_output_size(transaction_body, protocol_params).map_err(|err| err.into())?;

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
        let bytes = hex::decode("820785828A1A00153DF41A01AA8A0458201BBF3961F179735B68D8F85BCFF85B1EAAA6EC3FA6218E4B6F4BE7C6129E37BA5820472A53A312467A3B66EDE974399B40D1EA428017BC83CF9647D421B21D1CB74358206EE6456894A5931829207E497E0BE77898D090D0AC0477A276712DEE34E51E05825840D35E871FF75C9A243B02C648BCCC5EDF2860EDBA0CC2014C264BBBDB51B2DF50EFF2DB2DA1803AA55C9797E0CC25BDB4486A4059C4687364AD66ED15B4EC199F58508AF7F535948FAC488DC74123D19C205EA2B02CBBF91104BBAD140D4BA4BB4D75F7FDB762586802F116BDBA3ECAA0840614A2B96D619006C3274B590BCD2599E39A17951CBC3DB6348FA2688158384F081901965820D8038B5679FFC770B060578BCD7B33045F2C3AA5ACC7BD8CDE8B705CFE673D7584582030449BE32AE7B8363FDE830FC9624945862B281E481EC7F5997C75D1F2316C560018CA5840F5D96CE2055A67709C8E6809C882F71EBD7FC6350018D36D803A55B9230EC6C4CBCD41A09255DB45214E278F89B39005AC0F213473ACBF455165CDCAA9558E0C8209005901C02BA5DDA40DAA84B3F9C524016C21D7CE13F585062E35298AA31EA590FEE809E75AE999DFF9B3EE188E01CFCECC384FABA50CA673AF2388C3CF7407206019920E99E195BC8E6D1A42EF2B7FB549A8DA0591180DA17DB7A24334B098BFEF839334761EC51C2BD8A044FD1785B4E216F811DBDCBA63EB853A477D3EA87A3B2D61CCFEAE74765C51EC1313FFB121573BAE4FC3A742825168760F615A0B2B6EF8A42084F9465501774310772DE17A574D8D6BEF6B14F4277C8B792B4F60F6408262E7AEE5E95B8539DF07F953D16B209B6D8FA598A6C51AB90659523720C98FFD254BF305106C0B9C6938C33323E191B5AFBAD8939270C76A82DC2124525AAB11396B9DE746BE6D7FAE2C1592C6546474CEBE07D1F48C05F36F762D218D9D2CA3E67C27F0A3D82CDD1BAB4AFA7F3F5D3ECB10C6449300C01B55E5D83F6CEFC6A12382577FC7F3DE09146B5F9D78F48113622EE923C3484E53BFF74DF65895EC0DDD43BC9F00BF330681811D5D20D0E30EED4E0D4CC2C75D1499E05572B13FB4E7B0DABF6E36D1988B47FBDECFFC01316885F802CD6C60E044BF50A15418530D628CFFD506D4EB0DB6155BE94CE84FBF6529EE06EC78E9C3009C0F5504978DD150926281A400D90102828258202E6B2226FD74AB0CADC53AAA18759752752BD9B616EA48C0E7B7BE77D1AF4BF400825820D5DC99581E5F479D006ACA0CD836C2BB7DDCD4A243F8E9485D3C969DF66462CB00018182583900BBE56449BA4EE08C471D69978E01DB384D31E29133AF4546E6057335061771EAD84921C0CA49A4B48AB03C2AD1B45A182A46485ED1C965411B0000000BA4332169021A0002C71D14D9010281841B0000000BA43B7400581DE0061771EAD84921C0CA49A4B48AB03C2AD1B45A182A46485ED1C965418400F6A2001BFFFFFFFFFFFFFFFF09D81E821BFFFFFFFFFFFFFFFE1BFFFFFFFFFFFFFFFFF68275687474703A2F2F636F73746D646C732E74657374735820931F1D8CDFDC82050BD2BAADFE384DF8BF99B00E36CB12BFB8795BEAB3AC7FE581A100D9010281825820794FF60D3C35B97F55896D1B2A455FE5E89B77FB8094D27063FF1F260D21A67358403894A10BF9FCA0592391CDEABD39891FC2F960FAE5A2743C73391C495DFDF4BA4F1CB5EDE761BEBD7996EBA6BBE4C126BCD1849AFB9504F4AE7FB4544A93FF0EA080").expect("Failed to decode Conway3.block hex");

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
                        matches!(rule_violation, RuleViolation::BlockHeaderSizeTooBig(_))
                    })
                }
                _ => false,
            })
        )
    }

    #[test]
    #[allow(clippy::wildcard_enum_match_arm)]
    fn validate_block_body_size_too_big() {
        // These bytes are Conway3.block from Pallas https://github.com/txpipe/pallas/blob/main/test_data/conway3.block
        let bytes = hex::decode("820785828a1a00153df41a01aa8a0458201bbf3961f179735b68d8f85bcff85b1eaaa6ec3fa6218e4b6f4be7c6129e37ba5820472a53a312467a3b66ede974399b40d1ea428017bc83cf9647d421b21d1cb74358206ee6456894a5931829207e497e0be77898d090d0ac0477a276712dee34e51e05825840d35e871ff75c9a243b02c648bccc5edf2860edba0cc2014c264bbbdb51b2df50eff2db2da1803aa55c9797e0cc25bdb4486a4059c4687364ad66ed15b4ec199f58508af7f535948fac488dc74123d19c205ea2b02cbbf91104bbad140d4ba4bb4d75f7fdb762586802f116bdba3ecaa0840614a2b96d619006c3274b590bcd2599e39a17951cbc3db6348fa2688158384f081901965820d8038b5679ffc770b060578bcd7b33045f2c3aa5acc7bd8cde8b705cfe673d7584582030449be32ae7b8363fde830fc9624945862b281e481ec7f5997c75d1f2316c560018ca5840f5d96ce2055a67709c8e6809c882f71ebd7fc6350018d36d803a55b9230ec6c4cbcd41a09255db45214e278f89b39005ac0f213473acbf455165cdcaa9558e0c8209005901c02ba5dda40daa84b3f9c524016c21d7ce13f585062e35298aa31ea590fee809e75ae999dff9b3ee188e01cfcecc384faba50ca673af2388c3cf7407206019920e99e195bc8e6d1a42ef2b7fb549a8da0591180da17db7a24334b098bfef839334761ec51c2bd8a044fd1785b4e216f811dbdcba63eb853a477d3ea87a3b2d61ccfeae74765c51ec1313ffb121573bae4fc3a742825168760f615a0b2b6ef8a42084f9465501774310772de17a574d8d6bef6b14f4277c8b792b4f60f6408262e7aee5e95b8539df07f953d16b209b6d8fa598a6c51ab90659523720c98ffd254bf305106c0b9c6938c33323e191b5afbad8939270c76a82dc2124525aab11396b9de746be6d7fae2c1592c6546474cebe07d1f48c05f36f762d218d9d2ca3e67c27f0a3d82cdd1bab4afa7f3f5d3ecb10c6449300c01b55e5d83f6cefc6a12382577fc7f3de09146b5f9d78f48113622ee923c3484e53bff74df65895ec0ddd43bc9f00bf330681811d5d20d0e30eed4e0d4cc2c75d1499e05572b13fb4e7b0dabf6e36d1988b47fbdecffc01316885f802cd6c60e044bf50a15418530d628cffd506d4eb0db6155be94ce84fbf6529ee06ec78e9c3009c0f5504978dd150926281a400d90102828258202e6b2226fd74ab0cadc53aaa18759752752bd9b616ea48c0e7b7be77d1af4bf400825820d5dc99581e5f479d006aca0cd836c2bb7ddcd4a243f8e9485d3c969df66462cb00018182583900bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965411b0000000ba4332169021a0002c71d14d9010281841b0000000ba43b7400581de0061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965418400f6a2001bffffffffffffffff09d81e821bfffffffffffffffe1bfffffffffffffffff68275687474703a2f2f636f73746d646c732e74657374735820931f1d8cdfdc82050bd2baadfe384df8bf99b00e36cb12bfb8795beab3ac7fe581a100d9010281825820794ff60d3c35b97f55896d1b2a455fe5e89b77fb8094d27063ff1f260d21a67358403894a10bf9fca0592391cdeabd39891fc2f960fae5a2743c73391c495dfdf4ba4f1cb5ede761bebd7996eba6bbe4c126bcd1849afb9504f4ae7fb4544a93ff0ea080").expect("Failed to decode Conway3.block hex");

        let pp = ProtocolParameters {
            max_block_body_size: 1,
            ..Default::default()
        };

        assert!(
            validate_block(bytes.as_slice(), pp).is_err_and(|e| match e {
                BlockValidationError::RuleViolations(violations) => {
                    violations.iter().any(|rule_violation| {
                        matches!(rule_violation, RuleViolation::BlockBodySizeTooBig(_))
                    })
                }
                _ => false,
            })
        )
    }
}
