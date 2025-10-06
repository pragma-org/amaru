use pallas_codec::minicbor::{Decode, Decoder, decode::Error};
use pallas_codec::utils::AnyCbor;
use pallas_codec::utils::{KeyValuePairs, NonEmptySet, Set};
use pallas_primitives::conway::{
    Multiasset, PseudoTransactionOutput, TransactionBody, TransactionOutput, Value,
};
use std::collections::BTreeSet;
use std::ops::Deref;

fn set_has_duplicates<T>(s: &Set<T>) -> bool
where
    T: std::cmp::Ord + std::cmp::Eq,
{
    let mut seen: BTreeSet<&T> = BTreeSet::new();
    for e in s {
        if !seen.insert(e) {
            return true;
        }
    }
    false
}

fn nonempty_set_has_duplicates<T>(s: &NonEmptySet<T>) -> bool
where
    T: std::cmp::Ord + std::cmp::Eq,
{
    let mut seen: BTreeSet<&T> = BTreeSet::new();
    for e in s {
        if !seen.insert(e) {
            return true;
        }
    }
    false
}

fn is_multiasset_small_enough<T: Clone>(ma: &Multiasset<T>) -> bool {
    let per_asset_size = 44;
    let per_policy_size = 28;

    let policy_count = ma.deref().len();
    let mut asset_count = 0;
    for (_policy, assets) in ma.deref().iter() {
        asset_count += assets.len();
    }

    let size = per_asset_size * asset_count + per_policy_size * policy_count;
    size <= 65535
}

fn validate_multiasset<T>(ma: &Multiasset<T>) -> Result<(), String>
where
    u64: From<T>,
    T: Clone + Copy,
{
    for (_policy, asset) in ma.iter() {
        if asset.is_empty() {
            return Err("Value must not contain empty assets".to_string());
        }
        for (_token, amount) in asset.iter() {
            if u64::from(*amount) == 0 {
                return Err("Value must not contain zero values".to_string());
            }
        }
    }
    if !is_multiasset_small_enough(ma) {
        return Err("Multiasset must be small enough for compact representation".to_string());
    }
    Ok(())
}

fn validate_value(v: &Value) -> Result<(), String> {
    match v {
        Value::Coin(_c) => Ok(()),
        Value::Multiasset(_c, ma) => {
            let () = validate_multiasset(ma)?;
            Ok(())
        }
    }
}

fn validate_tx_output(o: &TransactionOutput) -> Result<(), String> {
    match o {
        PseudoTransactionOutput::Legacy(_legacy_output) => Ok(()),
        PseudoTransactionOutput::PostAlonzo(output) => {
            let () = validate_value(&output.value)?;
            Ok(())
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct Strict<T> {
    inner: T,
}

impl<T> Strict<T> {
    pub fn unwrap(self) -> T {
        self.inner
    }
}

impl<'b, C> Decode<'b, C> for Strict<u64> {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<Self, Error> {
        let n = d.u64()?;
        Ok(Strict { inner: n })
    }
}

impl<'b, C> Decode<'b, C> for Strict<TransactionBody> {
    fn decode(d: &mut Decoder<'b>, ctx: &mut C) -> Result<Self, Error> {
        // Need to check that the cbor object does not contain duplicates. It's not possible to
        // determine this information from the lenient pallas type because any duplicates are
        // simply ignored by the default minicbor map decoder
        {
            let mut d2 = d.clone();
            let raw_: KeyValuePairs<u64, AnyCbor> = d2.decode_with(ctx)?;
            let raw: Vec<(u64, AnyCbor)> = raw_.to_vec();

            let mut seen: BTreeSet<u64> = BTreeSet::new();
            for (ix, _value) in raw {
                if !seen.insert(ix) {
                    return Err(Error::message(format!(
                        "duplicate key found in TransactionBody: {}",
                        ix
                    )));
                }
            }

            if !seen.contains(&0) {
                return Err(Error::message("field inputs is required".to_string()));
            }
            if !seen.contains(&1) {
                return Err(Error::message("field outputs is required".to_string()));
            }
            if !seen.contains(&2) {
                return Err(Error::message("field fee is required".to_string()));
            }
        }

        let tx_body: TransactionBody = d.decode_with(ctx)?;

        // TODO: Are we missing any invariants of any of the fields?
        //
        // Need to validate conway multiasset invariants (cannot contain zeroes, cannot contain
        // empty assets) (check if multiasset is "small enough") on the mint and on the outputs

        if set_has_duplicates(&tx_body.inputs) {
            return Err(Error::message(
                "TransactionBody inputs has duplicates".to_string(),
            ));
        }

        for o in &tx_body.outputs {
            if let Err(e) = validate_tx_output(o) {
                return Err(Error::message(e));
            }
        }

        if tx_body.mint.as_ref().is_some_and(|x| x.is_empty()) {
            return Err(Error::message(
                "mint must be non-empty if present".to_string(),
            ));
        }

        if tx_body.certificates.as_ref().is_some_and(|x| x.is_empty()) {
            return Err(Error::message(
                "TransactionBody certificates are empty".to_string(),
            ));
        }

        if tx_body.withdrawals.as_ref().is_some_and(|x| x.is_empty()) {
            return Err(Error::message(
                "withdrawals must be non-empty if present".to_string(),
            ));
        }

        if tx_body
            .voting_procedures
            .as_ref()
            .is_some_and(|x| x.is_empty())
        {
            return Err(Error::message(
                "voting procedures must be non-empty if present".to_string(),
            ));
        }

        if tx_body
            .proposal_procedures
            .as_ref()
            .is_some_and(|x| x.is_empty())
        {
            return Err(Error::message(
                "proposal procedures must be non-empty if present".to_string(),
            ));
        }

        if tx_body.collateral.as_ref().is_some_and(|x| x.is_empty()) {
            return Err(Error::message(
                "collaterals must be non-empty if present".to_string(),
            ));
        }

        if tx_body
            .collateral
            .as_ref()
            .is_some_and(nonempty_set_has_duplicates)
        {
            return Err(Error::message(
                "TransactionBody collaterals has duplicates".to_string(),
            ));
        }

        if tx_body
            .required_signers
            .as_ref()
            .is_some_and(|r| r.is_empty())
        {
            return Err(Error::message(
                "required signers must be non-empty if present".to_string(),
            ));
        }

        if tx_body
            .required_signers
            .as_ref()
            .is_some_and(nonempty_set_has_duplicates)
        {
            return Err(Error::message(
                "TransactionBody required signers has duplicates".to_string(),
            ));
        }

        if tx_body
            .reference_inputs
            .as_ref()
            .is_some_and(|r| r.is_empty())
        {
            return Err(Error::message(
                "reference inputs must be non-empty if present".to_string(),
            ));
        }

        if tx_body
            .voting_procedures
            .as_ref()
            .is_some_and(|v| v.is_empty())
        {
            return Err(Error::message(
                "voting procedures must be non-empty if present".to_string(),
            ));
        }

        if tx_body
            .proposal_procedures
            .as_ref()
            .is_some_and(|p| p.is_empty())
        {
            return Err(Error::message(
                "proposal procedures must be non-empty if present".to_string(),
            ));
        }

        if tx_body.treasury_value.as_ref().is_some_and(|t| *t == 0) {
            return Err(Error::message(
                "TransactionBody treasury donation is zero".to_string(),
            ));
        }

        Ok(Strict {
            inner: TransactionBody {
                auxiliary_data_hash: tx_body.auxiliary_data_hash,
                certificates: tx_body.certificates,
                collateral: tx_body.collateral,
                collateral_return: tx_body.collateral_return,
                donation: tx_body.donation,
                fee: tx_body.fee,
                inputs: tx_body.inputs,
                mint: tx_body.mint,
                network_id: tx_body.network_id,
                outputs: tx_body.outputs,
                proposal_procedures: tx_body.proposal_procedures,
                reference_inputs: tx_body.reference_inputs,
                required_signers: tx_body.required_signers,
                script_data_hash: tx_body.script_data_hash,
                total_collateral: tx_body.total_collateral,
                treasury_value: tx_body.treasury_value,
                ttl: tx_body.ttl,
                validity_interval_start: tx_body.validity_interval_start,
                voting_procedures: tx_body.voting_procedures,
                withdrawals: tx_body.withdrawals,
            },
        })
    }
}

#[cfg(test)]
mod tests_transaction {
    use super::super::{Strict, TransactionBody};
    use pallas_codec::minicbor;

    // A simple tx with just inputs, outputs, and fee. Address is not well-formed, since the
    // 00 header implies both a payment part and a staking part are present.
    #[test]
    fn decode_simple_tx() {
        let tx_bytes = hex::decode("a300828258206767676767676767676767676767676767676767676767676767676767676767008258206767676767676767676767676767676767676767676767676767676767676767010200018182581c000000000000000000000000000000000000000000000000000000001a04000000").unwrap();
        let tx: Strict<TransactionBody> = minicbor::decode(&tx_bytes).unwrap();
        assert_eq!(tx.inner.fee, 0);
    }

    // The decoder for ConwayTxBodyRaw rejects transaction bodies missing inputs, outputs, or
    // fee
    #[test]
    fn reject_empty_tx() {
        let tx_bytes = hex::decode("a0").unwrap();
        let tx: Result<Strict<TransactionBody>, _> = minicbor::decode(&tx_bytes);
        assert_eq!(
            tx.map_err(|e| e.to_string()),
            Err("decode error: field inputs is required".to_owned())
        );
    }

    // Single input, no outputs, fee present but zero
    #[test]
    fn reject_tx_missing_outputs() {
        let tx_bytes = hex::decode(
            "a200818258200000000000000000000000000000000000000000000000000000000000000008090200",
        )
        .unwrap();
        let tx: Result<Strict<TransactionBody>, _> = minicbor::decode(&tx_bytes);
        assert_eq!(
            tx.map_err(|e| e.to_string()),
            Err("decode error: field outputs is required".to_owned())
        );
    }

    // Single input, single output, no fee
    #[test]
    fn reject_tx_missing_fee() {
        let tx_bytes = hex::decode("a20081825820000000000000000000000000000000000000000000000000000000000000000809018182581c000000000000000000000000000000000000000000000000000000001affffffff").unwrap();
        let tx: Result<Strict<TransactionBody>, _> = minicbor::decode(&tx_bytes);
        assert_eq!(
            tx.map_err(|e| e.to_string()),
            Err("decode error: field fee is required".to_owned())
        );
    }

    // The mint may not be present if it is empty
    // TODO: equivalent tests for certs, withdrawals, collateral inputs, required signer
    // hashes, reference inputs, voting procedures, and proposal procedures
    #[test]
    fn reject_empty_present_mint() {
        let tx_bytes = hex::decode("a400828258206767676767676767676767676767676767676767676767676767676767676767008258206767676767676767676767676767676767676767676767676767676767676767010200018182581c000000000000000000000000000000000000000000000000000000001a0400000009a0").unwrap();
        let tx: Result<Strict<TransactionBody>, _> = minicbor::decode(&tx_bytes);
        assert_eq!(
            tx.map_err(|e| e.to_string()),
            Err("decode error: mint must be non-empty if present".to_owned())
        );
    }

    #[test]
    fn reject_empty_present_certs() {
        let tx_bytes = hex::decode("a400828258206767676767676767676767676767676767676767676767676767676767676767008258206767676767676767676767676767676767676767676767676767676767676767010200018182581c000000000000000000000000000000000000000000000000000000001a040000000480").unwrap();
        let tx: Result<Strict<TransactionBody>, _> = minicbor::decode(&tx_bytes);
        assert_eq!(
            tx.map_err(|e| e.to_string()),
            Err("decode error: TransactionBody certificates are empty".to_owned())
        );
    }

    #[test]
    fn reject_empty_present_withdrawals() {
        let tx_bytes = hex::decode("a400828258206767676767676767676767676767676767676767676767676767676767676767008258206767676767676767676767676767676767676767676767676767676767676767010200018182581c000000000000000000000000000000000000000000000000000000001a0400000005a0").unwrap();
        let tx: Result<Strict<TransactionBody>, _> = minicbor::decode(&tx_bytes);
        assert_eq!(
            tx.map_err(|e| e.to_string()),
            Err("decode error: withdrawals must be non-empty if present".to_owned())
        );
    }

    #[test]
    fn reject_empty_present_collateral_inputs() {
        let tx_bytes = hex::decode("a400828258206767676767676767676767676767676767676767676767676767676767676767008258206767676767676767676767676767676767676767676767676767676767676767010200018182581c000000000000000000000000000000000000000000000000000000001a040000000d80").unwrap();
        let tx: Result<Strict<TransactionBody>, _> = minicbor::decode(&tx_bytes);
        assert_eq!(
            tx.map_err(|e| e.to_string()),
            Err("decode error: collaterals must be non-empty if present".to_owned())
        );
    }

    #[test]
    fn reject_empty_present_required_signers() {
        let tx_bytes = hex::decode("a400828258206767676767676767676767676767676767676767676767676767676767676767008258206767676767676767676767676767676767676767676767676767676767676767010200018182581c000000000000000000000000000000000000000000000000000000001a040000000e80").unwrap();
        let tx: Result<Strict<TransactionBody>, _> = minicbor::decode(&tx_bytes);
        assert_eq!(
            tx.map_err(|e| e.to_string()),
            Err("decode error: required signers must be non-empty if present".to_owned())
        );
    }

    #[test]
    fn reject_empty_present_voting_procedures() {
        let tx_bytes = hex::decode("a400828258206767676767676767676767676767676767676767676767676767676767676767008258206767676767676767676767676767676767676767676767676767676767676767010200018182581c000000000000000000000000000000000000000000000000000000001a0400000013a0").unwrap();
        let tx: Result<Strict<TransactionBody>, _> = minicbor::decode(&tx_bytes);
        assert_eq!(
            tx.map_err(|e| e.to_string()),
            Err("decode error: voting procedures must be non-empty if present".to_owned())
        );
    }

    #[test]
    fn reject_empty_present_proposal_procedures() {
        let tx_bytes = hex::decode("a400828258206767676767676767676767676767676767676767676767676767676767676767008258206767676767676767676767676767676767676767676767676767676767676767010200018182581c000000000000000000000000000000000000000000000000000000001a040000001480").unwrap();
        let tx: Result<Strict<TransactionBody>, _> = minicbor::decode(&tx_bytes);
        assert_eq!(
            tx.map_err(|e| e.to_string()),
            Err("decode error: proposal procedures must be non-empty if present".to_owned())
        );
    }

    #[test]
    fn reject_empty_present_donation() {
        let tx_bytes = hex::decode("a400828258206767676767676767676767676767676767676767676767676767676767676767008258206767676767676767676767676767676767676767676767676767676767676767010200018182581c000000000000000000000000000000000000000000000000000000001a040000001600").unwrap();
        let tx: Result<Strict<TransactionBody>, _> = minicbor::decode(&tx_bytes);
        assert_eq!(
            tx.map_err(|e| e.to_string()),
            Err("decode error: decoding 0 as PositiveCoin".to_owned())
        );
    }

    #[test]
    fn reject_duplicate_keys() {
        let tx_bytes = hex::decode("a40081825820000000000000000000000000000000000000000000000000000000000000000809018182581c000000000000000000000000000000000000000000000000000000001affffffff02010201").unwrap();
        let tx: Result<Strict<TransactionBody>, _> = minicbor::decode(&tx_bytes);
        assert_eq!(
            tx.map_err(|e| e.to_string()),
            Err("decode error: duplicate key found in TransactionBody: 2".to_owned())
        );
    }
}
