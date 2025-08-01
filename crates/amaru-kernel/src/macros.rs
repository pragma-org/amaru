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

/// Generate a roundtrip property to assert that Cbor encoder and decoder for a given type can
/// safely be called in sequence and yield the original input.
///
/// Requires:
/// - proptest
///
/// Usage:
///
/// # ```
/// # prop_cbor_roundtrip!(MyType, my_strategy())
/// #
/// # // Or with an explicit test title in case a module contains multiple calls to the macro:
/// # prop_cbor_roundtrip!(prop_cbor_roundtrip_MyType, MyType, my_strategy())
/// # ```
#[macro_export]
macro_rules! prop_cbor_roundtrip {
    ($title:ident, $ty:ty, $strategy:expr) => {
        proptest::proptest! {
            #[test]
            fn $title(val in $strategy) {
                let bytes = $crate::to_cbor(&val);
                proptest::prop_assert_eq!(Some(val), $crate::from_cbor::<$ty>(&bytes));
            }
        }
    };

    ($ty:ty, $strategy:expr) => {
        prop_cbor_roundtrip!(prop_cbor_roundtrip, $ty, $strategy);
    };
}

/// Easily create a hash from a hex-encoded literal string. Useful for testing.
///
/// Requires:
/// - hex
///
/// Usage:
///
/// # ```
/// # let my_hash32: Hash<32> = hash!("a7c4477e9fcfd519bf7dcba0d4ffe35a399125534bc8c60fa89ff6b50a060a7a"),
/// # let my_hash28: Hash<28> = hash!("a7c4477e9fcfd519bf7dcba0d4ffe35a399125534bc8c60fa89ff6b5"),
/// # ```
#[macro_export]
macro_rules! hash {
    ($str:literal $(,)?) => {{
        // Raise a compile-time error if the literal string is looking dubious
        const _ASSERT_IS_HEX: () = {
            let bytes = $str.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                match bytes[i] {
                    b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' => {}
                    _ => panic!("not a valid hex literal"),
                }
                i += 1;
            }
            if i != 64 && i != 56 {
                panic!("invalid hash literal length");
            }
        };
        $crate::Hash::from(hex::decode($str).unwrap().as_slice())
    }};
}

/// Includes and deserialize a CBOR-encoded test data file. The file must located under the
/// project's tests/data folder, relative to the project's Cargo.toml.
#[macro_export]
macro_rules! include_cbor {
    ($filepath:expr) => {
        $crate::cbor::decode(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/",
            $filepath,
        )))
        .expect(concat!("invalid cbor file: ", $filepath))
    };
}

/// Includes and deserialize a JSON-encoded test data file. The file must located under the
/// project's tests/data folder, relative to the project's Cargo.toml.
#[macro_export]
macro_rules! include_json {
    ($filepath:expr) => {{
        $crate::json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/",
            $filepath,
        )))
        .expect(concat!("invalid json file: ", $filepath))
    }};
}
