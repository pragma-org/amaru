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

use bumpalo::collections::Vec as BumpVec;
use chumsky::{Parser, prelude::*};

use super::{
    constant,
    types::{Extra, MapExtra},
    utils::{comments, name},
};
use crate::{arena::Arena, binder::DeBruijn, term::Term};

pub fn parser<'a>() -> impl Parser<'a, &'a str, &'a Term<'a, DeBruijn>, Extra<'a>> {
    recursive(|term| {
        choice((
            // Var
            name().padded().map_with(|v, e: &mut MapExtra<'a, '_>| {
                let state = e.state();

                let position = state.env.iter().rposition(|&x| x == v);

                if position.is_none() {
                    let placeholder = Term::var(state.arena, DeBruijn::zero(state.arena));

                    // this will fail at eval time
                    // the conformance tests don't expect this
                    // to fail at parse time
                    placeholder
                } else {
                    let debruijn_index = state.env.len() - position.unwrap_or_default();

                    let d = DeBruijn::new(state.arena, debruijn_index);

                    Term::var(state.arena, d)
                }
            }),
            // Delay
            text::keyword("delay")
                .padded()
                .ignore_then(term.clone().padded())
                .delimited_by(just('('), just(')'))
                .map_with(|term: &Term<'_, DeBruijn>, e: &mut MapExtra<'a, '_>| {
                    let state = e.state();

                    term.delay(state.arena)
                }),
            // Force
            text::keyword("force")
                .padded()
                .ignore_then(term.clone().padded())
                .delimited_by(just('('), just(')'))
                .map_with(|term, e| {
                    let state = e.state();

                    term.force(state.arena)
                }),
            // Lambda
            text::keyword("lam")
                .padded()
                .ignore_then(name().padded())
                .map_with(|v, e: &mut MapExtra<'a, '_>| {
                    let state = e.state();

                    state.env.push(v);

                    0
                })
                .then(term.clone().padded())
                .delimited_by(just('('), just(')'))
                .map_with(|(v, term), e| {
                    let state = e.state();

                    state.env.pop();

                    let d = DeBruijn::new(state.arena, v);

                    term.lambda(state.arena, d)
                }),
            // Apply
            term.clone()
                .padded()
                .foldl_with(term.clone().padded().repeated().at_least(1), |a, b, e| {
                    let state = e.state();

                    a.apply(state.arena, b)
                })
                .delimited_by(just('['), just(']')),
            // Constant
            constant::parser().map_with(|c, e: &mut MapExtra<'a, '_>| {
                let state = e.state();

                Term::constant(state.arena, c)
            }),
            // Builtin
            text::keyword("builtin")
                .padded()
                .ignore_then(text::ident().padded())
                .delimited_by(just('('), just(')'))
                .validate(|v, e: &mut MapExtra<'a, '_>, emitter| {
                    let state = e.state();

                    if let Some(builtin) = builtin_from_str(state.arena, v) {
                        builtin
                    } else {
                        let builtin = Term::error(state.arena);

                        emitter.emit(Rich::custom(e.span(), format!("unknown builtin {v}")));

                        builtin
                    }
                }),
            // Error
            text::keyword("error").padded().ignored().delimited_by(just('('), just(')')).map_with(
                |_, e: &mut MapExtra<'a, '_>| {
                    let state = e.state();

                    Term::error(state.arena)
                },
            ),
            text::keyword("constr")
                .padded()
                .ignore_then(text::int(10).padded())
                .then(term.clone().padded().repeated().collect::<Vec<&Term<'_, DeBruijn>>>())
                .delimited_by(just('('), just(')'))
                .validate(|(tag, fields), e: &mut MapExtra<'a, '_>, emitter| {
                    let span = e.span();
                    let state = e.state();

                    let fields = BumpVec::from_iter_in(fields, state.arena.as_bump());
                    let fields = state.arena.alloc(fields);

                    match tag.parse::<usize>() {
                        Ok(t) => {
                            let ret = Term::constr(state.arena, t, fields);

                            if !state.is_constr_case_available() {
                                emitter
                                    .emit(Rich::custom(e.span(), "constr is not available for this protocol version"));
                            }

                            ret
                        }
                        Err(_) => {
                            emitter.emit(Rich::custom(span, format!("invalid constr tag: {tag}")));
                            Term::error(state.arena)
                        }
                    }
                }),
            text::keyword("case")
                .padded()
                .ignore_then(term.clone().padded())
                .then(term.padded().repeated().collect::<Vec<&Term<'_, DeBruijn>>>())
                .delimited_by(just('('), just(')'))
                .validate(|(tag, branches), e: &mut MapExtra<'a, '_>, emitter| {
                    let state = e.state();

                    let branches = BumpVec::from_iter_in(branches, state.arena.as_bump());
                    let branches = state.arena.alloc(branches);

                    let ret = Term::case(state.arena, tag, branches);

                    if !state.is_constr_case_available() {
                        emitter.emit(Rich::custom(e.span(), "case is not available for this protocol version"));
                    }

                    ret
                }),
        ))
        .padded_by(comments())
        .boxed()
    })
}

pub fn builtin_from_str<'a>(arena: &'a Arena, name: &str) -> Option<&'a Term<'a, DeBruijn>> {
    match name {
        "addInteger" => Some(Term::add_integer(arena)),
        "subtractInteger" => Some(Term::subtract_integer(arena)),
        "equalsInteger" => Some(Term::equals_integer(arena)),
        "lessThanEqualsInteger" => Some(Term::less_than_equals_integer(arena)),
        "multiplyInteger" => Some(Term::multiply_integer(arena)),
        "divideInteger" => Some(Term::divide_integer(arena)),
        "quotientInteger" => Some(Term::quotient_integer(arena)),
        "remainderInteger" => Some(Term::remainder_integer(arena)),
        "modInteger" => Some(Term::mod_integer(arena)),
        "lessThanInteger" => Some(Term::less_than_integer(arena)),
        "ifThenElse" => Some(Term::if_then_else(arena)),
        "appendByteString" => Some(Term::append_byte_string(arena)),
        "equalsByteString" => Some(Term::equals_byte_string(arena)),
        "consByteString" => Some(Term::cons_byte_string(arena)),
        "sliceByteString" => Some(Term::slice_byte_string(arena)),
        "lengthOfByteString" => Some(Term::length_of_byte_string(arena)),
        "indexByteString" => Some(Term::index_byte_string(arena)),
        "lessThanByteString" => Some(Term::less_than_byte_string(arena)),
        "lessThanEqualsByteString" => Some(Term::less_than_equals_byte_string(arena)),
        "sha2_256" => Some(Term::sha2_256(arena)),
        "sha3_256" => Some(Term::sha3_256(arena)),
        "blake2b_256" => Some(Term::blake2b_256(arena)),
        "keccak_256" => Some(Term::keccak_256(arena)),
        "blake2b_224" => Some(Term::blake2b_224(arena)),
        "verifySignature" | "verifyEd25519Signature" => Some(Term::verify_ed25519_signature(arena)),
        "verifyEcdsaSecp256k1Signature" => Some(Term::verify_ecdsa_secp256k1_signature(arena)),
        "verifySchnorrSecp256k1Signature" => Some(Term::verify_schnorr_secp256k1_signature(arena)),
        "appendString" => Some(Term::append_string(arena)),
        "equalsString" => Some(Term::equals_string(arena)),
        "encodeUtf8" => Some(Term::encode_utf8(arena)),
        "decodeUtf8" => Some(Term::decode_utf8(arena)),
        "chooseUnit" => Some(Term::choose_unit(arena)),
        "trace" => Some(Term::trace(arena)),
        "fstPair" => Some(Term::fst_pair(arena)),
        "sndPair" => Some(Term::snd_pair(arena)),
        "chooseList" => Some(Term::choose_list(arena)),
        "mkCons" => Some(Term::mk_cons(arena)),
        "headList" => Some(Term::head_list(arena)),
        "tailList" => Some(Term::tail_list(arena)),
        "nullList" => Some(Term::null_list(arena)),
        "chooseData" => Some(Term::choose_data(arena)),
        "constrData" => Some(Term::constr_data(arena)),
        "mapData" => Some(Term::map_data(arena)),
        "listData" => Some(Term::list_data(arena)),
        "iData" => Some(Term::i_data(arena)),
        "bData" => Some(Term::b_data(arena)),
        "unConstrData" => Some(Term::un_constr_data(arena)),
        "unMapData" => Some(Term::un_map_data(arena)),
        "unListData" => Some(Term::un_list_data(arena)),
        "unIData" => Some(Term::un_i_data(arena)),
        "unBData" => Some(Term::un_b_data(arena)),
        "equalsData" => Some(Term::equals_data(arena)),
        "mkPairData" => Some(Term::mk_pair_data(arena)),
        "mkNilData" => Some(Term::mk_nil_data(arena)),
        "mkNilPairData" => Some(Term::mk_nil_pair_data(arena)),
        "serialiseData" => Some(Term::serialise_data(arena)),
        "bls12_381_G1_add" => Some(Term::bls12_381_g1_add(arena)),
        "bls12_381_G1_neg" => Some(Term::bls12_381_g1_neg(arena)),
        "bls12_381_G1_scalarMul" => Some(Term::bls12_381_g1_scalar_mul(arena)),
        "bls12_381_G1_equal" => Some(Term::bls12_381_g1_equal(arena)),
        "bls12_381_G1_compress" => Some(Term::bls12_381_g1_compress(arena)),
        "bls12_381_G1_uncompress" => Some(Term::bls12_381_g1_uncompress(arena)),
        "bls12_381_G1_hashToGroup" => Some(Term::bls12_381_g1_hash_to_group(arena)),
        "bls12_381_G2_add" => Some(Term::bls12_381_g2_add(arena)),
        "bls12_381_G2_neg" => Some(Term::bls12_381_g2_neg(arena)),
        "bls12_381_G2_scalarMul" => Some(Term::bls12_381_g2_scalar_mul(arena)),
        "bls12_381_G2_equal" => Some(Term::bls12_381_g2_equal(arena)),
        "bls12_381_G2_compress" => Some(Term::bls12_381_g2_compress(arena)),
        "bls12_381_G2_uncompress" => Some(Term::bls12_381_g2_uncompress(arena)),
        "bls12_381_G2_hashToGroup" => Some(Term::bls12_381_g2_hash_to_group(arena)),
        "bls12_381_millerLoop" => Some(Term::bls12_381_miller_loop(arena)),
        "bls12_381_mulMlResult" => Some(Term::bls12_381_mul_ml_result(arena)),
        "bls12_381_finalVerify" => Some(Term::bls12_381_final_verify(arena)),
        "integerToByteString" => Some(Term::integer_to_byte_string(arena)),
        "byteStringToInteger" => Some(Term::byte_string_to_integer(arena)),
        "andByteString" => Some(Term::and_byte_string(arena)),
        "orByteString" => Some(Term::or_byte_string(arena)),
        "xorByteString" => Some(Term::xor_byte_string(arena)),
        "complementByteString" => Some(Term::complement_byte_string(arena)),
        "readBit" => Some(Term::read_bit(arena)),
        "writeBits" => Some(Term::write_bits(arena)),
        "replicateByte" => Some(Term::replicate_byte(arena)),
        "shiftByteString" => Some(Term::shift_byte_string(arena)),
        "rotateByteString" => Some(Term::rotate_byte_string(arena)),
        "countSetBits" => Some(Term::count_set_bits(arena)),
        "findFirstSetBit" => Some(Term::find_first_set_bit(arena)),
        "ripemd_160" => Some(Term::ripemd_160(arena)),
        "expModInteger" => Some(Term::exp_mod_integer(arena)),
        "dropList" => Some(Term::drop_list(arena)),
        "lengthOfArray" => Some(Term::length_of_array(arena)),
        "listToArray" => Some(Term::list_to_array(arena)),
        "indexArray" => Some(Term::index_array(arena)),
        "bls12_381_G1_multiScalarMul" => Some(Term::bls12_381_g1_multi_scalar_mul(arena)),
        "bls12_381_G2_multiScalarMul" => Some(Term::bls12_381_g2_multi_scalar_mul(arena)),
        "insertCoin" => Some(Term::insert_coin(arena)),
        "lookupCoin" => Some(Term::lookup_coin(arena)),
        "unionValue" => Some(Term::union_value(arena)),
        "valueContains" => Some(Term::value_contains(arena)),
        "valueData" => Some(Term::value_data(arena)),
        "unValueData" => Some(Term::un_value_data(arena)),
        "scaleValue" => Some(Term::scale_value(arena)),
        _ => None,
    }
}
