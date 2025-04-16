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

use std::ops::Deref;

use amaru_kernel::{
    protocol_parameters::ProtocolParameters, sum_ex_units, ExUnits, MintedBlock, Redeemers,
};

use crate::context::ValidationContext;

use super::{BlockValidation, InvalidBlock};

pub enum InvalidExUnits {
    TooMany { provided: ExUnits, max: ExUnits },
}

pub fn block_ex_units_valid<C: ValidationContext>(
    _context: &mut C,
    block: &MintedBlock<'_>,
    protocol_parameters: &ProtocolParameters,
) -> BlockValidation {
    // TODO: rewrite this to use iterators defined on `Redeemers` and `MaybeIndefArray`, ideally
    let ex_units = block
        .transaction_witness_sets
        .iter()
        .flat_map(|witness_set| {
            witness_set
                .redeemer
                .iter()
                .map(|redeemers| match redeemers.deref() {
                    Redeemers::List(list) => list.iter().map(|r| r.ex_units).collect::<Vec<_>>(),
                    Redeemers::Map(map) => map.iter().map(|(_, r)| r.ex_units).collect::<Vec<_>>(),
                })
        })
        .flatten()
        .collect::<Vec<_>>();

    let pp_max_ex_units = protocol_parameters.max_block_ex_units;
    let ex_units = ex_units
        .into_iter()
        .fold(ExUnits { mem: 0, steps: 0 }, sum_ex_units);

    if ex_units.mem <= pp_max_ex_units.mem && ex_units.steps <= pp_max_ex_units.steps {
        BlockValidation::Valid
    } else {
        BlockValidation::Invalid(InvalidBlock::ExUnits(InvalidExUnits::TooMany {
            provided: ex_units,
            max: ExUnits {
                mem: pp_max_ex_units.mem,
                steps: pp_max_ex_units.steps,
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::InvalidExUnits;
    use amaru_kernel::{
        include_cbor, protocol_parameters::ProtocolParameters, ExUnits, HasExUnits, MintedBlock,
    };
    use test_case::test_case;

    macro_rules! fixture {
        ($number:literal) => {
            (
                include_cbor!(concat!("blocks/preprod/", $number, "/valid.cbor")),
                ProtocolParameters::default(),
            )
        };
        ($number:literal, $pp:expr) => {
            (
                include_cbor!(concat!("blocks/preprod/", $number, "/valid.cbor")),
                $pp,
            )
        };
    }

    #[test_case(fixture!("2667657"); "valid ex units")]
    #[test_case(fixture!("2667657", ProtocolParameters {
        max_block_ex_units: ExUnits {
            mem: 0,
            steps: 0
        },
        ..Default::default()
    }) => matches Err(InvalidExUnits::TooMany{provided, max: _})
        if provided == ExUnits {mem: 1267029, steps: 289959162}; "invalid ex units")]
    fn test_ex_units(
        (block, protocol_parameters): (MintedBlock<'_>, ProtocolParameters),
    ) -> Result<(), InvalidExUnits> {
        super::block_ex_units_valid(block.ex_units(), &protocol_parameters)
    }
}
