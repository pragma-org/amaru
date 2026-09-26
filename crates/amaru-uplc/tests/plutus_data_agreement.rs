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

//! Two decoders read the same Plutus data grammar: `amaru_kernel::PlutusData`, for the data
//! carried by transactions, and `amaru_uplc::data::PlutusData`, for the constants embedded in
//! scripts. They deserialise into different representations on purpose — the latter allocates into
//! an arena for evaluation — but they must agree on which bytes are valid, or a script and the
//! transaction that carries it would disagree about the same payload.
//!
//! This test pins that agreement on the cases where the two have drifted apart before.

use amaru_kernel::{PlutusData as KernelPlutusData, cbor, protocol_version::PROTOCOL_VERSION_10};
use amaru_uplc::{arena::Arena, data::PlutusData as UplcPlutusData};
use test_case::test_case;

#[test_case(vec![0x00]                                  ; "a small integer")]
#[test_case(vec![0x80]                                  ; "an empty array")]
#[test_case(vec![0xa0]                                  ; "an empty map")]
#[test_case(bytes(0)                                    ; "an empty byte string")]
#[test_case(bytes(64)                                   ; "a byte string at the 64-byte limit")]
#[test_case(bytes(65)                                   ; "a byte string over the limit")]
#[test_case(chunked(&[64, 64])                          ; "two chunks at the limit")]
#[test_case(chunked(&[65])                              ; "a chunk over the limit")]
#[test_case(bignum(64)                                  ; "a bignum payload at the limit")]
#[test_case(bignum(65)                                  ; "a bignum payload over the limit")]
#[test_case(vec![0xd8, 0x79, 0x80]                      ; "constr tag 121, no fields")]
#[test_case(vec![0xd8, 0x78, 0x80]                      ; "tag 120, below the constr range")]
#[test_case(vec![0xd9, 0x05, 0x00, 0x80]                ; "tag 1280, the second constr range")]
#[test_case(vec![0xd9, 0x05, 0x79, 0x80]                ; "tag 1401, past the second range")]
#[test_case(vec![0xd8, 0x66, 0x82, 0x00, 0x80]          ; "tag 102 as a definite pair")]
#[test_case(vec![0xd8, 0x66, 0x9f, 0x00, 0x80, 0xff]    ; "tag 102 as an indefinite pair")]
#[test_case(vec![0xd8, 0x66, 0x83, 0x00, 0x80, 0x80]    ; "tag 102 with three elements")]
#[test_case(vec![0xd8, 0x66, 0x81, 0x00]                ; "tag 102 with one element")]
fn both_decoders_agree(input: Vec<u8>) {
    let mut version = PROTOCOL_VERSION_10;
    let kernel: Result<KernelPlutusData, _> = cbor::decode_with(&input, &mut version);

    let arena = Arena::new();
    let uplc = UplcPlutusData::from_cbor(&arena, &input);

    assert_eq!(
        kernel.is_ok(),
        uplc.is_ok(),
        "the two decoders disagree on {}:\n  kernel: {:?}\n  uplc:   {:?}",
        hex::encode(&input),
        kernel.map(|_| ()).map_err(|e| e.to_string()),
        uplc.map(|_| ()).map_err(|e| e.to_string()),
    );
}

/// The table above only asserts that the two decoders agree, which would still hold if both of
/// them rejected everything. This pins one case on each side so the corpus cannot go vacuous.
#[test]
fn the_corpus_contains_both_outcomes() {
    let mut version = PROTOCOL_VERSION_10;
    let arena = Arena::new();

    let accepted = bytes(64);
    assert!(cbor::decode_with::<_, KernelPlutusData>(&accepted, &mut version).is_ok());
    assert!(UplcPlutusData::from_cbor(&arena, &accepted).is_ok());

    let rejected = bytes(65);
    assert!(cbor::decode_with::<_, KernelPlutusData>(&rejected, &mut version).is_err());
    assert!(UplcPlutusData::from_cbor(&arena, &rejected).is_err());
}

// HELPERS

/// A definite-length byte string of `len` zeroes.
fn bytes(len: usize) -> Vec<u8> {
    [vec![0x58, len as u8], vec![0x00; len]].concat()
}

/// An indefinite-length byte string made of the given chunks.
fn chunked(chunks: &[usize]) -> Vec<u8> {
    let mut out = vec![0x5f];
    for len in chunks {
        out.extend(bytes(*len));
    }
    out.push(0xff);
    out
}

/// A bignum with a payload of `len` zeroes.
fn bignum(len: usize) -> Vec<u8> {
    [vec![0xc2], bytes(len)].concat()
}
