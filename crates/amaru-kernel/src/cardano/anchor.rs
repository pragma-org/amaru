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

use crate::{Hash, cbor};

// NOTE: keep fields in lexicographic order
//
// The `Serialize` instance is used for canonical ledger state comparisons.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Anchor {
    pub content_hash: Hash<32>,
    pub url: String,
}

impl<'b, C> cbor::Decode<'b, C> for Anchor {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        Ok(Self { url: d.decode_with(ctx)?, content_hash: d.decode_with(ctx)? })
    }
}

impl<C> cbor::Encode<C> for Anchor {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode_with(&self.url, ctx)?;
        e.encode_with(self.content_hash, ctx)?;
        Ok(())
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::{prelude::*, prop_compose, string};

    use super::Anchor;
    use crate::Hash;

    prop_compose! {
        pub fn any_anchor()(
            url in {
                #[expect(clippy::unwrap_used)]
                string::string_regex(
                    r"(https:)?[a-zA-Z0-9]{2,}(\.[a-zA-Z0-9]{2,})(\.[a-zA-Z0-9]{2,})?"
                ).unwrap()
            },
            content_hash in any::<[u8; 32]>(),
        ) -> Anchor {
            Anchor {
                url,
                content_hash: Hash::from(content_hash),
            }
        }
    }
}
