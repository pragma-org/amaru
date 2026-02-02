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

use super::InvalidBlockDetails;
use amaru_kernel::Block;

/// This validation checks that the purported block body size matches the actual block body size.
/// The validation of the bounds happens in the networking layer
pub fn block_body_size_valid(block: &Block) -> Result<(), InvalidBlockDetails> {
    let announced_size = block.header.header_body.block_body_size;
    let actual_size = block.body_len();

    if announced_size != actual_size {
        return Err(InvalidBlockDetails::BlockSizeMismatch {
            supplied: announced_size,
            actual: actual_size,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::rules::block::InvalidBlockDetails;
    use amaru_kernel::{Block, include_cbor};
    use test_case::test_case;

    macro_rules! fixture {
        ($number:literal) => {
            include_cbor!(concat!("blocks/preprod/", $number, "/valid.cbor"))
        };
        ($number:literal, $variant:literal) => {
            include_cbor!(concat!("blocks/preprod/", $number, "/", $variant, ".cbor"))
        };
    }

    #[test_case(fixture!("2667660"); "valid")]
    #[test_case(
        fixture!("2667660", "invalid_block_body_size")
        => matches Err(InvalidBlockDetails::BlockSizeMismatch {supplied, actual})
            if supplied == 0 && actual == 3411
        ; "block body size mismatch"
    )]
    fn test_block_size(block: Block) -> Result<(), InvalidBlockDetails> {
        super::block_body_size_valid(&block)
    }
}
