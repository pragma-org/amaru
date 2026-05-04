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

use amaru_kernel::Block;

use super::InvalidBlockDetails;

pub fn block_body_hash_valid(block: &Block) -> Result<(), InvalidBlockDetails> {
    let header = block.header.header_body.block_body_hash;
    let actual = block.body_hash();
    if header != actual {
        return Err(InvalidBlockDetails::InvalidBodyHash { header, actual });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{Block, include_cbor};
    use test_case::test_case;

    use crate::rules::block::InvalidBlockDetails;

    macro_rules! fixture {
        ($number:literal) => {
            include_cbor!(concat!("blocks/preprod/", $number, "/valid.cbor"))
        };
    }

    #[test_case(fixture!("2667657"); "valid")]
    fn test_body_hash(block: Block) -> Result<(), InvalidBlockDetails> {
        super::block_body_hash_valid(&block)
    }
}
