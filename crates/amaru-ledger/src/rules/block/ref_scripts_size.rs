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

// NOTE: Haskell Divergences
//
// This logic operates on some iterator of `TransactionInput`, which should include both the reference inputs and inputs.
// In Haskell, that is implemented with a `Set.Union`. Here, we just chain iterators.
// That difference means that we could have the same input in both the inputs and reference inputs, double counting a script in it.
// However, that would tx would be invalid due to the `NonDisjointInputs` rule regardless, so that doesn't impact conformance.

// TODO: Duplicate work and PV11 changes
//
// Currently, we are calcuating reference script sizes per transaction as well as at the block level.
// This is not particularly expensive, and bailing here in the worst case saves significant work.
//
// That being said, in pv11+, we must also resolve inputs that could've been created in the current block.
// We can deduplicate this logic when we implement that.

/// Sum the on-wire byte size of every script
/// reachable through the reference inputs of every transaction in a block, fail
/// if the total exceeds [`ProtocolParameters::max_ref_script_size_per_block`]
pub fn block_ref_scripts_size_valid<'a, C>(
    inputs: impl IntoIterator<Item = &'a TransactionInput>,
    context: &C,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), InvalidBlockDetails>
where
    C: UtxoSlice,
{
    let allowed = protocol_parameters.max_ref_script_size_per_block as u64;
    let mut total: u64 = 0;
    for input in inputs {
        if let Some(output) = context.lookup(input)
            && let Some(script) = output.script.as_ref()
        {
            total += script_size(script);
        }
    }
    if total > allowed {
        return Err(InvalidBlockDetails::RefScriptSizeTooBig { provided: total, allowed });
    }
    Ok(())
}
