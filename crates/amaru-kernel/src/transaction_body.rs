// Copyright 2026 PRAGMA
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
    AssetName, Bytes, Certificate, Coin, Debug, Hash, Hasher, MemoizedTransactionOutput, NetworkId,
    NonEmptyKeyValuePairs, NonEmptySet, NonZeroInt, PolicyId, PositiveCoin,
    Proposal as ProposalProcedure, RequiredSigners, RewardAccount, Set, TransactionInput,
    VotingProcedures,
    cbor::{self},
};
use amaru_minicbor_extra::heterogeneous_map;
use std::mem;

pub static DEFAULT_HASH: [u8; TransactionBody::HASH_SIZE] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// A multi-era transaction body. This type is meant to represent all transaction body in eras that
/// we may encounter.
///
///
// NOTE: Obsolete fields from previous eras:
//
// 6: governance updates, prior to Conway.
// 10: has somewhat never existed, or existed but was removed without having been used.
// 12: same
#[derive(Debug, Clone, PartialEq, cbor::Encode)]
#[cbor(map)]
pub struct TransactionBody {
    #[cbor(skip)]
    hash: Hash<{ Self::HASH_SIZE }>,

    #[cbor(skip)]
    original_size: u64,

    #[n(0)]
    pub inputs: Set<TransactionInput>,

    #[n(1)]
    pub outputs: Vec<MemoizedTransactionOutput>,

    #[n(2)]
    pub fee: Coin,

    #[n(3)]
    pub validity_interval_end: Option<u64>,

    #[n(4)]
    pub certificates: Option<NonEmptySet<Certificate>>,

    #[n(5)]
    pub withdrawals: Option<NonEmptyKeyValuePairs<RewardAccount, Coin>>,

    #[n(7)]
    pub auxiliary_data_hash: Option<Bytes>,

    #[n(8)]
    pub validity_interval_start: Option<u64>,

    #[n(9)]
    pub mint: Option<NonEmptyKeyValuePairs<PolicyId, NonEmptyKeyValuePairs<AssetName, NonZeroInt>>>,

    #[n(11)]
    pub script_data_hash: Option<Hash<32>>,

    #[n(13)]
    pub collateral: Option<NonEmptySet<TransactionInput>>,

    #[n(14)]
    pub required_signers: Option<RequiredSigners>,

    #[n(15)]
    pub network_id: Option<NetworkId>,

    #[n(16)]
    pub collateral_return: Option<MemoizedTransactionOutput>,

    #[n(17)]
    pub total_collateral: Option<Coin>,

    #[n(18)]
    pub reference_inputs: Option<NonEmptySet<TransactionInput>>,

    #[n(19)]
    pub votes: Option<VotingProcedures>,

    #[n(20)]
    pub proposals: Option<NonEmptySet<ProposalProcedure>>,

    #[n(21)]
    pub treasury_value: Option<Coin>,

    #[n(22)]
    pub donation: Option<PositiveCoin>,
}

impl TransactionBody {
    // Hash digest size, in bytes.,
    pub const HASH_SIZE: usize = 32;

    /// The original id (i.e. blake2b-256 hash digest) of the transaction body. Note that the hash
    /// computed from the original bytes and memoized; it is not re-computed from re-serialised
    /// data.
    pub fn id(&self) -> Hash<{ Self::HASH_SIZE }> {
        self.hash
    }

    pub fn len(&self) -> u64 {
        self.original_size
    }
}

impl Default for TransactionBody {
    fn default() -> Self {
        Self {
            hash: Hash::new(DEFAULT_HASH),
            original_size: 0,
            inputs: Set::from(vec![]),
            outputs: vec![],
            fee: 0,
            validity_interval_end: None,
            certificates: None,
            withdrawals: None,
            auxiliary_data_hash: None,
            validity_interval_start: None,
            mint: None,
            script_data_hash: None,
            collateral: None,
            required_signers: None,
            network_id: None,
            collateral_return: None,
            total_collateral: None,
            reference_inputs: None,
            votes: None,
            proposals: None,
            treasury_value: None,
            donation: None,
        }
    }
}

// NOTE: Multi-era transaction decoding.
//
// Parsing of transactions must be done according to a specific era, and the exact decoding
// rules may vary per era.
//
// The following decoder assumes Conway as an era since that's all we support at the moment.
// Yet, this means that we will end up rejected perfectly well-formed transactions from other
// eras.
//
// For example, empty but present fields were generally allowed prior to Conway.
//
// Ultimately, we have to suppose multi-era decoders, and promote transactions into a common
// model.
impl<'b, C> cbor::Decode<'b, C> for TransactionBody {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        #[derive(Default)]
        struct RequiredFields {
            inputs: Option<Set<TransactionInput>>,
            outputs: Option<Vec<MemoizedTransactionOutput>>,
            fee: Option<Coin>,
        }

        #[derive(Default)]
        struct State {
            optional: TransactionBody,
            required: RequiredFields,
        }

        let original_bytes = d.input();
        let start_position = d.position();

        let mut state = State::default();

        heterogeneous_map(
            d,
            &mut state,
            |d| d.u64(),
            |d, st, k| {
                match k {
                    0 => blanket(&mut st.required.inputs, k, d, ctx)?,
                    1 => blanket(&mut st.required.outputs, k, d, ctx)?,
                    2 => blanket(&mut st.required.fee, k, d, ctx)?,
                    3 => blanket(&mut st.optional.validity_interval_end, k, d, ctx)?,
                    4 => blanket(&mut st.optional.certificates, k, d, ctx)?,
                    5 => blanket(&mut st.optional.withdrawals, k, d, ctx)?,
                    // 6: governance updates, obsolete in Conway
                    7 => blanket(&mut st.optional.auxiliary_data_hash, k, d, ctx)?,
                    8 => blanket(&mut st.optional.validity_interval_start, k, d, ctx)?,
                    9 => blanket(&mut st.optional.mint, k, d, ctx)?,
                    // 10: there's no 10, has never been used.
                    11 => blanket(&mut st.optional.script_data_hash, k, d, ctx)?,
                    // 12: there's no 12, has never been used.
                    13 => blanket(&mut st.optional.collateral, k, d, ctx)?,
                    14 => blanket(&mut st.optional.required_signers, k, d, ctx)?,
                    15 => blanket(&mut st.optional.network_id, k, d, ctx)?,
                    16 => blanket(&mut st.optional.collateral_return, k, d, ctx)?,
                    17 => blanket(&mut st.optional.total_collateral, k, d, ctx)?,
                    18 => blanket(&mut st.optional.reference_inputs, k, d, ctx)?,
                    19 => blanket(&mut st.optional.votes, k, d, ctx)?,
                    20 => blanket(&mut st.optional.proposals, k, d, ctx)?,
                    21 => blanket(&mut st.optional.treasury_value, k, d, ctx)?,
                    22 => blanket(&mut st.optional.donation, k, d, ctx)?,
                    _ => {
                        let position = d.position();
                        return Err(cbor::decode::Error::message(format!(
                            "unrecognised field key: {k}"
                        ))
                        .at(position));
                    }
                };

                Ok(())
            },
        )?;

        let end_position = d.position();

        Ok(TransactionBody {
            hash: Hasher::<{ TransactionBody::HASH_SIZE * 8 }>::hash(
                &original_bytes[start_position..end_position],
            ),
            original_size: (end_position - start_position) as u64, // from usize
            inputs: expect_field(mem::take(&mut state.required.inputs), 0, "inputs")?,
            outputs: expect_field(mem::take(&mut state.required.outputs), 1, "outputs")?,
            fee: expect_field(mem::take(&mut state.required.fee), 2, "fee")?,
            ..state.optional
        })
    }
}

fn blanket<'d, C, T>(
    field: &mut Option<T>,
    k: u64,
    d: &mut cbor::Decoder<'d>,
    ctx: &mut C,
) -> Result<(), cbor::decode::Error>
where
    T: cbor::Decode<'d, C>,
{
    if field.is_some() {
        return Err(cbor::decode::Error::message(format!(
            "duplicate field entry with key {k}"
        )));
    }

    *field = Some(d.decode_with(ctx)?);

    Ok(())
}

fn expect_field<T>(decoded: Option<T>, key: u64, name: &str) -> Result<T, cbor::decode::Error> {
    decoded.ok_or(cbor::decode::Error::message(format!(
        "missing expected field '{name}' at key {key}"
    )))
}

#[cfg(test)]
mod tests {
    use super::TransactionBody;
    use crate::cbor;
    use test_case::test_case;

    macro_rules! fixture {
        // Allowed eras
        ("conway", $id:expr) => { fixture!(@inner "conway", $id) };

        // Catch-all: invalid era
        ($era:literal, $id:expr) => {
            compile_error!(
                "invalid era: expected one of: \"conway\""
            );
        };

        (@inner $era:literal, $id:expr) => {{
            $crate::try_include_cbor!(concat!(
                "decode_transaction_body/",
                $era,
                "/",
                $id,
                "/sample.cbor",
            ))
        }};
    }

    #[test_case(
        fixture!("conway", "70beb79b18459ff5b826ebeea82ecf566ab79e166ff5749f761ed402ad459466"), 86;
        "simple input -> output payout"
    )]
    #[test_case(
        fixture!("conway", "950bde838976daf2f0019ac6cc5a995e86d99f5ff1b2ffddaaa9ef44b558e4db"), 48;
        "present but empty inputs set"
    )]
    #[test_case(
        fixture!("conway", "ab17a2693346d625ea9afd9bdb9f9b357e60533b3e4cb7576e2c117b94d1c180"), 49;
        "present but empty outputs set"
    )]
    #[test_case(
        fixture!("conway", "c20c7e395ef81d8a6172510408446afc240d533bff18f9dca905e78187c2bcd8"), 80;
        "null fee"
    )]
    fn decode_wellformed(result: Result<TransactionBody, cbor::decode::Error>, expected_size: u64) {
        match result {
            Err(err) => panic!("{err}"),
            Ok(body) => assert_eq!(body.len(), expected_size),
        }
    }

    #[test_case(
        fixture!("conway", "d36a2619a672494604e11bb447cbcf5231e9f2ba25c2169177edc941bd50ad6c"),
        "decode error: missing expected field 'inputs' at key 0";
        "empty body"
    )]
    #[test_case(
        fixture!("conway", "b563891d222561e435b475632a3bdcca58cc3c8ec80ab6b51e0a5c96b6a35e1b"),
        "decode error: missing expected field 'outputs' at key 1";
        "missing outputs"
    )]
    #[test_case(
        fixture!("conway", "9d34025191e23c5996e20c2c0d1718f5cb1d9c4a37a5cb153cbd03c66b59128f"),
        "decode error: missing expected field 'fee' at key 2";
        "missing fee"
    )]
    #[test_case(
        fixture!("conway", "c5f2d5b7e9b8f615c52296e04b3050cf35ad4e8a457a25adaeb2a933de1bf624"),
        "decode error at position 81: empty set when expecting at least one element";
        "empty certificates"
    )]
    #[test_case(
        fixture!("conway", "3b5478c6446496b6ff71c738c83fbf251841dd45cda074b0ac935b1428a52f66"),
        "unexpected type map at position 81: expected array";
        "malformed certificates"
    )]
    #[test_case(
        fixture!("conway", "5123113da4c8e2829748dbcd913ac69f572516836731810c2fc1f8b86351bfee"),
        "decode error at position 87: empty map when expecting at least one key/value pair";
        "empty votes"
    )]
    #[test_case(
        fixture!("conway", "6c6596eda4e61f6f294b522c17f3c9fb6fbddcfac0e55af88ddc96747b3e0478"),
        "unexpected type array at position 87: expected map";
        "malformed votes"
    )]
    #[test_case(
        fixture!("conway", "402a8a9024d4160928e574c73aa66c66d92f9856c3fa2392242f7a92b8e9c347"),
        "decode error at position 81: empty map when expecting at least one key/value pair";
        "empty mint"
    )]
    #[test_case(
        fixture!("conway", "48d5440656ceefda1ac25506dcd175e77a486113733a89e48a5a2f401d2cbfda"),
        "decode error at position 87: empty set when expecting at least one element";
        "empty collateral inputs"
    )]
    #[test_case(
        fixture!("conway", "5cbed05f218d893dac6d9af847aa7429576019a1314b633e3fde55cb74e43be1"),
        "decode error at position 87: empty set when expecting at least one element";
        "empty required signers"
    )]
    #[test_case(
        fixture!("conway", "71d780bdcc0cf8d1a8dafc6641797d46f1be835be6dd63b2b4bb5651df808d79"),
        "decode error at position 81: empty map when expecting at least one key/value pair";
        "empty withdrawals"
    )]
    #[test_case(
        fixture!("conway", "477981b76e218802d5ce8c673abefe0b4031f09b0be5283a5b577ca109671771"),
        "decode error at position 87: empty set when expecting at least one element";
        "empty proposals"
    )]
    #[test_case(
        fixture!("conway", "675954a2fe5ad3638a360902a4c7307a598d6e13b977279df640a663023c14bd"),
        "decode error: decoding 0 as PositiveCoin";
        "null donation"
    )]
    #[test_case(
        fixture!("conway", "5280ac2b10897dd26c9d7377ae681a6ea1dc3eec197563ab5bf3ab7907e0e709"),
        "decode error: duplicate field entry with key 2";
        "duplicate fields keys"
    )]
    fn decode_malformed(
        result: Result<TransactionBody, cbor::decode::Error>,
        expected_error: &str,
    ) {
        assert_eq!(
            result.map_err(|e| e.to_string()),
            Err(expected_error.to_string())
        );
    }
}
