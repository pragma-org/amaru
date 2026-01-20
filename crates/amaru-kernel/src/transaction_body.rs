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
    BTreeMap, Bytes, Certificate, Coin, Debug, Hash, KeepRaw, MintedTransactionOutput, Multiasset,
    NetworkId, NonEmptyKeyValuePairs, NonEmptySet, NonZeroInt, PositiveCoin,
    Proposal as ProposalProcedure, RequiredSigners, RewardAccount, Set, TransactionInput,
    TransactionOutput, VotingProcedures,
    cbor::{Encode, data::Type},
};

#[derive(Encode, Debug, PartialEq, Clone)]
#[cbor(map)]
pub struct PseudoTransactionBody<T1> {
    #[n(0)]
    pub inputs: Set<TransactionInput>,

    #[n(1)]
    pub outputs: Vec<T1>,

    #[n(2)]
    pub fee: Coin,

    #[n(3)]
    pub ttl: Option<u64>,

    #[n(4)]
    pub certificates: Option<NonEmptySet<Certificate>>,

    #[n(5)]
    pub withdrawals: Option<NonEmptyKeyValuePairs<RewardAccount, Coin>>,

    #[n(7)]
    pub auxiliary_data_hash: Option<Bytes>,

    #[n(8)]
    pub validity_interval_start: Option<u64>,

    #[n(9)]
    pub mint: Option<Multiasset<NonZeroInt>>,

    #[n(11)]
    pub script_data_hash: Option<Hash<32>>,

    #[n(13)]
    pub collateral: Option<NonEmptySet<TransactionInput>>,

    #[n(14)]
    pub required_signers: Option<RequiredSigners>,

    #[n(15)]
    pub network_id: Option<NetworkId>,

    #[n(16)]
    pub collateral_return: Option<T1>,

    #[n(17)]
    pub total_collateral: Option<Coin>,

    #[n(18)]
    pub reference_inputs: Option<NonEmptySet<TransactionInput>>,

    // -- NEW IN CONWAY
    #[n(19)]
    pub voting_procedures: Option<VotingProcedures>,

    #[n(20)]
    pub proposal_procedures: Option<NonEmptySet<ProposalProcedure>>,

    #[n(21)]
    pub treasury_value: Option<Coin>,

    #[n(22)]
    pub donation: Option<PositiveCoin>,
}

pub type TransactionBody = PseudoTransactionBody<TransactionOutput>;

#[derive(Clone, Debug)]
enum TxBodyField<T1> {
    Inputs(Set<TransactionInput>),
    Outputs(Vec<T1>),
    Fee(Coin),
    Ttl(Option<u64>),
    Certificates(Option<NonEmptySet<Certificate>>),
    Withdrawals(Option<NonEmptyKeyValuePairs<RewardAccount, Coin>>),
    AuxiliaryDataHash(Option<Bytes>),
    ValidityIntervalStart(Option<u64>),
    Mint(Option<Multiasset<NonZeroInt>>),
    ScriptDataHash(Option<Hash<32>>),
    Collateral(Option<NonEmptySet<TransactionInput>>),
    RequiredSigners(Option<RequiredSigners>),
    NetworkId(Option<NetworkId>),
    CollateralReturn(Option<T1>),
    TotalCollateral(Option<Coin>),
    ReferenceInputs(Option<NonEmptySet<TransactionInput>>),
    VotingProcedures(Option<VotingProcedures>),
    ProposalProcedures(Option<NonEmptySet<ProposalProcedure>>),
    TreasuryValue(Option<Coin>),
    Donation(Option<PositiveCoin>),
}

fn decode_tx_body_field<'b, T1, C>(
    d: &mut minicbor::Decoder<'b>,
    k: u64,
    ctx: &mut C,
) -> Result<TxBodyField<T1>, minicbor::decode::Error>
where
    T1: minicbor::Decode<'b, C>,
{
    match k {
        0 => {
            let inputs = d.decode_with(ctx)?;
            Ok(TxBodyField::Inputs(inputs))
        }
        1 => {
            let outputs = d.decode_with(ctx)?;
            Ok(TxBodyField::Outputs(outputs))
        }
        2 => {
            let coin = d.decode_with(ctx)?;
            Ok(TxBodyField::Fee(coin))
        }
        3 => {
            let ttl = d.decode_with(ctx)?;
            Ok(TxBodyField::Ttl(ttl))
        }
        4 => {
            let certificates = d.decode_with(ctx)?;
            Ok(TxBodyField::Certificates(certificates))
        }
        5 => {
            let withdrawals = d.decode_with(ctx)?;
            Ok(TxBodyField::Withdrawals(withdrawals))
        }
        7 => {
            let auxiliary_data_hash = d.decode_with(ctx)?;
            Ok(TxBodyField::AuxiliaryDataHash(auxiliary_data_hash))
        }
        8 => {
            let validity_interval_start = d.decode_with(ctx)?;
            Ok(TxBodyField::ValidityIntervalStart(validity_interval_start))
        }
        9 => {
            let mint = d.decode_with(ctx)?;
            Ok(TxBodyField::Mint(mint))
        }
        11 => {
            let script_data_hash = d.decode_with(ctx)?;
            Ok(TxBodyField::ScriptDataHash(script_data_hash))
        }
        13 => {
            let collateral = d.decode_with(ctx)?;
            Ok(TxBodyField::Collateral(collateral))
        }
        14 => {
            let required_signers = d.decode_with(ctx)?;
            Ok(TxBodyField::RequiredSigners(required_signers))
        }
        15 => {
            let network_id = d.decode_with(ctx)?;
            Ok(TxBodyField::NetworkId(network_id))
        }
        16 => {
            let collateral_return = d.decode_with(ctx)?;
            Ok(TxBodyField::CollateralReturn(collateral_return))
        }
        17 => {
            let total_collateral = d.decode_with(ctx)?;
            Ok(TxBodyField::TotalCollateral(total_collateral))
        }
        18 => {
            let reference_inputs = d.decode_with(ctx)?;
            Ok(TxBodyField::ReferenceInputs(reference_inputs))
        }
        19 => {
            let voting_procedures = d.decode_with(ctx)?;
            Ok(TxBodyField::VotingProcedures(voting_procedures))
        }
        20 => {
            let proposal_procedures = d.decode_with(ctx)?;
            Ok(TxBodyField::ProposalProcedures(proposal_procedures))
        }
        21 => {
            let treasury_value = d.decode_with(ctx)?;
            Ok(TxBodyField::TreasuryValue(treasury_value))
        }
        22 => {
            let donation = d.decode_with(ctx)?;
            Ok(TxBodyField::Donation(donation))
        }
        k => Err(minicbor::decode::Error::message(format!(
            "Unknown txbody field key {}",
            k
        ))),
    }
}

struct TxBodyFields<T1> {
    entries: BTreeMap<u64, Vec<TxBodyField<T1>>>,
}

impl<'b, T1, C> minicbor::Decode<'b, C> for TxBodyFields<T1>
where
    T1: Clone + minicbor::Decode<'b, C>,
{
    fn decode(d: &mut minicbor::Decoder<'b>, ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        let mut entries = BTreeMap::new();
        let map_size = d.map()?;
        match map_size {
            None => loop {
                let ty = d.datatype()?;
                if ty == Type::Break {
                    d.skip()?;
                    break;
                }
                let k = d.u64()?;
                let v = decode_tx_body_field(d, k, ctx)?;
                entries
                    .entry(k)
                    .and_modify(|ar: &mut Vec<TxBodyField<T1>>| ar.push(v.clone()))
                    .or_insert(vec![v]);
            },
            Some(n) => {
                for _ in 0..n {
                    let k = d.u64()?;
                    let v = decode_tx_body_field(d, k, ctx)?;
                    entries
                        .entry(k)
                        .and_modify(|ar: &mut Vec<TxBodyField<T1>>| ar.push(v.clone()))
                        .or_insert(vec![v]);
                }
            }
        }
        Ok(TxBodyFields { entries })
    }
}

fn make_basic_tx_body<T1>(
    inputs: Set<TransactionInput>,
    outputs: Vec<T1>,
    fee: Coin,
) -> PseudoTransactionBody<T1> {
    PseudoTransactionBody {
        inputs,
        outputs,
        fee,
        ttl: None,
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
        voting_procedures: None,
        proposal_procedures: None,
        treasury_value: None,
        donation: None,
    }
}

fn set_tx_body_field<T1>(
    txbody: &mut PseudoTransactionBody<T1>,
    index: u64,
    field: TxBodyField<T1>,
) -> Result<(), String>
where
    T1: Debug,
{
    match (index, field) {
        (0, TxBodyField::Inputs(i)) => {
            txbody.inputs = i;
        }
        (1, TxBodyField::Outputs(o)) => {
            txbody.outputs = o;
        }
        (2, TxBodyField::Fee(f)) => {
            txbody.fee = f;
        }
        (3, TxBodyField::Ttl(t)) => {
            txbody.ttl = t;
        }
        (4, TxBodyField::Certificates(c)) => {
            txbody.certificates = c;
        }
        (5, TxBodyField::Withdrawals(w)) => {
            txbody.withdrawals = w;
        }
        (7, TxBodyField::AuxiliaryDataHash(a)) => {
            txbody.auxiliary_data_hash = a;
        }
        (8, TxBodyField::ValidityIntervalStart(v)) => {
            txbody.validity_interval_start = v;
        }
        (9, TxBodyField::Mint(m)) => {
            txbody.mint = m;
        }
        (11, TxBodyField::ScriptDataHash(s)) => {
            txbody.script_data_hash = s;
        }
        (13, TxBodyField::Collateral(c)) => {
            txbody.collateral = c;
        }
        (14, TxBodyField::RequiredSigners(r)) => {
            txbody.required_signers = r;
        }
        (15, TxBodyField::NetworkId(n)) => {
            txbody.network_id = n;
        }
        (16, TxBodyField::CollateralReturn(c)) => {
            txbody.collateral_return = c;
        }
        (17, TxBodyField::TotalCollateral(t)) => {
            txbody.total_collateral = t;
        }
        (18, TxBodyField::ReferenceInputs(r)) => {
            txbody.reference_inputs = r;
        }
        (19, TxBodyField::VotingProcedures(v)) => {
            txbody.voting_procedures = v;
        }
        (20, TxBodyField::ProposalProcedures(p)) => {
            txbody.proposal_procedures = p;
        }
        (21, TxBodyField::TreasuryValue(t)) => {
            txbody.treasury_value = t;
        }
        (22, TxBodyField::Donation(d)) => {
            txbody.donation = d;
        }
        (ix, f) => return Err(format!("Wrong index {} for txbody field {:?}", ix, f)),
    }
    Ok(())
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
impl<'b, T1, C> minicbor::Decode<'b, C> for PseudoTransactionBody<T1>
where
    T1: Clone + Debug + minicbor::Decode<'b, C>,
{
    fn decode(d: &mut minicbor::Decoder<'b>, ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        let fields: TxBodyFields<T1> = d.decode_with(ctx)?;
        let entries = fields.entries;
        let inputs = entries.get(&0).and_then(|v| v.first());
        let outputs = entries.get(&1).and_then(|v| v.first());
        let fee = entries.get(&2).and_then(|v| v.first());
        let mut tx_body = match (inputs, outputs, fee) {
            (
                Some(TxBodyField::Inputs(inputs)),
                Some(TxBodyField::Outputs(outputs)),
                Some(TxBodyField::Fee(fee)),
            ) => make_basic_tx_body(inputs.clone(), outputs.clone(), *fee),
            _ => {
                return Err(minicbor::decode::Error::message(
                    "inputs, outputs, and fee fields are required",
                ));
            }
        };
        for (key, val) in entries {
            if val.len() > 1 {
                return Err(minicbor::decode::Error::message(format!(
                    "duplicate txbody entries for key {}",
                    key
                )));
            }
            match val.first() {
                Some(first) => {
                    let result = set_tx_body_field(&mut tx_body, key, first.clone());
                    if let Err(e) = result {
                        return Err(minicbor::decode::Error::message(format!(
                            "could not set txbody field: {}",
                            e
                        )));
                    }
                }
                None => {
                    // This is impossible because we always initialize TxBodyFields entries with
                    // singleton arrays. Could maybe use a NonEmpty Vec type to eliminate this
                    // branch
                    return Err(minicbor::decode::Error::message(
                        "TxBodyFields entry was empty",
                    ));
                }
            }
        }
        if tx_body.mint.as_ref().is_some_and(|x| x.is_empty()) {
            return Err(minicbor::decode::Error::message("mint must not be empty"));
        }
        Ok(tx_body)
    }
}

pub type MintedTransactionBody<'a> = PseudoTransactionBody<MintedTransactionOutput<'a>>;

impl<'a> From<MintedTransactionBody<'a>> for TransactionBody {
    fn from(value: MintedTransactionBody<'a>) -> Self {
        Self {
            inputs: value.inputs,
            outputs: value.outputs.into_iter().map(|x| x.into()).collect(),
            fee: value.fee,
            ttl: value.ttl,
            certificates: value.certificates,
            withdrawals: value.withdrawals,
            auxiliary_data_hash: value.auxiliary_data_hash,
            validity_interval_start: value.validity_interval_start,
            mint: value.mint,
            script_data_hash: value.script_data_hash,
            collateral: value.collateral,
            required_signers: value.required_signers,
            network_id: value.network_id,
            collateral_return: value.collateral_return.map(|x| x.into()),
            total_collateral: value.total_collateral,
            reference_inputs: value.reference_inputs,
            voting_procedures: value.voting_procedures,
            proposal_procedures: value.proposal_procedures,
            treasury_value: value.treasury_value,
            donation: value.donation,
        }
    }
}

pub fn get_original_hash<'a, T>(thing: &KeepRaw<'a, T>) -> pallas_crypto::hash::Hash<32> {
    pallas_crypto::hash::Hasher::<256>::hash(thing.raw_cbor())
}

#[cfg(test)]
mod tests {
    use super::TransactionBody;
    use crate::cbor;
    use test_case::test_case;

    macro_rules! fixture {
        ($id:expr) => {{
            $crate::try_include_cbor!(concat!(
                "decode_transaction_body/conway/",
                $id,
                "/sample.cbor",
            ))
        }};
    }

    #[test_case(
        fixture!("70beb79b18459ff5b826ebeea82ecf566ab79e166ff5749f761ed402ad459466");
        "simple input -> output payout"
    )]
    #[test_case(
        fixture!("c20c7e395ef81d8a6172510408446afc240d533bff18f9dca905e78187c2bcd8");
        "null fees"
    )]

    fn decode_wellformed(result: Result<TransactionBody, cbor::decode::Error>) {
        assert!(dbg!(result).is_ok());
    }

    #[test_case(
        fixture!("9d34025191e23c5996e20c2c0d1718f5cb1d9c4a37a5cb153cbd03c66b59128f"),
        "decode error: inputs, outputs, and fee fields are required";
        "missing fees"
    )]
    #[test_case(
        fixture!("b563891d222561e435b475632a3bdcca58cc3c8ec80ab6b51e0a5c96b6a35e1b"),
        "decode error: inputs, outputs, and fee fields are required";
        "missing outputs"
    )]
    #[test_case(
        fixture!("c5f2d5b7e9b8f615c52296e04b3050cf35ad4e8a457a25adaeb2a933de1bf624"),
        "decode error at position 81: empty set when expecting at least one element";
        "empty certificates"
    )]
    #[test_case(
        fixture!("3b5478c6446496b6ff71c738c83fbf251841dd45cda074b0ac935b1428a52f66"),
        "unexpected type map at position 81: expected array";
        "malformed certificates"
    )]
    #[test_case(
        fixture!("5123113da4c8e2829748dbcd913ac69f572516836731810c2fc1f8b86351bfee"),
        "decode error at position 87: empty map when expecting at least one key/value pair";
        "empty votes"
    )]
    #[test_case(
        fixture!("6c6596eda4e61f6f294b522c17f3c9fb6fbddcfac0e55af88ddc96747b3e0478"),
        "unexpected type array at position 87: expected map";
        "malformed votes"
    )]
    #[test_case(
        fixture!("402a8a9024d4160928e574c73aa66c66d92f9856c3fa2392242f7a92b8e9c347"),
        "decode error: mint must not be empty";
        "empty mint"
    )]
    #[test_case(
        fixture!("48d5440656ceefda1ac25506dcd175e77a486113733a89e48a5a2f401d2cbfda"),
        "decode error at position 87: empty set when expecting at least one element";
        "empty collateral inputs"
    )]
    #[test_case(
        fixture!("5cbed05f218d893dac6d9af847aa7429576019a1314b633e3fde55cb74e43be1"),
        "decode error at position 87: empty set when expecting at least one element";
        "empty required signers"
    )]
    #[test_case(
        fixture!("71d780bdcc0cf8d1a8dafc6641797d46f1be835be6dd63b2b4bb5651df808d79"),
        "decode error at position 81: empty map when expecting at least one key/value pair";
        "empty withdrawals"
    )]
    #[test_case(
        fixture!("477981b76e218802d5ce8c673abefe0b4031f09b0be5283a5b577ca109671771"),
        "decode error at position 87: empty set when expecting at least one element";
        "empty proposals"
    )]
    #[test_case(
        fixture!("675954a2fe5ad3638a360902a4c7307a598d6e13b977279df640a663023c14bd"),
        "decode error: decoding 0 as PositiveCoin";
        "null donation"
    )]
    #[test_case(
        fixture!("d36a2619a672494604e11bb447cbcf5231e9f2ba25c2169177edc941bd50ad6c"),
        "decode error: inputs, outputs, and fee fields are required";
        "empty body"
    )]
    #[test_case(
        fixture!("5280ac2b10897dd26c9d7377ae681a6ea1dc3eec197563ab5bf3ab7907e0e709"),
        "decode error: duplicate txbody entries for key 2";
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
