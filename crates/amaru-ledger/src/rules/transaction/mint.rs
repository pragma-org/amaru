use amaru_kernel::{Multiasset, NonZeroInt, StakeCredential};

use crate::context::{UtxoSlice, WitnessSlice};

pub fn execute<C>(context: &mut C, mint: Option<&Multiasset<NonZeroInt>>)
where
    C: UtxoSlice + WitnessSlice,
{
    if let Some(mint) = mint {
        mint.iter()
            .for_each(|(policy, _)| context.require_witness(StakeCredential::ScriptHash(*policy)));
    }
}
