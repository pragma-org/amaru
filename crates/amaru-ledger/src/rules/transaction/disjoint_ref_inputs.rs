use amaru_kernel::MintedTransactionBody;

use crate::rules::TransactionRuleViolation;

pub fn disjoint_ref_inputs(
    transaction: &MintedTransactionBody<'_>,
) -> Result<(), TransactionRuleViolation> {
    let intersection = match &transaction.reference_inputs {
        Some(ref_inputs) => ref_inputs
            .iter()
            .filter(|ref_input| transaction.inputs.contains(ref_input))
            .cloned()
            .collect(),
        None => Vec::new(),
    };

    if !intersection.is_empty() {
        Err(TransactionRuleViolation::NonDisjointRefInputs { intersection })
    } else {
        Ok(())
    }
}
