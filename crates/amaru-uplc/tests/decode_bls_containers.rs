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

use amaru_kernel::ProtocolVersion;
use amaru_uplc::{
    arena::Arena,
    binder::DeBruijn,
    flat::{FlatDecodeError, decode},
};

// This test makes sure that we can successfully validate lists and arrays of BLS values
// or pairs of values (or any nested type containing BLS value types), when there are no actual elements.

const BLS_TYPE_TAGS: [u8; 3] = [9, 10, 11]; // G1 element, G2 element, MlResult
const TYPE_APPLICATION: u8 = 7;
const INTEGER: u8 = 0;
const LIST: u8 = 5;
const PAIR: u8 = 6;
const ARRAY: u8 = 12;

/// `(con (list t) [])` and `(con (array t) [])` for each BLS type `t`: the type decodes and the
/// empty container never reads an element value, exactly as on the Haskell side (which
/// flat-encodes arrays as lists too).
#[test]
fn empty_containers_of_bls_values_decode() {
    for container in [LIST, ARRAY] {
        for type_tag in BLS_TYPE_TAGS {
            let flat = flat_program(|bits| {
                constant_with_type(bits, &[TYPE_APPLICATION, container, type_tag]);
                bits.push(0, 1); // empty container value: a single nil bit
            });
            assert_decodes(&flat, &format!("an empty container {container} of BLS type tag {type_tag}"));
        }
    }
}

/// `(con (list (pair t integer)) [])` for each BLS type `t`: type-level nesting also decodes as
/// long as no BLS value is present.
#[test]
fn empty_lists_of_pairs_with_bls_components_decode() {
    for type_tag in BLS_TYPE_TAGS {
        let flat = flat_program(|bits| {
            constant_with_type(
                bits,
                &[TYPE_APPLICATION, LIST, TYPE_APPLICATION, TYPE_APPLICATION, PAIR, type_tag, INTEGER],
            );
            bits.push(0, 1); // empty list value: a single nil bit
        });
        assert_decodes(&flat, &format!("an empty list of pairs with BLS type tag {type_tag}"));
    }
}

/// `(con t …)` for each BLS type `t`: decoding must fail on the *value*, mirroring the Haskell
/// `Flat` instances for BLS values (which always fail), before any value bytes are even read.
#[test]
fn bls_values_fail_to_decode() {
    for type_tag in BLS_TYPE_TAGS {
        let flat = flat_program(|bits| {
            constant_with_type(bits, &[type_tag]);
        });
        assert_fails_on_bls_value(&flat, &format!("a bare BLS constant of type tag {type_tag}"));
    }
}

/// `(con (list t) [x])` for each BLS type `t`: a non-empty container reaches the element value
/// and must fail like the bare constant.
#[test]
fn non_empty_lists_of_bls_values_fail_to_decode() {
    for type_tag in BLS_TYPE_TAGS {
        let flat = flat_program(|bits| {
            constant_with_type(bits, &[TYPE_APPLICATION, LIST, type_tag]);
            bits.push(1, 1); // cons: one element follows
        });
        assert_fails_on_bls_value(&flat, &format!("a non-empty list of BLS type tag {type_tag}"));
    }
}

/// Pairs have no empty form: their values always carry both components, so any pair mentioning a
/// BLS type fails on the value, in both component orders, and identically on the Haskell side.
#[test]
fn pairs_with_bls_components_fail_to_decode() {
    for type_tag in BLS_TYPE_TAGS {
        let bls_first = flat_program(|bits| {
            constant_with_type(bits, &[TYPE_APPLICATION, TYPE_APPLICATION, PAIR, type_tag, INTEGER]);
        });
        assert_fails_on_bls_value(&bls_first, &format!("a pair (bls {type_tag}, integer)"));

        let bls_second = flat_program(|bits| {
            constant_with_type(bits, &[TYPE_APPLICATION, TYPE_APPLICATION, PAIR, INTEGER, type_tag]);
            bits.push(0, 8); // first component: the integer 0, then the BLS value fails
        });
        assert_fails_on_bls_value(&bls_second, &format!("a pair (integer, bls {type_tag})"));
    }
}

/// `(con (pair (list t) integer) ([], 0))` for each BLS type `t`: a BLS type occurring inside a
/// pair *value* is fine as long as it is shielded by an empty container — the pair decodes both
/// components without ever reading a BLS element.
#[test]
fn pairs_holding_empty_lists_of_bls_values_decode() {
    for type_tag in BLS_TYPE_TAGS {
        let flat = flat_program(|bits| {
            constant_with_type(
                bits,
                &[TYPE_APPLICATION, TYPE_APPLICATION, PAIR, TYPE_APPLICATION, LIST, type_tag, INTEGER],
            );
            bits.push(0, 1); // first component: an empty list
            bits.push(0, 8); // second component: the integer 0
        });
        assert_decodes(&flat, &format!("a pair (empty list of bls {type_tag}, integer)"));
    }
}

// HELPERS

fn assert_decodes(flat: &[u8], description: &str) {
    let arena = Arena::new();
    let (_, remainder) = decode::<DeBruijn>(&arena, flat, ProtocolVersion::new(11, 0))
        .unwrap_or_else(|e| panic!("{description} must decode: {e}"));
    assert_eq!(remainder, 0, "{description} must decode without trailing bytes");
}

fn assert_fails_on_bls_value(flat: &[u8], description: &str) {
    let arena = Arena::new();
    let error = decode::<DeBruijn>(&arena, flat, ProtocolVersion::new(11, 0))
        .expect_err(&format!("{description} contains a BLS value and must not decode"));
    assert!(matches!(error, FlatDecodeError::BlsValueNotSupported), "unexpected error for {description}: {error}");
}

/// Emit a `con` term tag followed by its type tag list, with the cons/nil bits of the flat list
/// encoding around each 4-bit type tag.
fn constant_with_type(bits: &mut BitWriter, type_tags: &[u8]) {
    bits.push(4, 4); // term tag: constant
    for tag in type_tags {
        bits.push(1, 1); // cons
        bits.push(*tag, 4);
    }
    bits.push(0, 1); // nil
}

/// Assemble a flat program: version words 1.1.0, the bits produced by `build`, and the
/// end-of-program filler (zero bits up to the last bit of the byte, which is one).
fn flat_program(build: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut bits = BitWriter::default();
    for word in [1, 1, 0] {
        bits.push(word, 8);
    }
    build(&mut bits);
    bits.filler();
    bits.bytes
}

/// Writes bit fields MSB-first, as the flat format does.
#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    used: u8,
}

impl BitWriter {
    fn push(&mut self, value: u8, width: u8) {
        for i in (0..width).rev() {
            if self.used == 0 {
                self.bytes.push(0);
            }
            let bit = (value >> i) & 1;
            let byte = self.bytes.last_mut().unwrap();
            *byte |= bit << (7 - self.used);
            self.used = (self.used + 1) % 8;
        }
    }

    fn filler(&mut self) {
        while self.used != 7 {
            self.push(0, 1);
        }
        self.push(1, 1);
    }
}
