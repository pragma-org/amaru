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

use crate::{Anchor, Hash, cbor, size::SCRIPT};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Constitution {
    pub anchor: Anchor,
    pub guardrail_script: Option<Hash<SCRIPT>>,
}

impl<'b, C> cbor::Decode<'b, C> for Constitution {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            let anchor = d.decode_with(ctx)?;
            let guardrail_script = d.decode_with(ctx)?;
            Ok(Self { anchor, guardrail_script })
        })
    }
}

impl<C> cbor::Encode<C> for Constitution {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode_with(&self.anchor, ctx)?;
        e.encode_with(self.guardrail_script, ctx)?;
        Ok(())
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::{option, prelude::*};

    use crate::{Constitution, any_anchor, any_hash28};

    prop_compose! {
        pub fn any_constitution()(
            anchor in any_anchor(),
            guardrail_script in option::of(any_hash28())
        ) -> Constitution {
            Constitution {
                anchor,
                guardrail_script,
            }
        }
    }
}
