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

use amaru_kernel::{HasLovelace, MemoizedTransactionOutput, ProtocolParameters};

use super::{InvalidOutput, value_size::cardano_node_value_size};

pub fn execute(
    protocol_parameters: &ProtocolParameters,
    output: &MemoizedTransactionOutput,
) -> Result<(), InvalidOutput> {
    // This conversion is safe with no loss of information
    let minimum_value = output.original_size() as u64 * protocol_parameters.lovelace_per_utxo_byte;
    let given_value = output.lovelace();

    if given_value < minimum_value {
        return Err(InvalidOutput::TooSmall { minimum_value, given_value });
    }

    let max_value_size = protocol_parameters.max_value_size;
    let given_val_size = cardano_node_value_size(output.value.as_ref());

    // This conversion is safe because max_value_size will never be big enough to cause a problem
    if given_val_size > max_value_size as usize {
        return Err(InvalidOutput::ValueTooLarge { maximum_size: max_value_size as usize, given_size: given_val_size });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{
        Address, AssetName, Bytes, Hash, MemoizedDatum, MemoizedTransactionOutput, MemoizedValue,
        NonEmptyKeyValuePairs, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, PositiveCoin, ProtocolParameters, Value, to_cbor,
    };

    use super::*;

    #[test]
    fn the_value_size_is_measured_via_the_cardano_node_serialization_not_the_wire_bytes() {
        let output = output_with(amaru_encoded_value(1_000_000_000));
        assert_eq!(output.value.original_bytes().len(), 9);

        let result = execute(&protocol_parameters_with_max_size(5), &output);
        assert!(result.is_ok(), "ledger size 5 must satisfy max_value_size 5: {result:?}");
    }

    #[test]
    fn a_value_exceeding_the_ledger_size_limit_is_rejected() {
        let output = output_with(amaru_encoded_value(1_000_000_000));

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
        // Maps with less than or equal to 255 entries have the same size regardless of the amaru
        // or haskell encoding
        let value = multiasset_value(20);
        assert_eq!(cardano_node_value_size(&value), to_cbor(&value).len());

        let value = multiasset_value(100);
        assert_eq!(cardano_node_value_size(&value), to_cbor(&value).len());

        // From 256 entries on, there is a one-byte difference between the amaru and haskell encodings.
        // The indefinite-length header is one byte shorter than the definite-length header.
        // The ledger size is therefore one byte smaller than the amaru encoding.
        let value = multiasset_value(324);
        assert_eq!(cardano_node_value_size(&value), to_cbor(&value).len() - 1);

        let output = output_with(MemoizedValue::new(value).expect("valid value"));
        // The amaru encoding would fail with this max size
        let result = execute(&protocol_parameters_with_max_size(1335), &output);
        assert!(result.is_ok(), "the value is accepted {}", result.unwrap_err());
    }

    #[test]
    fn boundary_sized_value_accepted_by_the_network_is_accepted() {
        // This test case reproduces a real value found in preprod.
        //
        // The value of output #1 of the preprod transaction 96ae78f7... (block b2c00c16..., epoch
        // 303): 358 assets across 6 policies, one of them holding 324 assets. The network
        // accepted it at exactly maxValueSize=5000 while our the amaru encoding originally returned 5001.
        let value: MemoizedValue = amaru_kernel::include_cbor!("phase-one/preprod/b2c00c16/output-1-value.cbor");
        let ledger_size = cardano_node_value_size(value.as_ref()) as u64;
        assert_eq!(ledger_size, 5000, "the ledger-side size must be 5000 bytes");
        assert_eq!(
            value.original_bytes().len() as u64,
            ledger_size + 1,
            "the wire encoding is one byte longer than the haskell serialization"
        );

        let output = output_with(value);
        let result = execute(&protocol_parameters_with_max_size(ledger_size), &output);
        assert!(result.is_ok(), "the value is accepted");
    }

    // HELPERS

    /// A value whose on-wire encoding is one CBOR argument wider than necessary: the coin is
    /// encoded with the 8-byte argument form although it fits in 4 bytes, giving 9 wire bytes
    /// against a 5-byte for the haskell node serialization.
    fn amaru_encoded_value(coin: u32) -> MemoizedValue {
        let mut bytes = vec![0x1b];
        bytes.extend_from_slice(&u64::from(coin).to_be_bytes());
        MemoizedValue::try_from(Bytes::from(bytes)).expect("valid non-minimal CBOR value")
    }

    /// A value carrying `assets` distinct single-unit assets under one policy.
    fn multiasset_value(assets: u16) -> Value {
        let assets = NonEmptyKeyValuePairs::try_from(
            (0..assets)
                .map(|i| {
                    let name = AssetName::try_from(i.to_be_bytes().to_vec()).unwrap();
                    (name, PositiveCoin::try_from(1).unwrap())
                })
                .collect::<Vec<_>>(),
        )
        .unwrap();
        Value::Multiasset(2_000_000, std::iter::once((Hash::from([7u8; 28]), assets)).collect())
    }

    fn output_with(value: MemoizedValue) -> MemoizedTransactionOutput {
        let address = Address::from_bech32("addr_test1vp0ksclfnd0zjtfu70npnccut6sjex8w9k0h246xrsl089qnvvmuc")
            .expect("valid address");
        MemoizedTransactionOutput::new(false, address, value, MemoizedDatum::None, None)
    }

    fn protocol_parameters_with_max_size(max_value_size: u64) -> ProtocolParameters {
        ProtocolParameters { max_value_size, lovelace_per_utxo_byte: 0, ..PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone() }
    }
}
