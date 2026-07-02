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

pub use pallas_primitives::conway::Anchor;
use serde::ser::SerializeStruct;

pub fn serialize<S: serde::Serializer>(anchor: &Option<Anchor>, serializer: S) -> Result<S::Ok, S::Error> {
    if let Some(anchor) = anchor {
        let mut s = serializer.serialize_struct("Anchor", 2)?;
        // NOTE: keep fields in lexicographic order
        //
        // This instance is used for canonical ledger state comparisons.
        s.serialize_field("content_hash", &anchor.content_hash)?;
        s.serialize_field("url", &anchor.url)?;
        s.end()
    } else {
        serializer.serialize_none()
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
