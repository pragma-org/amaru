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

mod encoder;
mod error;

pub use encoder::Encoder;
pub use error::FlatEncodeError;
use stacksafe::stacksafe;

use super::tag;
use crate::{
    binder::Binder,
    constant::{Constant, List},
    ledger_value::LedgerValue,
    program::Program,
    term::Term,
    typ::Type,
};

pub fn encode<'a, V>(program: &'a Program<'a, V>) -> Result<Vec<u8>, FlatEncodeError>
where
    V: Binder<'a>,
{
    let mut encoder = Encoder::default();

    encoder.word(program.version.major).word(program.version.minor).word(program.version.patch);

    encode_term(&mut encoder, program.term)?;

    encoder.filler();

    Ok(encoder.buffer)
}

#[stacksafe]
fn encode_term<'a, V>(encoder: &mut Encoder, term: &'a Term<'a, V>) -> Result<(), FlatEncodeError>
where
    V: Binder<'a>,
{
    match term {
        Term::Var(name) => {
            encode_term_tag(encoder, tag::VAR)?;

            name.var_encode(encoder)?;
        }
        Term::Lambda { parameter, body } => {
            encode_term_tag(encoder, tag::LAMBDA)?;

            parameter.parameter_encode(encoder)?;

            encode_term(encoder, body)?;
        }
        Term::Apply { function, argument } => {
            encode_term_tag(encoder, tag::APPLY)?;

            encode_term(encoder, function)?;

            encode_term(encoder, argument)?;
        }
        Term::Delay(body) => {
            encode_term_tag(encoder, tag::DELAY)?;

            encode_term(encoder, body)?;
        }
        Term::Force(body) => {
            encode_term_tag(encoder, tag::FORCE)?;

            encode_term(encoder, body)?;
        }
        Term::Case { constr, branches } => {
            encode_term_tag(encoder, tag::CASE)?;

            encode_term(encoder, constr)?;

            encoder.list_with(branches, |e, t| encode_term(e, t))?;
        }
        Term::Constr { tag, fields } => {
            encode_term_tag(encoder, tag::CONSTR)?;

            encoder.word(*tag);

            encoder.list_with(fields, |e, t| encode_term(e, t))?;
        }
        Term::Constant(c) => {
            encode_term_tag(encoder, tag::CONSTANT)?;

            encode_constant(encoder, c)?;
        }
        Term::Builtin(b) => {
            encode_term_tag(encoder, tag::BUILTIN)?;

            encoder.bits(tag::BUILTIN_TAG_WIDTH as i64, *b as u8);
        }
        Term::Error => {
            encode_term_tag(encoder, tag::ERROR)?;
        }
    }

    Ok(())
}

fn encode_constant<'a>(e: &mut Encoder, constant: &'a Constant<'a>) -> Result<(), FlatEncodeError> {
    match constant {
        Constant::Integer(i) => {
            e.list_with(&[tag::INTEGER], encode_constant_tag)?;

            e.integer(i);
        }
        Constant::ByteString(b) => {
            e.list_with(&[tag::BYTE_STRING], encode_constant_tag)?;

            e.bytes(b)?;
        }
        Constant::String(s) => {
            e.list_with(&[tag::STRING], encode_constant_tag)?;

            e.utf8(s)?;
        }
        Constant::Unit => {
            e.list_with(&[tag::UNIT], encode_constant_tag)?;
        }
        Constant::Boolean(b) => {
            e.list_with(&[tag::BOOL], encode_constant_tag)?;

            e.bool(*b);
        }
        Constant::Data(data) => {
            e.list_with(&[tag::DATA], encode_constant_tag)?;

            encode_data_constant(e, data)?;
        }
        Constant::ProtoList(typ, list) => {
            let mut type_encodings = vec![tag::PROTO_LIST_ONE, tag::PROTO_LIST_TWO];

            encode_type(typ, &mut type_encodings)?;

            e.list_with(&type_encodings, encode_constant_tag)?;

            encode_constant_list(e, list)?;
        }
        Constant::ProtoArray(typ, array) => {
            let mut type_encodings = vec![tag::PROTO_ARRAY_ONE, tag::PROTO_ARRAY_TWO];

            encode_type(typ, &mut type_encodings)?;

            e.list_with(&type_encodings, encode_constant_tag)?;

            e.list_with(array, encode_constant_value)?;
        }
        Constant::ProtoPair(fst_type, snd_type, fst, snd) => {
            let mut type_encodings = vec![tag::PROTO_PAIR_ONE, tag::PROTO_PAIR_TWO, tag::PROTO_PAIR_THREE];

            encode_type(fst_type, &mut type_encodings)?;

            encode_type(snd_type, &mut type_encodings)?;

            e.list_with(&type_encodings, encode_constant_tag)?;

            encode_constant_value(e, fst)?;
            encode_constant_value(e, snd)?;
        }
        Constant::Value(v) => {
            e.list_with(&[tag::VALUE], encode_constant_tag)?;

            encode_value(e, v)?;
        }
        Constant::Bls12_381G1Element(_) | Constant::Bls12_381G2Element(_) | Constant::Bls12_381MlResult(_) => {
            return Err(FlatEncodeError::BlsElementNotSupported);
        }
    }

    Ok(())
}

fn encode_term_tag(e: &mut Encoder, tag: u8) -> Result<(), FlatEncodeError> {
    safe_encode_bits(e, tag::TERM_TAG_WIDTH, tag)
}

fn encode_constant_tag(e: &mut Encoder, tag: &u8) -> Result<(), FlatEncodeError> {
    safe_encode_bits(e, tag::CONST_TAG_WIDTH, *tag)
}

#[stacksafe]
fn encode_type(typ: &Type<'_>, bytes: &mut Vec<u8>) -> Result<(), FlatEncodeError> {
    match typ {
        Type::Integer => bytes.push(tag::INTEGER),
        Type::ByteString => bytes.push(tag::BYTE_STRING),
        Type::String => bytes.push(tag::STRING),
        Type::Unit => bytes.push(tag::UNIT),
        Type::Bool => bytes.push(tag::BOOL),
        Type::List(sub_typ) => {
            bytes.extend(vec![tag::PROTO_LIST_ONE, tag::PROTO_LIST_TWO]);

            encode_type(sub_typ, bytes)?;
        }
        Type::Array(sub_typ) => {
            bytes.extend(vec![tag::PROTO_ARRAY_ONE, tag::PROTO_ARRAY_TWO]);

            encode_type(sub_typ, bytes)?;
        }
        Type::Pair(type1, type2) => {
            bytes.extend(vec![tag::PROTO_PAIR_ONE, tag::PROTO_PAIR_TWO, tag::PROTO_PAIR_THREE]);

            encode_type(type1, bytes)?;
            encode_type(type2, bytes)?;
        }
        Type::Data => bytes.push(tag::DATA),
        Type::Value => bytes.push(tag::VALUE),
        Type::Bls12_381G1Element | Type::Bls12_381G2Element | Type::Bls12_381MlResult => {
            return Err(FlatEncodeError::BlsElementNotSupported);
        }
    }

    Ok(())
}

#[stacksafe]
fn encode_constant_value(e: &mut Encoder, x: &&Constant<'_>) -> Result<(), FlatEncodeError> {
    match *x {
        Constant::Integer(x) => {
            e.integer(x);
        }
        Constant::ByteString(b) => {
            e.bytes(b)?;
        }
        Constant::String(s) => {
            e.utf8(s)?;
        }
        Constant::Unit => (),
        Constant::Boolean(b) => {
            e.bool(*b);
        }
        Constant::ProtoList(_, list) => {
            encode_constant_list(e, list)?;
        }
        Constant::ProtoArray(_, array) => {
            e.list_with(array, encode_constant_value)?;
        }
        Constant::ProtoPair(_, _, a, b) => {
            encode_constant_value(e, a)?;

            encode_constant_value(e, b)?;
        }
        Constant::Data(data) => {
            encode_data_constant(e, data)?;
        }
        Constant::Value(v) => {
            encode_value(e, v)?;
        }
        Constant::Bls12_381G1Element(_) | Constant::Bls12_381G2Element(_) | Constant::Bls12_381MlResult(_) => {
            return Err(FlatEncodeError::BlsElementNotSupported);
        }
    }

    Ok(())
}

fn encode_constant_list(e: &mut Encoder, list: &List<'_>) -> Result<(), FlatEncodeError> {
    for item in list {
        e.one();
        encode_constant_value(e, &item)?;
    }

    e.zero();

    Ok(())
}

/// Encode the CBOR payload carried by a Flat `Data` constant.
///
/// The outer `Encoder::bytes` is Flat's byte-array encoding. The nested CBOR
/// encoder is responsible for applying Plutus' 64-byte chunking rule to Data
/// leaves; ordinary UPLC byte-string constants must not be chunked this way.
fn encode_data_constant(e: &mut Encoder, data: &crate::data::PlutusData<'_>) -> Result<(), FlatEncodeError> {
    let mut cbor = Vec::new();
    minicbor::encode(data, &mut cbor)?;
    e.bytes(&cbor)?;

    Ok(())
}

fn encode_value(e: &mut Encoder, v: &LedgerValue<'_>) -> Result<(), FlatEncodeError> {
    for entry in v.entries {
        e.one();

        e.bytes(entry.currency)?;

        for token in entry.tokens {
            e.one();

            e.bytes(token.name)?;

            e.integer(token.quantity);
        }

        e.zero();
    }

    e.zero();

    Ok(())
}

fn safe_encode_bits(e: &mut Encoder, num_bits: usize, byte: u8) -> Result<(), FlatEncodeError> {
    if 2_u8.pow(num_bits as u32) <= byte {
        Err(FlatEncodeError::Overflow { byte, num_bits })
    } else {
        e.bits(num_bits as i64, byte);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{PlutusVersion, protocol_version::PROTOCOL_VERSION_10};

    use super::*;
    use crate::{
        arena::Arena, binder::DeBruijn, constant::Constant, data::PlutusData, flat::decode, machine::MachineVersion,
        program::Program, term::Term, typ::Type,
    };

    #[test]
    fn roundtrip_program_big_constr_tag() {
        // (program 1.1.0
        //   [
        //     [
        //       (builtin addInteger)
        //       (con integer 1)
        //     ]
        //     [ (force (force (builtin fstPair)))
        //       [ (builtin unConstrData)
        //         (con data (Constr 128 [B #00, B #0101]))
        //       ]
        //     ]
        //   ])
        let bytes_hex = "0101003370090011aab9d37549810cd8668218809f4100420101ff0001";
        let bytes = hex::decode(bytes_hex).unwrap();
        let arena = Arena::new();
        let program: Result<(&Program<'_, DeBruijn>, _), _> =
            decode(&arena, &bytes, PlutusVersion::V3, PROTOCOL_VERSION_10);
        match program {
            Ok((program, _)) => {
                let encoded = encode(program);
                match encoded {
                    Ok(roundtripped) => {
                        assert_eq!(bytes_hex, hex::encode(roundtripped));
                    }
                    Err(_) => {
                        panic!()
                    }
                }
            }
            Err(_) => {
                panic!();
            }
        }
    }

    #[test]
    fn roundtrip_program_bigint() {
        // (program 1.1.0
        //   [
        //     [
        //       (builtin addInteger)
        //       (con integer 1)
        //     ]
        //     [ (builtin unIData)
        //       [ (force (builtin headList))
        //         [ (force (force (builtin sndPair)))
        //           [ (builtin unConstrData)
        //             (con data (Constr 0 [I 999999999999999999999999999]))
        //           ]
        //         ]
        //       ]
        //     ]
        //   ])
        let bytes_hex = "0101003370090011bad357426aae78dd526112d8799fc24c033b2e3c9fd0803ce7ffffffff0001";
        let bytes = hex::decode(bytes_hex).unwrap();
        let arena = Arena::new();
        let program: Result<(&Program<'_, DeBruijn>, _), _> =
            decode(&arena, &bytes, PlutusVersion::V3, PROTOCOL_VERSION_10);
        match program {
            Ok((program, _)) => {
                let encoded = encode(program);
                match encoded {
                    Ok(roundtripped) => {
                        assert_eq!(bytes_hex, hex::encode(roundtripped));
                    }
                    Err(e) => {
                        panic!("{}", e);
                    }
                }
            }
            Err(e) => {
                panic!("{}", e);
            }
        }
    }

    #[test]
    fn roundtrip_program_list() {
        // (program 1.1.0
        //   [
        //     [
        //       (builtin multiplyInteger)
        //       (con integer 2)
        //     ]
        //     [ (builtin unIData)
        //       [ (force (builtin headList))
        //         [ (force (builtin tailList))
        //           [ (builtin unListData)
        //             (con data (List [I 7, I 14]))
        //           ]
        //         ]
        //       ]
        //     ]
        //   ])
        let bytes_hex = "0101003370490021bad357426ae88dd62601049f070eff0001";
        let bytes = hex::decode(bytes_hex).unwrap();
        let arena = Arena::new();
        let program: Result<(&Program<'_, DeBruijn>, _), _> =
            decode(&arena, &bytes, PlutusVersion::V3, PROTOCOL_VERSION_10);
        match program {
            Ok((program, _)) => {
                let encoded = encode(program);
                match encoded {
                    Ok(roundtripped) => {
                        assert_eq!(bytes_hex, hex::encode(roundtripped));
                    }
                    Err(e) => {
                        panic!("{}", e);
                    }
                }
            }
            Err(e) => {
                panic!("{}", e);
            }
        }
    }

    #[test]
    fn encode_constant_value_chunks_large_data_bytestring() {
        let arena = Arena::new();
        let bytes: &[u8] = arena.alloc([0_u8; 69]);
        let data = PlutusData::byte_string(&arena, bytes);
        let data_constant = Constant::data(&arena, data);
        let values: &[&Constant<'_>] = arena.alloc([data_constant]);
        let constant = Constant::proto_list(&arena, Type::data(&arena), values);
        let term = Term::<DeBruijn>::constant(&arena, constant);
        let program = Program::new(&arena, MachineVersion::V1_1_0, term);
        let encoded = encode(program).expect("encoding should succeed");

        let decoded_arena = Arena::new();
        decode::<DeBruijn>(&decoded_arena, &encoded, PlutusVersion::V3, PROTOCOL_VERSION_10)
            .expect("strict decoding should accept chunked Data bytestrings");
    }
}
