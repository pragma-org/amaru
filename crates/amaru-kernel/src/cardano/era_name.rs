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

use minicbor::{Decode, Decoder, Encode};
use std::{fmt, str::FromStr};

#[derive(Debug, PartialEq, Eq, Clone, Copy, PartialOrd, Ord)]
#[repr(u8)]
pub enum EraName {
    Byron = 1,
    Shelley = 2,
    Allegra = 3,
    Mary = 4,
    Alonzo = 5,
    Babbage = 6,
    Conway = 7,
    Dijkstra = 8,
}

impl<'de> serde::Deserialize<'de> for EraName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            let s = String::deserialize(deserializer)?;
            Self::from_str(&s).map_err(serde::de::Error::custom)
        } else {
            let value = u8::deserialize(deserializer)?;
            Self::try_from(value).map_err(serde::de::Error::custom)
        }
    }
}

impl serde::Serialize for EraName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if serializer.is_human_readable() {
            serializer.serialize_str(self.as_str())
        } else {
            serializer.serialize_u8(*self as u8)
        }
    }
}

pub const ERA_NAMES: [EraName; 8] = [
    EraName::Byron,
    EraName::Shelley,
    EraName::Allegra,
    EraName::Mary,
    EraName::Alonzo,
    EraName::Babbage,
    EraName::Conway,
    EraName::Dijkstra,
];

const ERA_STRINGS: [&str; 9] = [
    "", "Byron", "Shelley", "Allegra", "Mary", "Alonzo", "Babbage", "Conway", "Dijkstra",
];

impl EraName {
    pub const fn is_tagged_on_network(self) -> bool {
        !matches!(self, EraName::Byron)
    }

    pub const fn as_str(self) -> &'static str {
        ERA_STRINGS[self as usize]
    }
}

impl fmt::Display for EraName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl FromStr for EraName {
    type Err = EraNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Byron" => Ok(EraName::Byron),
            "Shelley" => Ok(EraName::Shelley),
            "Allegra" => Ok(EraName::Allegra),
            "Mary" => Ok(EraName::Mary),
            "Alonzo" => Ok(EraName::Alonzo),
            "Babbage" => Ok(EraName::Babbage),
            "Conway" => Ok(EraName::Conway),
            "Dijkstra" => Ok(EraName::Dijkstra),
            _ => Err(EraNameError::InvalidEraName(s.to_string())),
        }
    }
}

impl From<EraName> for u8 {
    fn from(value: EraName) -> Self {
        value as u8
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EraNameError {
    #[error("Invalid era name: {0}")]
    InvalidEraName(String),
    #[error("Invalid era tag: {0}")]
    InvalidEraTag(u8),
}

impl TryFrom<u8> for EraName {
    type Error = EraNameError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(EraName::Byron),
            2 => Ok(EraName::Shelley),
            3 => Ok(EraName::Allegra),
            4 => Ok(EraName::Mary),
            5 => Ok(EraName::Alonzo),
            6 => Ok(EraName::Babbage),
            7 => Ok(EraName::Conway),
            8 => Ok(EraName::Dijkstra),
            _ => Err(EraNameError::InvalidEraTag(value)),
        }
    }
}

impl Encode<()> for EraName {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        e.u8(*self as u8)?;
        Ok(())
    }
}

impl<'b> Decode<'b, ()> for EraName {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut ()) -> Result<Self, minicbor::decode::Error> {
        let value = d.u8()?;
        EraName::try_from(value).map_err(minicbor::decode::Error::message)
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub fn any_era_name() -> impl proptest::prelude::Strategy<Value = EraName> {
    proptest::sample::select(&ERA_NAMES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use amaru_minicbor_extra::to_cbor;
    use proptest::prelude::*;
    use std::str::FromStr;

    proptest! {
        #[test]
        fn era_name_string_roundtrip(era_name in any_era_name()) {
            let string = format!("{}", era_name);
            assert_eq!(EraName::from_str(&string), Ok(era_name));
        }

        #[test]
        fn era_name_debug_roundtrip(era_name in any_era_name()) {
            let string = format!("{:?}", era_name);
            assert_eq!(EraName::from_str(&string), Ok(era_name));
        }

        #[test]
        fn era_name_encode_decode_roundtrip(era_name in any_era_name()) {
            let buffer = minicbor::to_vec(&era_name).unwrap();
            let decoded: EraName = minicbor::decode(&buffer).unwrap();
            assert_eq!(era_name, decoded);
        }

        #[test]
        fn era_name_serde_string_roundtrip(era_name in any_era_name()) {
            let string = serde_json::to_string(&era_name).unwrap();
            let decoded: EraName = serde_json::from_str(&string).unwrap();
            assert_eq!(era_name, decoded);
        }

        #[test]
        fn era_name_serde_binary_roundtrip(era_name in any_era_name()) {
            let bytes = cbor4ii::serde::to_vec(Vec::new(), &era_name).unwrap();
            let decoded: EraName = cbor4ii::serde::from_slice(&bytes).unwrap();
            assert_eq!(era_name, decoded);
        }
    }

    #[test]
    fn cbor_encoding() {
        let cbor = to_cbor(&ERA_NAMES);
        assert_eq!(
            cbor,
            &[0x88, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]
        );
    }

    #[test]
    fn serde_string_encoding() {
        let string = serde_json::to_string(&ERA_NAMES).unwrap();
        assert_eq!(
            string,
            r#"["Byron","Shelley","Allegra","Mary","Alonzo","Babbage","Conway","Dijkstra"]"#
        );
    }

    #[test]
    fn serde_binary_encoding() {
        let bytes = cbor4ii::serde::to_vec(Vec::new(), &ERA_NAMES).unwrap();
        assert_eq!(
            bytes,
            &[0x88, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]
        );
    }
}
