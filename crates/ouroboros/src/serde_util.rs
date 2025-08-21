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

pub mod bytes {
    use amaru_kernel::ed25519;
    use serde::Deserialize;

    pub fn serialize<S>(bytes: &impl Bytes, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bytes(bytes.as_slice())
    }

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<T, D::Error>
    where
        D: serde::Deserializer<'de>,
        T: Bytes,
    {
        let bytes = Vec::<u8>::deserialize(deserializer)?;
        T::from_slice(&bytes).map_err(serde::de::Error::custom)
    }

    pub trait Bytes {
        fn as_slice(&self) -> &[u8];
        fn from_slice(slice: &[u8]) -> Result<Self, String>
        where
            Self: Sized;
    }

    impl<const N: usize> Bytes for Box<[u8; N]> {
        fn as_slice(&self) -> &[u8] {
            &self[..]
        }

        fn from_slice(slice: &[u8]) -> Result<Self, String> {
            Ok(Box::new(<[u8; N]>::try_from(slice).map_err(|_| {
                format!("expected {} bytes, got {}", N, slice.len())
            })?))
        }
    }

    impl Bytes for ed25519::PublicKey {
        fn as_slice(&self) -> &[u8] {
            self.as_ref()
        }

        fn from_slice(slice: &[u8]) -> Result<Self, String> {
            Self::try_from(slice)
                .map_err(|_| format!("expected {} bytes, got {}", Self::SIZE, slice.len()))
        }
    }
}
