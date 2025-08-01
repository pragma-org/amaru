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

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::Anchor;
    use crate::Hash;
    use proptest::{prelude::*, prop_compose, string};

    prop_compose! {
        pub fn any_anchor()(
            url in {
                #[allow(clippy::unwrap_used)]
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
