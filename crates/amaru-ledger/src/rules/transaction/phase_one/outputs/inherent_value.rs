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

use amaru_kernel::{HasLovelace, MemoizedTransactionOutput, ProtocolParameters, cbor};

use super::InvalidOutput;

/// Constant charged on top of the serialized output size, approximating the in-memory overhead
/// of the corresponding transaction input and UTxO map entry.
const UTXO_ENTRY_OVERHEAD: u64 = 160;

pub fn execute(
    protocol_parameters: &ProtocolParameters,
    output: &MemoizedTransactionOutput,
) -> Result<(), InvalidOutput> {
    // This conversion is safe with no loss of information
    let minimum_value =
        (UTXO_ENTRY_OVERHEAD + output.original_size() as u64) * protocol_parameters.lovelace_per_utxo_byte;
    let given_value = output.lovelace();

    if given_value < minimum_value {
        return Err(InvalidOutput::TooSmall { minimum_value, given_value });
    }

    let max_value_size = protocol_parameters.max_value_size;
    let given_val_size = cbor::count_bytes(&output.value);

    // This conversion is safe because max_value_size will never be big enough to cause a problem
    if given_val_size > max_value_size as usize {
        return Err(InvalidOutput::ValueTooLarge { maximum_size: max_value_size as usize, given_size: given_val_size });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use amaru_kernel::{
        Address, AssetName, Assets, Hash, MemoizedDatum, MemoizedTransactionOutput, Multiasset,
        PREPROD_DEFAULT_PROTOCOL_PARAMETERS, PositiveCoin, ProtocolParameters, Value, cbor::count_bytes, from_cbor,
        to_cbor, utils::tests::random_bytes,
    };

    use super::*;

    #[test]
    fn the_value_size_is_measured_via_reserialization_not_the_wire_bytes() {
        let output = output_with(non_canonical_value(1_000_000_000));
        let result = execute(&protocol_parameters_with_max_size(5), &output);
        assert!(result.is_ok(), "ledger size 5 must satisfy max_value_size 5: {result:?}");
    }

    #[test]
    fn a_value_exceeding_the_ledger_size_limit_is_rejected() {
        let output = output_with(non_canonical_value(1_000_000_000));

        match execute(&protocol_parameters_with_max_size(4), &output) {
            Err(InvalidOutput::ValueTooLarge { maximum_size, given_size }) => {
                assert_eq!(maximum_size, 4);
                assert_eq!(given_size, 5);
            }
            other => panic!("expected ValueTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn large_maps_with_indefinite_length_headers_are_valid_with_the_cardano_node_encoding() {
        // Up to 23 entries the node writes a definite-length header, and an indefinite one above,
        // which is what the ledger-side measurement counts.
        let small = to_cbor(&multiassets_of(20));
        assert_eq!(small[0], 0xb4, "20 policies are written with a definite-length header");
        assert_eq!(count_bytes(&multiassets_of(20)), small.len());

        let large = to_cbor(&multiassets_of(324));
        assert_eq!(large[0], 0xbf, "324 policies are written with an indefinite-length header");
        assert_eq!(large[large.len() - 1], 0xff, "an indefinite-length map ends with a break");
        assert_eq!(count_bytes(&multiassets_of(324)), large.len());

        let multiassets = multiassets_of(324);
        let bytes = to_cbor(&(2000000, &multiassets));
        let output = output_with(from_cbor(&bytes).expect("valid value"));
        let result = execute(&protocol_parameters_with_max_size(10700), &output);
        assert!(result.is_ok(), "the value should have been accepted {}", result.unwrap_err());
    }

    #[test]
    fn boundary_sized_value_accepted_by_the_network_is_accepted() {
        // This test case reproduces a real value found in preprod.
        //
        // The value of output #1 of the preprod transaction 96ae78f7... (block b2c00c16..., epoch
        // 303): 358 assets across 6 policies, one of them holding 324 assets. The network
        // accepted it at exactly maxValueSize=5000 while our the amaru encoding originally returned 5001.
        let value: Value = amaru_kernel::include_cbor!("phase-one/preprod/b2c00c16/output-1-value.cbor");
        let ledger_size = count_bytes(&value) as u32;
        assert_eq!(ledger_size, 5000, "the ledger-side size must be 5000 bytes");

        let output = output_with(value);
        let result = execute(&protocol_parameters_with_max_size(ledger_size), &output);
        assert!(result.is_ok(), "the value is accepted");
    }

    // HELPERS

    /// A value whose on-wire encoding is one CBOR argument wider than necessary: the coin is
    /// encoded with the 8-byte argument form although it fits in 4 bytes, giving 9 wire bytes
    /// against a 5-byte for the haskell node serialization.
    fn non_canonical_value(coin: u32) -> Value {
        let mut bytes = vec![0x1b];
        bytes.extend_from_slice(&u64::from(coin).to_be_bytes());
        from_cbor(&bytes).expect("valid non-minimal CBOR value")
    }

    /// A value carrying `assets` distinct single-unit assets under one policy.
    fn multiassets_of(n: u16) -> Multiasset<PositiveCoin> {
        let assets = Assets::try_from(BTreeMap::from([(AssetName::empty(), PositiveCoin::try_from(1).unwrap())]))
            .expect("one asset");
        (0..n)
            .map(|_| (Hash::try_from(random_bytes(28).as_slice()).unwrap(), assets.clone()))
            .collect::<BTreeMap<_, _>>()
            .into()
    }

    fn output_with(value: Value) -> MemoizedTransactionOutput {
        let address = Address::from_bech32("addr_test1vp0ksclfnd0zjtfu70npnccut6sjex8w9k0h246xrsl089qnvvmuc")
            .expect("valid address");
        MemoizedTransactionOutput::new(address, value, MemoizedDatum::None, None)
    }

    fn protocol_parameters_with_max_size(max_value_size: u32) -> ProtocolParameters {
        ProtocolParameters { max_value_size, lovelace_per_utxo_byte: 0, ..PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone() }
    }
}
