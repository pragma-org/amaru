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

use amaru_minicbor_extra::{decode_bytes, decode_string};

use crate::{Int, cbor};

/// A piece of (structured) metadata found in transaction
#[derive(Debug, PartialEq, Eq, Clone, serde::Deserialize)]
#[serde(try_from = "Vec<Node>")]
pub struct Metadatum {
    nodes: Vec<Node>,
}

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
enum Node {
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
    Bytes(#[serde(with = "crate::utils::serde::bytes")] Vec<u8>),
    Text(String),
    Array { children: usize },
    // NOTE: Association list, not a dictionary
    //
    // The ledger preserves both the order of the entries and any duplicate keys; on-chain metadata
    // does contain duplicate keys, and the auxiliary data digest is computed over those exact
    // bytes. Collapsing them into a map would silently drop entries.
    Map { entries: usize },
}

impl Metadatum {
    pub fn int(value: Int) -> Self {
        Self { nodes: vec![Node::Int(value)] }
    }

    pub fn bytes(value: Vec<u8>) -> Self {
        Self { nodes: vec![Node::Bytes(value)] }
    }

    pub fn text(value: String) -> Self {
        Self { nodes: vec![Node::Text(value)] }
    }

    pub fn array(items: Vec<Self>) -> Self {
        let mut nodes = vec![Node::Array { children: items.len() }];
        nodes.extend(items.into_iter().flat_map(|item| item.nodes));

        Self { nodes }
    }

    pub fn map(entries: Vec<(Self, Self)>) -> Self {
        let mut nodes = vec![Node::Map { entries: entries.len() }];
        nodes.extend(entries.into_iter().flat_map(|(key, value)| key.nodes.into_iter().chain(value.nodes)));

        Self { nodes }
    }
}

impl serde::Serialize for Metadatum {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&self.nodes, serializer)
    }
}

impl TryFrom<Vec<Node>> for Metadatum {
    type Error = String;

    fn try_from(nodes: Vec<Node>) -> Result<Self, Self::Error> {
        let mut owed: usize = 1;

        for (position, node) in nodes.iter().enumerate() {
            if owed == 0 {
                return Err(format!("node {position} follows a complete metadatum"));
            }

            let children = node.children();
            let remaining = nodes.len() - position - 1;

            if children > remaining {
                return Err(format!("node {position} claims {children} children with {remaining} left"));
            }

            owed = owed - 1 + children;
        }

        if owed > 0 {
            return Err(format!("metadatum ends owing {owed} more node(s)"));
        }

        Ok(Self { nodes })
    }
}

impl Node {
    fn children(&self) -> usize {
        match *self {
            Self::Array { children } => children,
            Self::Map { entries } => entries.saturating_mul(2),
            Self::Int(..) | Self::Bytes(..) | Self::Text(..) => 0,
        }
    }

    fn set_count(&mut self, count: usize) {
        match self {
            Self::Array { children } => *children = count,
            Self::Map { entries } => *entries = count,
            Self::Int(..) | Self::Bytes(..) | Self::Text(..) => {
                unreachable!("only a container opens a frame")
            }
        }
    }
}

/// A container whose children are still being decoded.
struct Frame {
    index: usize,
    /// Nodes the container still owes, or `None` when it is indefinite and so ends on a break.
    remaining: Option<u64>,
    counted: usize,
    stride: usize,
}

impl Frame {
    fn next_child(&mut self, d: &mut cbor::Decoder<'_>) -> Result<bool, cbor::decode::Error> {
        match self.remaining {
            Some(0) => return Ok(false),
            Some(remaining) => self.remaining = Some(remaining - 1),
            None if self.counted.is_multiple_of(self.stride) && cbor::decode_break(d, None)? => return Ok(false),
            None => {}
        }

        self.counted += 1;

        Ok(true)
    }

    fn count(&self) -> usize {
        self.counted / self.stride
    }
}

fn open(stack: &mut Vec<Frame>, len: Option<u64>, index: usize, stride: usize) -> usize {
    stack.push(Frame { index, remaining: len.map(|len| len.saturating_mul(stride as u64)), counted: 0, stride });

    len.unwrap_or(0) as usize
}

fn decode_nodes(d: &mut cbor::Decoder<'_>) -> Result<Vec<Node>, cbor::decode::Error> {
    use cbor::data::Type::*;

    let mut nodes = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();

    loop {
        let index = nodes.len();

        #[allow(clippy::wildcard_enum_match_arm)]
        match d.datatype()? {
            U8 | U16 | U32 | U64 | I8 | I16 | I32 | I64 | Int => {
                let i = d.decode()?;
                nodes.push(Node::Int(i));
            }
            // Conformance: the Haskell node accepts indefinite-length bytes and text inside metadata
            // at every protocol version (`decodeMetadatum` has explicit TypeBytesIndef/TypeStringIndef branches),
            // so this decoder is deliberately not version-dependent.
            Bytes | BytesIndef => {
                let bytes = decode_bytes(d)?.into_owned();
                if bytes.len() > 64 {
                    return Err(cbor::decode::Error::message(format!("bytes exceeds 64 bytes: got {}", bytes.len())));
                }
                nodes.push(Node::Bytes(bytes));
            }
            String | StringIndef => {
                let text: std::string::String = decode_string(d)?.into_owned();
                if text.len() > 64 {
                    return Err(cbor::decode::Error::message(format!("text exceeds 64 bytes: got {}", text.len())));
                }
                nodes.push(Node::Text(text));
            }
            Array | ArrayIndef => {
                let children = open(&mut stack, d.array()?, index, 1);
                nodes.push(Node::Array { children });
            }
            Map | MapIndef => {
                let entries = open(&mut stack, d.map()?, index, 2);
                nodes.push(Node::Map { entries });
            }
            any => {
                return Err(cbor::decode::Error::message(format!(
                    "unexpected CBOR datatype {any:?} when decoding metadatum"
                )));
            }
        }

        while let Some(frame) = stack.last_mut() {
            if frame.next_child(d)? {
                break;
            }

            let frame = stack.pop().unwrap_or_else(|| unreachable!("frame observed on the line above"));
            nodes[frame.index].set_count(frame.count());
        }

        if stack.is_empty() {
            return Ok(nodes);
        }
    }
}

/// FIXME(cbor): Multi-era
///
/// Ensure that this decoder is multi-era capable
impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for Metadatum {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        Ok(Self { nodes: decode_nodes(d)? })
    }
}

impl<C: cbor::HasProtocolVersion> cbor::Encode<C> for Metadatum {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        for node in &self.nodes {
            match node {
                Node::Int(a) => {
                    e.encode_with(a, ctx)?;
                }
                Node::Bytes(a) => {
                    e.encode_with(<&cbor::bytes::ByteSlice>::from(a.as_slice()), ctx)?;
                }
                Node::Text(a) => {
                    e.encode_with(a, ctx)?;
                }
                Node::Array { children } => {
                    e.array(*children as u64)?;
                }
                Node::Map { entries } => {
                    e.map(*entries as u64)?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::Metadatum;
    use crate::{Int, from_cbor_no_leftovers, to_cbor};

    fn int(n: i128) -> Metadatum {
        Metadatum::int(Int::try_from(n).unwrap())
    }

    fn bytes(b: &[u8]) -> Metadatum {
        Metadatum::bytes(b.to_vec())
    }

    fn text(s: &str) -> Metadatum {
        Metadatum::text(s.to_string())
    }

    fn list(xs: &[Metadatum]) -> Metadatum {
        Metadatum::array(xs.to_vec())
    }

    fn map(kvs: &[(Metadatum, Metadatum)]) -> Metadatum {
        Metadatum::map(kvs.to_vec())
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
    #[test_case(
        "5840F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9",
        bytes("💩💩💩💩💩💩💩💩💩💩💩💩💩💩💩💩".as_bytes());
        "bytes - exactly 64"
    )]
    #[test_case("60", text(""))]
    #[test_case("63666F6F", text("foo"))]
    #[test_case(
        "7840F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9F09F92A9",
        text("💩💩💩💩💩💩💩💩💩💩💩💩💩💩💩💩");
        "text - exactly 64"
    )]
    #[test_case("5F5820444444444444444444444444444444444444444444444444444444444444444458204444444444444444444444444444444444444444444444444444444444444444FF", bytes(&[0x44; 64]); "bytes - two 32-byte chunks")]
    #[test_case("7F7820616161616161616161616161616161616161616161616161616161616161616178206161616161616161616161616161616161616161616161616161616161616161FF", text(&"a".repeat(64)); "text - two 32-byte chunks")]
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
    #[test_case("A2416101416102", map(&[(bytes(b"a"), int(1)), (bytes(b"a"), int(2))]); "duplicate keys")]
    #[test_case("A2036162036161", map(&[(int(3), text("b")), (int(3), text("a"))]); "duplicate keys, unsorted values")]
    #[test_case("A2026161016161", map(&[(int(2), text("a")), (int(1), text("a"))]); "unsorted keys")]
    fn decode_wellformed(fixture: &str, expected: Metadatum) {
        let bytes = hex::decode(fixture).unwrap();
        match from_cbor_no_leftovers::<Metadatum>(bytes.as_slice()) {
            Err(err) => panic!("{err}"),
            Ok(result) => assert_eq!(result, expected),
        }
    }

    #[test_case("C249010000000000000000", "decode error: unexpected CBOR datatype Tag when decoding metadatum")]
    #[test_case("1901", "end of input bytes")]
    #[test_case("6261", "end of input bytes")]
    #[test_case("4261", "end of input bytes")]
    #[test_case("784C6F72656D20697073756D20646F6C6F722073697420616D6574", "end of input bytes")]
    #[test_case("C349010000000000000000", "decode error: unexpected CBOR datatype Tag when decoding metadatum")]
    #[test_case("830102", "end of input bytes")]
    #[test_case("9F0102", "end of input bytes")]
    #[test_case("82010203", "decode error: leftovers bytes after decoding after position 3")]
    #[test_case("9F0102FF03", "decode error: leftovers bytes after decoding after position 4")]
    #[test_case("A20102", "end of input bytes")]
    #[test_case("BF0102", "end of input bytes")]
    #[test_case("BF01FF", "decode error: unexpected CBOR datatype Break when decoding metadatum")]
    #[test_case("A101020304", "decode error: leftovers bytes after decoding after position 3")]
    #[test_case("BF0102FF0304", "decode error: leftovers bytes after decoding after position 4")]
    #[test_case(
        "58E74C6F72656D20697073756D20646F6C6F722073697420616D65742C20636F6E73656374657475722061646970697363696E6720656C69742C2073656420646F20656975736D6F642074656D706F7220696E6369646964756E74207574206C61626F726520657420646F6C6F7265206D61676E6120616C697175612E20557420656E696D206164206D696E696D2076656E69616D2C2071756973206E6F737472756420657865726369746174696F6E20756C6C616D636F206C61626F726973206E69736920757420616C697175697020657820656120636F6D6D6F646F20636F6E7365717561742E",
        "decode error: bytes exceeds 64 bytes: got 231"
    )]
    #[test_case(
        "78E74C6F72656D20697073756D20646F6C6F722073697420616D65742C20636F6E73656374657475722061646970697363696E6720656C69742C2073656420646F20656975736D6F642074656D706F7220696E6369646964756E74207574206C61626F726520657420646F6C6F7265206D61676E6120616C697175612E20557420656E696D206164206D696E696D2076656E69616D2C2071756973206E6F737472756420657865726369746174696F6E20756C6C616D636F206C61626F726973206E69736920757420616C697175697020657820656120636F6D6D6F646F20636F6E7365717561742E",
        "decode error: text exceeds 64 bytes: got 231"
    )]
    #[test_case("5F582144444444444444444444444444444444444444444444444444444444444444444458204444444444444444444444444444444444444444444444444444444444444444FF", "decode error: bytes exceeds 64 bytes: got 65"; "bytes - 33-byte and 32-byte chunks")]
    #[test_case("7F782161616161616161616161616161616161616161616161616161616161616161616178206161616161616161616161616161616161616161616161616161616161616161FF", "decode error: text exceeds 64 bytes: got 65"; "text - 33-byte and 32-byte chunks")]
    fn decode_malformed(fixture: &str, expected: &str) {
        let bytes = hex::decode(fixture).unwrap();
        match from_cbor_no_leftovers::<Metadatum>(bytes.as_slice()) {
            Err(err) => assert_eq!(err.to_string(), expected),
            Ok(result) => panic!("{result:#?}"),
        }
    }

    #[test_case("A2416101416102"; "duplicate keys")]
    #[test_case("A2026161016161"; "unsorted keys")]
    #[test_case("A2416102036162"; "distinct keys")]
    fn encode_preserves_entries_verbatim(fixture: &str) {
        let original_bytes = hex::decode(fixture).unwrap();
        let metadatum: Metadatum = from_cbor_no_leftovers(original_bytes.as_slice()).unwrap();
        assert_eq!(hex::encode_upper(to_cbor(&metadatum)), fixture);
    }

    #[test]
    fn bytes_node_json_is_hex_string_and_cbor_is_byte_string() {
        let value = Metadatum::bytes(vec![0xab, 0xcd]);
        let json = serde_json::to_value(&value).expect("json");
        assert_eq!(json, serde_json::json!([{"Bytes": "abcd"}]));
        assert_eq!(serde_json::from_value::<Metadatum>(json).expect("parse hex json"), value);
        assert_eq!(
            serde_json::from_value::<Metadatum>(serde_json::json!([{"Bytes": [171, 205]}]))
                .expect("parse integer-array json"),
            value
        );

        let mut buf = Vec::new();
        cbor4ii::serde::to_writer(&mut buf, &value).expect("cbor");
        let decoded: Metadatum = cbor4ii::serde::from_slice(&buf).expect("decode");
        assert_eq!(decoded, value);
        let cbor_value: cbor4ii::core::Value = cbor4ii::serde::from_slice(&buf).expect("value");
        let cbor4ii::core::Value::Array(nodes) = cbor_value else {
            panic!("expected array encoding of the node list, got {cbor_value:?}");
        };
        let Some(cbor4ii::core::Value::Map(entries)) = nodes.into_iter().next() else {
            panic!("expected map encoding of enum");
        };
        let Some((_, cbor4ii::core::Value::Bytes(_))) = entries.into_iter().next() else {
            panic!("Bytes node payload should be a CBOR byte string");
        };
    }

    #[test_case(r#"[]"#; "no root")]
    #[test_case(r#"[{"Int":1},{"Int":2}]"#; "trailing node")]
    #[test_case(r#"[{"Array":{"children":5}}]"#; "array missing its children")]
    #[test_case(r#"[{"Map":{"entries":1}},{"Int":1}]"#; "map missing its value")]
    fn deserialize_rejects_an_ill_formed_node_list(json: &str) {
        if let Ok(metadatum) = serde_json::from_str::<Metadatum>(json) {
            panic!("{metadatum:#?}");
        }
    }

    /// Each case runs on a thread with an explicitly sized stack. `.cargo/config.toml` raises
    /// `RUST_MIN_STACK` for cargo-launched processes and a deployed node does not inherit it.
    /// A regression here crashes the test binary rather than reporting a failure.
    mod depth {
        use std::{error::Error, thread};

        use test_case::test_case;

        use crate::{Metadatum, from_cbor_no_leftovers, to_cbor};

        type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

        const DEFAULT_STACK: usize = 2 * 1024 * 1024;

        const MAX_TX_SIZE: usize = 16384;

        #[test_case(&[0x81], &[]; "definite arrays")]
        #[test_case(&[0x9f], &[0xff]; "indefinite arrays")]
        #[test_case(&[0xa1, 0x00], &[]; "definite maps")]
        #[test_case(&[0xbf, 0x00], &[0xff]; "indefinite maps")]
        fn handles_the_deepest_metadatum_a_transaction_can_hold(
            open: &'static [u8],
            close: &'static [u8],
        ) -> TestResult {
            on_a_default_stack(move || {
                let bytes = nested(MAX_TX_SIZE / (open.len() + close.len()), open, close);
                let metadatum: Metadatum = from_cbor_no_leftovers(&bytes)?;

                let clone = metadatum.clone();
                assert_eq!(clone, metadatum);
                drop(clone);

                assert_eq!(from_cbor_no_leftovers::<Metadatum>(&to_cbor(&metadatum))?, metadatum);

                Ok(())
            })
        }

        #[test]
        fn serde_round_trips_the_deepest_metadatum_a_transaction_can_hold() -> TestResult {
            on_a_default_stack(|| {
                let bytes = nested(MAX_TX_SIZE, &[0x81], &[]);
                let metadatum: Metadatum = from_cbor_no_leftovers(&bytes)?;

                let json = serde_json::to_string(&metadatum)?;
                assert_eq!(serde_json::from_str::<Metadatum>(&json)?, metadatum);

                Ok(())
            })
        }

        fn nested(depth: usize, open: &[u8], close: &[u8]) -> Vec<u8> {
            const LEAF: [u8; 1] = [0x00];

            let mut bytes = Vec::with_capacity((open.len() + close.len()) * depth + LEAF.len());

            for _ in 0..depth {
                bytes.extend_from_slice(open);
            }
            bytes.extend_from_slice(&LEAF);
            for _ in 0..depth {
                bytes.extend_from_slice(close);
            }

            bytes
        }

        fn on_a_default_stack(test: impl FnOnce() -> TestResult + Send + 'static) -> TestResult {
            match thread::Builder::new().stack_size(DEFAULT_STACK).spawn(test)?.join() {
                Ok(result) => result,
                Err(_) => Err("the test thread panicked, see the failure reported above".into()),
            }
        }
    }
}
