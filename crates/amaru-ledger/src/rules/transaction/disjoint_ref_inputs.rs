use amaru_kernel::{TransactionBody, TransactionInput};

use crate::rules::TransactionRuleViolation;

pub struct NonDisjointRefInputs {
    pub intersection: Vec<TransactionInput>,
}

impl Into<TransactionRuleViolation> for NonDisjointRefInputs {
    fn into(self) -> TransactionRuleViolation {
        TransactionRuleViolation::NonDisjointRefInputs(self)
    }
}

// impl From<NonDisjointRefInputs> for TransactionRuleViolation {
//     fn from(value: NonDisjointRefInputs) -> Self {
//         TransactionRuleViolation::NonDisjointRefInputs(value)
//     }
// }

pub fn disjoint_ref_inputs(transaction: &TransactionBody) -> Result<(), NonDisjointRefInputs> {
    let intersection = match &transaction.reference_inputs {
        Some(ref_inputs) => ref_inputs
            .iter()
            .filter(|ref_input| transaction.inputs.contains(ref_input))
            .cloned()
            .collect(),
        None => Vec::new(),
    };

    if !intersection.is_empty() {
        Err(NonDisjointRefInputs { intersection })
    } else {
        Ok(())
    }
}
