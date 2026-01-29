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

use crate::{Int, cbor, key_value_pairs::KeyValuePairs};

/// A piece of (structured) metadata found in transaction.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, serde::Serialize, serde::Deserialize)]
pub enum Metadatum {
    // NOTE: CBOR (signed) integers
    //
    // We use CBOR's Int here and not a Rust's i64 because CBOR's signed integers are encoded next
    // to their signs, meaning that they range from -2^64 to 2^64 - 1; unlike Rust's i64 which
    // ranges from -2^63 .. 2^63 - 1.
    //
    // Simply using an i128 isn't satisfactory because it now allows the representation of invalid
    // states on the Rust's side (we may end up with integers that are far beyond what's
    // acceptable).
    //
    // "Funny-enough", the Haskell code uses arbitrary-length integers here; although only allow
    // decoding in the [-2^64; 2^64 - 1] range. Encoding is fine with arbitrary large integers;
    // thus violating roundtripping invariants.
    Int(Int),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Metadatum>),
    Map(KeyValuePairs<Metadatum, Metadatum>),
}

/// FIXME: Multi-era + length checks on bytes and text
///
/// Ensure that this decoder is multi-era capable and also correctly checks for bytes and
/// (utf-8-encoded) text to be encoded as chunks.
impl<'b, C> cbor::Decode<'b, C> for Metadatum {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        use cbor::data::Type::*;

        #[allow(clippy::wildcard_enum_match_arm)]
        match d.datatype()? {
            U8 | U16 | U32 | U64 | I8 | I16 | I32 | I64 | Int => {
                let i = d.decode()?;
                Ok(Metadatum::Int(i))
            }
            Bytes => Ok(Metadatum::Bytes(Vec::from(
                d.decode_with::<C, cbor::bytes::ByteVec>(ctx)?,
            ))),
            String => Ok(Metadatum::Text(d.decode_with(ctx)?)),
            Array | ArrayIndef => Ok(Metadatum::Array(d.decode_with(ctx)?)),
            Map | MapIndef => Ok(Metadatum::Map(d.decode_with(ctx)?)),
            any => Err(cbor::decode::Error::message(format!(
                "unexpected CBOR datatype {any:?} when decoding metadatum"
            ))),
        }
    }
}

impl<C> cbor::Encode<C> for Metadatum {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Metadatum::Int(a) => {
                e.encode_with(a, ctx)?;
            }
            // FIXME: Use stream encoding for length > 64
            Metadatum::Bytes(a) => {
                e.encode_with(<&cbor::bytes::ByteSlice>::from(a.as_slice()), ctx)?;
            }
            // FIXME: Use stream encoding for length > 64
            Metadatum::Text(a) => {
                e.encode_with(a, ctx)?;
            }
            Metadatum::Array(a) => {
                e.encode_with(a, ctx)?;
            }
            Metadatum::Map(a) => {
                e.encode_with(a, ctx)?;
            }
        };

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Metadatum;
    use crate::{Int, from_cbor_no_leftovers, key_value_pairs::KeyValuePairs};
    use test_case::test_case;

    fn int(n: i128) -> Metadatum {
        Metadatum::Int(Int::try_from(n).unwrap())
    }

    fn bytes(b: &[u8]) -> Metadatum {
        Metadatum::Bytes(b.to_vec())
    }

    fn text(s: &str) -> Metadatum {
        Metadatum::Text(s.to_string())
    }

    fn list(xs: &[Metadatum]) -> Metadatum {
        Metadatum::Array(xs.to_vec())
    }

    fn map(kvs: &[(Metadatum, Metadatum)]) -> Metadatum {
        Metadatum::Map(KeyValuePairs::try_from(kvs.to_vec()).unwrap())
    }

    #[test_case("00", int(0))]
    #[test_case("01", int(1))]
    #[test_case("21", int(-2))]
    #[test_case("0e", int(14))]
    #[test_case("37", int(-24))]
    #[test_case("1819", int(25))]
    #[test_case("387f", int(-128))]
    #[test_case("191bfe", int(7166))]
    #[test_case("39df7a", int(-57211))]
    #[test_case("1A000186A0", int(100000))]
    #[test_case("1B1000000000000019", int(1152921504606847001))]
    #[test_case("3B0000000100000000", int(-4294967297))]
    #[test_case("1B8000000000000000", int(9223372036854775808))]
    #[test_case("1B8000000000000001", int(9223372036854775809))]
    #[test_case("3B7FFFFFFFFFFFFFFF", int(-9223372036854775808))]
    #[test_case("3B8000000000000000", int(-9223372036854775809))]
    #[test_case("1BFFFFFFFFFFFFFFFF", int(18446744073709551615))]
    #[test_case("3BFFFFFFFFFFFFFFFF", int(-18446744073709551616))]
    #[test_case("40", bytes(b""))]
    #[test_case("43666F6F", bytes(b"foo"))]
    #[test_case(
        "5820667A841296E8057AB7792BFB8FD16F8A39B0B648F1E6F0FA586C7785033EC00C",
        bytes(hex::decode("667a841296e8057ab7792bfb8fd16f8a39b0b648f1e6f0fa586c7785033ec00c").unwrap().as_slice());
        "bytes - some hash"
    )]
    // NOTE: on invalid Metadatum
    //
    // Interestingly, the ledger doesn't allow text and bytes chunks over 64 bytes; but will still
    // allow to deserialize them. The metadatum validation happens through a ledger rule, rather
    // than being a decoder failure. While it should be equivalent, we will match the Haskell's
    // behaviour here and allow decoding of over-64 bytes text and bytes. Note that this is
    // "generally" safe provided that the size of the transaction / block is somewhat checked
    // beforehand.
    #[test_case(
        "58E74C6F72656D20697073756D20646F6C6F722073697420616D65742C20636F6E73656374657475722061646970697363696E6720656C69742C2073656420646F20656975736D6F642074656D706F7220696E6369646964756E74207574206C61626F726520657420646F6C6F7265206D61676E6120616C697175612E20557420656E696D206164206D696E696D2076656E69616D2C2071756973206E6F737472756420657865726369746174696F6E20756C6C616D636F206C61626F726973206E69736920757420616C697175697020657820656120636F6D6D6F646F20636F6E7365717561742E",
        bytes(b"Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.");
        "bytes - over 64"
    )]
    #[test_case(
        "5840F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9",
        bytes("💩💩💩💩💩💩💩💩💩💩💩💩💩💩💩💩".as_bytes());
        "bytes - exactly 64"
    )]
    #[test_case("60", text(""))]
    #[test_case("63666F6F", text("foo"))]
    // NOTE: on invalid Metadatum
    //
    // See above.
    #[test_case(
        "78E74C6F72656D20697073756D20646F6C6F722073697420616D65742C20636F6E73656374657475722061646970697363696E6720656C69742C2073656420646F20656975736D6F642074656D706F7220696E6369646964756E74207574206C61626F726520657420646F6C6F7265206D61676E6120616C697175612E20557420656E696D206164206D696E696D2076656E69616D2C2071756973206E6F737472756420657865726369746174696F6E20756C6C616D636F206C61626F726973206E69736920757420616C697175697020657820656120636F6D6D6F646F20636F6E7365717561742E",
        text("Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.");
        "text - over 64"
    )]
    #[test_case(
        "7840F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9",
        text("💩💩💩💩💩💩💩💩💩💩💩💩💩💩💩💩");
        "text - exactly 64"
    )]
    #[test_case("80", list(&[]))]
    #[test_case("9FFF", list(&[]))]
    #[test_case("8101", list(&[int(1)]))]
    #[test_case("9F020304FF", list(&[int(2), int(3), int(4)]))]
    #[test_case("83020304", list(&[int(2), int(3), int(4)]))]
    #[test_case("9F189F801880FF", list(&[int(159), list(&[]), int(128)]))]
    #[test_case("83189F801880", list(&[int(159), list(&[]), int(128)]))]
    #[test_case("A0", map(&[]))]
    #[test_case("BFFF", map(&[]))]
    #[test_case("A1416101", map(&[(bytes(b"a"), int(1))]))]
    #[test_case("BF416101FF", map(&[(bytes(b"a"), int(1))]))]
    #[test_case("A2416102036162", map(&[(bytes(b"a"), int(2)), (int(3), text("b"))]))]
    #[test_case("BF416102036162FF", map(&[(bytes(b"a"), int(2)), (int(3), text("b"))]))]
    #[test_case(
        "A2019FFF1880BFFF",
        map(&[(int(1), list(&[])), (int(128), map(&[]))])
    )]
    #[test_case(
        "BF019FFF1880BFFFFF",
        map(&[(int(1), list(&[])), (int(128), map(&[]))])
    )]
    fn decode_wellformed(fixture: &str, expected: Metadatum) {
        let bytes = hex::decode(fixture).unwrap();
        match from_cbor_no_leftovers::<Metadatum>(bytes.as_slice()) {
            Err(err) => panic!("{err}"),
            Ok(result) => assert_eq!(result, expected),
        }
    }

    #[test_case(
        "C249010000000000000000",
        "decode error: unexpected CBOR datatype Tag when decoding metadatum"
    )]
    #[test_case("1901", "end of input bytes")]
    #[test_case("6261", "end of input bytes")]
    #[test_case("4261", "end of input bytes")]
    #[test_case(
        "784C6F72656D20697073756D20646F6C6F722073697420616D6574",
        "end of input bytes"
    )]
    #[test_case(
        "C349010000000000000000",
        "decode error: unexpected CBOR datatype Tag when decoding metadatum"
    )]
    #[test_case("830102", "end of input bytes")]
    #[test_case("9F0102", "end of input bytes")]
    #[test_case(
        "82010203",
        "decode error: leftovers bytes after decoding after position 3"
    )]
    #[test_case(
        "9F0102FF03",
        "decode error: leftovers bytes after decoding after position 4"
    )]
    #[test_case("A20102", "end of input bytes")]
    #[test_case("BF0102", "end of input bytes")]
    #[test_case(
        "BF01FF",
        "decode error: unexpected CBOR datatype Break when decoding metadatum"
    )]
    #[test_case(
        "A101020304",
        "decode error: leftovers bytes after decoding after position 3"
    )]
    #[test_case(
        "BF0102FF0304",
        "decode error: leftovers bytes after decoding after position 4"
    )]
    fn decode_malformed(fixture: &str, expected: &str) {
        let bytes = hex::decode(fixture).unwrap();
        match from_cbor_no_leftovers::<Metadatum>(bytes.as_slice()) {
            Err(err) => assert_eq!(err.to_string(), expected),
            Ok(result) => panic!("{result:#?}"),
        }
    }
}
