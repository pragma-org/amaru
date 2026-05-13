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

use amaru_kernel::{ProtocolParameters, TransactionInput, cardano::memoized::script_size};

use super::InvalidBlockDetails;
use crate::context::UtxoSlice;

/// Sum the on-wire byte size of every script
/// reachable through the reference inputs of every transaction in a block, fail
/// if the total exceeds [`ProtocolParameters::max_ref_script_size_per_block`]
pub fn block_ref_scripts_size_valid<'a, C>(
    reference_inputs: impl IntoIterator<Item = &'a TransactionInput>,
    context: &C,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), InvalidBlockDetails>
where
    C: UtxoSlice,
{
    // NOTE: Duplicate work
    //
    // We are separately calculating reference script sizes for each transaction.
    // While this could be avoided, the added cost here is not significant and bailing here
    // in the worst case saves significant work. If we were to calculate per-tx sizes here
    // and thread them to phase one, it wouldn't save any iterations since we have to iterate the inputs anyway.
    // TL;DR: I am acknowledging the duplicate work, but not optimizing this away until it is needed.
    let allowed = protocol_parameters.max_ref_script_size_per_block as u64;
    let mut total: u64 = 0;
    for input in reference_inputs {
        if let Some(output) = context.lookup(input)
            && let Some(script) = output.script.as_ref()
        {
            total = total.saturating_add(script_size(script));
        }
    }
    if total > allowed {
        return Err(InvalidBlockDetails::RefScriptSizeTooBig { provided: total, allowed });
    }
    Ok(())
}
