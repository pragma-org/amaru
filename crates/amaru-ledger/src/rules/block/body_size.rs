// Copyright 2025 PRAGMA
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

use crate::rules::RuleViolation;
use amaru_kernel::{to_cbor, HeaderBody, MintedBlock};

/// This validation checks that the purported block body size matches the actual block body size.
/// The validation of the bounds happens in the networking layer
pub fn block_body_size_valid(
    block_header: &HeaderBody,
    block: &MintedBlock<'_>,
) -> Result<(), RuleViolation> {
    let bh_size = block_header.block_body_size as usize;
    let actual_block_size = calculate_block_body_size(block);

    if bh_size != actual_block_size {
        Err(RuleViolation::BlockBodySizeMismatch {
            supplied: bh_size,
            actual: actual_block_size,
        })
    } else {
        Ok(())
    }
}

fn calculate_block_body_size(block: &MintedBlock<'_>) -> usize {
    let tx_bodies_raw = to_cbor(&block.transaction_bodies);
    let tx_witness_sets_raw = to_cbor(&block.transaction_witness_sets);
    let auxiliary_data_raw = to_cbor(&block.auxiliary_data_set);
    let invalid_transactions_raw = to_cbor(&block.invalid_transactions);

    tx_bodies_raw.len()
        + tx_witness_sets_raw.len()
        + auxiliary_data_raw.len()
        + invalid_transactions_raw.len()
}
