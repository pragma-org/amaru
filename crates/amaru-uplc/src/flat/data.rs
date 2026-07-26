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

use amaru_minicbor_extra::{decode_bigint, encode_bigint, encode_bytestring};
use bumpalo::collections::Vec as BumpVec;
use minicbor::data::{IanaTag, Tag};

use crate::{data::PlutusData, flat::SimpleCtx};

impl<'a, 'b> minicbor::decode::Decode<'b, SimpleCtx<'a>> for &'a PlutusData<'a> {
    fn decode(decoder: &mut minicbor::Decoder<'b>, ctx: &mut SimpleCtx<'a>) -> Result<Self, minicbor::decode::Error> {
        let typ = decoder.datatype()?;

        match typ {
            minicbor::data::Type::Tag => {
                let mut probe = decoder.probe();

                let tag = probe.tag()?;

                if matches!(tag.as_u64(), 121..=127 | 1280..=1400 | 102) {
                    let x = decoder.tag()?.as_u64();

                    return match x {
                        121..=127 => {
                            let mut fields = BumpVec::new_in(ctx.arena.as_bump());

                            for x in decoder.array_iter_with(ctx)? {
                                fields.push(x?);
                            }

                            let fields = ctx.arena.alloc(fields);

                            let data = PlutusData::constr(ctx.arena, x - 121, fields);

                            Ok(data)
                        }
                        1280..=1400 => {
                            let mut fields = BumpVec::new_in(ctx.arena.as_bump());

                            for x in decoder.array_iter_with(ctx)? {
                                fields.push(x?);
                            }

                            let fields = ctx.arena.alloc(fields);

                            let data = PlutusData::constr(ctx.arena, (x - 1280) + 7, fields);

                            Ok(data)
                        }
                        102 => {
                            let mut fields = BumpVec::new_in(ctx.arena.as_bump());

                            let count = decoder.array()?;
                            if count != Some(2) {
                                return Err(minicbor::decode::Error::message(
                                    "expected array of length 2 following plutus data tag 102",
                                ));
                            }

                            let discriminator_i128: i128 = decoder.int()?.into();
                            let discriminator: u64 = match u64::try_from(discriminator_i128) {
                                Ok(n) => n,
                                Err(_) => {
                                    return Err(minicbor::decode::Error::message(format!(
                                        "could not cast discriminator from plutus data tag 102 into u64: {discriminator_i128}",
                                    )));
                                }
                            };

                            for x in decoder.array_iter_with(ctx)? {
                                fields.push(x?);
                            }

                            let fields = ctx.arena.alloc(fields);

                            let data = PlutusData::constr(ctx.arena, discriminator, fields);

                            Ok(data)
                        }
                        _ => {
                            let e =
                                minicbor::decode::Error::message(format!("unknown tag for plutus data tag: {tag}",));

                            Err(e)
                        }
                    };
                }

                match tag.try_into() {
                    Ok(IanaTag::PosBignum | IanaTag::NegBignum) => {
                        let integer = ctx.arena.alloc_integer(decode_bigint(decoder)?);

                        Ok(PlutusData::integer(ctx.arena, integer))
                    }

                    _ => {
                        let e = minicbor::decode::Error::message(format!("unknown tag for plutus data tag: {tag}",));

                        Err(e)
                    }
                }
            }
            minicbor::data::Type::Map | minicbor::data::Type::MapIndef => {
                let mut fields = BumpVec::new_in(ctx.arena.as_bump());

                for x in decoder.map_iter_with(ctx)? {
                    let x = x?;

                    fields.push(x);
                }

                let fields = ctx.arena.alloc(fields);

                Ok(PlutusData::map(ctx.arena, fields))
            }
            minicbor::data::Type::Bytes | minicbor::data::Type::BytesIndef => {
                let mut bs = BumpVec::new_in(ctx.arena.as_bump());

                for chunk in decoder.bytes_iter()? {
                    let chunk = chunk?;

                    bs.extend_from_slice(chunk);
                }

                let bs = ctx.arena.alloc(bs);

                Ok(PlutusData::byte_string(ctx.arena, bs))
            }
            minicbor::data::Type::Array | minicbor::data::Type::ArrayIndef => {
                let mut fields = BumpVec::new_in(ctx.arena.as_bump());

                for x in decoder.array_iter_with(ctx)? {
                    fields.push(x?);
                }

                let fields = ctx.arena.alloc(fields);

                Ok(PlutusData::list(ctx.arena, fields))
            }
            minicbor::data::Type::U8
            | minicbor::data::Type::U16
            | minicbor::data::Type::U32
            | minicbor::data::Type::U64
            | minicbor::data::Type::I8
            | minicbor::data::Type::I16
            | minicbor::data::Type::I32
            | minicbor::data::Type::I64
            | minicbor::data::Type::Int => {
                let integer = ctx.arena.alloc_integer(decode_bigint(decoder)?);

                Ok(PlutusData::integer(ctx.arena, integer))
            }
            any => {
                let e = minicbor::decode::Error::message(format!("bad cbor data type ({any:?}) for plutus data"));

                Err(e)
            }
        }
    }
}

impl<C> minicbor::encode::Encode<C> for PlutusData<'_> {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        match self {
            PlutusData::Constr { tag, fields } => {
                if *tag < 7 {
                    e.tag(Tag::new(*tag + 121))?;
                } else if *tag <= 127 {
                    e.tag(Tag::new((*tag - 7) + 1280))?;
                } else {
                    e.tag(Tag::new(102))?;
                    e.array(2)?;
                    e.u64(*tag)?;
                }

                // defaultEncodeList in Codec.Serialise emits definite in case of 0-length list
                // https://github.com/well-typed/cborg/blob/1e9d079d382f237a1a282e268eecce2b395acb9c/serialise/src/Codec/Serialise/Class.hs#L165-L171
                if fields.is_empty() {
                    e.array(0)?;
                } else {
                    // TODO: figure out if we need to care about def vs indef
                    // The encoding implementation in plutus-core uses indefinite here,
                    // though both forms are accepted when decoding
                    // https://github.com/IntersectMBO/plutus/blob/9538fc9829426b2ecb0628d352e2d7af96ec8204/plutus-core/plutus-core/src/PlutusCore/Data.hs#L198
                    e.begin_array()?;
                    for f in fields.iter() {
                        f.encode(e, ctx)?;
                    }
                    e.end()?;
                }
            }
            // stolen from pallas
            // we use definite array to match the approach used by haskell's plutus
            // implementation https://github.com/input-output-hk/plutus/blob/9538fc9829426b2ecb0628d352e2d7af96ec8204/plutus-core/plutus-core/src/PlutusCore/Data.hs#L152
            PlutusData::Map(map) => {
                let len: u64 = map.len().try_into().expect("setting map length should work fine");

                e.map(len)?;

                for (k, v) in map.iter() {
                    k.encode(e, ctx)?;
                    v.encode(e, ctx)?;
                }
            }
            PlutusData::Integer(n) => {
                encode_bigint(e, n)?;
            }
            // we match the haskell implementation by encoding bytestrings longer than 64
            // bytes as indefinite lists of bytes
            PlutusData::ByteString(bs) => {
                encode_bytestring(e, bs)?;
            }
            PlutusData::List(xs) => {
                if xs.is_empty() {
                    e.array(0)?;
                } else {
                    e.begin_array()?;
                    for x in xs.iter() {
                        x.encode(e, ctx)?;
                    }
                    e.end()?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::Arena;

    #[test]
    fn encode_empty_record() {
        let d = PlutusData::Constr { tag: 0, fields: &[] };
        let mut v = vec![];
        minicbor::encode(d, &mut v).expect("invalid PlutusData");
        assert_eq!(hex::encode(v), "d87980");
    }

    #[test]
    fn encode_record() {
        let b1 = PlutusData::ByteString(&[0x00]);
        let b2 = PlutusData::ByteString(&[0x00, 0x01]);
        let d = PlutusData::Constr { tag: 1, fields: &[&b1, &b2] };
        let mut v = vec![];
        minicbor::encode(d, &mut v).expect("invalid PlutusData");
        assert_eq!(hex::encode(v), "d87a9f4100420001ff");
    }

    #[test]
    fn encode_record_integer() {
        let zero = num::BigInt::from(0);
        let one = num::BigInt::from(1);
        let d = PlutusData::Constr { tag: 128, fields: &[&PlutusData::Integer(&zero), &PlutusData::Integer(&one)] };
        let mut v = vec![];
        minicbor::encode(d, &mut v).expect("invalid PlutusData");
        assert_eq!(hex::encode(v), "d8668218809f0001ff");
    }

    #[test]
    fn encode_cbor_data_bigint() {
        let big = num::BigInt::from_bytes_be(num_bigint::Sign::Plus, &hex::decode("033b2e3c9fd0803ce7ffffff").unwrap());
        let d = PlutusData::Constr { tag: 0, fields: &[&PlutusData::Integer(&big)] };
        let mut v = vec![];
        minicbor::encode(d, &mut v).expect("invalid PlutusData");
        assert_eq!(hex::encode(v), "d8799fc24c033b2e3c9fd0803ce7ffffffff");
    }

    #[test]
    fn encode_cbor_data_negative_bigint() {
        let n = -num::BigInt::from_bytes_be(num_bigint::Sign::Plus, &hex::decode("033b2e3c9fd0803ce7ffffff").unwrap())
            - num::BigInt::from(1);
        let d = PlutusData::Constr { tag: 0, fields: &[&PlutusData::Integer(&n)] };
        let mut v = vec![];
        minicbor::encode(d, &mut v).expect("invalid PlutusData");
        assert_eq!(hex::encode(v), "d8799fc34c033b2e3c9fd0803ce7ffffffff");
    }

    #[test]
    fn decode_cbor_data_negative_bigint() {
        let cbor = hex::decode("c34c033b2e3c9fd0803ce7ffffff").unwrap();
        let arena = Arena::new();
        let decoded = PlutusData::from_cbor(&arena, &cbor).expect("failed to decode negative bigint");
        let expected =
            -num::BigInt::from_bytes_be(num_bigint::Sign::Plus, &hex::decode("033b2e3c9fd0803ce7ffffff").unwrap())
                - num::BigInt::from(1);
        assert_eq!(decoded, &PlutusData::Integer(&expected));
    }

    #[test]
    fn roundtrip_cbor_data_negative_bigint() {
        let n = -num::BigInt::from_bytes_be(num_bigint::Sign::Plus, &hex::decode("033b2e3c9fd0803ce7ffffff").unwrap())
            - num::BigInt::from(1);
        let encoded = minicbor::to_vec(PlutusData::Integer(&n)).expect("encode failed");
        let arena = Arena::new();
        let decoded = PlutusData::from_cbor(&arena, &encoded).expect("decode failed");
        assert_eq!(decoded, &PlutusData::Integer(&n));
    }

    /// Test that the encoding is correct at both 2^64 - 1 and -2^64
    #[test]
    fn encode_integer_word_boundaries() {
        let one = num::BigInt::from(1);
        let two_64: num::BigInt = num::BigInt::from(1) << 64;
        let two_64_minus_one: num::BigInt = &two_64 - &one;

        // Largest values still encoded as native CBOR integers.
        assert_eq!(encode_integer_hex(&two_64_minus_one), "1bffffffffffffffff");
        assert_eq!(encode_integer_hex(&-two_64_minus_one.clone()), "3bfffffffffffffffe");

        // -2^64 is the smallest native negative integer (arg = 2^64 - 1 fits a word).
        assert_eq!(encode_integer_hex(&-two_64.clone()), "3bffffffffffffffff");

        // Just past the boundary, both directions switch to a bignum.
        assert_eq!(encode_integer_hex(&two_64), "c249010000000000000000");
        assert_eq!(encode_integer_hex(&(-two_64.clone() - &one)), "c349010000000000000000");
    }

    #[test]
    fn roundtrip_integer_min_native() {
        let two_64: num::BigInt = num::BigInt::from(1) << 64;
        let n = -two_64;
        let encoded = minicbor::to_vec(PlutusData::Integer(&n)).expect("encode failed");
        let arena = Arena::new();
        let decoded = PlutusData::from_cbor(&arena, &encoded).expect("decode failed");
        assert_eq!(decoded, &PlutusData::Integer(&n));
    }

    #[test]
    fn encode_cbor_data_list() {
        let zero = num::BigInt::from(0);
        let one = num::BigInt::from(1);
        let list = [&PlutusData::Integer(&zero), &PlutusData::Integer(&one)];
        let d = PlutusData::Constr { tag: 0, fields: &[&PlutusData::List(&list)] };
        let mut v = vec![];
        minicbor::encode(d, &mut v).expect("invalid PlutusData");
        assert_eq!(hex::encode(v), "d8799f9f0001ffff");
    }

    // HELPERS

    /// Encode a BigInt as hex-encoded CBOR
    fn encode_integer_hex(n: &num::BigInt) -> String {
        let mut v = vec![];
        minicbor::encode(PlutusData::Integer(n), &mut v).expect("invalid PlutusData");
        hex::encode(v)
    }
}
